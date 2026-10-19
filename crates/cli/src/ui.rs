//! `rmind ui`: the rmind window, a page served from this binary on
//! 127.0.0.1 and opened in your browser. The page talks to a small JSON API.
//!
//! The API can write to your CLAUDE.md files (accepting Forge
//! suggestions), so it only answers requests that name this machine as the
//! host (no DNS rebinding) and carry a token that's only in the page itself
//! (no other website can read it).

use crate::render::project;
use rmind_core::db::{Db, Message, SessionInfo, local_time};
use rmind_core::forge::{self, Suggestion};
use rmind_core::parse::clip;
use rmind_core::paths;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

const PAGE: &str = include_str!("ui/index.html");

/// The page's own fonts (Geist, SIL Open Font License, see
/// ui/fonts/OFL-Geist.txt) and icon, served from the binary so the window
/// needs no network.
const ASSETS: [(&str, &str, &[u8]); 3] = [
    ("/fonts/Geist.ttf", "font/ttf", include_bytes!("ui/fonts/Geist.ttf")),
    ("/fonts/GeistMono.ttf", "font/ttf", include_bytes!("ui/fonts/GeistMono.ttf")),
    ("/favicon.svg", "image/svg+xml", include_bytes!("ui/favicon.svg")),
];

struct Forging {
    running: AtomicBool,
    /// The mode running, or that ran last.
    mode: Mutex<String>,
    error: Mutex<Option<String>>,
}

pub fn serve(port: u16, open: bool) -> crate::Result<()> {
    let (server, port) = (port..port + 20)
        .find_map(|p| Server::http(("127.0.0.1", p)).ok().map(|s| (s, p)))
        .ok_or("no free port")?;
    let token = token();
    let url = format!("http://127.0.0.1:{port}/");
    let mut db = Db::open_default()?;
    let forging = Arc::new(Forging { running: AtomicBool::new(false), mode: Mutex::new(String::new()), error: Mutex::new(None) });
    println!("rmind is open at {url} (Ctrl-C to quit)");
    if open {
        let _ = std::process::Command::new("open").arg(&url).status();
    }
    let page = PAGE.replace("__RMIND_TOKEN__", &token);
    for mut request in server.incoming_requests() {
        let response = if !local_host(&request, port) {
            error(403, "wrong host")
        } else if request.url() == "/" || request.url().starts_with("/?") {
            Response::from_string(page.clone())
                .with_header(header("Content-Type", "text/html; charset=utf-8"))
                .with_header(header("Content-Security-Policy", "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'"))
                .with_status_code(200)
        } else if let Some((_, kind, bytes)) = ASSETS.iter().find(|(path, _, _)| *path == request.url()) {
            Response::from_data(bytes.to_vec())
                .with_header(header("Content-Type", kind))
                .with_header(header("Cache-Control", "max-age=31536000, immutable"))
                .with_status_code(200)
        } else if request.url().starts_with("/api/") {
            if header_value(&request, "X-Rmind-Token") != Some(token.as_str()) {
                error(403, "missing token")
            } else {
                match api(&mut db, &mut request, &forging) {
                    Ok(v) => json_response(200, &v),
                    Err(e) => error(400, &e.to_string()),
                }
            }
        } else {
            error(404, "not found")
        };
        let _ = request.respond(response);
    }
    Ok(())
}

