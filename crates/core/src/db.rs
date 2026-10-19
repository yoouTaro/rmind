//! The index: one SQLite file with every session's text (full-text
//! searchable) and token usage. Updating it only re-reads logs whose size
//! or modified time changed, so it's cheap to call before every search.
//!
//! Sessions stay in the index after Claude Code deletes their logs (it
//! cleans up old ones after a while), so your history only grows.

use crate::forge::{Run, Suggestion};
use crate::parse::{self, Role};
use crate::price;
use chrono::{DateTime, Local};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Bump when the schema or what gets indexed changes; the index is rebuilt.
const VERSION: i64 = 6;

const SCHEMA: &str = "
CREATE TABLE files (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, size INTEGER NOT NULL);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY, cwd TEXT NOT NULL, title TEXT NOT NULL,
    first_ts TEXT NOT NULL, last_ts TEXT NOT NULL, day TEXT NOT NULL,
    prompts INTEGER NOT NULL, last_prompt TEXT NOT NULL,
    -- The cwd and the folders of files Claude changed, one per line.
    places TEXT NOT NULL
);
CREATE INDEX sessions_last ON sessions (last_ts);
CREATE TABLE msgs (id INTEGER PRIMARY KEY, session TEXT NOT NULL, uuid TEXT NOT NULL, ts TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL);
CREATE INDEX msgs_session ON msgs (session);
CREATE VIRTUAL TABLE msgs_fts USING fts5 (text, content = 'msgs', content_rowid = 'id', tokenize = \"porter unicode61 tokenchars '_'\");
CREATE TRIGGER msgs_ai AFTER INSERT ON msgs BEGIN
    INSERT INTO msgs_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER msgs_ad AFTER DELETE ON msgs BEGIN
    INSERT INTO msgs_fts (msgs_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TABLE usage (
    file TEXT NOT NULL, session TEXT NOT NULL, day TEXT NOT NULL, model TEXT NOT NULL,
    input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL,
    -- USD at API prices; see price.rs.
    cost REAL NOT NULL
);
CREATE INDEX usage_file ON usage (file);
CREATE TABLE agents (
    file TEXT PRIMARY KEY, session TEXT NOT NULL, day TEXT NOT NULL, agent_type TEXT NOT NULL, description TEXT NOT NULL
);
CREATE INDEX usage_session ON usage (session);
CREATE INDEX usage_day ON usage (day);
";

/// Forge's suggestions are yours, so they live outside the index proper and
/// survive rebuilds.
const FORGE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS forge (
    id INTEGER PRIMARY KEY, created TEXT NOT NULL, kind TEXT NOT NULL, scope TEXT NOT NULL, project TEXT NOT NULL,
    title TEXT NOT NULL, why TEXT NOT NULL, evidence TEXT NOT NULL, content TEXT NOT NULL, skill_name TEXT NOT NULL,
    status TEXT NOT NULL, target TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS forge_runs (ts TEXT NOT NULL, suggestions INTEGER NOT NULL, cost REAL NOT NULL, error TEXT);
";

pub struct Db {
    conn: Connection,
}

/// One indexed message with its session's folder, for mining.
pub struct TextRow {
    pub session: String,
    pub uuid: String,
    pub cwd: String,
    pub ts: String,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct IndexReport {
    pub files: usize,
    pub changed: usize,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub session: String,
    pub title: String,
    pub cwd: String,
    pub uuid: String,
    pub ts: String,
    pub role: String,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub first_ts: String,
    pub last_ts: String,
    pub prompts: i64,
    pub last_prompt: String,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub uuid: String,
    pub ts: String,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost: f64,
}

#[derive(Debug, Default)]
pub struct UsageReport {
    pub since: String,
    pub cost: f64,
    pub sessions: i64,
    /// (folder, cost, sessions), most expensive first.
    pub projects: Vec<(String, f64, i64)>,
    /// (model, tokens), most expensive first.
    pub models: Vec<(String, Tokens)>,
    /// (day, cost), oldest first.
    pub days: Vec<(String, f64)>,
    /// The priciest sessions.
    pub top: Vec<SessionInfo>,
    pub agents: Vec<AgentUse>,
}

/// How one kind of subagent was used.
#[derive(Debug, Clone)]
pub struct AgentUse {
    pub agent_type: String,
    pub runs: i64,
    pub cost: f64,
    /// Models it ran on, most expensive first.
    pub models: Vec<(String, f64)>,
}

/// One subagent run, for Forge.
#[derive(Debug, Clone)]
pub struct AgentRun {
    pub day: String,
    pub agent_type: String,
    pub description: String,
    pub model: String,
    pub cost: f64,
    pub cwd: String,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        // Several Claude sessions (each with its own rmind) and the hooks can
        // use the index at once.
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != VERSION {
            conn.execute_batch(
                "DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS sessions; DROP TABLE IF EXISTS msgs_fts;
                 DROP TABLE IF EXISTS msgs; DROP TABLE IF EXISTS usage; DROP TABLE IF EXISTS agents;",
            )?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", VERSION)?;
        }
        conn.execute_batch(FORGE_SCHEMA)?;
        Ok(Db { conn })
    }

    pub fn open_default() -> Result<Db> {
        Db::open(&crate::paths::db_path())
    }

    /// Read every log that's new or changed since last time. `progress`
    /// gets (done, to do) as it goes.
    pub fn update(&mut self, root: &Path, mut progress: impl FnMut(usize, usize)) -> Result<IndexReport> {
        let logs = find_logs(root);
        let known: HashMap<String, (i64, i64)> = {
            let mut stmt = self.conn.prepare("SELECT path, mtime, size FROM files")?;
            stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<rusqlite::Result<_>>()?
        };
        let changed: Vec<&Log> = logs.iter().filter(|l| known.get(&l.key()) != Some(&(l.mtime, l.size))).collect();
        for (i, log) in changed.iter().enumerate() {
            progress(i, changed.len());
            // A bad file shouldn't stop the rest from being indexed.
            if let Err(e) = self.add(log) {
                eprintln!("rmind: skipped {}: {e}", log.path.display());
            }
        }
        progress(changed.len(), changed.len());
        Ok(IndexReport { files: logs.len(), changed: changed.len() })
    }

    fn add(&mut self, log: &Log) -> Result<()> {
        let session = parse::read(&log.path)?;
        let key = log.key();
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Someone else may have indexed it while we were reading.
        let now: Option<(i64, i64)> =
            tx.query_row("SELECT mtime, size FROM files WHERE path = ?1", [&key], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        if now == Some((log.mtime, log.size)) {
            return Ok(());
        }
        tx.execute("DELETE FROM usage WHERE file = ?1", [&key])?;
        {
            let mut usage = tx.prepare(
                "INSERT INTO usage (file, session, day, model, input, output, cache_read, cache_write, cost)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for u in &session.usage {
                let cost = price::cost(&u.model, u.input, u.output, u.cache_read, u.cache_write);
                usage.execute(params![key, log.session, day(&u.ts), u.model, u.input, u.output, u.cache_read, u.cache_write, cost])?;
            }
        }
        if log.subagent {
            // What the subagent was and was asked, from the file beside its log.
            let meta: serde_json::Value = std::fs::read(log.path.with_extension("meta.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            tx.execute(
                "INSERT OR REPLACE INTO agents (file, session, day, agent_type, description) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key,
                    log.session,
                    day(&session.first_ts),
                    // Older Claude Code versions didn't write the meta file.
                    meta["agentType"].as_str().unwrap_or("unlabelled"),
                    meta["description"].as_str().map(str::to_string).unwrap_or_else(|| {
                        session.entries.iter().find(|e| e.role == Role::User).map(|e| parse::clip(&e.text, 200)).unwrap_or_default()
                    })
                ],
            )?;
        }
        if !log.subagent && !session.first_ts.is_empty() {
            tx.execute("DELETE FROM msgs WHERE session = ?1", [&log.session])?;
            let mut msg = tx.prepare("INSERT INTO msgs (session, uuid, ts, role, text) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for e in &session.entries {
                msg.execute(params![log.session, e.uuid, e.ts, e.role.key(), e.text])?;
            }
            let prompts: Vec<_> = session.entries.iter().filter(|e| e.role == Role::User).collect();
            let last_prompt = prompts.last().map(|e| parse::clip(&e.text, 300)).unwrap_or_default();
            tx.execute(
                "INSERT OR REPLACE INTO sessions (id, cwd, title, first_ts, last_ts, day, prompts, last_prompt, places)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    log.session,
                    session.cwd,
                    session.title(),
                    session.first_ts,
                    session.last_ts,
                    day(&session.last_ts),
                    prompts.len() as i64,
                    last_prompt,
                    std::iter::once(session.cwd.as_str()).chain(session.places.iter().map(String::as_str)).collect::<Vec<_>>().join("\n")
                ],
            )?;
            drop(msg);
        }
        tx.execute("INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)", params![key, log.mtime, log.size])?;
        tx.commit()?;
        Ok(())
    }

    /// Best matches for `query`, at most two per session. `project` keeps
    /// sessions whose folder, or a folder Claude changed files in, contains it.
    pub fn search(&self, query: &str, project: Option<&str>, limit: usize) -> Result<Vec<Hit>> {
        let words = words(query);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        // All the words if anything has them all, otherwise any of them.
        let all = self.matches(&words.join(" "), project, limit)?;
        if !all.is_empty() || words.len() == 1 {
            return Ok(all);
        }
        self.matches(&words.join(" OR "), project, limit)
    }

    fn matches(&self, fts: &str, project: Option<&str>, limit: usize) -> Result<Vec<Hit>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.session, s.title, s.cwd, m.uuid, m.ts, m.role, snippet(msgs_fts, 0, '«', '»', '…', 40)
             FROM msgs_fts JOIN msgs m ON m.id = msgs_fts.rowid JOIN sessions s ON s.id = m.session
             WHERE msgs_fts MATCH ?1 AND (?2 IS NULL OR s.places LIKE '%' || ?2 || '%')
             -- bm25 is negative, lower is better: favour what was said over
             -- commands and thinking.
             ORDER BY bm25(msgs_fts) * CASE m.role
                 WHEN 'user' THEN 1.3 WHEN 'assistant' THEN 1.2 WHEN 'summary' THEN 1.1
                 WHEN 'thinking' THEN 0.8 WHEN 'tool' THEN 0.6 ELSE 1.0 END
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![fts, project, (limit * 6) as i64], |r| {
            Ok(Hit {
                session: r.get(0)?,
                title: r.get(1)?,
                cwd: r.get(2)?,
                uuid: r.get(3)?,
                ts: r.get(4)?,
                role: r.get(5)?,
                snippet: r.get(6)?,
            })
        })?;
        let mut per_session: HashMap<String, usize> = HashMap::new();
        // Forked and resumed sessions repeat earlier messages under the same id.
        let mut seen = std::collections::HashSet::new();
        let mut hits = Vec::new();
        for hit in rows {
            let hit = hit?;
            if !seen.insert(format!("{}{}", hit.uuid, hit.role)) {
                continue;
            }
            let n = per_session.entry(hit.session.clone()).or_default();
            if *n < 2 {
                *n += 1;
                hits.push(hit);
            }
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }

    /// The session whose id starts with `id`.
    pub fn session(&self, id: &str) -> Result<Option<SessionInfo>> {
        let id = id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare(&format!("{SESSION_COLUMNS} WHERE id LIKE ?1 || '%' ORDER BY last_ts DESC LIMIT 1"))?;
        Ok(stmt.query_row([id], session_info).optional()?)
    }

    pub fn messages(&self, session: &str) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare("SELECT uuid, ts, role, text FROM msgs WHERE session = ?1 ORDER BY id")?;
        let rows = stmt.query_map([session], |r| Ok(Message { uuid: r.get(0)?, ts: r.get(1)?, role: r.get(2)?, text: r.get(3)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Latest sessions first. `project` filters like in `search`; `cwd`
    /// keeps sessions started in exactly that folder.
    pub fn recent(&self, project: Option<&str>, cwd: Option<&str>, limit: usize) -> Result<Vec<SessionInfo>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SESSION_COLUMNS} WHERE (?1 IS NULL OR places LIKE '%' || ?1 || '%') AND (?2 IS NULL OR cwd = ?2)
             ORDER BY last_ts DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![project, cwd, limit as i64], session_info)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Spending and tokens over the last `days` days (local time),
    /// subagents included.
    pub fn usage(&self, days: u32) -> Result<UsageReport> {
        let since = (Local::now() - chrono::Duration::days(i64::from(days.max(1)) - 1)).format("%Y-%m-%d").to_string();
        let (cost, sessions) = self.conn.query_row(
            "SELECT COALESCE(SUM(cost), 0), COUNT(DISTINCT session) FROM usage WHERE day >= ?1",
            [&since],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let projects = self
            .conn
            .prepare(
                "SELECT COALESCE(s.cwd, '?'), SUM(u.cost) c, COUNT(DISTINCT u.session) FROM usage u
                 LEFT JOIN sessions s ON s.id = u.session WHERE u.day >= ?1 GROUP BY s.cwd ORDER BY c DESC",
            )?
            .query_map([&since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let models = self
            .conn
            .prepare(
                "SELECT model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write), SUM(cost) FROM usage WHERE day >= ?1
                 GROUP BY model ORDER BY SUM(cost) DESC",
            )?
            .query_map([&since], |r| {
                Ok((
                    r.get(0)?,
                    Tokens { input: r.get(1)?, output: r.get(2)?, cache_read: r.get(3)?, cache_write: r.get(4)?, cost: r.get(5)? },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let days = self
            .conn
            .prepare("SELECT day, SUM(cost) FROM usage WHERE day >= ?1 GROUP BY day ORDER BY day")?
            .query_map([&since], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let top = self
            .conn
            .prepare(&format!(
                "{SESSION_COLUMNS} WHERE id IN (SELECT session FROM usage WHERE day >= ?1) ORDER BY cost DESC LIMIT 5"
            ))?
            .query_map([&since], session_info)?
            .collect::<rusqlite::Result<_>>()?;
        let agents = self.agent_use(Some(&since))?;
        Ok(UsageReport { since, cost, sessions, projects, models, days, top, agents })
    }
}

impl Db {
    /// Every prompt, reply and command, session by session, in order.
    pub fn all_text(&self) -> Result<Vec<TextRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.session, m.uuid, s.cwd, m.ts, m.role, m.text FROM msgs m JOIN sessions s ON s.id = m.session
             WHERE m.role IN ('user', 'assistant', 'tool') ORDER BY m.session, m.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TextRow { session: r.get(0)?, uuid: r.get(1)?, cwd: r.get(2)?, ts: r.get(3)?, role: r.get(4)?, text: r.get(5)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Folders with at least `min` sessions, busiest first.
    pub fn busy_projects(&self, min: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT cwd FROM sessions GROUP BY cwd HAVING COUNT(*) >= ?1 ORDER BY COUNT(*) DESC")?;
        let rows = stmt.query_map([min], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_suggestion(&self, s: &Suggestion) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO forge (created, kind, scope, project, title, why, evidence, content, skill_name, status, target)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                s.created,
                s.kind,
                s.scope,
                s.project,
                s.title,
                s.why,
                serde_json::to_string(&s.evidence)?,
                s.content,
                s.skill_name,
                s.status,
                s.target
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Suggestions, newest first; `status` keeps only those.
    pub fn suggestions(&self, status: Option<&str>) -> Result<Vec<Suggestion>> {
        let mut stmt = self.conn.prepare(&format!("{FORGE_COLUMNS} WHERE ?1 IS NULL OR status = ?1 ORDER BY id DESC"))?;
        let rows = stmt.query_map([status], suggestion)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn suggestion(&self, id: i64) -> Result<Option<Suggestion>> {
        Ok(self.conn.query_row(&format!("{FORGE_COLUMNS} WHERE id = ?1"), [id], suggestion).optional()?)
    }

    pub fn set_suggestion(&self, id: i64, status: &str, content: &str, target: &str) -> Result<()> {
        self.conn.execute("UPDATE forge SET status = ?2, content = ?3, target = ?4 WHERE id = ?1", params![id, status, content, target])?;
        Ok(())
    }

    pub fn add_run(&self, run: &Run) -> Result<()> {
        self.conn.execute(
            "INSERT INTO forge_runs (ts, suggestions, cost, error) VALUES (?1, ?2, ?3, ?4)",
            params![run.ts, run.suggestions, run.cost, run.error],
        )?;
        Ok(())
    }

    pub fn last_run(&self) -> Result<Option<Run>> {
        Ok(self
            .conn
            .query_row("SELECT ts, suggestions, cost, error FROM forge_runs ORDER BY ts DESC LIMIT 1", [], |r| {
                Ok(Run { ts: r.get(0)?, suggestions: r.get(1)?, cost: r.get(2)?, error: r.get(3)? })
            })
            .optional()?)
    }

    /// Sessions and messages indexed.
    pub fn counts(&self) -> Result<(i64, i64)> {
        Ok(self.conn.query_row("SELECT (SELECT COUNT(*) FROM sessions), (SELECT COUNT(*) FROM msgs)", [], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }
}

impl Db {
    /// Subagent use by type, since `since` (a day) if given.
    pub fn agent_use(&self, since: Option<&str>) -> Result<Vec<AgentUse>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.agent_type, u.model, COUNT(DISTINCT a.file), COALESCE(SUM(u.cost), 0) FROM agents a
             LEFT JOIN usage u ON u.file = a.file WHERE ?1 IS NULL OR a.day >= ?1
             GROUP BY a.agent_type, u.model",
        )?;
        let rows: Vec<(String, Option<String>, i64, f64)> =
            stmt.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut by_type: Vec<AgentUse> = Vec::new();
        for (agent_type, model, runs, cost) in rows {
            let i = match by_type.iter().position(|a| a.agent_type == agent_type) {
                Some(i) => i,
                None => {
                    by_type.push(AgentUse { agent_type, runs: 0, cost: 0.0, models: Vec::new() });
                    by_type.len() - 1
                }
            };
            let a = &mut by_type[i];
            a.runs += runs;
            a.cost += cost;
            if let Some(m) = model {
                a.models.push((m, cost));
            }
        }
        for a in &mut by_type {
            a.models.sort_by(|x, y| y.1.total_cmp(&x.1));
        }
        by_type.sort_by(|x, y| y.cost.total_cmp(&x.cost));
        Ok(by_type)
    }

    /// The latest `limit` subagent runs, newest first.
    pub fn agent_runs(&self, limit: usize) -> Result<Vec<AgentRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.day, a.agent_type, a.description,
                    COALESCE((SELECT model FROM usage WHERE file = a.file GROUP BY model ORDER BY SUM(cost) DESC LIMIT 1), ''),
                    COALESCE((SELECT SUM(cost) FROM usage WHERE file = a.file), 0),
                    COALESCE(s.cwd, '')
             FROM agents a LEFT JOIN sessions s ON s.id = a.session ORDER BY a.day DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(AgentRun { day: r.get(0)?, agent_type: r.get(1)?, description: r.get(2)?, model: r.get(3)?, cost: r.get(4)?, cwd: r.get(5)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const FORGE_COLUMNS: &str = "SELECT id, created, kind, scope, project, title, why, evidence, content, skill_name, status, target FROM forge";

fn suggestion(r: &rusqlite::Row) -> rusqlite::Result<Suggestion> {
    let evidence: String = r.get(7)?;
    Ok(Suggestion {
        id: r.get(0)?,
        created: r.get(1)?,
        kind: r.get(2)?,
        scope: r.get(3)?,
        project: r.get(4)?,
        title: r.get(5)?,
        why: r.get(6)?,
        evidence: serde_json::from_str(&evidence).unwrap_or_default(),
        content: r.get(8)?,
        skill_name: r.get(9)?,
        status: r.get(10)?,
        target: r.get(11)?,
    })
}

const SESSION_COLUMNS: &str = "SELECT id, title, cwd, first_ts, last_ts, prompts, last_prompt,
    (SELECT SUM(cost) FROM usage WHERE usage.session = sessions.id) AS cost FROM sessions";

fn session_info(r: &rusqlite::Row) -> rusqlite::Result<SessionInfo> {
    Ok(SessionInfo {
        id: r.get(0)?,
        title: r.get(1)?,
        cwd: r.get(2)?,
        first_ts: r.get(3)?,
        last_ts: r.get(4)?,
        prompts: r.get(5)?,
        last_prompt: r.get(6)?,
        cost: r.get(7)?,
    })
}

/// The words of a search, each quoted so FTS syntax in them is literal.
fn words(query: &str) -> Vec<String> {
    query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect()
}

/// The local date of an RFC 3339 timestamp.
pub fn day(ts: &str) -> String {
    DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| ts.get(..10).unwrap_or_default().to_string())
}

/// Local "2026-10-17 14:05" for an RFC 3339 timestamp.
pub fn local_time(ts: &str) -> String {
    DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| ts.to_string())
}

struct Log {
    path: PathBuf,
    /// The main session it belongs to.
    session: String,
    subagent: bool,
    mtime: i64,
    size: i64,
}

impl Log {
    fn key(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

/// `<folder>/<session>.jsonl`, `<folder>/<session>/subagents/*.jsonl` and
/// `<folder>/<session>/subagents/workflows/<run>/*.jsonl`.
fn find_logs(root: &Path) -> Vec<Log> {
    let mut logs = Vec::new();
    let Ok(folders) = std::fs::read_dir(root) else { return logs };
    for folder in folders.flatten() {
        let Ok(items) = std::fs::read_dir(folder.path()) else { continue };
        for item in items.flatten() {
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            if let Some(session) = name.strip_suffix(".jsonl") {
                push_log(&mut logs, path.clone(), session.to_string(), false);
            } else if path.is_dir() {
                // Subagents, and the agents of workflows one level down.
                let mut dirs = vec![path.join("subagents")];
                if let Ok(flows) = std::fs::read_dir(path.join("subagents/workflows")) {
                    dirs.extend(flows.flatten().map(|f| f.path()));
                }
                for dir in dirs {
                    let Ok(agents) = std::fs::read_dir(dir) else { continue };
                    for agent in agents.flatten() {
                        if agent.file_name().to_string_lossy().ends_with(".jsonl") {
                            push_log(&mut logs, agent.path(), name.clone(), true);
                        }
                    }
                }
            }
        }
    }
    logs
}

fn push_log(logs: &mut Vec<Log>, path: PathBuf, session: String, subagent: bool) {
    let Ok(meta) = std::fs::metadata(&path) else { return };
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64);
    logs.push(Log { path, session, subagent, mtime, size: meta.len() as i64 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n")).unwrap();
    }

    fn prompt(uuid: &str, ts: &str, text: &str) -> String {
        serde_json::json!({"type": "user", "uuid": uuid, "timestamp": ts, "cwd": "/p/rpet", "message": {"role": "user", "content": text}}).to_string()
    }

    fn reply(id: &str, ts: &str, model: &str, text: &str) -> String {
        serde_json::json!({"type": "assistant", "uuid": format!("a-{id}"), "timestamp": ts,
            "message": {"id": id, "model": model, "content": [{"type": "text", "text": text}], "usage": {"input_tokens": 10, "output_tokens": 100}}})
        .to_string()
    }

    #[test]
    fn indexes_searches_and_updates() {
        let root = std::env::temp_dir().join(format!("rmind-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        let main = projects.join("-p-rpet/s1.jsonl");
        write(
            &main,
            &[
                prompt("u1", "2026-10-01T01:00:00Z", "the pet crashes with a RefCell already borrowed error"),
                reply("m1", "2026-10-01T01:00:05Z", "claude-opus-5-5", "Split step() into advance() so schedule isn't called during the borrow."),
            ],
        );
        write(&projects.join("-p-rpet/s1/subagents/agent-x.jsonl"), &[reply("m2", "2026-10-01T01:00:07Z", "claude-haiku-5-5", "found it")]);
        std::fs::write(projects.join("-p-rpet/s1/subagents/agent-x.meta.json"), r#"{"agentType":"Explore","description":"Find the timer"}"#).unwrap();
        write(&projects.join("-p-rpet/s1/subagents/workflows/wf_1/agent-y.jsonl"), &[reply("m3", "2026-10-01T01:00:08Z", "claude-haiku-5-5", "ok")]);

        let mut db = Db::open(&root.join("index.db")).unwrap();
        let report = db.update(&projects, |_, _| {}).unwrap();
        assert_eq!((report.files, report.changed), (3, 3));
        let agents = db.agent_use(None).unwrap();
        assert_eq!(agents.iter().map(|a| (a.agent_type.as_str(), a.runs)).collect::<Vec<_>>().len(), 2);
        assert!(agents.iter().any(|a| a.agent_type == "Explore" && a.runs == 1));

        let hits = db.search("refcell borrowed", None, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session, "s1");
        assert!(hits[0].snippet.contains("«RefCell»"), "{}", hits[0].snippet);
        // Stemming, and any-word fallback.
        assert_eq!(db.search("borrowing", None, 5).unwrap().len(), 2, "borrowed, borrow");
        assert_eq!(db.search("advance nonsenseword", None, 5).unwrap().len(), 1);
        assert!(db.search("advance", Some("rsnap"), 5).unwrap().is_empty());
        // FTS syntax is taken literally.
        assert!(db.search("\"OR NEAR(", None, 5).unwrap().is_empty());

        let s = db.session("s").unwrap().unwrap();
        // 10 in + 100 out on Opus 5.5, plus 10 + 100 on Haiku 5.5 in each of two subagents.
        let expected = (10.0 * 4.0 + 100.0 * 20.0 + 2.0 * (10.0 * 0.1 + 100.0 * 0.5)) / 1e6;
        assert_eq!(s.prompts, 1);
        assert!((s.cost.unwrap() - expected).abs() < 1e-12, "{:?}", s.cost);
        assert_eq!(db.messages("s1").unwrap().len(), 2);

        // Nothing changed: nothing re-read.
        assert_eq!(db.update(&projects, |_, _| {}).unwrap().changed, 0);

        // The session goes on: re-read, old text replaced.
        std::thread::sleep(Duration::from_millis(20));
        write(
            &main,
            &[
                prompt("u1", "2026-10-01T01:00:00Z", "the pet crashes with a RefCell already borrowed error"),
                reply("m1", "2026-10-01T01:00:05Z", "claude-opus-5-5", "Split step() into advance()."),
                prompt("u2", "2026-10-01T02:00:00Z", "now add skins"),
            ],
        );
        assert_eq!(db.update(&projects, |_, _| {}).unwrap().changed, 1);
        assert_eq!(db.messages("s1").unwrap().len(), 3);
        assert_eq!(db.search("schedule", None, 5).unwrap().len(), 0, "old text is gone from the index");
        assert_eq!(db.recent(None, Some("/p/rpet"), 5).unwrap()[0].last_prompt, "now add skins");

        let usage = db.usage(100_000).unwrap();
        assert_eq!(usage.models.len(), 2, "subagent usage counts too");

        // Logs Claude Code cleaned up stay searchable.
        std::fs::remove_dir_all(projects.join("-p-rpet")).unwrap();
        db.update(&projects, |_, _| {}).unwrap();
        assert_eq!(db.search("skins", None, 5).unwrap().len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
