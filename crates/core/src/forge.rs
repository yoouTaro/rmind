//! Forge: turn habits in your history into Claude Code rules and skills.
//!
//! 1. Mine (local, free): moments you redirected Claude (right after
//!    interrupting it, or "no, ...", "why did you ...") with what it had
//!    just said, prompts you repeat across sessions, each project's
//!    most-run commands, skills you use.
//! 2. Draft: one headless `claude -p` call turns that evidence into
//!    suggestions, knowing your current CLAUDE.md and skills so it doesn't
//!    repeat them.
//! 3. Review: nothing is written until you accept a suggestion; then a rule
//!    is appended to a CLAUDE.md and a skill becomes a SKILL.md.

use crate::db::{Db, Result};
use crate::parse::clip;
use crate::paths;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub id: i64,
    pub created: String,
    /// "rule" or "skill".
    pub kind: String,
    /// "global" or "project".
    pub scope: String,
    /// The project folder, for project scope.
    pub project: String,
    pub title: String,
    pub why: String,
    pub evidence: Vec<String>,
    /// CLAUDE.md lines for a rule, the whole SKILL.md for a skill.
    pub content: String,
    pub skill_name: String,
    /// "new", "accepted" or "dismissed".
    pub status: String,
    /// Where it was written, once accepted.
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub ts: String,
    pub suggestions: i64,
    pub cost: f64,
    pub error: Option<String>,
}

const MODEL: &str = "sonnet";
/// Most redirect moments sent to Claude, newest first.
const MAX_MOMENTS: usize = 220;
/// The heading rules are appended under.
const HEADING: &str = "## Learned from past sessions (rmind)";

/// Mine, draft, and store the new suggestions. Returns how many.
pub fn run(db: &Db) -> Result<usize> {
    let evidence = mine(db)?;
    let prompt = prompt(db, &evidence)?;
    let started = chrono::Local::now().to_rfc3339();
    match draft(&prompt) {
        Ok((suggestions, cost)) => {
            let n = suggestions.len();
            for s in suggestions {
                db.add_suggestion(&s)?;
            }
            db.add_run(&Run { ts: started, suggestions: n as i64, cost, error: None })?;
            Ok(n)
        }
        Err(e) => {
            db.add_run(&Run { ts: started, suggestions: 0, cost: 0.0, error: Some(e.to_string()) })?;
            Err(e)
        }
    }
}

