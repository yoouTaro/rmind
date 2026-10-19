//! Reading Claude Code's session logs: one JSON object per line, in
//! `~/.claude/projects/<folder>/<session>.jsonl`, with subagents' logs in
//! `<session>/subagents/agent-*.jsonl`.
//!
//! Only what's worth searching is kept: what you asked, what Claude said,
//! the commands it ran, the files it changed and the errors it hit. Tool
//! output, thinking and images are skipped.

use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A command Claude ran, or a file it changed.
    Tool,
    /// A tool call that failed.
    Error,
    /// The summary written when a long session was compacted.
    Summary,
    /// Claude thinking it through.
    Thinking,
}

impl Role {
    pub fn key(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::Error => "error",
            Role::Summary => "summary",
            Role::Thinking => "thinking",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub uuid: String,
    pub ts: String,
    pub role: Role,
    pub text: String,
}

/// Tokens one model used in one API response.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub ts: String,
    pub model: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

#[derive(Debug, Default)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub entries: Vec<Entry>,
    pub usage: Vec<Usage>,
    pub first_ts: String,
    pub last_ts: String,
    /// Folders of the files Claude changed, which may be outside `cwd`.
    pub places: BTreeSet<String>,
}

impl Session {
    /// The AI title, or else the first thing you asked.
    pub fn title(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        self.entries
            .iter()
            .find(|e| e.role == Role::User)
            .map(|e| clip(e.text.lines().next().unwrap_or(""), 80))
            .unwrap_or_else(|| "(untitled)".into())
    }
}

/// Longest text kept per entry.
const MAX_TEXT: usize = 6000;
const MAX_SUMMARY: usize = 60_000;
const MAX_ERROR: usize = 1500;

pub fn read(path: &Path) -> std::io::Result<Session> {
    let file = std::fs::File::open(path)?;
    let mut session = Session { id: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), ..Default::default() };
    let mut seen_responses = HashSet::new();
    for line in BufReader::new(file).split(b'\n') {
        let line = line?;
        // Skip the big lines we never need (cached request bodies, ...)
        // without parsing them.
        if !wanted(&line) {
            continue;
        }
        // The last line of a session that's still running may be half written.
        let Ok(record) = serde_json::from_slice::<Value>(&line) else { continue };
        add(&mut session, &record, &mut seen_responses);
    }
    Ok(session)
}

fn wanted(line: &[u8]) -> bool {
    const KINDS: [&[u8]; 3] = [b"\"type\":\"user\"", b"\"type\":\"assistant\"", b"\"type\":\"ai-title\""];
    KINDS.iter().any(|k| line.windows(k.len()).any(|w| w == *k))
}

fn add(session: &mut Session, record: &Value, seen_responses: &mut HashSet<String>) {
    match record["type"].as_str() {
        Some("ai-title") => {
            if let Some(t) = record["aiTitle"].as_str() {
                session.title = Some(t.to_string());
            }
            return;
        }
        Some("user" | "assistant") => {}
        _ => return,
    }
    let ts = record["timestamp"].as_str().unwrap_or_default().to_string();
    if !ts.is_empty() {
        if session.first_ts.is_empty() {
            session.first_ts = ts.clone();
        }
        session.last_ts = ts.clone();
    }
    if session.cwd.is_empty() {
        if let Some(cwd) = record["cwd"].as_str() {
            session.cwd = cwd.to_string();
        }
    }
    let uuid = record["uuid"].as_str().unwrap_or_default().to_string();
    let message = &record["message"];
    let entries = &mut session.entries;
    let mut push = |role: Role, text: String| {
        let text = text.trim();
        if !text.is_empty() {
            let max = if role == Role::Summary { MAX_SUMMARY } else { MAX_TEXT };
            entries.push(Entry { uuid: uuid.clone(), ts: ts.clone(), role, text: clip(text, max) });
        }
    };

    if record["type"] == "user" {
        if record["isMeta"] == true {
            return;
        }
        if record["isCompactSummary"] == true {
            push(Role::Summary, text_of(&message["content"]));
            return;
        }
        match &message["content"] {
            Value::String(s) => push(Role::User, prompt(s)),
            Value::Array(blocks) => {
                let mut said = String::new();
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => {
                            said.push_str(&prompt(b["text"].as_str().unwrap_or_default()));
                            said.push('\n');
                        }
                        Some("tool_result") if b["is_error"] == true => {
                            push(Role::Error, clip(&text_of(&b["content"]), MAX_ERROR));
                        }
                        _ => {}
                    }
                }
                push(Role::User, said);
            }
            _ => {}
        }
        return;
    }

    // Assistant: one record per content block, the usage repeated on each.
    let id = message["id"].as_str().unwrap_or_default();
    let model = message["model"].as_str().unwrap_or_default();
    if !id.is_empty() && model != "<synthetic>" && seen_responses.insert(id.to_string()) {
        let u = &message["usage"];
        let n = |k: &str| u[k].as_i64().unwrap_or(0);
        session.usage.push(Usage {
            ts: ts.clone(),
            model: model.to_string(),
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        });
    }
    let Value::Array(blocks) = &message["content"] else { return };
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => push(Role::Assistant, b["text"].as_str().unwrap_or_default().to_string()),
            Some("thinking") => push(Role::Thinking, b["thinking"].as_str().unwrap_or_default().to_string()),
            Some("tool_use") => {
                let input = &b["input"];
                if let Some(t) = tool(b["name"].as_str().unwrap_or_default(), input) {
                    push(Role::Tool, t);
                }
                if let Some(dir) = ["file_path", "notebook_path"]
                    .iter()
                    .find_map(|k| input[*k].as_str())
                    .and_then(|f| Path::new(f).parent())
                    .filter(|d| d.is_absolute())
                {
                    session.places.insert(dir.to_string_lossy().into_owned());
                }
            }
            _ => {}
        }
    }
}

