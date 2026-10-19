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
- **Forge** – finds what you keep repeating or correcting (what you said
  right after interrupting Claude, "no, …", prompts you repeat, each
  project's commands) and asks Claude (Sonnet, one call, a few cents) to draft
  CLAUDE.md rules and skills. Nothing is written until you accept one.
- **Forge a team** – designs a roster of agents (`~/.claude/agents/*.md`) from
  how you actually delegate: every subagent run with its type, model, task and
  cost, your current agents and which go unused, each project's commands, and
  your model routing rule. Plus a "Team playbook" rule telling the lead which
  agent to use when. Accepting an agent that replaces yours backs yours up as
  `<name>.md.before-rmind`.
- **UI** – `rmind ui` opens all of it in your browser: search, sessions,
  usage charts, and Forge suggestions to edit, accept or dismiss.

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
rmind ui                              # the window: Search, Sessions, Usage, Habits, Team
rmind forge                           # draft suggestions; rmind forge --evidence shows what it mined
rmind forge --team                    # design a team of agents
rmind accept <id> / rmind dismiss <id>
```

The UI is served on 127.0.0.1 only, and its API needs a token that's only in
the page, so other websites can't use it.

Nothing to run in the background: Claude Code starts `rmind mcp` for each
session, the SessionEnd hook indexes the session that just closed, and every
search first catches up on changed logs (a few ms when nothing changed).