fn api(db: &mut Db, request: &mut Request, forging: &Arc<Forging>) -> crate::Result<Value> {
    let (path, query) = split_url(request.url());
    let q = |k: &str| query.get(k).map(String::as_str).filter(|s| !s.is_empty());
    let n = |k: &str, default: usize| q(k).and_then(|v| v.parse().ok()).unwrap_or(default);
    let method = request.method().clone();
    let body: Value = if method == Method::Post {
        let mut text = String::new();
        request.as_reader().take(1 << 20).read_to_string(&mut text)?;
        serde_json::from_str(&text).unwrap_or_default()
    } else {
        Value::Null
    };
    if method == Method::Get && path != "/api/forge" {
        db.update(&paths::projects_dir(), |_, _| {})?;
    }
    Ok(match (method, path.as_str()) {
        (Method::Get, "/api/info") => {
            let (sessions, messages) = db.counts()?;
            let size = std::fs::metadata(paths::db_path()).map(|m| m.len()).unwrap_or(0);
            json!({"sessions": sessions, "messages": messages, "bytes": size, "version": env!("CARGO_PKG_VERSION")})
        }
        (Method::Get, "/api/search") => {
            let hits = db.search(q("q").unwrap_or_default(), q("project"), n("limit", 30).min(100))?;
            json!({"hits": hits.iter().map(|h| json!({
                "session": h.session, "title": h.title, "project": project(&h.cwd), "cwd": h.cwd,
                "uuid": h.uuid, "time": local_time(&h.ts), "role": h.role, "snippet": h.snippet,
            })).collect::<Vec<_>>()})
        }
        (Method::Get, "/api/recent") => {
            let list = db.recent(q("project"), None, n("limit", 60).min(500))?;
            json!({"sessions": list.iter().map(session_json).collect::<Vec<_>>()})
        }
        (Method::Get, "/api/session") => {
            let id = q("id").ok_or("id is required")?;
            let info = db.session(id)?.ok_or("no such session")?;
            let messages = db.messages(&info.id)?;
            if q("outline").is_some() {
                // Every prompt, so a part can be picked to read.
                let outline: Vec<_> = messages
                    .iter()
                    .filter(|m| m.role == "user" || m.role == "summary")
                    .map(|m| json!({"uuid": m.uuid, "time": local_time(&m.ts), "role": m.role, "text": clip(&m.text, 400)}))
                    .collect();
                json!({"session": session_json(&info), "total": messages.len(), "outline": outline})
            } else {
                let (shown, start, focus) = window(&messages, q("around"), q("from").and_then(|v| v.parse().ok()), n("before", 15), n("limit", 40));
                json!({
                    "session": session_json(&info),
                    "total": messages.len(),
                    "start": start,
                    "focus": focus,
                    "messages": shown.iter().map(message_json).collect::<Vec<_>>(),
                })
            }
        }
        (Method::Get, "/api/usage") => {
            let days = n("days", 30) as u32;
            let r = db.usage(days)?;
            json!({
                "since": r.since, "cost": r.cost, "sessions": r.sessions,
                "days": r.days.iter().map(|(d, c)| json!({"day": d, "cost": c})).collect::<Vec<_>>(),
                "projects": r.projects.iter().map(|(cwd, c, n)| json!({"project": project(cwd), "cwd": cwd, "cost": c, "sessions": n})).collect::<Vec<_>>(),
                "models": r.models.iter().map(|(m, t)| json!({
                    "model": m, "cost": t.cost, "input": t.input, "output": t.output, "cache_read": t.cache_read, "cache_write": t.cache_write,
                })).collect::<Vec<_>>(),
                "top": r.top.iter().map(session_json).collect::<Vec<_>>(),
                "agents": r.agents.iter().map(|a| json!({
                    "type": a.agent_type, "runs": a.runs, "cost": a.cost,
                    "models": a.models.iter().map(|(m, c)| json!({"model": m, "cost": c})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        }
        (Method::Get, "/api/forge") => {
            let mode = q("mode").unwrap_or("habits");
            let run = db.last_run(mode)?;
            let agents_dir = paths::claude_dir().join("agents");
            let this_mode = *forging.mode.lock().unwrap() == mode;
            json!({
                "running": forging.running.load(Ordering::SeqCst),
                "running_mode": forging.mode.lock().unwrap().clone(),
                "error": if this_mode { forging.error.lock().unwrap().clone() } else { None },
                "last_run": run.map(|r| json!({"time": local_time(&r.ts), "suggestions": r.suggestions, "cost": r.cost, "error": r.error})),
                "suggestions": db.suggestions_of(Some(mode), None)?.iter().map(|s| {
                    let mut v = suggestion_json(s);
                    v["replaces"] = json!(s.kind == "agent" && s.status == "new" && agents_dir.join(format!("{}.md", s.skill_name)).exists());
                    v
                }).collect::<Vec<_>>(),
            })
        }
        (Method::Post, "/api/forge/run") => {
            if forging.running.swap(true, Ordering::SeqCst) {
                return Err("Forge is already running".into());
            }
            *forging.error.lock().unwrap() = None;
            let forging = Arc::clone(forging);
            let mode = if body["mode"] == "team" { forge::Mode::Team } else { forge::Mode::Habits };
            *forging.mode.lock().unwrap() = mode.key().into();
            std::thread::spawn(move || {
                let result = Db::open_default().and_then(|db| forge::run(&db, mode));
                if let Err(e) = result {
                    *forging.error.lock().unwrap() = Some(e.to_string());
                }
                forging.running.store(false, Ordering::SeqCst);
            });
            json!({"started": true})
        }
        (Method::Get, "/api/team") => {
            let uses = db.agent_use(None)?;
            let agents = forge::agents();
            let defined: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
            json!({
                "agents": agents.iter().map(|a| {
                    let u = uses.iter().find(|u| u.agent_type == a.name);
                    json!({
                        "name": a.name, "description": a.description, "model": a.model, "effort": a.effort, "tools": a.tools,
                        "path": a.path.to_string_lossy(), "locked": a.locked,
                        "runs": u.map_or(0, |u| u.runs), "cost": u.map_or(0.0, |u| u.cost),
                    })
                }).collect::<Vec<_>>(),
                "others": uses.iter().filter(|u| !defined.contains(&u.agent_type.as_str())).map(|u| json!({
                    "type": u.agent_type, "runs": u.runs, "cost": u.cost,
                })).collect::<Vec<_>>(),
            })
        }
        (Method::Post, "/api/forge/accept") => {
            let id = body["id"].as_i64().ok_or("id is required")?;
            let path = forge::accept(db, id, body["content"].as_str())?;
            json!({"target": path.to_string_lossy()})
        }
        (Method::Post, "/api/forge/dismiss" | "/api/forge/restore") => {
            let id = body["id"].as_i64().ok_or("id is required")?;
            let s = db.suggestion(id)?.ok_or("no such suggestion")?;
            if s.status == "accepted" {
                return Err("already written; edit the file to change it".into());
            }
            let status = if path.ends_with("dismiss") { "dismissed" } else { "new" };
            db.set_suggestion(id, status, &s.content, "")?;
            json!({"ok": true})
        }
        _ => return Err(format!("no such endpoint {path}").into()),
    })
}

/// Up to `limit` messages: from `before` ahead of message `around`, else
/// from index `from`. Also returns the start index and where `around` is.
fn window<'a>(
    messages: &'a [Message],
    around: Option<&str>,
    from: Option<usize>,
    before: usize,
    limit: usize,
) -> (&'a [Message], usize, Option<usize>) {
    let at = around.and_then(|u| messages.iter().position(|m| m.uuid == u));
    let start = at.map_or(from.unwrap_or(0), |i| i.saturating_sub(before)).min(messages.len());
    let end = (start + limit.clamp(1, 400)).min(messages.len());
    (&messages[start..end], start, at.map(|i| i - start))
}

fn session_json(s: &SessionInfo) -> Value {
    json!({
        "id": s.id, "title": s.title, "project": project(&s.cwd), "cwd": s.cwd,
        "first": local_time(&s.first_ts), "last": local_time(&s.last_ts),
        "prompts": s.prompts, "last_prompt": s.last_prompt, "cost": s.cost,
    })
}

fn message_json(m: &Message) -> Value {
    json!({"uuid": m.uuid, "time": local_time(&m.ts), "role": m.role, "text": m.text})
}

fn suggestion_json(s: &Suggestion) -> Value {
    let target = forge::target(s).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|e| format!("({e})"));
    json!({
        "id": s.id, "created": local_time(&s.created), "kind": s.kind, "scope": s.scope, "project": s.project,
        "title": s.title, "why": s.why, "evidence": s.evidence, "content": s.content, "skill_name": s.skill_name,
        "status": s.status, "target": if s.target.is_empty() { target } else { s.target.clone() },
    })
}

fn split_url(url: &str) -> (String, HashMap<String, String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let params = query
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            Some((decode(k), decode(v))).filter(|(k, _)| !k.is_empty())
        })
        .collect();
    (path.to_string(), params)
}

/// Percent-decoding, with `+` as a space.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(b) => {
                    out.push(b);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn local_host(request: &Request, port: u16) -> bool {
    header_value(request, "Host").is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"))
}

fn header_value<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn json_response(status: u16, v: &Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string()).with_header(header("Content-Type", "application/json")).with_status_code(status)
}

fn error(status: u16, message: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(status, &json!({"error": message}))
}

/// 128 random bits from the OS, as hex.
fn token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let (path, q) = split_url("/api/search?q=khmer+font%20x&project=rg%2Fx&bad=%zz");
        assert_eq!(path, "/api/search");
        assert_eq!(q["q"], "khmer font x");
        assert_eq!(q["project"], "rg/x");
        assert_eq!(q["bad"], "%zz");
        assert_eq!(decode("%"), "%");
        assert_eq!(decode("ab%4"), "ab%4");
        assert_eq!(decode("%E1%9E%9F"), "ស");
    }
}
