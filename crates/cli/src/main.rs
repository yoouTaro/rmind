//! rmind: search, usage and memory over your Claude Code history.

mod mcp;
mod render;
mod setup;
mod ui;

use clap::{Parser, Subcommand};
use rmind_core::db::Db;
use rmind_core::forge;
use rmind_core::paths;
use std::io::{IsTerminal, Read, Write};

#[derive(Parser)]
#[command(name = "rmind", version, about = "Search, usage and memory over your Claude Code history")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Search every past session
    Search {
        query: Vec<String>,
        /// Only sessions whose folder contains this
        #[arg(short, long)]
        project: Option<String>,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Read a session: an outline, or the part around a message
    Show {
        /// Session id (a unique prefix is enough)
        session: String,
        /// Message id to center on
        #[arg(short, long)]
        around: Option<String>,
        #[arg(short = 'n', long, default_value_t = 40)]
        limit: usize,
    },
    /// Latest sessions
    Recent {
        #[arg(short, long)]
        project: Option<String>,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Spending and tokens
    Usage {
        #[arg(short, long, default_value_t = 30)]
        days: u32,
    },
    /// Catch the index up with new and changed sessions
    Index {
        #[arg(short, long)]
        quiet: bool,
    },
    /// Suggest CLAUDE.md rules and skills from your habits
    Forge {
        /// Only print the evidence mined from your history (free)
        #[arg(long)]
        evidence: bool,
        /// List suggestions instead of making new ones
        #[arg(long)]
        list: bool,
        /// Design a team of agents from how you delegate, instead of rules
        #[arg(long)]
        team: bool,
    },
    /// Write a Forge suggestion into place
    Accept { id: i64 },
    /// Drop a Forge suggestion
    Dismiss { id: i64 },
    /// Open the rmind window in your browser
    Ui {
        #[arg(short, long, default_value_t = 4777)]
        port: u16,
        /// Don't open the browser
        #[arg(long)]
        no_open: bool,
    },
    /// Hook it into Claude Code (MCP server + hooks)
    Setup,
    /// Unhook it from Claude Code (the index is kept)
    Uninstall,
    /// Run the MCP server (Claude Code starts this itself)
    Mcp,
    /// SessionStart hook: recent sessions in this folder, for Claude
    Brief,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Search { query, project, limit } => fresh().and_then(|db| {
            let query = query.join(" ");
            print!("{}", render::hits(&query, &db.search(&query, project.as_deref(), limit)?));
            Ok(())
        }),
        Cmd::Show { session, around, limit } => fresh().and_then(|db| {
            let info = db.session(&session)?.ok_or_else(|| format!("no session starting with {session}"))?;
            println!("{}", render::session(&info, &db.messages(&info.id)?, around.as_deref(), limit));
            Ok(())
        }),
        Cmd::Recent { project, limit } => fresh().and_then(|db| {
            print!("{}", render::sessions(&db.recent(project.as_deref(), None, limit)?));
            Ok(())
        }),
        Cmd::Usage { days } => fresh().and_then(|db| {
            print!("{}", render::usage(&db.usage(days)?, days));
            Ok(())
        }),
        Cmd::Index { quiet } => index(quiet),
        Cmd::Forge { evidence, list, team } => fresh().and_then(|db| {
            if evidence {
                print!("{}", if team { forge::mine_team(&db)? } else { forge::mine(&db)? });
                return Ok(());
            }
            if !list {
                eprintln!("Mining your history and asking Claude (Sonnet) for suggestions; this takes a minute or two…");
                let n = forge::run(&db, if team { forge::Mode::Team } else { forge::Mode::Habits })?;
                eprintln!("{n} new suggestions.\n");
            }
            let mode = if team { forge::Mode::Team } else { forge::Mode::Habits };
            print!("{}", render::suggestions(&db.suggestions_of(Some(mode.key()), Some("new"))?));
            Ok(())
        }),
        Cmd::Accept { id } => fresh().and_then(|db| {
            println!("Wrote {}", forge::accept(&db, id, None)?.display());
            Ok(())
        }),
        Cmd::Dismiss { id } => fresh().and_then(|db| {
            let s = db.suggestion(id)?.ok_or("no such suggestion")?;
            db.set_suggestion(id, "dismissed", &s.content, "")?;
            Ok(())
        }),
        Cmd::Ui { port, no_open } => ui::serve(port, !no_open),
        Cmd::Setup => setup::install(),
        Cmd::Uninstall => setup::uninstall(),
        Cmd::Mcp => mcp::serve(),
        Cmd::Brief => brief(),
    };
    if let Err(e) = result {
        eprintln!("rmind: {e}");
        std::process::exit(1);
    }
}

pub type Result<T> = rmind_core::db::Result<T>;

/// The index, caught up.
fn fresh() -> Result<Db> {
    let mut db = Db::open_default().map_err(|e| e.to_string())?;
    db.update(&paths::projects_dir(), progress(false)).map_err(|e| e.to_string())?;
    Ok(db)
}

pub fn index(quiet: bool) -> Result<()> {
    let mut db = Db::open_default().map_err(|e| e.to_string())?;
    let report = db.update(&paths::projects_dir(), progress(quiet)).map_err(|e| e.to_string())?;
    if !quiet {
        println!("{} logs, {} read.", report.files, report.changed);
    }
    Ok(())
}

/// A progress line on the terminal when there's enough to read to wait for.
fn progress(quiet: bool) -> impl FnMut(usize, usize) {
    let show = !quiet && std::io::stderr().is_terminal();
    move |done, total| {
        if show && total > 5 {
            let mut err = std::io::stderr();
            let _ = write!(err, "\rIndexing {done}/{total} logs…");
            if done == total {
                let _ = writeln!(err);
            }
        }
    }
}

/// Claude Code sends `{"cwd", "session_id", "source", ...}` on stdin; what
/// this prints is added to the new session's context.
fn brief() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let event: serde_json::Value = serde_json::from_str(&input).unwrap_or_default();
    // Resumed and compacted sessions already have their context.
    if matches!(event["source"].as_str(), Some("resume" | "compact")) {
        return Ok(());
    }
    let Some(cwd) = event["cwd"].as_str() else { return Ok(()) };
    let this = event["session_id"].as_str().unwrap_or_default();
    // Read-only and quick: the SessionEnd hook keeps the index current.
    if !paths::db_path().exists() {
        return Ok(());
    }
    let db = Db::open_default().map_err(|e| e.to_string())?;
    let past: Vec<_> = db.recent(None, Some(cwd), 4).map_err(|e| e.to_string())?.into_iter().filter(|s| s.id != this).take(3).collect();
    if past.is_empty() {
        return Ok(());
    }
    println!("rmind: recent Claude sessions in this folder (use the rmind search_history / read_session tools for details):");
    for s in past {
        let last = rmind_core::parse::clip(&s.last_prompt.split_whitespace().collect::<Vec<_>>().join(" "), 140);
        println!("- {} \"{}\" ({} prompts), last asked: \"{last}\" [session {}]", rmind_core::db::day(&s.last_ts), s.title, s.prompts, s.id);
    }
    Ok(())
}
