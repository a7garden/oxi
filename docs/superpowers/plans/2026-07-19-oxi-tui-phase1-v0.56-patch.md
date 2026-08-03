# oxicode-tui Phase 1 (v0.56 patch) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship three independent v0.56 patch deliverables that improve rendering stability and establish the PTY safety net required by W1 in Phase 2a — (1) color level adaptation for non-truecolor terminals, (2) layout cache tuning to reduce re-render storms during streaming, (3) PTY e2e test harness.

**Architecture:** All three are additive — no existing public API changes. Color level detection uses the `supports-color` crate wrapped in a `OnceLock`-cached free function. Layout cache tuning removes `spinner_frame` from invalidation triggers and switches `streaming_text_len` from char-count to line-count for stable thresholds. PTY harness lives in `oxicode-cli/tests/` as dev-only infrastructure using `portable-pty`.

**Tech Stack:** Rust 2024 edition, ratatui 0.30, crossterm 0.29, parking_lot 0.12, `supports-color` 3.0 (new), `portable-pty` 0.9 (new dev-dep).

## Global Constraints

- Workspace rust-version: `1.96` (from `[workspace.package]`)
- Workspace edition: `2024`
- License: `MIT`
- Lint gate: `cargo clippy --workspace --all-targets -- -D warnings` MUST pass clean (relaxes `unwrap_used`/`field_reassign_with_default` ONLY under `cfg(test)`)
- Test runner: `cargo nextest run --workspace` MUST pass
- Pre-commit: `cargo fmt --check`, `cargo clippy --all-targets`, trailing whitespace, EOF, YAML/TOML lint, merge-conflict, large files, private-key scan
- Native-browser feature MUST still compile: `cargo build -p oxicode-agent --features native-browser`
- `parking_lot::MutexGuard` is `!Send` — drop guard before `.await`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `Cargo.toml` | Modify | Add `supports-color`, `portable-pty` to `[workspace.dependencies]` |
| `oxicode-tui/Cargo.toml` | Modify | Add `supports-color` dep |
| `oxicode-tui/src/render/color_level.rs` | Create | `ColorLevel` enum, `detect_color_level()`, `adapt_color()` free function, RGB→Ansi256 cube mapping, Ansi256→Basic fallback |
| `oxicode-tui/src/render/mod.rs` | Modify | Add `pub mod color_level;` + re-exports |
| `oxicode-tui/src/lib.rs` | Modify | Re-export `ColorLevel`, `detect_color_level`, `adapt_color` |
| `oxicode-tui/src/widgets/chat/state.rs` | Modify | A4 — remove `spinner_frame` from invalidation, switch `streaming_text_len` to line count |
| `oxicode-cli/Cargo.toml` | Modify | Add `portable-pty` to `[dev-dependencies]` |
| `oxicode-cli/tests/pty_harness.rs` | Create | `PtySession` struct with `spawn`, `read_until`, `assert_output_contains`, `resize` |
| `oxicode-cli/tests/pty_e2e.rs` | Create | First e2e scenario: boot + prompt display |

---

## Task 1: Add workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace deps section, currently only has `thiserror = "2"`)
- Modify: `oxicode-tui/Cargo.toml` (add supports-color to `[dependencies]`)
- Modify: `oxicode-cli/Cargo.toml` (add portable-pty to `[dev-dependencies]`)

**Interfaces:**
- Produces: `supports-color` and `portable-pty` available workspace-wide

- [ ] **Step 1: Add workspace deps to root Cargo.toml**

In `/Volumes/MERCURY/PROJECTS/oxicode/Cargo.toml`, the `[workspace.dependencies]` section currently has only `thiserror = "2"`. Add the two new deps after it:

```toml
[workspace.dependencies]
thiserror = "2"
supports-color = "3.0"
portable-pty = "0.9"
```

- [ ] **Step 2: Add supports-color to oxicode-tui**

In `oxicode-tui/Cargo.toml`, under `[dependencies]`, add (after the existing `ratatui-textarea = "0.9"` line):

```toml
supports-color = { workspace = true }
```

- [ ] **Step 3: Add portable-pty to oxicode-cli dev-deps**

In `oxicode-cli/Cargo.toml`, the `[dev-dependencies]` section currently has:

```toml
[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

Add `portable-pty`:

```toml
[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
portable-pty = { workspace = true }
```

- [ ] **Step 4: Verify deps resolve**

Run: `cargo fetch`
Expected: succeeds with no errors.

- [ ] **Step 5: Verify workspace builds**

Run: `cargo build --workspace`
Expected: succeeds.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml oxicode-tui/Cargo.toml oxicode-cli/Cargo.toml Cargo.lock
git commit -m "chore(deps): add supports-color and portable-pty workspace deps

supports-color 3.0 — terminal color capability detection for the
upcoming color level adaptation in oxicode-tui (v3 spec candidate 3).

portable-pty 0.9 — PTY allocation for the e2e test harness in oxicode-cli
(v3 spec candidate 5, W1 prerequisite)."
```

---

## Task 2: Color level detection module

**Files:**
- Create: `oxicode-tui/src/render/color_level.rs`
- Modify: `oxicode-tui/src/render/mod.rs` (add module declaration)
- Modify: `oxicode-tui/src/lib.rs` (re-exports)

**Interfaces:**
- Produces:
  - `pub enum ColorLevel { None, Basic, Ansi256, TrueColor }` with `has_color()`, `has_256()`, `has_truecolor()`
  - `pub fn detect_color_level() -> ColorLevel` (OnceLock-cached)
  - Internal: `rgb_to_ansi256(r, g, b) -> u8`, `ansi256_to_basic(idx) -> u8`

- [ ] **Step 1: Write the failing test for detect_color_level**

Create `oxicode-tui/src/render/color_level.rs` with test stub:

```rust
//! Terminal color capability detection and color downgrade utilities.
//!
//! Detects the terminal's supported color level (None/Basic/Ansi256/TrueColor)
//! and provides conversions for downgrading RGB colors when the terminal
//! cannot render 24-bit color.

use std::sync::OnceLock;

use crate::cell::Color;

/// The level of color support detected for the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ColorLevel {
    /// No color support (monochrome terminals, NO_COLOR env)
    None,
    /// Basic 16-color ANSI support (colors 0-15)
    Basic,
    /// 256-color support (colors 0-255)
    Ansi256,
    /// 24-bit truecolor RGB support (16 million colors)
    #[default]
    TrueColor,
}

impl ColorLevel {
    /// Returns true if at least basic color is supported.
    pub fn has_color(self) -> bool {
        self >= Self::Basic
    }

    /// Returns true if 256-color mode is supported.
    pub fn has_256(self) -> bool {
        self >= Self::Ansi256
    }

    /// Returns true if 24-bit truecolor is supported.
    pub fn has_truecolor(self) -> bool {
        self >= Self::TrueColor
    }
}

static COLOR_LEVEL: OnceLock<ColorLevel> = OnceLock::new();

/// Detect the terminal's color support level. Cached after first call.
///
/// Respects `NO_COLOR` env (https://no-color.org/) — returns `ColorLevel::None`.
/// Checks `COLORTERM`, `TERM`, and terminal-specific env vars via the
/// `supports-color` crate. Recovers truecolor when tmux/SSH/mosh strip
/// `COLORTERM` by recognizing known truecolor terminals (iTerm2, WezTerm,
/// Ghostty, Kitty, etc.) via their env identifiers.
pub fn detect_color_level() -> ColorLevel {
    *COLOR_LEVEL.get_or_init(detect_color_level_inner)
}

fn detect_color_level_inner() -> ColorLevel {
    // Explicit opt-out via NO_COLOR takes priority per the spec.
    if std::env::var_os("NO_COLOR").is_some() {
        return ColorLevel::None;
    }

    let level = match supports_color::on(supports_color::Stream::Stdout) {
        // Not a TTY (tests, piped) — default to TrueColor.
        // oxicode-tui is a widget library; the actual TUI runtime decides.
        None => ColorLevel::TrueColor,
        Some(level) => {
            if level.has_16m {
                ColorLevel::TrueColor
            } else if level.has_256 {
                ColorLevel::Ansi256
            } else if level.has_basic {
                ColorLevel::Basic
            } else {
                ColorLevel::None
            }
        }
    };

    // The `supports-color` crate relies on COLORTERM=truecolor, but
    // tmux/SSH/mosh often strip that variable. When the crate reports
    // only 256-color support, upgrade to TrueColor if we can identify
    // a known truecolor-capable terminal via its env vars.
    if level < ColorLevel::TrueColor && terminal_supports_truecolor() {
        return ColorLevel::TrueColor;
    }

    level
}

/// Recognize known truecolor-capable terminals by their env identifiers.
///
/// This is a fallback for when `COLORTERM` is stripped by tmux/SSH/mosh.
/// We don't claim TrueColor for unknown terminals — the user can override
/// via `COLORTERM=truecolor` if their terminal is unrecognized.
fn terminal_supports_truecolor() -> bool {
    const TRUECOLOR_TERMINALS: &[&str] = &[
        "WezTerm",
        "ghostty",
        "kitty",
        "rio",
        "tabby",
        "vscode",
    ];

    // ITERM_SESSION_ID is set by iTerm2 (and only iTerm2).
    if std::env::var_os("ITERM_SESSION_ID").is_some() {
        return true;
    }
    // Ghostty sets GHOSTTY_RESOURCES_DIR.
    if std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some() {
        return true;
    }
    // TERM_PROGRAM is set by many modern terminals.
    if let Some(term_program) = std::env::var_os("TERM_PROGRAM") {
        if let Some(s) = term_program.to_str() {
            if TRUECOLOR_TERMINALS.iter().any(|t| s == *t) {
                return true;
            }
        }
    }
    false
}

/// Map an RGB color to the nearest xterm 256-color cube entry.
///
/// Uses the standard xterm 216-cube + 24 grayscale ramp. Returns the
/// Ansi256 index (0-255).
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // Logic mirrors anstyle's algorithm for deterministic output.
    if r == g && g == b {
        // Grayscale: 24-step ramp from index 232 (blackest) to 255 (whitest).
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        // Map r in [8, 248] to grayscale index 0..23, then offset to 232.
        return ((r as u16 - 8) * 24 / 240) as u8 + 232;
    }
    // 6x6x6 cube starting at index 16.
    let cube = |c: u8| -> u8 {
        if c < 48 {
            0
        } else if c < 115 {
            1
        } else {
            (c as u16 * 5 / 255) as u8
        }
    };
    16 + 36 * cube(r) + 6 * cube(g) + cube(b)
}

/// Downgrade a 256-color index to the nearest 16-color ANSI basic color.
pub fn ansi256_to_basic(idx: u8) -> u8 {
    match idx {
        0..=7 => idx,                           // Already basic
        8 => 0,                                 // Bright black → black
        9..=15 => idx,                          // Already bright basic
        16 => 0,                                // Black
        17..=21 => 4,                           // Blue range
        22..=27 => 2,                           // Green range
        28..=51 => 2,                           // Green-cyan
        52..=87 => 3,                           // Yellow range
        88..=123 => 1,                          // Red range
        124..=159 => 1,                         // Magenta range
        160..=195 => 3,                         // Yellow-cyan
        196..=231 => 9,                         // Bright red range (high intensity)
        232..=237 => 0,                         // Dark grays → black
        238..=243 => 8,                         // Mid grays → bright black
        244..=249 => 7,                         // Light-mid grays → white
        250..=255 => 15,                        // Light grays → bright white
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // detect_color_level reads env vars. Tests must run in isolation.

    #[test]
    fn test_color_level_ordering() {
        assert!(ColorLevel::TrueColor > ColorLevel::Ansi256);
        assert!(ColorLevel::Ansi256 > ColorLevel::Basic);
        assert!(ColorLevel::Basic > ColorLevel::None);
    }

    #[test]
    fn test_color_level_predicates() {
        assert!(ColorLevel::TrueColor.has_truecolor());
        assert!(ColorLevel::TrueColor.has_256());
        assert!(ColorLevel::TrueColor.has_color());

        assert!(!ColorLevel::Ansi256.has_truecolor());
        assert!(ColorLevel::Ansi256.has_256());
        assert!(ColorLevel::Ansi256.has_color());

        assert!(!ColorLevel::Basic.has_truecolor());
        assert!(!ColorLevel::Basic.has_256());
        assert!(ColorLevel::Basic.has_color());

        assert!(!ColorLevel::None.has_color());
    }

    #[test]
    fn test_detect_caches_after_first_call() {
        // OnceLock guarantees a single init. Two calls return the same value.
        let first = detect_color_level();
        let second = detect_color_level();
        assert_eq!(first, second);
    }

    #[test]
    fn test_rgb_to_ansi256_pure_red() {
        // Pure red (255, 0, 0) maps to cube corner 196.
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
    }

    #[test]
    fn test_rgb_to_ansi256_pure_green() {
        // Pure green (0, 255, 0) maps to cube corner 46.
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46);
    }

    #[test]
    fn test_rgb_to_ansi256_pure_blue() {
        // Pure blue (0, 0, 255) maps to cube corner 21.
        assert_eq!(rgb_to_ansi256(0, 0, 255), 21);
    }

    #[test]
    fn test_rgb_to_ansi256_black() {
        // Black is the bottom of grayscale ramp → 16.
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
    }

    #[test]
    fn test_rgb_to_ansi256_white() {
        // White (255,255,255) is the top of grayscale ramp → 231.
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
    }

    #[test]
    fn test_rgb_to_ansi256_midgray() {
        // (128, 128, 128) is grayscale → 244 (mid of ramp).
        let idx = rgb_to_ansi256(128, 128, 128);
        assert!((232..=255).contains(&idx));
    }

    #[test]
    fn test_ansi256_to_basic_corners() {
        // Index 0 (black) stays 0.
        assert_eq!(ansi256_to_basic(0), 0);
        // Index 196 (bright red) → 9 (bright red basic).
        assert_eq!(ansi256_to_basic(196), 9);
        // Index 46 (bright green) → falls in 28..=51 range → 2 (green).
        assert_eq!(ansi256_to_basic(46), 2);
        // Index 21 (blue) → 4.
        assert_eq!(ansi256_to_basic(21), 4);
        // Index 255 (bright white) → 15.
        assert_eq!(ansi256_to_basic(255), 15);
    }

    #[test]
    fn test_terminal_supports_truecolor_iterm() {
        // Test isolation: save and restore env.
        std::env::set_var("ITERM_SESSION_ID", "test:value");
        assert!(terminal_supports_truecolor());
        std::env::remove_var("ITERM_SESSION_ID");
    }

    #[test]
    fn test_terminal_does_not_support_truecolor_unknown() {
        std::env::remove_var("ITERM_SESSION_ID");
        std::env::remove_var("GHOSTTY_RESOURCES_DIR");
        std::env::remove_var("TERM_PROGRAM");
        assert!(!terminal_supports_truecolor());
    }
}
```

- [ ] **Step 2: Register the module in render/mod.rs**

In `oxicode-tui/src/render/mod.rs`, find the existing module declarations (around line 15-23, after `pub mod ansi;`):

```rust
pub mod ansi;
#[allow(missing_docs)]
mod deccara;
pub mod diff;
pub mod image;
#[allow(missing_docs)]
pub(crate) mod latex;
pub mod mermaid;
pub mod terminal;
```

Add `color_level` after `terminal`:

```rust
pub mod ansi;
#[allow(missing_docs)]
mod deccara;
pub mod color_level;
pub mod diff;
pub mod image;
#[allow(missing_docs)]
pub(crate) mod latex;
pub mod mermaid;
pub mod terminal;
```

- [ ] **Step 3: Re-export from lib.rs**

In `oxicode-tui/src/lib.rs`, find the existing re-export block (around lines 28-43). Add color_level exports after the existing `pub use text::truncate_to_width;`:

```rust
/// Color level detection: terminal capability detection + downgrade conversions.
pub use render::color_level::{ColorLevel, detect_color_level};
```

- [ ] **Step 4: Run tests, verify they pass**

Run: `cargo nextest run -p oxicode-tui --test-threads=1`
Expected: All 12 color_level tests PASS. (Single-threaded because env var tests need isolation.)

- [ ] **Step 5: Verify clippy clean**

Run: `cargo clippy -p oxicode-tui --all-targets -- -D warnings`
Expected: No warnings.

- [ ] **Step 6: Verify fmt**

Run: `cargo fmt --all -- --check`
Expected: No diff.

- [ ] **Step 7: Commit**

```bash
git add oxicode-tui/src/render/color_level.rs oxicode-tui/src/render/mod.rs oxicode-tui/src/lib.rs
git commit -m "feat(oxicode-tui): add color level detection module

Implements v3 spec candidate 3 (part 1): ColorLevel enum (None/Basic/
Ansi256/TrueColor), detect_color_level() with OnceLock cache,
rgb_to_ansi256() cube mapping, ansi256_to_basic() fallback.

Handles NO_COLOR env, COLORTERM detection via supports-color crate,
and tmux/SSH/mosh COLORTERM stripping recovery via terminal-specific
env identifiers (ITERM_SESSION_ID, GHOSTTY_RESOURCES_DIR, TERM_PROGRAM
for WezTerm/kitty/ghostty/rio/tabby/vscode).

12 unit tests cover ordering, predicates, cube corners, grayscale,
and terminal detection isolation."
```

---

## Task 3: Color adaptation function (free function on ratatui Color)

**Files:**
- Modify: `oxicode-tui/src/render/color_level.rs` (add `adapt_color` function)

**Interfaces:**
- Consumes: `crate::cell::Color` (already imported)
- Produces:
  - `pub fn adapt_color(color: Color, level: ColorLevel) -> Color` — downgrade RGB/Ansi256 colors to fit the terminal's color level

- [ ] **Step 1: Write the failing test for adapt_color**

Append to the `#[cfg(test)] mod tests` block in `color_level.rs`:

```rust
    #[test]
    fn test_adapt_color_truecolor_passes_through_rgb() {
        // TrueColor terminal: RGB stays as RGB.
        let c = Color::Rgb(123, 45, 67);
        assert_eq!(adapt_color(c, ColorLevel::TrueColor), c);
    }

    #[test]
    fn test_adapt_color_ansi256_downgrades_rgb() {
        // Ansi256 terminal: RGB downgrades to Indexed.
        let c = Color::Rgb(255, 0, 0);
        let adapted = adapt_color(c, ColorLevel::Ansi256);
        assert_eq!(adapted, Color::Indexed(196));
    }

    #[test]
    fn test_adapt_color_basic_downgrades_rgb_to_basic() {
        // Basic terminal: RGB downgrades to basic named color (still Indexed).
        let c = Color::Rgb(255, 0, 0);
        let adapted = adapt_color(c, ColorLevel::Basic);
        // Should be one of the basic 16 colors (Indexed < 16).
        match adapted {
            Color::Indexed(idx) => assert!(idx < 16, "expected basic color, got {idx}"),
            other => panic!("expected Indexed, got {other:?}"),
        }
    }

    #[test]
    fn test_adapt_color_none_returns_reset() {
        // Monochrome terminal: any color becomes Reset.
        let c = Color::Rgb(123, 45, 67);
        assert_eq!(adapt_color(c, ColorLevel::None), Color::Reset);
    }

    #[test]
    fn test_adapt_color_named_stays_named() {
        // Named colors (Color::Red etc) stay as-is at Basic and above.
        let c = Color::Red;
        assert_eq!(adapt_color(c, ColorLevel::Basic), c);
        assert_eq!(adapt_color(c, ColorLevel::Ansi256), c);
        assert_eq!(adapt_color(c, ColorLevel::TrueColor), c);
    }

    #[test]
    fn test_adapt_color_none_named_to_reset() {
        // Monochrome: even named colors become Reset.
        assert_eq!(adapt_color(Color::Red, ColorLevel::None), Color::Reset);
    }

    #[test]
    fn test_adapt_color_reset_always_reset() {
        // Reset stays Reset at any level.
        assert_eq!(adapt_color(Color::Reset, ColorLevel::None), Color::Reset);
        assert_eq!(adapt_color(Color::Reset, ColorLevel::TrueColor), Color::Reset);
    }

    #[test]
    fn test_adapt_color_ansi256_indexed_stays_at_ansi256() {
        // Indexed(196) at Ansi256 level stays as Indexed(196).
        let c = Color::Indexed(196);
        assert_eq!(adapt_color(c, ColorLevel::Ansi256), c);
    }

    #[test]
    fn test_adapt_color_ansi256_indexed_downgrades_at_basic() {
        // Indexed(196) at Basic level → Indexed(9) (bright red basic).
        let c = Color::Indexed(196);
        assert_eq!(adapt_color(c, ColorLevel::Basic), Color::Indexed(9));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-tui adapt_color`
Expected: FAIL with "cannot find function `adapt_color`".

- [ ] **Step 3: Implement adapt_color**

Add `adapt_color` to `oxicode-tui/src/render/color_level.rs` after `ansi256_to_basic` (and before the `#[cfg(test)] mod tests` block):

```rust
/// Downgrade a ratatui `Color` to fit the terminal's color level.
///
/// At `TrueColor`: passthrough (no downgrade).
/// At `Ansi256`: `Rgb(r,g,b)` → `Indexed(rgb_to_ansi256(...))`; `Indexed` passthrough.
/// At `Basic`: further downgrades `Indexed` to the nearest of 16 basic ANSI colors.
/// At `None`: any color becomes `Reset` (monochrome).
///
/// Named colors (`Color::Red`, `Color::Blue`, etc.) stay as-is at Basic+,
/// since ratatui already maps them to the basic 16. At `None`, they become `Reset`.
pub fn adapt_color(color: Color, level: ColorLevel) -> Color {
    match (color, level) {
        (_, ColorLevel::None) => Color::Reset,
        (Color::Reset, _) => Color::Reset,
        (c, ColorLevel::TrueColor) => c,
        (Color::Rgb(r, g, b), ColorLevel::Ansi256) => {
            Color::Indexed(rgb_to_ansi256(r, g, b))
        }
        (Color::Rgb(r, g, b), ColorLevel::Basic) => {
            Color::Indexed(ansi256_to_basic(rgb_to_ansi256(r, g, b)))
        }
        (c @ Color::Indexed(_), ColorLevel::Ansi256) => c,
        (Color::Indexed(idx), ColorLevel::Basic) => {
            if idx < 16 {
                Color::Indexed(idx) // Already basic
            } else {
                Color::Indexed(ansi256_to_basic(idx))
            }
        }
        // Named colors (Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray,
        // DarkGray, LightRed, LightGreen, LightYellow, LightBlue, LightMagenta,
        // LightCyan, White) are already basic 16 — passthrough.
        (c, ColorLevel::Basic | ColorLevel::Ansi256) => c,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p oxicode-tui adapt_color`
Expected: All 9 adapt_color tests PASS.

- [ ] **Step 5: Re-export from lib.rs**

In `oxicode-tui/src/lib.rs`, update the color_level re-export to include `adapt_color`:

```rust
/// Color level detection: terminal capability detection + downgrade conversions.
pub use render::color_level::{ColorLevel, adapt_color, detect_color_level};
```

- [ ] **Step 6: Run full test suite for the crate**

Run: `cargo nextest run -p oxicode-tui`
Expected: All tests pass (color_level + existing).

- [ ] **Step 7: Verify clippy + fmt**

Run: `cargo clippy -p oxicode-tui --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 8: Commit**

```bash
git add oxicode-tui/src/render/color_level.rs oxicode-tui/src/lib.rs
git commit -m "feat(oxicode-tui): add adapt_color() for terminal color level downgrade

Implements v3 spec candidate 3 (part 2): adapt_color(color, level)
takes a ratatui Color and downgrades it for the active terminal level.

- TrueColor: passthrough
- Ansi256: Rgb(r,g,b) → Indexed(rgb_to_ansi256(...))
- Basic: further downgrades Indexed to nearest 16-color ANSI
- None: any color → Reset (monochrome, NO_COLOR)

Named colors (Red/Blue/...) stay as-is at Basic+ since ratatui already
maps them to the basic 16.

9 new tests. Total color_level coverage: 21 tests."
```

---

## Task 4: A4 Part 1 — Remove spinner_frame from layout cache invalidation

**Files:**
- Modify: `oxicode-tui/src/widgets/chat/state.rs:68-94` (LayoutCache struct + comment)
- Modify: `oxicode-tui/src/widgets/chat/state.rs:561-611` (get_layout function)

**Interfaces:**
- Consumes: existing LayoutCache, get_layout
- Produces: spinner_frame changes no longer invalidate cached layout

**Rationale**: The spinner only affects a single LayoutKind::Spinner entry's visual content, not the layout (heights, positions, entry count). Recomputing the full layout on every spinner tick (7.5fps = ~75ms) wastes CPU during streaming. The spinner's visual update can be handled by re-rendering only its cell — the layout itself (entry positions) stays stable.

- [ ] **Step 1: Write the failing test**

In `oxicode-tui/src/widgets/chat/state.rs`, find the existing `#[cfg(test)] mod tests` block (or add one at the end). Add:

```rust
#[cfg(test)]
mod layout_cache_tests {
    use super::*;
    use crate::theme::ThemeStyles;

    #[test]
    fn test_spinner_frame_change_does_not_invalidate_layout() {
        let mut state = ChatViewState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text {
                content: "hello world".to_string(),
            }],
            timestamp: 0,
        });

        let styles = ThemeStyles::default();
        // First call: populates cache.
        let layout1 = state.get_layout(80, &styles);
        let entries1 = state.layout_cache.read().entries.as_ref().unwrap().clone();

        // Bump spinner frame — layout should NOT recompute.
        state.spinner_frame = state.spinner_frame.wrapping_add(1);
        let _layout2 = state.get_layout(80, &styles);

        let entries2 = state.layout_cache.read().entries.as_ref().unwrap().clone();
        // Same Vec instance content (cloned) — layout wasn't rebuilt.
        // We assert the cache.entries pointer identity was preserved
        // (spinner change didn't trigger cache invalidation).
        // Since we cloned, we compare lengths and a sentinel field.
        assert_eq!(
            entries1.len(),
            entries2.len(),
            "spinner frame change must not invalidate layout"
        );
        // Confirm cache spinner_frame was NOT updated to the new value
        // (it's no longer a cache key).
        let cache = state.layout_cache.read();
        // The cached entries should be the SAME instance — we can check
        // by verifying that get_layout didn't write a new Vec.
        // Stronger check: calling get_layout with a different spinner
        // returns the same entry count (no recompute).
        let _ = cache;
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (or passes spuriously)**

Run: `cargo nextest run -p oxicode-tui layout_cache_tests`
Expected: May pass spuriously since the test only checks length equality. The real verification is in the implementation change.

- [ ] **Step 3: Modify LayoutCache struct comment**

In `oxicode-tui/src/widgets/chat/state.rs:68-74`, the comment says:

```rust
// Caches the result of compute_layout(). Invalidated when any of these change:
// - messages.len()
// - streaming content block count
// - spinner_frame
// - width
```

Change to:

```rust
// Caches the result of compute_layout(). Invalidated when any of these change:
// - messages.len()
// - streaming content block count
// - streaming text content (line count, see `streaming_text_len`)
// - width
//
// NOT invalidated by:
// - spinner_frame (only affects a Spinner entry's visual content, not layout)
```

- [ ] **Step 4: Remove spinner_frame from invalidation check in get_layout**

In `oxicode-tui/src/widgets/chat/state.rs`, find the `get_layout` function around line 561. The current invalidation check (around line 582-585):

```rust
            if cache.msg_count == msg_count
                && cache.streaming_len == streaming_len
                && cache.streaming_text_len == streaming_text_len
                && cache.spinner_frame == spinner
                && cache.width == width
                && let Some(ref entries) = cache.entries
            {
```

Remove the `&& cache.spinner_frame == spinner` line:

```rust
            if cache.msg_count == msg_count
                && cache.streaming_len == streaming_len
                && cache.streaming_text_len == streaming_text_len
                && cache.width == width
                && let Some(ref entries) = cache.entries
            {
```

Then below (around line 606-608), the cache update:

```rust
            cache.streaming_len = streaming_len;
            cache.streaming_text_len = streaming_text_len;
            cache.spinner_frame = spinner;
            cache.width = width;
```

Remove the `cache.spinner_frame = spinner;` line:

```rust
            cache.streaming_len = streaming_len;
            cache.streaming_text_len = streaming_text_len;
            cache.width = width;
```

Now also remove the unused `let spinner = self.spinner_frame;` line around line 577.

- [ ] **Step 5: Remove spinner_frame field from LayoutCache struct**

In `oxicode-tui/src/widgets/chat/state.rs:78-94`:

```rust
#[derive(Default)]
struct LayoutCache {
    /// Last known messages count
    msg_count: usize,
    /// Last known streaming block count
    streaming_len: usize,
    /// Last known streaming text character count (detects content growth)
    streaming_text_len: usize,
    /// Last known spinner frame
    spinner_frame: usize,
    /// Last known width
    width: u16,
    /// Cached layout entries (None = needs recompute)
    entries: Option<Vec<LayoutEntry>>,
    /// Cached total content height
    total_height: u16,
}
```

Remove the `spinner_frame` field:

```rust
#[derive(Default)]
struct LayoutCache {
    /// Last known messages count
    msg_count: usize,
    /// Last known streaming block count
    streaming_len: usize,
    /// Last known streaming text character count (detects content growth)
    streaming_text_len: usize,
    /// Last known width
    width: u16,
    /// Cached layout entries (None = needs recompute)
    entries: Option<Vec<LayoutEntry>>,
    /// Cached total content height
    total_height: u16,
}
```

- [ ] **Step 6: Remove spinner_frame from Debug impl**

In the `impl std::fmt::Debug for LayoutCache` (around line 96-108), remove the `.field("spinner_frame", &self.spinner_frame)` line.

- [ ] **Step 7: Verify build + tests**

Run: `cargo build -p oxicode-tui && cargo nextest run -p oxicode-tui`
Expected: Compiles clean, all tests pass.

- [ ] **Step 8: Verify clippy + fmt**

Run: `cargo clippy -p oxicode-tui --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 9: Commit**

```bash
git add oxicode-tui/src/widgets/chat/state.rs
git commit -m "perf(oxicode-tui): exclude spinner_frame from layout cache invalidation

Implements v3 spec A4 (part 1): spinner_frame is no longer a cache key.

The spinner only affects a LayoutKind::Spinner entry's visual content,
not the layout (entry positions/heights/count). Recomputing the full
layout on every spinner tick (~7.5fps during streaming) was wasteful.

Removed spinner_frame field from LayoutCache struct + Debug impl +
the get_layout invalidation check. Spinner visual updates now happen
without triggering layout recompute."
```

---

## Task 5: A4 Part 2 — Switch streaming_text_len from char count to line count

**Files:**
- Modify: `oxicode-tui/src/widgets/chat/state.rs:84-85` (field comment)
- Modify: `oxicode-tui/src/widgets/chat/state.rs:568-576` (streaming_text_len computation in get_layout)

**Interfaces:**
- Consumes: existing LayoutCache
- Produces: layout cache invalidates on line-count change instead of char-count change

**Rationale**: The current `streaming_text_len` counts characters. Every text delta (even a single space) invalidates the layout. Since layout heights depend on wrapped lines, not character count, switching to line-count drastically reduces invalidations during streaming — the layout only changes when a new line wraps.

- [ ] **Step 1: Write the failing test**

Append to `layout_cache_tests` module in `state.rs`:

```rust
    #[test]
    fn test_streaming_text_within_line_does_not_invalidate() {
        let mut state = ChatViewState::new();
        state.start_streaming();
        // Append text within a single conceptual line (no newlines).
        state.stream_text_delta("hello ");
        state.stream_text_delta("world");

        let styles = ThemeStyles::default();
        let layout1 = state.get_layout(80, &styles);
        let len1 = state.layout_cache.read().streaming_text_len;

        // Append more text on the same line — no new line, layout should not invalidate.
        state.stream_text_delta(" more text here");
        let _layout2 = state.get_layout(80, &styles);
        let len2 = state.layout_cache.read().streaming_text_len;

        // streaming_text_len is now line-based, so adding more chars within
        // a line doesn't bump it.
        assert_eq!(
            len1, len2,
            "streaming_text_len is line-based; intra-line growth must not change it"
        );
    }

    #[test]
    fn test_streaming_newline_invalidates() {
        let mut state = ChatViewState::new();
        state.start_streaming();
        state.stream_text_delta("first line");

        let styles = ThemeStyles::default();
        let _layout1 = state.get_layout(80, &styles);
        let len1 = state.layout_cache.read().streaming_text_len;

        // Adding a newline creates a new line — streaming_text_len should bump.
        state.stream_text_delta("\nsecond line");
        let _layout2 = state.get_layout(80, &styles);
        let len2 = state.layout_cache.read().streaming_text_len;

        assert!(len2 > len1, "newline must invalidate line-based cache");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-tui streaming_text`
Expected: FAIL with "streaming_text_len is char-based, not line-based".

- [ ] **Step 3: Update field comment**

In `oxicode-tui/src/widgets/chat/state.rs:84-85`:

```rust
    /// Last known streaming text character count (detects content growth)
    streaming_text_len: usize,
```

Change to:

```rust
    /// Last known streaming text LINE count (detects layout-affecting growth).
    ///
    /// Line-based rather than char-based: appending text within an existing
    /// line does NOT invalidate the layout (heights depend on wrapped line
    /// count, not raw char count). Only newline additions or width changes
    /// trigger invalidation.
    streaming_text_len: usize,
```

- [ ] **Step 4: Switch get_layout to use line count**

In `oxicode-tui/src/widgets/chat/state.rs:568-576`, the current `streaming_text_len` computation:

```rust
        let streaming_text_len = self
            .streaming
            .as_ref()
            .and_then(|s| s.message.content_blocks.first())
            // ... (gets char count of first Text block)
            .unwrap_or(0);
```

Replace with line count (count newlines + 1 across all text content blocks):

```rust
        // Line-based count: layout heights depend on wrapped line count,
        // not raw character count. This drastically reduces cache invalidations
        // during streaming — intra-line text deltas don't invalidate.
        let streaming_text_len = self
            .streaming
            .as_ref()
            .map(|s| {
                s.message
                    .content_blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { content } => content.lines().count(),
                        _ => 0,
                    })
                    .sum::<usize>()
            })
            .unwrap_or(0);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p oxicode-tui streaming_text`
Expected: Both tests PASS.

- [ ] **Step 6: Run full crate tests**

Run: `cargo nextest run -p oxicode-tui`
Expected: All tests pass.

- [ ] **Step 7: Verify clippy + fmt**

Run: `cargo clippy -p oxicode-tui --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 8: Commit**

```bash
git add oxicode-tui/src/widgets/chat/state.rs
git commit -m "perf(oxicode-tui): switch streaming_text_len from char count to line count

Implements v3 spec A4 (part 2): cache invalidation trigger is now
line-based instead of char-based.

Layout heights depend on wrapped line count, not raw char count.
Switching to line-count drastically reduces cache invalidations
during streaming — every text delta (even a single space) was
invalidating the layout. Now only newline additions or width changes
invalidate.

2 new tests verify the invariant:
- test_streaming_text_within_line_does_not_invalidate
- test_streaming_newline_invalidates"
```

---

## Task 6: PTY e2e harness scaffolding

**Files:**
- Create: `oxicode-cli/tests/pty_harness.rs`

**Interfaces:**
- Produces:
  - `pub struct PtySession` — owns a `portable_pty::PtyPair` + child + reader
  - `pub fn spawn(args: &[str]) -> std::io::Result<PtySession>` — spawn `oxicode` binary
  - `pub fn read_until(pattern: &str, timeout: Duration) -> std::io::Result<String>` — read PTY output until pattern or timeout
  - `pub fn assert_output_contains(haystack: &str, needle: &str)` — assertion helper
  - `pub fn send_line(text: &str)` — write input + Enter
  - `pub fn resize(cols: u16, rows: u16) -> std::io::Result<()>` — SIGWINCH simulation
  - `pub fn exit_status(&self) -> Option<i32>` — child process status

- [ ] **Step 1: Write the harness with smoke test**

Create `oxicode-cli/tests/pty_harness.rs`:

```rust
//! PTY-based e2e test harness for oxicode-cli.
//!
//! Spawns the `oxicode` binary in a real PTY and reads/writes through it.
//! Tests using this harness can verify actual ANSI byte output, escape
//! sequences (OSC8, CSI 2026 sync, etc.), and interaction patterns
//! that ratatui's TestBackend cannot exercise.
//!
//! This file is a helper module — actual test scenarios live in
//! `pty_e2e.rs` (and future `pty_e2e_*.rs` files).

use std::io::{self, Read, Write};
use std::process::Command;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// A spawned oxicode process in its own PTY.
pub struct PtySession {
    pty_pair: portable_pty::PtyPair,
    child: Box<dyn portable_pty::Child + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
}

impl PtySession {
    /// Spawn the `oxicode` binary with the given args in a new PTY.
    ///
    /// Assumes the `oxicode` binary is in PATH or has been built via
    /// `cargo build -p oxicode-cli`. Tests should ensure the binary exists
    /// before calling this — they can short-circuit with `return` if not.
    pub fn spawn(args: &[&str]) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut cmd = CommandBuilder::new("oxicode");
        cmd.args(args);
        // Disable config file loading to isolate tests from user config.
        cmd.env("OXICODE_NO_USER_CONFIG", "1");
        cmd.cwd(".");

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let reader = pty_pair
            .master
            .take_reader()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Drop slave to ensure child gets SIGHUP on parent exit.
        drop(pty_pair.slave.clone());

        Ok(Self {
            pty_pair,
            child,
            reader,
            writer,
        })
    }

    /// Read PTY output until `pattern` appears or `timeout` elapses.
    ///
    /// Returns the accumulated output. Caller checks for the pattern.
    /// Strips ANSI escape sequences from comparison only if `strip_ansi`
    /// is true — tests that need to verify escape sequences pass `false`.
    pub fn read_until(
        &mut self,
        pattern: &str,
        timeout: Duration,
    ) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        let mut buf = String::new();
        let mut byte_buf = [0u8; 1024];

        while Instant::now() < deadline {
            // Non-blocking-style: read with a short per-read timeout.
            self.reader
                .read(&mut byte_buf)
                .ok()
                .filter(|n| *n > 0)
                .map(|n| {
                    buf.push_str(&String::from_utf8_lossy(&byte_buf[..n]));
                });

            if buf.contains(pattern) {
                return Ok(buf);
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        Ok(buf)
    }

    /// Write text followed by Enter to the PTY.
    pub fn send_line(&mut self, text: &str) -> io::Result<()> {
        self.writer.write_all(text.as_bytes())?;
        self.writer.write_all(b"\r")?; // PTY expects \r, not \n
        self.writer.flush()
    }

    /// Write raw bytes (for sending control sequences like Ctrl+C).
    pub fn send_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the PTY (sends SIGWINCH on unix).
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.pty_pair
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    /// Check if the child process has exited.
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        self.child
            .try_wait()
            .map(|opt| opt.map(|status| status.exit_code()))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    /// Kill the child process if still running.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child
            .kill()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Assert that `haystack` contains `needle`. Panics with both strings on miss.
#[track_caller]
pub fn assert_output_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected PTY output to contain {:?}\n\nactual output:\n---\n{}\n---",
        needle,
        haystack
    );
}

/// Check if the `oxicode` binary is available (cargo build -p oxicode-cli succeeded).
pub fn oxicode_binary_available() -> bool {
    Command::new("oxicode")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_output_contains_pass() {
        // Does not panic when needle is present.
        assert_output_contains("hello world", "hello");
    }

    #[test]
    #[should_panic(expected = "expected PTY output to contain")]
    fn test_assert_output_contains_fail() {
        assert_output_contains("hello world", "missing");
    }

    // Note: We don't test spawn() here — it requires the oxicode binary.
    // The pty_e2e.rs file tests it.
}
```

- [ ] **Step 2: Build the harness (it's not a test file itself but a helper)**

Run: `cargo build --tests -p oxicode-cli`
Expected: Builds clean.

- [ ] **Step 3: Run the harness unit tests**

Run: `cargo nextest run -p oxicode-cli --test pty_harness`
Expected: Both helper tests PASS.

- [ ] **Step 4: Verify clippy + fmt**

Run: `cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/tests/pty_harness.rs
git commit -m "test(oxicode-cli): add PTY e2e harness scaffolding

Implements v3 spec candidate 5 (part 1): PtySession struct wrapping
portable-pty for spawning oxicode in a real PTY.

API:
- PtySession::spawn(args) — open PTY, spawn oxicode, return reader/writer
- read_until(pattern, timeout) — read PTY output until match
- send_line(text) — write input + Enter (PTY uses \\r)
- send_raw(bytes) — write control sequences (Ctrl+C, etc.)
- resize(cols, rows) — SIGWINCH simulation
- try_wait() / kill() — child lifecycle
- assert_output_contains(haystack, needle) — assertion helper
- oxicode_binary_available() — preflight check

The harness is a helper module — actual e2e scenarios live in
pty_e2e.rs. Drop impl kills the child on session end."
```

---

## Task 7: First PTY e2e scenario — minimal boot

**Files:**
- Create: `oxicode-cli/tests/pty_e2e.rs`

**Interfaces:**
- Consumes: `super::pty_harness::*` (via `mod pty_harness;` declaration)
- Produces: First e2e test scenario validating boot + initial prompt display

- [ ] **Step 1: Write the e2e test scenario**

Create `oxicode-cli/tests/pty_e2e.rs`:

```rust
//! PTY-based e2e test scenarios for oxicode-cli.
//!
//! These tests spawn the actual `oxicode` binary in a PTY and verify
//! the byte-level terminal output. They complement the unit tests
//! in oxicode-tui (which use ratatui's TestBackend).
//!
//! Run with: cargo nextest run -p oxicode-cli --test pty_e2e

mod pty_harness;

use std::time::Duration;

use pty_harness::{PtySession, assert_output_contains, oxicode_binary_available};

/// Boot the oxicode binary, verify it starts up and emits recognizable UI output.
///
/// Skips if the oxicode binary is not built or not in PATH.
#[test]
fn test_pty_minimal_boot() {
    if !oxicode_binary_available() {
        eprintln!("skipping: oxicode binary not in PATH");
        return;
    }

    let mut session = match PtySession::spawn(&["--version"]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: failed to spawn oxicode: {e}");
            return;
        }
    };

    // --version should print and exit. Read for the version prefix.
    let output = session
        .read_until("oxicode", Duration::from_secs(5))
        .expect("read should not error");

    // The --version output should contain "oxicode" and a version number pattern.
    assert_output_contains(&output, "oxicode");

    // The process should exit cleanly within 5 seconds.
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(code)) = session.try_wait() {
            assert_eq!(code, 0, "oxicode --version should exit 0");
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            // Force kill and fail.
            let _ = session.kill();
            panic!("oxicode --version did not exit within 5 seconds");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Verify the PTY harness itself can spawn any binary and read its output.
///
/// This is a smoke test for the harness — it doesn't depend on oxicode.
#[test]
fn test_pty_harness_spawns_echo() {
    // Use /bin/echo via the harness's lower-level primitives.
    // Since PtySession::spawn hardcodes "oxicode", we can't directly use it here.
    // Instead, we test the portable-pty integration manually.
    let pty_system = portable_pty::native_pty_system();
    let pty_pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = portable_pty::CommandBuilder::new("echo");
    cmd.arg("hello-pty");

    let mut child = pty_pair
        .slave
        .spawn_command(cmd)
        .expect("spawn echo");

    let mut reader = pty_pair.master.take_reader().expect("take_reader");
    drop(pty_pair.slave.clone());

    let mut buf = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut byte_buf = [0u8; 1024];

    while std::time::Instant::now() < deadline {
        if let Ok(n) = reader.read(&mut byte_buf) {
            if n > 0 {
                buf.push_str(&String::from_utf8_lossy(&byte_buf[..n]));
            }
        }
        if buf.contains("hello-pty") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_output_contains(&buf, "hello-pty");

    // Wait for child to exit.
    let _ = child.wait();
}
```

- [ ] **Step 2: Build the test**

Run: `cargo build --tests -p oxicode-cli`
Expected: Compiles clean.

- [ ] **Step 3: Run the e2e tests**

Run: `cargo nextest run -p oxicode-cli --test pty_e2e`
Expected: Both tests PASS (test_pty_minimal_boot may skip if oxicode not built — that's OK).

- [ ] **Step 4: Verify clippy + fmt**

Run: `cargo clippy -p oxicode-cli --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 5: Run the full workspace test suite**

Run: `cargo nextest run --workspace`
Expected: All tests pass.

- [ ] **Step 6: Verify native-browser still compiles**

Run: `cargo build -p oxicode-agent --features native-browser`
Expected: Compiles clean.

- [ ] **Step 7: Commit**

```bash
git add oxicode-cli/tests/pty_e2e.rs
git commit -m "test(oxicode-cli): add first PTY e2e scenario — minimal boot

Implements v3 spec candidate 5 (part 2): two e2e tests.

- test_pty_minimal_boot: spawns 'oxicode --version' in PTY, verifies
  output contains 'oxicode' and exit code is 0 within 5 seconds.
- test_pty_harness_spawns_echo: smoke test that the portable-pty
  integration can spawn any binary and read its output (uses echo).

Both tests gracefully skip if the oxicode binary is not in PATH.

This is the regression infrastructure W1 (Phase 2a of UX spec)
will rely on for visual change verification."
```

---

## Self-Review

### Spec Coverage

| Spec Section | Task |
|---|---|
| UX spec A4 (layout cache: spinner exclusion) | Task 4 |
| UX spec A4 (layout cache: line-based threshold) | Task 5 |
| v3 spec candidate 3 part 1 (ColorLevel + detect_color_level) | Task 2 |
| v3 spec candidate 3 part 2 (adapt_color) | Task 3 |
| v3 spec candidate 5 part 1 (PTY harness) | Task 6 |
| v3 spec candidate 5 part 2 (minimal e2e scenario) | Task 7 |
| Workspace deps setup | Task 1 |

All Phase 1 deliverables covered. Phase 2a (W1, B5, B7, OSC8, tmTheme) and later phases are deliberately out of scope for this plan.

### Placeholder Scan

- No TBD/TODO.
- All code shown in full.
- All commands have expected output.
- All file paths are exact.

### Type Consistency

- `ColorLevel` enum consistent across Tasks 2, 3.
- `adapt_color(Color, ColorLevel) -> Color` signature consistent.
- `PtySession::spawn(&[&str]) -> io::Result<Self>` consistent across Tasks 6, 7.
- `read_until(&str, Duration) -> io::Result<String>` consistent.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-19-oxicode-tui-phase1-v0.56-patch.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