/// The evidence, as text for the drafting prompt.
pub fn mine(db: &Db) -> Result<String> {
    let mut rows = db.all_text()?;
    // Forked and resumed sessions repeat earlier messages under the same id;
    // count each once.
    let mut seen = HashSet::new();
    rows.retain(|r| r.uuid.is_empty() || seen.insert((r.uuid.clone(), r.role.clone(), r.text.len())));
    let mut out = String::new();

    // Moments you redirected Claude, with what it had just said.
    let correction = |t: &str| {
        let t = t.trim_start().to_lowercase();
        const STARTS: [&str; 14] =
            ["no ", "no,", "nah", "nope", "not ", "don't", "dont", "stop", "wait", "why did", "why u ", "why you", "instead", "revert"];
        const HAS: [&str; 10] =
            ["i said", "i told", "don't ", "dont ", "never ", "always ", "should not", "shouldn't", "no need", "not what i"];
        STARTS.iter().any(|s| t.starts_with(s)) || HAS.iter().any(|s| t.contains(s))
    };
    let mut moments = Vec::new();
    let mut last_reply: HashMap<&str, &str> = HashMap::new();
    let mut interrupted: HashSet<&str> = HashSet::new();
    for r in &rows {
        match r.role.as_str() {
            "assistant" => {
                last_reply.insert(&r.session, &r.text);
            }
            "user" if r.text.starts_with("[Request interrupted") => {
                interrupted.insert(&r.session);
            }
            "user" => {
                let after_interrupt = interrupted.remove(r.session.as_str());
                if after_interrupt || correction(&r.text) {
                    let before = last_reply.get(r.session.as_str()).map(|t| one_line(&clip(t, 220))).unwrap_or_default();
                    moments.push((r.ts.clone(), r.cwd.clone(), after_interrupt, before, one_line(&clip(&r.text, 260))));
                }
            }
            _ => {}
        }
    }
    moments.sort_by(|a, b| b.0.cmp(&a.0));
    let _ = writeln!(out, "## Moments the user redirected Claude ({} of {}, newest first)\n", moments.len().min(MAX_MOMENTS), moments.len());
    for (ts, cwd, interrupt, before, said) in moments.iter().take(MAX_MOMENTS) {
        let how = if *interrupt { "interrupted, then" } else { "said" };
        let _ = writeln!(out, "- {} [{}] Claude: \"{before}\" → user {how}: \"{said}\"", &ts[..ts.len().min(10)], project(cwd));
    }

    // Prompts repeated across sessions.
    let mut repeated: HashMap<String, HashSet<&str>> = HashMap::new();
    for r in rows.iter().filter(|r| r.role == "user" && r.text.len() <= 80 && !r.text.starts_with('[')) {
        let key = r.text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '/').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
        if !key.is_empty() {
            repeated.entry(key).or_default().insert(&r.session);
        }
    }
    let mut repeated: Vec<_> = repeated.into_iter().filter(|(_, s)| s.len() >= 3).map(|(k, s)| (s.len(), k)).collect();
    repeated.sort_by(|a, b| b.cmp(a));
    out.push_str("\n## Prompts repeated across sessions (sessions: prompt)\n\n");
    for (n, k) in repeated.iter().take(40) {
        let _ = writeln!(out, "- {n}: \"{k}\"");
    }

    // Each project's most-run commands, and skills used.
    let mut commands: BTreeMap<&str, HashMap<String, usize>> = BTreeMap::new();
    let mut skills: HashMap<&str, usize> = HashMap::new();
    for r in rows.iter().filter(|r| r.role == "tool") {
        if let Some(cmd) = r.text.strip_prefix("$ ") {
            // The command line itself, not a heredoc's or script's body.
            let cmd = cmd.split("  # ").next().unwrap_or(cmd);
            let cmd = cmd.split("<<").next().unwrap_or(cmd);
            let cmd = cmd.lines().next().unwrap_or(cmd);
            for program in programs(cmd) {
                *commands.entry(&r.cwd).or_default().entry(program).or_default() += 1;
            }
        } else if let Some(skill) = r.text.strip_prefix("used skill ") {
            *skills.entry(skill).or_default() += 1;
        }
    }
    out.push_str("\n## Most-run commands per project folder (runs)\n\n");
    for (cwd, counts) in &commands {
        let mut top: Vec<_> = counts.iter().filter(|(_, n)| **n >= 3).collect();
        if top.len() < 2 || is_scratch(cwd) {
            continue;
        }
        top.sort_by(|a, b| b.1.cmp(a.1));
        let list = top.iter().take(10).map(|(c, n)| format!("`{c}` {n}")).collect::<Vec<_>>().join(", ");
        let _ = writeln!(out, "- {cwd}: {list}");
    }
    let mut skills: Vec<_> = skills.into_iter().collect();
    skills.sort_by(|a, b| b.1.cmp(&a.1));
    out.push_str("\n## Skills used (times)\n\n");
    for (s, n) in skills.iter().take(20) {
        let _ = writeln!(out, "- {s}: {n}");
    }
    Ok(out)
}

/// The programs a shell command runs, as "program subcommand" keys, minus
/// the ones that only look around (ls, grep, ...).
fn programs(cmd: &str) -> Vec<String> {
    const BORING: [&str; 34] = [
        "cd", "ls", "cat", "echo", "grep", "rg", "head", "tail", "sed", "awk", "find", "wc", "sort", "uniq", "export", "true", "printf",
        "mkdir", "rm", "cp", "mv", "sleep", "test", "du", "cut", "tr", "xargs", "for", "do", "done", "if", "then", "fi", "while",
    ];
    cmd.split(['&', ';', '|', '\n'])
        .filter_map(|part| {
            let words: Vec<&str> = part.split_whitespace().take_while(|w| !w.starts_with('-') && !w.contains(['/', '=', '"', '\'', '$', '(', '>', '<'])).take(3).collect();
            let first = *words.first()?;
            let command_like = first.len() > 1
                && first.starts_with(|c: char| c.is_ascii_lowercase())
                && first.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c));
            if BORING.contains(&first) || !command_like {
                return None;
            }
            Some(words.join(" "))
        })
        .collect()
}

fn is_scratch(cwd: &str) -> bool {
    cwd.contains("/.t3/scratch/") || cwd.contains("/Claude/scratch")
}

