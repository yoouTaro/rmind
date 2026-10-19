# rmind

Search, usage stats and memory over your Claude Code history. Everything
stays on your Mac.

Claude Code saves every session in `~/.claude/projects`. rmind indexes all of
it (prompts, replies, thinking, commands, edited files, errors) into one
SQLite full-text index, then:

- **Recall** – Claude gets an MCP server with `search_history`,
  `read_session`, `recent_sessions` and `usage_report`, so it can look up how
  you solved something before, in any project.
- **Brief** – a SessionStart hook hands Claude a few lines about your last
  sessions in the same folder.
- **Usage** – cost by day, project and model (at API prices, subagents
  included), cache hit rate, priciest sessions.

Sessions stay in the index after Claude Code cleans up old logs.

## Install

```sh
cargo install --path crates/cli
rmind setup        # registers the MCP server, adds the hooks, builds the index
```

`rmind uninstall` removes the MCP server and hooks (the index stays in
`~/Library/Application Support/rmind`). Setup backs up the original
`~/.claude/settings.json` to `settings.json.before-rmind` the first time.

## Use

```sh
rmind search objc2 RefCell borrow     # -p rsnap to filter by project
rmind show <session> --around <message>
rmind recent
rmind usage --days 30
```

Nothing to run in the background: Claude Code starts `rmind mcp` for each
session, the SessionEnd hook indexes the session that just closed, and every
search first catches up on changed logs (a few ms when nothing changed).
