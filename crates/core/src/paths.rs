use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Claude Code's config folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
pub fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

/// Where Claude Code keeps the session logs.
pub fn projects_dir() -> PathBuf {
    claude_dir().join("projects")
}

/// rmind's own folder (`RMIND_DIR`, else `~/Library/Application Support/rmind`).
pub fn data_dir() -> PathBuf {
    std::env::var_os("RMIND_DIR").map(PathBuf::from).unwrap_or_else(|| home().join("Library/Application Support/rmind"))
}

pub fn db_path() -> PathBuf {
    data_dir().join("index.db")
}