fn project(cwd: &str) -> &str {
    cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What's already set up, so suggestions don't repeat it.
fn current_setup(db: &Db) -> Result<String> {
    let mut out = String::new();
    let claude = paths::claude_dir();
    let global = std::fs::read_to_string(claude.join("CLAUDE.md")).unwrap_or_default();
    let _ = writeln!(out, "## Current ~/.claude/CLAUDE.md\n\n{}\n", if global.trim().is_empty() { "(empty)".into() } else { clip(&global, 6000) });

    out.push_str("## Installed skills (name: description)\n\n");
    if let Ok(dirs) = std::fs::read_dir(claude.join("skills")) {
        let mut lines: Vec<String> = dirs
            .flatten()
            .filter_map(|d| {
                let text = std::fs::read_to_string(d.path().join("SKILL.md")).ok()?;
                let desc = text.lines().find_map(|l| l.strip_prefix("description:")).unwrap_or("").trim().to_string();
                Some(format!("- {}: {}", d.file_name().to_string_lossy(), clip(&desc, 160)))
            })
            .collect();
        lines.sort();
        out.push_str(&lines.join("\n"));
    }

    out.push_str("\n\n## CLAUDE.md files in active projects\n\n");
    for cwd in db.busy_projects(3)? {
        if is_scratch(&cwd) {
            continue;
        }
        let text = std::fs::read_to_string(Path::new(&cwd).join("CLAUDE.md")).unwrap_or_default();
        let _ = writeln!(out, "### {cwd}\n{}\n", if text.trim().is_empty() { "(none)".into() } else { clip(&text, 1500) });
    }

    let earlier = db.suggestions(None)?;
    if !earlier.is_empty() {
        out.push_str("## Earlier suggestions (don't suggest these again)\n\n");
        for s in earlier {
            let _ = writeln!(out, "- [{}] {} ({} {})", s.status, s.title, s.scope, s.kind);
        }
    }
    Ok(out)
}

fn prompt(db: &Db, evidence: &str) -> Result<String> {
    Ok(format!(
        "You are helping a developer improve how Claude Code works for them. Below is evidence mined from their past \
Claude Code sessions, and their current setup. Suggest rules (for a CLAUDE.md) and skills that would have saved them \
from repeating themselves or correcting Claude.

The user writes casual English with typos and some Khmer; read for meaning.

Guidelines:
- Only suggest something backed by a pattern: the same correction or request in at least two places, ideally across sessions.
- Rules are short, concrete instructions Claude can follow, in the imperative (\"Use bun, not npm, in this repo.\"). \
Global scope for preferences that hold everywhere, project scope (with the project's folder path) for facts about one \
project, such as how to run, test or deploy it. Write a rule's content as one to three markdown bullet lines.
- Skills are for multi-step workflows the user asks for repeatedly (deploying, releasing, a review routine). A skill's \
content is a complete SKILL.md: frontmatter with name (lowercase-hyphenated, same as skill_name) and a description that \
says what it does and when to use it, then concise steps.
- Don't repeat what the current CLAUDE.md files or skills already cover, or any earlier suggestion.
- Quote evidence verbatim from the input (short quotes).
- Fewer, better suggestions: at most 10. An empty list is fine.
- Leave project empty for global scope and skill_name empty for rules.

# Evidence

{evidence}

# Current setup

{}",
        current_setup(db)?
    ))
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["suggestions"],
        "properties": {"suggestions": {"type": "array", "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["kind", "scope", "project", "title", "why", "evidence", "content", "skill_name"],
            "properties": {
                "kind": {"type": "string", "enum": ["rule", "skill"]},
                "scope": {"type": "string", "enum": ["global", "project"]},
                "project": {"type": "string"},
                "title": {"type": "string"},
                "why": {"type": "string"},
                "evidence": {"type": "array", "items": {"type": "string"}},
                "content": {"type": "string"},
                "skill_name": {"type": "string"}
            }
        }}}
    })
}

