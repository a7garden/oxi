//! Legacy-console fallbacks for chrome glyphs that don't ship in the
//! legacy Windows ConHost default font (Consolas / Lucida Console).
//!
//! Fallbacks are ASCII where possible (`x`, `o`, `c`, `*`), or a CP437
//! glyph when one reads better and still renders on the raster font
//! (`✓` → `√` U+221A, `⇣` → `↓` U+2193).
//!
//! ConHost does no font fallback, so missing glyphs render as tofu.
//! Windows Terminal, VS Code, and modern emulators bundle fonts (or
//! fall back to one) that cover the Dingbats / symbol glyphs we use as
//! chrome — `❯` (U+276F), `❙` (U+2759), `✗` (U+2717), `✓` (U+2713),
//! `↗` (U+2197), `⧉` (U+29C9), `⇣` (U+21E3), the diamonds `◆`/`◇`/`◈`
//! (U+25C6 / U+25C7 / U+25C8), and the braille / dot progress spinners —
//! so the substitution only fires for legacy `cmd.exe` / `powershell.exe`.

use std::borrow::Cow;
use std::sync::OnceLock;

use crate::render::grok::host::HostOs;
use crate::render::grok::terminal::{TerminalName, terminal_context};

/// `"❯ "` normally, `"> "` on legacy ConHost. Always 2 columns wide.
pub fn prompt_arrow() -> &'static str {
    if is_legacy_windows_console() {
        "> "
    } else {
        "\u{276F} "
    }
}

/// Display width of [`prompt_arrow`] in columns.
pub const PROMPT_ARROW_WIDTH: u16 = 2;

/// Record indicator glyph shown above the prompt while voice capture is
/// active — a dot inside a ring. Two states swapped on the pulse cadence:
/// FISHEYE (`◉`, filled center) on the bright half and BULLSEYE (`◎`, open
/// center) on the dim half, which together with a smooth color fade reads
/// like a studio recording light. ASCII fallback (`*`/`o`) on legacy
/// ConHost. Always 1 column wide.
pub fn record_dot(filled: bool) -> &'static str {
    if is_legacy_windows_console() {
        if filled { "*" } else { "o" }
    } else if filled {
        "\u{25C9}"
    } else {
        "\u{25CE}"
    }
}

/// `"❙"` normally, `"|"` on legacy ConHost. Always 1 column wide.
pub fn collapsed_accent() -> &'static str {
    if is_legacy_windows_console() {
        "|"
    } else {
        "\u{2759}"
    }
}

/// `"✗"` (U+2717 BALLOT X) normally, `"x"` on legacy ConHost. Always 1
/// column wide.
///
/// Used for close / cancel / kill buttons and failure status markers. The
/// Dingbats `✗` is not covered by Consolas / Lucida Console, so it renders
/// as tofu on legacy `cmd.exe` / `powershell.exe` — same coverage gap as
/// the chrome glyphs above.
pub fn ballot_x() -> &'static str {
    if is_legacy_windows_console() {
        "x"
    } else {
        "\u{2717}"
    }
}

/// `"✓"` (U+2713 CHECK MARK) normally, `"√"` (U+221A SQUARE ROOT) on legacy
/// ConHost. Always 1 column wide.
///
/// The success / done sibling of [`ballot_x`]; the Dingbats `✓` shares the
/// same coverage gap on legacy consoles. The fallback `√` is a CP437 glyph
/// (code 0xFB), so it renders even on the stripped-down raster font and
/// reads as a checkmark — pairing with the `x` failure mark.
pub fn check_mark() -> &'static str {
    if is_legacy_windows_console() {
        "\u{221A}"
    } else {
        "\u{2713}"
    }
}

/// `"↗"` (U+2197 NORTH EAST ARROW) normally, `"o"` on legacy ConHost.
/// Always 1 column wide.
///
/// The enlarge / view / fullscreen button glyph. The previous glyph
/// (`⛶` U+26F6 SQUARE FOUR CORNERS) lives in the Miscellaneous Symbols
/// block and is missing from many modern monospace fonts too — not just
/// legacy Windows consoles — so it rendered as tofu (`□`) in common
/// macOS/Linux terminals. U+2197 lives in the well-covered core Arrows
/// block and reads as the standard "open / maximize" affordance.
pub fn enlarge() -> &'static str {
    if is_legacy_windows_console() {
        "o"
    } else {
        "\u{2197}"
    }
}

