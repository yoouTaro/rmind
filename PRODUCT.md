# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

One developer (the author), on their own Mac, who uses Claude Code all day across work projects (TypeScript/Bun, Laravel, Postgres/PostGIS) and personal Rust Mac apps. They open the rmind window mainly to shape how Claude works for them: review what Forge found in their history, accept or edit rules and skills, and tune their team of agents. Searching past work and checking spending come second.

Not published yet; it may be open-sourced later like rjot, rsnap and rgrip, but the UI is designed for its one user now.

## Product Purpose

rmind turns the author's Claude Code history into memory and leverage. It indexes every session on the Mac, lets Claude search it (MCP), briefs new sessions, reports what the work costs, and uses Forge to turn repeated corrections into CLAUDE.md rules and skills, and delegation patterns into a team of agents. Success: Claude makes fewer of the same mistakes, uses the right agent at the right cost, and the author can find any past fix in seconds.

## Positioning

Built from the author's own, complete history (all projects, every session since logs began, kept after Claude Code deletes them), computed locally with no background service. Suggestions quote what the user actually said; nothing is written to their config until they accept it.

## Operating Context

- Opened with `rmind ui`, served on 127.0.0.1 in the browser (often a side panel next to Claude Code / T3 Code), dark environment, alongside long coding sessions.
- Data: ~100 sessions, ~29K indexed messages, ~400 logs, ~$2,900 of usage at API prices, 6 agents, a dozen accepted rules.
- Writes only to `~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/` (backing up replaced agents, keeping locked files locked).

## Capabilities and Constraints

- Tabs today: Search, Sessions (with a conversation side panel), Usage, Habits (Forge: rules and skills), Team (agents roster + Forge a team).
- Forge runs call Claude (Sonnet) headlessly: one to three minutes, a few cents each.
- Single self-contained HTML file embedded in the Rust binary; no build step, no external network requests (CSP: self only). Fonts must be system fonts or embedded.
- Content mixes English and Khmer; quotes from the user are casual with typos and must be shown verbatim.
- Costs are API-price estimates; the user is on a subscription.

## Brand Commitments

Name: rmind (lowercase). Its own identity, distinct from the r-apps' shared landing-page look.
Standing preference (2026-10-19): a clean developer-tool standard, benchmarked against Raycast: compact, Mac-native, keyboard-first, quiet neutrals with one accent, following the system light/dark setting. Playful or themed worlds (paper, handwriting) were tried and rejected.

## Evidence on Hand

Real data only: the local index (sessions, messages, usage, agent runs) and Forge suggestions with verbatim evidence quotes. No testimonials or marketing claims exist or should be invented.

## Product Principles

1. Nothing changes the user's Claude setup without an explicit accept.
2. Show the evidence: every suggestion and number traces back to real sessions.
3. Shaping Claude is the main job; search and spending support it.
4. Local, quiet, fast: no background work, no network, instant views.