/// A one-line note of a tool call worth remembering, or None for lookups
/// (reads, greps, ...).
fn tool(name: &str, input: &Value) -> Option<String> {
    let s = |k: &str| input[k].as_str().unwrap_or_default();
    Some(match name {
        "Bash" => {
            let desc = s("description");
            if desc.is_empty() { format!("$ {}", s("command")) } else { format!("$ {}  # {desc}", s("command")) }
        }
        "Edit" | "MultiEdit" => format!("edited {}", s("file_path")),
        "Write" => format!("wrote {}", s("file_path")),
        "NotebookEdit" => format!("edited {}", s("notebook_path")),
        "Agent" | "Task" => format!("agent: {}", s("description")),
        "WebSearch" => format!("searched the web: {}", s("query")),
        "WebFetch" => format!("fetched {}", s("url")),
        "Skill" => format!("used skill {}", s("skill")),
        _ => return None,
    })
}

/// Plain text of a message's content, whether a string or blocks.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// What you typed, without the context Claude Code adds around it.
/// Slash commands become `/name args`; notes about local commands and
/// their output are dropped.
fn prompt(text: &str) -> String {
    let text = strip_tag(text, "system-reminder");
    if text.contains("<local-command-caveat>") || text.contains("<local-command-stdout>") {
        return String::new();
    }
    if let Some(name) = between(&text, "<command-name>", "</command-name>") {
        let args = between(&text, "<command-args>", "</command-args>").unwrap_or_default();
        return format!("{name} {args}").trim().to_string();
    }
    text
}

fn strip_tag(text: &str, tag: &str) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(&close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(text[start..end].trim())
}

/// At most `max` bytes, cut on a character boundary.
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(lines: &[&str]) -> Session {
        let dir = std::env::temp_dir().join(format!("rmind-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.jsonl", lines.len()));
        std::fs::write(&path, lines.join("\n")).unwrap();
        let s = read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        s
    }

    #[test]
    fn keeps_prompts_replies_and_commands() {
        let s = session(&[
            r#"{"type":"user","uuid":"u1","timestamp":"2026-10-01T01:00:00Z","cwd":"/p/rpet","message":{"role":"user","content":"fix the RefCell crash<system-reminder>noise</system-reminder>"}}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-10-01T01:00:05Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"Split step into advance()."}],"usage":{"input_tokens":3,"output_tokens":40,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#,
            r#"{"type":"assistant","uuid":"a2","timestamp":"2026-10-01T01:00:06Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test","description":"Run tests"}}],"usage":{"input_tokens":3,"output_tokens":40}}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2026-10-01T01:00:09Z","message":{"role":"user","content":[{"type":"tool_result","is_error":true,"content":"error[E0499]: cannot borrow"}]}}"#,
            r#"{"type":"user","isMeta":true,"uuid":"u3","message":{"role":"user","content":"hidden"}}"#,
            r#"{"type":"api-request-blob","message":"huge"}"#,
            r#"{"type":"ai-title","aiTitle":"Fix RefCell crash"}"#,
            r#"{"type":"user","uuid":"u4","timestamp":"2026-10-01T01:01"#,
        ]);
        let got: Vec<_> = s.entries.iter().map(|e| (e.role, e.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Role::User, "fix the RefCell crash"),
                (Role::Assistant, "Split step into advance()."),
                (Role::Tool, "$ cargo test  # Run tests"),
                (Role::Error, "error[E0499]: cannot borrow"),
            ]
        );
        assert_eq!(s.usage.len(), 1, "one response, counted once");
        assert_eq!(s.usage[0].cache_read, 1000);
        assert_eq!(s.title(), "Fix RefCell crash");
        assert_eq!(s.cwd, "/p/rpet");
        assert_eq!(s.last_ts, "2026-10-01T01:00:09Z");
    }

    #[test]
    fn slash_commands_and_local_output() {
        assert_eq!(prompt("<command-name>/compact</command-name>\n<command-args></command-args>"), "/compact");
        assert_eq!(prompt("<local-command-stdout>Compacted</local-command-stdout>"), "");
        assert_eq!(strip_tag("a<x>b</x>c<x>d", "x"), "ac");
    }

    #[test]
    fn clips_on_char_boundaries() {
        assert_eq!(clip("ស្រី", 4), "ស…");
        assert_eq!(clip("abc", 3), "abc");
    }
}