/// `"⧉"` (U+29C9 TWO JOINED SQUARES) normally, `"c"` on legacy ConHost.
/// Always 1 column wide.
///
/// The copy button glyph on the scrollback selection box (pairs with
/// [`enlarge`]). U+29C9 lives in Miscellaneous Mathematical Symbols-B and
/// is absent from legacy console fonts.
pub fn copy_icon() -> &'static str {
    if is_legacy_windows_console() {
        "c"
    } else {
        "\u{29C9}"
    }
}

/// `"⇣"` (U+21E3 DOWNWARDS DASHED ARROW) normally, `"↓"` (U+2193) on legacy
/// ConHost. Always 1 column wide.
///
/// Used for the context-token count in the turn-status line. The dashed
/// arrow is missing from legacy console fonts, but the plain down arrow
/// (present in CP437) is a faithful, always-renderable stand-in.
pub fn token_arrow() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2193}"
    } else {
        "\u{21E3}"
    }
}

/// Pulsing monitor-indicator frames (`○ ◎ ◉ ◎` — U+25CB WHITE CIRCLE,
/// U+25CE BULLSEYE, U+25C9 FISHEYE, U+25CE BULLSEYE) normally; a 1-column
/// dot pulse (`·`, `○`, `•`, `○`) on legacy ConHost.
///
/// Animates the "watching · N monitors" cue in the turn-status line: a
/// concentric circle that breathes open → shut like a scanning scope. Of
/// the fancy frames only the white circle `○` (U+25CB, CP437 `0x09`) is
/// part of CP437 — the bullseye `◎` and fisheye `◉` live in the Geometric
/// Shapes block and render as tofu on the legacy raster font. The legacy
/// fallback keeps the same fixed-size breath using only CP437 dots —
/// middle dot `·` (U+00B7, `0xFA`), white circle `○` (`0x09`), bullet `•`
/// (U+2022, `0x07`) — so it pulses by fill (faint → ring → solid → ring)
/// rather than by size. Every frame in both sets is exactly 1 column so the
/// trailing label never shifts as the icon animates.
pub fn monitor_icon_frames() -> &'static [&'static str] {
    const FANCY: &[&str] = &["\u{25CB}", "\u{25CE}", "\u{25C9}", "\u{25CE}"];
    const FALLBACK: &[&str] = &["\u{00B7}", "\u{25CB}", "\u{2022}", "\u{25CB}"];
    if is_legacy_windows_console() {
        FALLBACK
    } else {
        FANCY
    }
}

/// `"◆"` (U+25C6 BLACK DIAMOND) normally, `"♦"` (U+2666 BLACK DIAMOND
/// SUIT) on legacy ConHost. Always 1 column wide.
///
/// The filled diamond used for the scrollback block bullet, the
/// "waiting on you" status cues (turn-status + plan-approval), the
/// `/context` usage bar (system / messages categories), picker leaf /
/// fold indicators, and the dashboard's non-idle row markers. U+25C6 is
/// absent from the legacy console raster font, but U+2666 is a CP437
/// glyph (code `0x04`) so it renders and still reads as a filled diamond.
pub fn diamond_filled() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2666}"
    } else {
        "\u{25C6}"
    }
}

/// `"◇"` (U+25C7 WHITE DIAMOND) normally, `"○"` (U+25CB WHITE CIRCLE) on
/// legacy ConHost. Always 1 column wide.
///
/// The hollow sibling of [`diamond_filled`] — the `/context` bar's free
/// (unused) cells and the dashboard's idle row marker. U+25C7 is absent
/// from the raster font; U+25CB is a CP437 glyph (code `0x09`) that
/// renders and reads as an empty / outline marker.
pub fn diamond_hollow() -> &'static str {
    if is_legacy_windows_console() {
        "\u{25CB}"
    } else {
        "\u{25C7}"
    }
}

/// `"◈"` (U+25C8 WHITE DIAMOND CONTAINING BLACK SMALL DIAMOND) normally,
/// `"♦"` (U+2666) on legacy ConHost. Always 1 column wide.
///
/// Used for the `/context` bar's tool-definitions category and the
/// collapsed-group scrollback header. Falls back to the same CP437
/// filled diamond as [`diamond_filled`]; both call sites already
/// distinguish this category by color, so collapsing the glyph on
/// legacy consoles is lossless in practice.
pub fn diamond_dotted() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2666}"
    } else {
        "\u{25C8}"
    }
}

/// Filled-diamond glyph as a [`char`] (see [`diamond_filled`]), for the
/// tool-usage sequence bar which builds its row from single `char`s.
pub fn diamond_filled_char() -> char {
    diamond_filled().chars().next().unwrap_or('\u{25C6}')
}

