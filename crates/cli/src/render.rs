//! Plain-text views, the same for you in the terminal and for Claude
//! through the MCP tools.

use rmind_core::db::{Hit, Message, SessionInfo, UsageReport, local_time};
use rmind_core::forge::Suggestion;
use rmind_core::parse::clip;
use std::fmt::Write;

/// The folder's name, which is usually the project's. Scratch folders
/// named `2026-10-16-<what you asked>-<hash>` keep just what you asked.
pub fn project(cwd: &str) -> String {
    let name = cwd.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(cwd);
    let dated = name.len() > 11 && name.as_bytes()[..10].iter().enumerate().all(|(i, b)| if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() });
    if !dated {
        return name.to_string();
    }
    let rest = &name[11..];
    let rest = match rest.rsplit_once('-') {
        Some((head, hash)) if hash.len() >= 6 && hash.chars().all(|c| c.is_ascii_hexdigit()) => head,
        _ => rest,
    };
    let rest = if rest.is_empty() { "scratch" } else { rest };
    clip(rest, 36)
}

pub fn hits(query: &str, hits: &[Hit]) -> String {
    if hits.is_empty() {
        return format!("Nothing in past sessions matches \"{query}\".");
    }
    let mut out = String::new();
    for (i, h) in hits.iter().enumerate() {
        let _ = writeln!(out, "{}. {} · {} · \"{}\"", i + 1, local_time(&h.ts), project(&h.cwd), h.title);
        let _ = writeln!(out, "   session {} · message {}", h.session, h.uuid);
        let _ = writeln!(out, "   {}: {}\n", h.role, one_line(&h.snippet));
    }
    out
}

pub fn sessions(list: &[SessionInfo]) -> String {
    if list.is_empty() {
        return "No sessions yet.".into();
    }
    let mut out = String::new();
    for s in list {
        let _ = writeln!(out, "{} · {} · \"{}\"{}", local_time(&s.last_ts), project(&s.cwd), s.title, cost(s.cost));
        let _ = writeln!(out, "   session {} · {} prompts · {}", s.id, s.prompts, s.cwd);
        if !s.last_prompt.is_empty() {
            let _ = writeln!(out, "   last asked: {}", one_line(&clip(&s.last_prompt, 160)));
        }
        out.push('\n');
    }
    out
}

/// A window of the conversation around message `around`, or without it an
/// outline: every prompt, so a part can be picked to read in full.
pub fn session(info: &SessionInfo, messages: &[Message], around: Option<&str>, limit: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\"{}\" · {}{}", info.title, info.cwd, cost(info.cost));
    let _ = writeln!(out, "session {} · {} → {}\n", info.id, local_time(&info.first_ts), local_time(&info.last_ts));
    let limit = limit.max(1);
    match around.and_then(|u| messages.iter().position(|m| m.uuid == u)) {
        Some(at) => {
            let start = at.saturating_sub(limit / 3);
            let end = (start + limit).min(messages.len());
            if start > 0 {
                let _ = writeln!(out, "[{start} earlier messages]\n");
            }
            for (i, m) in messages[start..end].iter().enumerate() {
                let max = if start + i == at { 6000 } else { 1500 };
                let _ = writeln!(out, "[{} · {} · {}]\n{}\n", m.role, local_time(&m.ts), m.uuid, clip(&m.text, max));
            }
            if end < messages.len() {
                let _ = writeln!(out, "[{} later messages]", messages.len() - end);
            }
        }
        None => {
            if around.is_some() {
                out.push_str("(That message isn't in this session; here's the outline.)\n\n");
            }
            let outline: Vec<_> = messages.iter().filter(|m| m.role == "user" || m.role == "summary").collect();
            for m in outline.iter().take(limit) {
                let _ = writeln!(out, "[{} · {} · {}] {}", m.role, local_time(&m.ts), m.uuid, one_line(&clip(&m.text, 400)));
            }
            if outline.len() > limit {
                let _ = writeln!(out, "[{} more prompts]", outline.len() - limit);
            }
            out.push_str("\nPass one of these message ids as `around` to read that part in full.");
        }
    }
    out
}

pub fn usage(r: &UsageReport, days: u32) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Last {days} days (since {}): ${:.2} over {} sessions", r.since, r.cost, r.sessions);
    out.push_str("(at API prices, subagents included; on a subscription it's what the work would have cost)\n\n");
    if !r.days.is_empty() {
        let max = r.days.iter().map(|d| d.1).fold(0.0, f64::max).max(0.01);
        out.push_str("By day\n");
        for (day, c) in &r.days {
            let bar = "█".repeat(((c / max) * 30.0).round() as usize);
            let _ = writeln!(out, "  {day}  {bar} ${c:.2}");
        }
        out.push('\n');
    }
    if !r.projects.is_empty() {
        out.push_str("By project\n");
        for (cwd, c, n) in r.projects.iter().take(12) {
            let _ = writeln!(out, "  ${c:>8.2}  {:<40} {n} sessions", project(cwd));
        }
        out.push('\n');
    }
    if !r.models.is_empty() {
        out.push_str("By model                      cost    output     input   cache read  cache write  cache hit\n");
        for (model, t) in &r.models {
            let fresh = t.input + t.cache_write;
            let hit = if fresh + t.cache_read > 0 { t.cache_read as f64 / (fresh + t.cache_read) as f64 * 100.0 } else { 0.0 };
            let _ = writeln!(
                out,
                "  {model:<24} {:>9} {:>8} {:>9} {:>12} {:>12} {hit:>9.0}%",
                format!("${:.2}", t.cost),
                big(t.output),
                big(t.input),
                big(t.cache_read),
                big(t.cache_write)
            );
        }
        out.push('\n');
    }
    if !r.top.is_empty() {
        out.push_str("Priciest sessions\n");
        for s in &r.top {
            let _ = writeln!(out, "  ${:>7.2}  {} · \"{}\" ({} prompts)", s.cost.unwrap_or(0.0), project(&s.cwd), s.title, s.prompts);
        }
    }
    out
}

pub fn suggestions(list: &[Suggestion]) -> String {
    if list.is_empty() {
        return "No open suggestions. Run `rmind forge` to make some.\n".into();
    }
    let mut out = String::new();
    for s in list {
        let place = if s.scope == "project" { s.project.as_str() } else { "global" };
        let _ = writeln!(out, "#{} {} · {} · {}", s.id, s.kind, place, s.title);
        let _ = writeln!(out, "   why: {}", one_line(&s.why));
        for e in s.evidence.iter().take(3) {
            let _ = writeln!(out, "   > {}", one_line(&clip(e, 160)));
        }
        for line in s.content.lines().take(12) {
            let _ = writeln!(out, "   | {line}");
        }
        out.push('\n');
    }
    out.push_str("Accept with `rmind accept <id>`, drop with `rmind dismiss <id>`, or review in `rmind ui`.\n");
    out
}

fn cost(c: Option<f64>) -> String {
    c.map(|c| format!(" · ${c:.2}")).unwrap_or_default()
}

fn big(n: i64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}K", n as f64 / 1e3),
        n => n.to_string(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_names() {
        assert_eq!(super::project("/Users/lucy/Main/2.Project/rgrip"), "rgrip");
        assert_eq!(super::project("/Users/lucy/.t3/scratch/2026-10-17-my-disk-seem-to-low-2c776614"), "my-disk-seem-to-low");
        assert_eq!(super::project("/x/scratch-2026-10-05-a9e430"), "scratch-2026-10-05-a9e430");
    }
}
