# Oxi Design System — oxicode-vtui Surface Notes

> **Color authority for the oxicode terminal UI.** This document is the
> terminal surface layer on top of the portable **oxi-design-system** brand
> spec (`DESIGN.md` v1.0, 2026-07-31). It defines how that brand spec maps onto
> the production renderer's theme model.
>
> **Production theme system = `oxicode-vtui`.** The interactive TUI (`tui_vt/`,
> entered via `bootstrap.rs::dispatch_run_mode → run_tui`) renders exclusively
> through `oxicode_vtui::theme` — a registry of `ThemeDefinition`s, each a
> 6-field `ThemePalette`, with a derivation pipeline that computes every UI
> color with contrast guarantees. Apply the oxi brand here, in `oxicode-vtui`.
> (A standalone `oxicode-tui` widget library with its own 28-slot `ColorScheme`
> was deleted as dead code — it was never in the production render path.)
>
> **Status:** Implemented 2026-08-07. The `"oxi"` theme is registered in
> `oxicode-vtui/src/theme/registry.rs`, is `DEFAULT_THEME`, and the
> settings/wizard default resolves to it. Pure-black canvas per owner
> constraint.

---

## 0. Authority & scope

- **Authority:** oxi-design-system `DESIGN.md` v1.0 (the portable brand spec).
  `project-oxi/.github/DESIGN.md` v1.0 is the same token set in project-specific
  form and may be read as equivalent for the values in §3.2–§3.3.
- **Scope:** color, contrast, and the visual ethos of the terminal UI.
- **"Replace, don't merge" (TUI-scoped):** the oxi palette is the default.
  Community themes (`oxide-dark`, `nord`, `catppuccin-*`, …) remain opt-in
  alternatives; `oxi` is the authority a fresh user sees.

---

## 1. The medium gap — what transfers, what does not

The brand spec is web-oriented (OKLCH, Tailwind, CSS vars, `.dark`, box-shadow,
web fonts, APCA). The TUI is a character-cell grid. Mechanism cannot be applied
verbatim; content can.

| Brand element | Transfers? | TUI adaptation |
|---|:---:|---|
| OKLCH neutral ramp (warm paper / cool ink) | ✅ | Convert to sRGB → `ThemePalette` fields |
| Six OKLCH label hues (Red/Amber/Green/Teal/Blue/Purple) | ✅ | Map onto `secondary_accent` / `alert` / `logo_accent` |
| APCA-optimized status hues | ✅ | `alert` ← dark error value |
| "Color is data, not branding" / ink-on-paper | ✅ | Identity from neutrals; hue only for meaning |
| **Derivation with contrast guarantees** | ✅ | `oxicode-vtui` already does this — `color_math` derives 14+ styles from the 6-field palette, enforced against the background |
| CSS `var()` / Tailwind utilities | ❌ | No DOM. Palette is direct RGB fields |
| `.dark` class / `dark:` variant | ❌ | Theme switching via `settings.theme` + `set_active_theme` (global runtime state) |
| box-shadow input borders | ❌ | Terminal cells = glyph + color |
| SUIT / SUITE / Geist Mono fonts | ❌ | Terminal emulator renders fonts; TUI can only suggest in a bundled app |
| Component radius tokens | ❌ | Box-drawing glyphs are fixed shapes |
| True APCA | ❌ | sRGB; the derivation pipeline approximates WCAG contrast ratios |

---

## 2. The vtui theme model (how color actually works)

`oxicode-vtui` themes are **not** a flat list of hand-set slots. Each theme is a
small seed palette; a derivation pipeline computes the full style set:

```
ThemePalette (6 seed fields)
   │  ThemePalette::build_styles_with_accessibility(&color_config)
   ▼
ColorContext { background, min_contrast, fallback_light }
   │  14 compute_* derivations (color_math.rs): ensure_contrast + balance_text_luminance
   ▼
ThemeStyles { text, info, tool, tool_body, pty_output, response, reasoning,
              user, alert, primary, secondary, logo, status, mcp, … }
```