/// Hollow-diamond glyph as a [`char`] (see [`diamond_hollow`]).
pub fn diamond_hollow_char() -> char {
    diamond_hollow().chars().next().unwrap_or('\u{25C7}')
}

/// Rotating braille progress-spinner frames (`⠋⠙⠹⠸⠼⠴⠦⠧`) normally; a
/// 1-column ASCII spinner (`|`, `/`, `-`, `\`) on legacy ConHost.
///
/// The U+2800 Braille Patterns block is not part of CP437 and renders as
/// tofu on the legacy console raster font, so the turn-status line, the
/// MCP-connecting chip, the image-viewer loader, and the `/btw` overlay
/// all fall back to the classic ASCII spinner there. Every frame in both
/// sets is exactly 1 column so the surrounding layout never shifts.
pub fn braille_spinner_frames() -> &'static [&'static str] {
    const FANCY: &[&str] = &[
        "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
        "\u{2827}",
    ];
    const FALLBACK: &[&str] = &["|", "/", "-", "\\"];
    if is_legacy_windows_console() {
        FALLBACK
    } else {
        FANCY
    }
}

/// Pulsing dot progress-spinner frames (`⋅ : ⸬ ⁙`) normally; a quiet
/// 1-column dot cycle (`.`, `:`, `·`) on legacy ConHost.
///
/// U+22C5 / U+2E2C / U+2059 are absent from the CP437 raster font, so the
/// running-subagent / task rows (Tasks pane + Dashboard), the dashboard
/// status chips, and the active-goal indicators fall back to a quiet dot
/// cycle there — period, colon, and `·` (U+00B7, CP437 `0xFA`) all render
/// on the raster font. Every frame in both sets is exactly 1 column.
pub fn dot_spinner_frames() -> &'static [&'static str] {
    const FANCY: &[&str] = &[
        "\u{22c5}", ":", "\u{2e2c}", "\u{2059}", "\u{22c5}", ":", "\u{2e2c}", "\u{2059}",
    ];
    const FALLBACK: &[&str] = &[".", ":", "\u{00b7}"];
    if is_legacy_windows_console() {
        FALLBACK
    } else {
        FANCY
    }
}

/// `"┃"` (U+2503 HEAVY VERTICAL) normally, `"│"` (U+2502 LIGHT VERTICAL,
/// CP437 `0xB3`) on legacy ConHost. Always 1 column wide.
///
/// The left accent rail painted beside scrollback blocks and modal
/// panels. The heavy box-drawing vertical is absent from CP437 (which
/// ships only the light `│` and double `║` verticals), so it falls back
/// to the light vertical that the raster font does render.
pub fn accent_bar() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2502}"
    } else {
        "\u{2503}"
    }
}

/// `"▴"` (U+25B4 SMALL UP-POINTING TRIANGLE) normally, `"▲"` (U+25B2,
/// CP437 `0x1E`) on legacy ConHost. Always 1 column wide.
///
/// The timeline sidebar's previous-turn chevron. The small triangles are
/// absent from CP437; the full-size ones are control-picture glyphs the
/// raster font renders.
pub fn timeline_chevron_up() -> &'static str {
    if is_legacy_windows_console() {
        "\u{25B2}"
    } else {
        "\u{25B4}"
    }
}

/// `"▾"` (U+25BE SMALL DOWN-POINTING TRIANGLE) normally, `"▼"` (U+25BC,
/// CP437 `0x1F`) on legacy ConHost. Always 1 column wide.
///
/// The timeline sidebar's next-turn chevron; see [`timeline_chevron_up`].
pub fn timeline_chevron_down() -> &'static str {
    if is_legacy_windows_console() {
        "\u{25BC}"
    } else {
        "\u{25BE}"
    }
}

/// `"━"` (U+2501 HEAVY HORIZONTAL) normally, `"─"` (U+2500 LIGHT
/// HORIZONTAL, CP437 `0xC4`) on legacy ConHost. Always 1 column wide.
///
/// Prefer [`timeline_tick_active`] for the sidebar rail — on legacy ConHost
/// this falls back to the same light stroke used for hover.
pub fn heavy_horizontal() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2500}"
    } else {
        "\u{2501}"
    }
}

/// `"─"` (U+2500 LIGHT HORIZONTAL, CP437 `0xC4`). Always 1 column wide and
/// present on every target, but exposed here so the timeline sidebar's
/// inactive ticks share one glyph source with [`heavy_horizontal`] instead
/// of hardcoding the codepoint.
pub fn light_horizontal() -> &'static str {
    "\u{2500}"
}

