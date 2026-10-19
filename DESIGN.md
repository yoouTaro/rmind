---
name: rmind
description: A clean, Raycast-grade developer tool for reviewing and shaping how Claude Code works, searching past sessions and checking spend.
colors:
  dark:
    bg: "#0f1012"
    bar: "#131416"
    surface: "#17181b"
    hover: "#1d1f22"
    selected: "#25272b"
    line: "rgba(255,255,255,.07)"
    line-2: "rgba(255,255,255,.12)"
    text: "#ecedef"
    muted: "#9da0a8"
    faint: "#82858d"
    accent: "#86c4a8"
    on-accent: "#0d1a14"
    danger: "#f08b80"
    warn: "#e6c27a"
  light:
    bg: "#f5f5f6"
    bar: "#fbfbfb"
    surface: "#ffffff"
    hover: "#eeeef0"
    selected: "#e6e7ea"
    line: "rgba(0,0,0,.07)"
    line-2: "rgba(0,0,0,.12)"
    text: "#18191b"
    muted: "#5f626a"
    faint: "#65686f"
    accent: "#23795a"
    on-accent: "#ffffff"
    danger: "#c0392b"
    warn: "#9a6a00"
typography:
  family: "Geist (variable, served from the binary), -apple-system, Khmer Sangam MN fallback"
  mono: "Geist Mono (variable), ui-monospace"
  page-title: { size: "22px", weight: 600, tracking: "-0.02em" }
  section: { size: "14px", weight: 600, tracking: "-0.01em" }
  detail-title: { size: "18px", weight: 600, tracking: "-0.015em" }
  body: { size: "13.5px", weight: 400, lineHeight: 1.5 }
  secondary: { size: "12.5px" }
  label: { size: "11.5px", weight: 600 }
  figures: "Geist Mono or tabular-nums for every number"
radius: { control: "7px", row: "8px", panel: "12px", sheet: "14px" }
spacing: { row-height: "44px", row-padding: "9px 10px", page-max: "1180px", split-list: "360px" }
---

# rmind design

**Direction:** the clean developer-tool standard, benchmarked against Raycast (see PRODUCT.md). Compact, keyboard-first, quiet. Follows macOS light/dark through `prefers-color-scheme`.

## Principles

- **Lists, not cards.** Every view is a list of 44px rows: icon, title, two-line subtitle, accessories on the right (tags, counts, dates). Selection is a filled row, never a border.
- **List and detail.** Habits and Team suggestions put the list on the left (360px) and the selected item's detail on the right, sticky. On narrow screens the detail stacks below.
- **One accent.** Sage (`accent`) is used only for primary actions, the selected state of meaningful data (chart bars, meters, focused message, accepted state) and highlights. Everything else is neutral.
- **Numbers are mono.** Costs, counts, token figures and dates in accessories use Geist Mono or tabular figures.
- **Keyboard first.** ↑↓ or j/k move the selection, ↵ opens, ⌘↵ accepts, ⌘⌫ dismisses, / focuses search, 1–5 switch tabs, Esc closes. The bottom hint bar shows the keys for the current context and hides on narrow screens.

## Components

- **Top bar:** sticky, translucent; mark, segmented tabs with their number key, index stats in mono on the right.
- **Segmented control:** grey track, the active segment lifts to `surface`. Used for tabs, status filters and usage periods.
- **Buttons:** 30px; default (surface + hairline), primary (accent fill), ghost (text only). Press scales to .98.
- **Search field:** 44px, surface fill, hairline border that strengthens on focus.
- **Tags:** small grey chips; `accent` and `warn` variants.
- **Session panel:** a sheet sliding in from the right over a dim scrim; messages labelled You / Claude / Tool / Thinking; tool output and thinking clamp to three lines and expand on click; the searched message is outlined in accent.
- **Loading:** skeleton rows shaped like list rows. **Empty:** centred title and one line, with suggestions where useful. **Toast:** small, bottom centre, check icon.

## Assets

- `crates/cli/src/ui/fonts/Geist.ttf`, `GeistMono.ttf`: Geist by the Geist Project Authors (Vercel), SIL OFL 1.1 (`OFL-Geist.txt`), shipped unmodified.
- `crates/cli/src/ui/favicon.svg`: authored for rmind (an "rm" ligature mark in accent on the dark surface).