**Consequence for the brand port:** you provide 6 OKLCH-derived seed values;
the pipeline guarantees every derived color meets `min_contrast` against the
background. You do **not** (and cannot) hand-set 28 slots — and you should not
try to fight the pipeline. Pick seeds that already read correctly on the chosen
background; the pipeline only nudges failing colors over the contrast line.

Seed fields (`oxicode-vtui/src/theme/types.rs::ThemePalette`):

| Field | Role |
|---|---|
| `background` | Canvas. Drives every contrast computation. |
| `foreground` | Body text seed → `compute_text_color`. |
| `primary_accent` | UI chrome seed → `compute_primary_color` / status banner. |
| `secondary_accent` | Info/structure seed → `compute_info_color` / `compute_secondary_color` / user input. |
| `alert` | Error seed → `compute_alert_color`. |
| `logo_accent` | Branding seed → `compute_logo_color` / MCP badge. |

---

## 3. The oxi palette mapping

Derived from DESIGN.md §3.3 dark tier. OKLCH → sRGB (Ottosson matrices + sRGB
gamma), computed 2026-08-07. Registered as the `"oxi"` theme in
`oxicode-vtui/src/theme/registry.rs`.

| `ThemePalette` field | OKLCH source (DESIGN.md) | sRGB hex | Mapping rationale |
|---|---|---|---|
| `background` | cool near-black | `#0d1117` | Soft slate canvas reduces glare while remaining terminal-dark |
| `foreground` | cool near-white | `#e6edf3` | Neutral, legible ink |
| `primary_accent` | neutral chrome | `#8b949e` | Quiet gray chrome; color is not decoration |
| `secondary_accent` | interactive accent | `#58a6a6` | Teal for focus, links, and active interaction |
| `alert` | restrained error | `#e06c75` | Red only for failure or destructive state |
| `logo_accent` | interaction accent | `#58a6a6` | Same teal: no separate branding hue |

All other visible colors (response text, reasoning, tool chrome, status banner,
PTY output, …) are **derived** by the pipeline against the slate canvas —
they are not listed here because they are not hand-set.

---

## 4. Slate canvas (recorded)

The default canvas is `#0d1117`: close enough to black for a terminal, but
less harsh on large displays. `#e6edf3` is cool neutral ink. Teal is the sole
interactive hue; red is reserved for errors. This keeps transcript text more
important than chrome and prevents a rainbow of status colors.

Guarded by `theme::tests::test_oxi_palette_is_soft_slate`.

---

## 5. Implementation reference (done)

1. **Theme definition** — `oxicode-vtui/src/theme/registry.rs`: the `"oxi"`
   `ThemeDefinition` with the §3 palette (inserted before `oxide-dark`).
2. **Suite** — `suite_id_for_theme` (`theme_id == "oxi"`), `suite_label`
   (`"Oxi"`), and `available_theme_suites` `ORDER` (oxi first).
3. **Default** — `DEFAULT_THEME = "oxi"` in
   `oxicode-vtui-compat/src/lib.rs` (the canonical constant;
   `DEFAULT_THEME_ID` in `theme/types.rs` re-exports it). The parallel
   `oxicode-vtui/src/tui/config/constants/defaults.rs::DEFAULT_THEME` is set
   to `"oxi"` too for consistency (it is currently unreferenced).
4. **Settings bridge** — `settings.rs::get_theme_name()` fallback is `"oxi"`,
   so `tui_vt/host.rs::activate_theme` resolves cleanly via
   `set_active_theme("oxi")` (no silent fallback).
5. **Wizard** — `setup_wizard.rs` theme defaults are `"oxi"`.
6. **Tests** — `theme::tests`: `test_oxi_is_default`, `test_oxi_theme_exists`,
   `test_oxi_contrast`, `test_oxi_suite`, `test_oxi_palette_is_soft_slate`.

`oxide-dark` (the prior GrokNight-inspired neutral-gray default) is preserved
as an opt-in alternative.

---

## 6. Changing the palette later

Edit **only** the 6 seed fields of the `"oxi"` `ThemeDefinition`. Do not add
per-slot overrides — the derivation pipeline owns the rest. If a derived color
looks wrong, the fix is almost always the seed (or `min_contrast`), never a
hand-patched derived value. Re-derive OKLCH→sRGB with any standard converter;
values are stable.