/// Precomposed 2-col active tick for the timeline rail: `"━━"` normally,
/// `"══"` (U+2550 BOX DRAWINGS DOUBLE HORIZONTAL, CP437 `0xCD`) on legacy
/// ConHost — distinct from the light hover/idle stroke there.
pub fn timeline_tick_active() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2550}\u{2550}"
    } else {
        "\u{2501}\u{2501}"
    }
}

/// Precomposed 2-col hover tick for the timeline rail: `"──"` (light
/// horizontal). Idle ticks reuse a single light cell; this is the wide
/// bright hover form.
pub fn timeline_tick_hover() -> &'static str {
    "\u{2500}\u{2500}"
}

/// `"●"` (U+25CF BLACK CIRCLE) normally, `"•"` (U+2022 BULLET, CP437
/// `0x07`) on legacy ConHost. Always 1 column wide.
///
/// The filled status / selection dot used in pickers, the settings and
/// permission modals, the session list, and the file-search view. Its
/// hollow partner `○` (U+25CB) is already a CP437 glyph (`0x09`) and
/// renders unchanged, so only the filled variant needs a stand-in.
pub fn filled_dot() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2022}"
    } else {
        "\u{25CF}"
    }
}

/// `"▏"` (U+258F LEFT ONE EIGHTH BLOCK) normally, `"│"` (U+2502, CP437
/// `0xB3`) on legacy ConHost. Always 1 column wide.
///
/// The thin left bar marking the selected row in the dashboard and the
/// settings panes. The eighth-width block glyphs are absent from CP437,
/// so it falls back to the light vertical.
pub fn selection_bar() -> &'static str {
    if is_legacy_windows_console() {
        "\u{2502}"
    } else {
        "\u{258F}"
    }
}

/// `"›"` (U+203A SINGLE RIGHT-POINTING ANGLE QUOTATION MARK) normally,
/// `">"` (ASCII) on legacy ConHost. Always 1 column wide.
///
/// The chevron used for collapsed fold indicators, settings breadcrumbs,
/// the integer-stepper increment affordance, and the dashboard "next"
/// button. U+203A is absent from CP437, so it falls back to the ASCII
/// greater-than sign.
pub fn chevron() -> &'static str {
    if is_legacy_windows_console() {
        ">"
    } else {
        "\u{203A}"
    }
}

/// `"‹"` (U+2039 SINGLE LEFT-POINTING ANGLE QUOTATION MARK) normally,
/// `"<"` (ASCII) on legacy ConHost. Always 1 column wide.
///
/// The mirror of [`chevron`] — the integer-stepper decrement affordance
/// and the dashboard "prev" button. Kept in lockstep so a fixed `›`/`>`
/// never sits next to a tofu `‹`.
pub fn chevron_left() -> &'static str {
    if is_legacy_windows_console() {
        "<"
    } else {
        "\u{2039}"
    }
}

/// `"⌄"` (U+2304 DOWN ARROWHEAD) normally, `"v"` (ASCII) on legacy
/// ConHost. Always 1 column wide.
///
/// The downward member of the [`chevron`] family — matches `›`'s light
/// visual weight (unlike the solid `▾` disclosure triangle). Used by the
/// scrollback expandable indicator when the selected row is an expanded
/// verb-group header. U+2304 is absent from CP437, so it falls back to a
/// lowercase `v`.
pub fn chevron_down() -> &'static str {
    if is_legacy_windows_console() {
        "v"
    } else {
        "\u{2304}"
    }
}

/// `"▾"` (U+25BE BLACK DOWN-POINTING SMALL TRIANGLE) normally, `"v"`
/// (ASCII) on legacy ConHost. Always 1 column wide.
///
/// The "expanded" disclosure indicator for a collapsible dashboard
/// section header (the section's rows are visible below it). U+25BE is
/// absent from CP437, so it falls back to a lowercase `v`.
pub fn disclosure_open() -> &'static str {
    if is_legacy_windows_console() {
        "v"
    } else {
        "\u{25BE}"
    }
}

/// `"▸"` (U+25B8 BLACK RIGHT-POINTING SMALL TRIANGLE) normally, `">"`
/// (ASCII) on legacy ConHost. Always 1 column wide.
///
/// The "collapsed" disclosure indicator for a collapsible dashboard
/// section header (the section's rows are hidden). Pairs with
/// [`disclosure_open`]; U+25B8 is absent from CP437, so it falls back to
/// the ASCII greater-than sign.
pub fn disclosure_closed() -> &'static str {
    if is_legacy_windows_console() {
        ">"
    } else {
        "\u{25B8}"
    }
}

