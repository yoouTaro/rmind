---
version: 1
slug: "crates-cli-src-ui-index-html"
primary_target: "crates/cli/src/ui/index.html"
related_targets: []
---

# rmind window

Scope: the whole rmind UI (`rmind ui`), one embedded HTML file. Mode: Operate.
Audience and job: the author, beside Claude Code all day; mainly reviewing Forge findings and shaping rules and agents, then searching past work and checking spend.
Constraints: no network (fonts served from the binary), verbatim user quotes (casual, typos, Khmer), nothing written without Accept. Benchmark: Raycast.
Default view: Habits, then Team, Search, Sessions, Usage.

## Direction contract

THESIS: rmind is a Raycast-grade tool: everything is a fast, keyboard-driven list with a detail pane. Refuses the card-and-stat-tile dashboard.

OWN-WORLD: Neutral greys from one cool family, one muted sage accent for primary actions and selection; Geist at a compact 13.5px scale, Geist Mono for code and figures; 44px list rows with icon, title, subtitle and right-aligned accessories; selected row fill instead of borders; follows macOS light/dark.

STORY: Open rmind, arrow through what Forge found, read the evidence in the detail pane, Accept with ⌘↵ or Dismiss; jump to Team, Search, Sessions, Usage with 1–5.

FIRST VIEWPORT: Top bar with rmind mark, five segmented tabs and index stats. Habits: title row with Find habits; status filter; list of lessons on the left (360px), selected lesson's detail on the right with quotes, editable rule and Accept/Dismiss; keyboard hint bar fixed at the bottom.

FORM: Category standard (canon) benchmarked against Raycast, chosen in the safer re-roll of seed 26ff59cb.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

Signature interaction: keyboard-first navigation (↑↓/jk move, ↵ open, ⌘↵ accept, / search, 1–5 tabs) with a hint bar that changes with context; lists grouped by day like Raycast's sections.
