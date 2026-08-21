# Transcript Turn Rendering — Design

**Date:** 2026-08-20
**Status:** Approved (owner delegated autonomous approval: "쭉 알아서 해")
**Scope:** `oxicode-cli/src/tui_vt/main_loop.rs` (transcript rendering),
`oxicode-cli/src/tui_vt/frame_layout.rs` (shortcuts bar right cluster)

## Problem

Every transcript line renders a per-line speaker label — `you: `,
`assistant: `, `tool: `, `shell: `, … (`transcript_line_marked`). The
transcript already carries speaker identity through the per-kind colored
accent rail (`┃`, animated while running), so the text labels are a second,
noisier encoding. The result reads like a log dump, not a chat surface:
labels repeat on every wrapped row, dominate short lines, and add no
information the rail doesn't already carry.

Additionally, the shortcuts bar's right cluster (`brain·ok  line 0/0`)
carries a scroll-position text chip that duplicates the scrollbar thumb —
an unnecessary chip.

## Goals

1. Speaker identity from structure (rail + weight), not prose labels.
2. Turn rhythm: user turns visually set apart; agent/tool flow contiguous.
3. Zero new chrome (no new rows, boxes, or pills).
4. Remove the redundant `line N/M` chip.

## Chosen design — "the rail speaks"

| Kind | Prefix | Notes |
|---|---|---|
| User | `> ` first line; two-space continuation indent | Echoes the composer prompt glyph; text bold. Explicit newlines stay in one block and do not repeat the turn marker. |
| Agent | none | Plain markdown in `response` color. The agent is the transcript's default voice. |
| Tool | none | Tool blocks already carry structured glyph rendering internally. |
| Pty | none | Same rationale as Tool. |
| Error / Warning / Info / Policy | `error: ` etc. | Kept — severity is data. Now rendered on the block's **first line only**; continuation lines are plain. |

- **Turn spacing:** one blank spacer row *before* each `User` block (except
  at the transcript top). Agent→tool→agent transitions stay contiguous —
  they are one assistant turn. Spacers paint no rail.
- **Fold marker:** `[+]` retained on folded block heads, kind-colored.
- **Sticky header** (scroll-pinned block head) renders the block's first
  line — labels (for system kinds) appear there since it is the block start.
- **Line invariant:** protocol segments are normalized at ingestion so every
  `TranscriptLine` contains exactly one explicit line. This prevents ratatui
  `Line` from flattening embedded newlines in user prompts or streamed output.
- **Shortcuts bar right cluster:** brain health chip only (`brain·ok` /
  `brain·down`, absent when memory is off). Scroll position removed — the
  scrollbar thumb already encodes it.

## Rejected alternatives

- **Per-block pill headers** (`❯ you`, `◆ assistant` chips): adds a chrome
  row per turn and re-introduces the chip furniture the owner just removed.
- **Bordered user boxes** (quote-style): +2 rows per user turn; boxes around
  one-line prompts are heavy; the rail already identifies the speaker.

## Testing

- `transcript_line_marked`: per-kind prefix contract (user glyph + aligned
  continuations, agent none, system first-line-only), fold marker, search
  highlight preserved.
- Transcript ingestion: explicit newlines in submitted prompts and streaming
  deltas render on consecutive rows in the same semantic block.
- Spacer rendering: blank row before user blocks, none at transcript top,
  no rail on spacer rows, scroll math unaffected.
- Shortcuts bar: brain-only right cluster by health.
- Full gates + PTY launch of the installed binary.

## Addendum (2026-08-21): plain surface, rail removed

Owner reviewed the shipped rail design against peer TUIs (Claude Code,
pi, OpenCode, Codex CLI — see research notes in the session log) and
rejected the accent rail: "레일은 일단 별로야" — omp-style agents carry no
speaker chrome at all. Four directions were mocked up in HTML (box+label
Claude-Code style / framed box / selectable styles / omp plain); the
owner chose **plain (omp-style)**.

Final contract:

- No rail column, no rail animation (wave), no sticky-header rail.
- No `> ` user prefix. User input = bold primary text; agent response =
  default ink; tool/shell = their kind colors.
- Blank spacer row before each user block remains the turn boundary.
- System severities (`error:` `warning:` `info:` `policy:`) keep their
  first-line label — severity is data, not speaker chrome.
- Sticky header keeps its faint background tint only.
- Scrollbar column unchanged.
