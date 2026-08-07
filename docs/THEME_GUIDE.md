# Theme Guide

oxicode's terminal UI supports **28 customizable color slots**, 3 glyph sets
(Unicode / ASCII / Nerd), and spacing configuration. Themes load from TOML
or JSON files with hot-reload.

## Color Authority

> **Production renders via `oxicode-vtui`, not this guide's `oxicode-tui`
> `ColorScheme`.** The interactive TUI (`tui_vt/`) uses `oxicode_vtui::theme`
> — a registry of 6-field `ThemePalette`s with a contrast-guaranteeing
> derivation pipeline. The authoritative brand mapping (OKLCH → palette) and
> the implementation reference live in `docs/oxi-design-system-tui.md`; the
> default theme is `"oxi"` (pure-black canvas, oxi-design-system palette).
>
> ⚠️ The guide body below documents the **deleted** `oxicode-tui` widget-library
> `ColorScheme` (28 slots, TOML/JSON themes, hot-reload) — kept as a historical
> reference only; that crate was removed as dead code and is not in any build.
> For all live theming, use `oxicode-vtui` (see `docs/oxi-design-system-tui.md`).

## Quick Start

### Using a built-in theme

Set in `~/.oxicode/settings.toml`:

```toml
theme = "catppuccin"
```

Built-in names: `oxicode_dark`, `oxicode_light`, `nord`, `catppuccin`, `github_dark`,
`monokai`. Or cycle via `/settings` overlay → `theme` row → Enter.

### Creating a custom theme

Place a `.toml` or `.json` file in `~/.oxicode/themes/` or `<project>/.oxicode/themes/`:

```bash
mkdir -p ~/.oxicode/themes
```

```toml
# ~/.oxicode/themes/my_theme.toml
name = "My Theme"

[colors]
# ── Core (required for a usable theme) ──
foreground = "#cdd6f4"
background = "#000000"
primary    = "#7aa2f7"
error      = "#f7768e"
success    = "#9ece6a"
warning    = "#e0af68"
muted      = "#7f849c"
accent     = "#bb9af7"
border     = "#585b70"

# ── Chat ──
user_border   = "#7aa2f7"
user_bg       = "#121626"
response_bg   = "#000000"   # assistant text bg (= background by default)
thinking_bg   = "#0b090f"   # thinking block bg

# ── Surfaces ──
surface_bg    = "#090b13"   # footer / status bar
panel_bg      = "#35384b"   # overlay popups

# ── Code ──
code_fg       = "#ffc864"
code_bg       = "#231e14"

# ── Diff ──
diff_add_bg    = "#101a0e"   # added-line bg
diff_remove_bg = "#201012"   # removed-line bg
diff_hunk_bg   = "#0f1013"   # hunk-header bg

# ── Tool call states ──
tool_pending_bg   = "#12141c"
tool_executing_bg = "#1c1810"
tool_success_bg   = "#101a0e"
tool_error_bg     = "#201012"

# ── Cursor / selection ──
cursor_fg    = "#000000"
cursor_bg    = "#cdd6f4"
selection_bg = "#28283c"

# ── Secondary ──
secondary = "#9ece6a"
```

### Color formats

| Format | Example | Notes |
|---|---|---|
| Hex `#rrggbb` | `#7aa2f7` | 6-digit RGB |
| Hex `#rgb` | `#7af` | Expanded to `#77aaff` |
| Named | `red`, `blue`, … | 8 ANSI + 8 bright |
| Bright named | `bright-red`, `brightblue` | ANSI 9–15 |
| Indexed | `i42` | 256-color palette (0–255) |
| Default | `default` | Terminal default (`Color::Reset`) |

## Brightness Hierarchy

The 7 background slots follow a consistent layering principle:

```
background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg
```

For **dark** themes, prominence = lighter (further from black).
For **light** themes, prominence = darker (further from white).

| Slot | Derivation rule | Used by |
|---|---|---|
| `background` | base color | viewport fill |
| `response_bg` | `= background` | assistant text rows |
| `thinking_bg` | `blend(bg, accent, 0.06)` | thinking blocks |
| `surface_bg` | `blend(bg, user_bg, 0.5)` | footer / status bar |
| `user_bg` | per-theme | user message rows |
| `panel_bg` | `blend(user_bg, border, 0.5)` | overlay popups |
| `diff_add_bg` | `= tool_success_bg` | diff added lines |
| `diff_remove_bg` | `= tool_error_bg` | diff removed lines |
| `diff_hunk_bg` | `blend(bg, muted, 0.12)` | diff hunk headers |

**Tip:** If you omit any slot, it falls back to the dark theme's default value.
Your theme file only needs the colors you want to override.

## Glyph Sets

```toml
# ~/.oxicode/settings.toml
glyph_set = "unicode"   # "unicode" (default) | "ascii" | "nerd"
```

- **Unicode** — full box-drawing, emoji, Nerd Font-free
- **ASCII** — pure ASCII fallback for legacy terminals
- **Nerd** — Nerd Font icons (requires a Nerd Font installed)

## Hot-Reload

The TUI polls `settings.toml` every 1 second. Editing the file externally
and saving triggers an automatic theme reload — no restart needed.

Custom theme files in `~/.oxicode/themes/` are also hot-reloaded when their
mtime changes.

## File Locations

| Path | Purpose |
|---|---|
| `~/.oxicode/settings.toml` | `theme` / `glyph_set` settings |
| `~/.oxicode/themes/*.toml` | Global custom themes |
| `~/.oxicode/themes/*.json` | Global custom themes (JSON format) |
| `<project>/.oxicode/themes/*.json` | Project-local themes |
