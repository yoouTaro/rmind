//! `rmind setup` / `rmind uninstall`: register the MCP server with Claude
//! Code and add the two hooks to `~/.claude/settings.json`:
//!
//! - SessionStart runs `rmind brief`: a few lines about your last sessions
//!   in that folder, handed to Claude as context.
//! - SessionEnd runs `rmind index`, so a session is searchable as soon as
//!   it closes.

use rmind_core::paths;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;

pub fn install() -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        eprintln!(
            "Note: running from a build folder ({}). Install first so cleaning target/ won't break Claude:\n  cargo install --path crates/cli && rmind setup\n",
            exe.display()
        );
    }
    let quoted = quote(&exe);

    let _ = claude(&["mcp", "remove", "--scope", "user", "rmind"]);
    claude(&["mcp", "add", "--scope", "user", "rmind", "--", &exe.to_string_lossy(), "mcp"])?;
    println!("✓ MCP server registered (rmind mcp, all projects)");

    edit_settings(|hooks| {
        remove_ours(hooks);
        add_hook(hooks, "SessionStart", format!("{quoted} brief"), 10);
        add_hook(hooks, "SessionEnd", format!("{quoted} index --quiet"), 120);
    })?;
    println!("✓ Hooks added to {}", settings_path().display());

    println!("Indexing your sessions…");
    crate::index(false)?;
    println!("\nAll set. New Claude sessions can search your history; try asking Claude \"what did we do last time?\".");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let _ = claude(&["mcp", "remove", "--scope", "user", "rmind"]);
    println!("✓ MCP server removed");
    edit_settings(remove_ours)?;
    println!("✓ Hooks removed");
    println!("The index is still in {} (delete the folder to remove it).", paths::data_dir().display());
    Ok(())
}

fn claude(args: &[&str]) -> Result<()> {
    let out = Command::new("claude").args(args).output().map_err(|e| format!("couldn't run `claude` ({e}); is Claude Code installed?"))?;
    if !out.status.success() {
        return Err(format!("claude {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    Ok(())
}

fn settings_path() -> PathBuf {
    paths::claude_dir().join("settings.json")
}

/// Change the `hooks` object of settings.json, keeping everything else and a
/// one-time backup of the original.
fn edit_settings(change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> Result<()> {
    let path = settings_path();
    let mut settings: Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} isn't valid JSON ({e}); fix it and retry", path.display()))?,
        Err(_) => json!({}),
    };
    let backup = path.with_extension("json.before-rmind");
    if path.exists() && !backup.exists() {
        std::fs::copy(&path, &backup)?;
    }
    let root = settings.as_object_mut().ok_or("settings.json isn't a JSON object")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("settings.json `hooks` isn't an object")?;
    change(hooks);
    if hooks.is_empty() {
        root.remove("hooks");
    }
    std::fs::write(&path, serde_json::to_string_pretty(&settings)? + "\n")?;
    Ok(())
}

fn add_hook(hooks: &mut serde_json::Map<String, Value>, event: &str, command: String, timeout: u32) {
    let list = hooks.entry(event).or_insert_with(|| json!([]));
    if let Some(list) = list.as_array_mut() {
        list.push(json!({"hooks": [{"type": "command", "command": command, "timeout": timeout}]}));
    }
}

/// Drop the hooks an earlier setup added, leaving everyone else's.
fn remove_ours(hooks: &mut serde_json::Map<String, Value>) {
    for event in ["SessionStart", "SessionEnd"] {
        let Some(list) = hooks.get_mut(event).and_then(Value::as_array_mut) else { continue };
        list.retain(|group| !group["hooks"].as_array().is_some_and(|hs| hs.iter().any(is_ours)));
        if list.is_empty() {
            hooks.remove(event);
        }
    }
}

fn is_ours(hook: &Value) -> bool {
    let cmd = hook["command"].as_str().unwrap_or_default();
    (cmd.contains("rmind' brief") || cmd.contains("rmind' index")) || cmd.starts_with("rmind ")
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_our_hooks() {
        let mut hooks = json!({
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}]}, {"hooks": [{"type": "command", "command": "'/x/rmind' brief"}]}],
            "SessionEnd": [{"hooks": [{"type": "command", "command": "'/x/rmind' index --quiet"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]
        });
        let map = hooks.as_object_mut().unwrap();
        remove_ours(map);
        assert_eq!(
            hooks,
            json!({"SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}]}], "Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]})
        );
    }
}