/// Ask Claude for suggestions; also returns what the call cost.
fn draft(prompt: &str) -> Result<(Vec<Suggestion>, f64)> {
    // A folder of its own, so the call doesn't pick up a project's CLAUDE.md.
    let dir = paths::data_dir().join("forge");
    std::fs::create_dir_all(&dir)?;
    let mut child = Command::new("claude")
        .current_dir(&dir)
        .args(["-p", "--model", MODEL, "--effort", "medium", "--tools", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--output-format", "json", "--json-schema", &schema().to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run `claude` ({e}); is Claude Code installed?"))?;
    std::io::Write::write_all(child.stdin.as_mut().ok_or("no stdin")?, prompt.as_bytes())?;
    drop(child.stdin.take());
    let out = child.wait_with_output()?;
    let reply: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        format!("claude failed: {}{}", String::from_utf8_lossy(&out.stderr).trim(), clip(&String::from_utf8_lossy(&out.stdout), 300))
    })?;
    if reply["is_error"] == true {
        return Err(format!("claude: {}", reply["result"].as_str().unwrap_or("error")).into());
    }
    let cost = reply["total_cost_usd"].as_f64().unwrap_or(0.0);
    let list = reply["structured_output"]["suggestions"].as_array().cloned().unwrap_or_default();
    let now = chrono::Local::now().to_rfc3339();
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().trim().to_string();
    let suggestions = list
        .iter()
        .map(|v| Suggestion {
            id: 0,
            created: now.clone(),
            kind: s(v, "kind"),
            scope: s(v, "scope"),
            project: s(v, "project"),
            title: s(v, "title"),
            why: s(v, "why"),
            evidence: v["evidence"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            content: s(v, "content"),
            skill_name: s(v, "skill_name"),
            status: "new".into(),
            target: String::new(),
        })
        .filter(|s| !s.title.is_empty() && !s.content.is_empty())
        .collect();
    Ok((suggestions, cost))
}

/// Where accepting `s` writes.
pub fn target(s: &Suggestion) -> Result<PathBuf> {
    let base = match s.scope.as_str() {
        "global" => paths::claude_dir(),
        "project" => {
            let dir = PathBuf::from(&s.project);
            if !dir.is_absolute() || !dir.is_dir() {
                return Err(format!("project folder {:?} doesn't exist", s.project).into());
            }
            dir
        }
        other => return Err(format!("unknown scope {other}").into()),
    };
    match s.kind.as_str() {
        "rule" => Ok(base.join("CLAUDE.md")),
        "skill" => {
            let name = &s.skill_name;
            if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                return Err(format!("bad skill name {name:?}").into());
            }
            let skills = if s.scope == "global" { base.join("skills") } else { base.join(".claude/skills") };
            Ok(skills.join(name).join("SKILL.md"))
        }
        other => Err(format!("unknown kind {other}").into()),
    }
}

/// Write `s` (with `content` as edited) and mark it accepted.
pub fn accept(db: &Db, id: i64, content: Option<&str>) -> Result<PathBuf> {
    let mut s = db.suggestion(id)?.ok_or("no such suggestion")?;
    if s.status == "accepted" {
        return Err(format!("already added to {}", s.target).into());
    }
    if let Some(c) = content {
        s.content = c.trim().to_string();
    }
    let path = target(&s)?;
    if s.kind == "skill" {
        if path.exists() {
            return Err(format!("{} already exists", path.display()).into());
        }
        if !s.content.starts_with("---") {
            return Err("a skill needs frontmatter (--- name/description ---) at the top".into());
        }
        std::fs::create_dir_all(path.parent().ok_or("bad path")?)?;
        std::fs::write(&path, format!("{}\n", s.content))?;
    } else {
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        if !text.contains(HEADING) {
            if !text.is_empty() && !text.ends_with("\n\n") {
                text.push_str(if text.ends_with('\n') { "\n" } else { "\n\n" });
            }
            text.push_str(HEADING);
            text.push_str("\n\n");
        } else if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&s.content);
        text.push('\n');
        std::fs::write(&path, text)?;
    }
    db.set_suggestion(id, "accepted", &s.content, &path.to_string_lossy())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_skip_lookups() {
        assert_eq!(programs("cd ~/x && cargo build --release 2>&1 | tail -1"), ["cargo build"]);
        assert_eq!(programs("bun run dev; git push origin main"), ["bun run dev", "git push origin"]);
        assert!(programs("ls -la | grep foo").is_empty());
        assert!(programs("for r in rows; do echo $r; done").is_empty());
    }

    #[test]
    fn targets() {
        let mut s = Suggestion {
            id: 0,
            created: String::new(),
            kind: "skill".into(),
            scope: "global".into(),
            project: String::new(),
            title: "t".into(),
            why: String::new(),
            evidence: Vec::new(),
            content: String::new(),
            skill_name: "deploy-prod".into(),
            status: "new".into(),
            target: String::new(),
        };
        assert!(target(&s).unwrap().ends_with("skills/deploy-prod/SKILL.md"));
        s.skill_name = "../evil".into();
        assert!(target(&s).is_err());
        s.kind = "rule".into();
        s.scope = "project".into();
        s.project = "/definitely/not/here".into();
        assert!(target(&s).is_err());
    }
}