/// `"[✗]"` normally, `"[x]"` on legacy ConHost. Always 3 columns wide.
///
/// Pre-composed bracketed form of [`ballot_x`] so per-frame render paths
/// (bg-task kill button, picker / dashboard close) reuse a `&'static str`
/// instead of allocating a `format!` each frame.
pub fn ballot_x_button() -> &'static str {
    if is_legacy_windows_console() {
        "[x]"
    } else {
        "[\u{2717}]"
    }
}

/// `"[↗]"` normally, `"[o]"` on legacy ConHost. Always 3 columns wide.
///
/// Pre-composed bracketed sibling of [`ballot_x_button`] for the bg-task
/// view / enlarge button.
pub fn enlarge_button() -> &'static str {
    if is_legacy_windows_console() {
        "[o]"
    } else {
        "[\u{2197}]"
    }
}

/// Substitute the chrome glyphs that legacy ConHost can't render with
/// legacy-console-safe equivalents (`✓` → `√`, `✗` → `x`, `⚠` → `!`) in
/// free-flowing status text such as toasts.
///
/// Unlike the fixed-width button helpers above, toasts are right-aligned
/// flowing text assembled in ~25 call sites, so a single funnel at the
/// point the toast enters view state is cleaner than threading a helper
/// through every builder. Returns a borrow unchanged on every non-legacy
/// platform, so toast strings stay byte-identical there.
pub fn legacy_glyph_fallback(s: &str) -> Cow<'_, str> {
    if !is_legacy_windows_console() {
        return Cow::Borrowed(s);
    }
    if !s.contains(['\u{2713}', '\u{2717}', '\u{26A0}']) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(to_legacy_glyphs(s))
}

/// Pure glyph → legacy-safe mapping behind [`legacy_glyph_fallback`], split
/// out so tests can exercise the substitution without faking the host probe.
/// `√` matches [`check_mark`]'s fallback; `x` matches [`ballot_x`]'s.
fn to_legacy_glyphs(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2713}' => '\u{221A}',
            '\u{2717}' => 'x',
            '\u{26A0}' => '!',
            other => other,
        })
        .collect()
}

/// True when running on native Windows in a console host whose default
/// font is known not to ship the Dingbats glyphs we use as chrome.
/// Cached for process lifetime.
///
/// `GROK_FORCE_LEGACY_CONSOLE=1` (or `true`) forces this on regardless of
/// host/terminal, and `=0` (or `false`) forces it off — a QA aid for
/// eyeballing the ASCII fallbacks (or confirming the fancy glyphs) on any
/// platform without a real ConHost.
pub fn is_legacy_windows_console() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        forced_legacy_console_override().unwrap_or_else(|| {
            // `env_brand`, not `brand`: a bare ConHost is detected as
            // `Unknown`, but `brand` optimistically becomes `WindowsTerminal`
            // on native Windows. Font capability needs the raw detection so
            // legacy consoles still get the ASCII glyph fallback.
            decide_legacy_windows_console(HostOs::current(), terminal_context().env_brand)
        })
    })
}

/// Read the `GROK_FORCE_LEGACY_CONSOLE` escape hatch from the environment.
fn forced_legacy_console_override() -> Option<bool> {
    parse_forced_legacy_console(std::env::var("GROK_FORCE_LEGACY_CONSOLE").ok().as_deref())
}

/// Pure parse of the override value so tests don't touch the environment.
/// `"1"` / `"true"` → force on, `"0"` / `"false"` → force off, anything
/// else (including unset) → `None` so normal host/brand detection runs.
fn parse_forced_legacy_console(value: Option<&str>) -> Option<bool> {
    match value {
        Some("1" | "true") => Some(true),
        Some("0" | "false") => Some(false),
        _ => None,
    }
}

/// Pure decision function so tests can drive (host, brand) pairs
/// without touching ambient state. Default-deny on Windows: an unknown
/// brand is treated as legacy, since bare `cmd.exe` / `powershell.exe`
/// in ConHost sets no terminal env vars and the brand probe returns
/// `Unknown` in exactly the case we need to catch.
fn decide_legacy_windows_console(host: HostOs, brand: TerminalName) -> bool {
    if host != HostOs::Windows {
        return false;
    }
    !matches!(
        brand,
        TerminalName::WindowsTerminal
            | TerminalName::VsCode
            | TerminalName::Cursor
            | TerminalName::Windsurf
            | TerminalName::Zed
            | TerminalName::WezTerm
            | TerminalName::Kitty
            | TerminalName::Alacritty
            | TerminalName::Ghostty
            | TerminalName::Rio
            | TerminalName::GrokDesktop
    )
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
