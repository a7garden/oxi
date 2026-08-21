//! Production bridge from the grok-build-style agent view layout
//! (`oxicode_vtui::design::layout`) to the live ratatui render path.
//!
//! `render_chrome` computes the [`AgentViewLayout`] for the current frame and
//! renders the bottom [`ShortcutsBar`] (keyboard hints + scroll position). It
//! returns the layout so the caller can place the transcript and composer
//! into `scrollback` / `prompt`. There is no top status bar — session facts
//! live on the composer's top border, so a dedicated chrome row was pure
//! space cost.
//!
//! All keyboard hints advertised by the shortcuts bar are verified against the
//! real key dispatch in `super::main_loop::spawn_input_thread` — a hint that
//! does not match a real handler is a misleading-UI defect.
use oxicode_vtui::design::layout::{
    AgentViewLayout, CompactConfig, HintItem, LayoutConfig, LayoutInput, PendingHint,
    ScrollbarConfig, ShortcutBarStyling, ShortcutsBar, effective_compact,
};
use oxicode_vtui::theme::{ThemeStyles, active_styles};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::main_loop::RenderState;

/// Prompt composer height (matches `main_loop::COMPOSER_HEIGHT`).
const COMPOSER_HEIGHT: u16 = 3;
/// Shortcuts bar height (1 row).
const SHORTCUTS_HEIGHT: u16 = 1;

/// The live chat surface is intentionally denser than the generic vtui
/// defaults: it should reserve space for conversation, not decorative frame
/// padding.  A single horizontal gutter keeps text off the terminal edge;
/// vertical gutters and their implied separator rows are unnecessary here.
const CHAT_LAYOUT: LayoutConfig = LayoutConfig {
    hpad_left: 1,
    hpad_right: 1,
    hpad_left_compact: 1,
    hpad_right_compact: 1,
    outer_vpad: 0,
    outer_vpad_compact: 0,
};

// ─────────────────────────────────────────────────────────────────────────
// Color helper (mirrors `main_loop::color_from_anstyle` — kept local so this
// module is self-contained and `main_loop` needs no extra `pub` edits).
// ─────────────────────────────────────────────────────────────────────────

fn color_from_anstyle(color: Option<anstyle::Color>) -> Color {
    match color {
        Some(anstyle::Color::Ansi(a)) => ansi_to_ratatui(a),
        Some(anstyle::Color::Ansi256(idx)) => Color::Indexed(idx.0),
        Some(anstyle::Color::Rgb(rgb)) => Color::Rgb(rgb.0, rgb.1, rgb.2),
        None => Color::Reset,
    }
}

fn ansi_to_ratatui(color: anstyle::AnsiColor) -> Color {
    use anstyle::AnsiColor as A;
    match color {
        A::Black => Color::Black,
        A::Red => Color::Red,
        A::Green => Color::Green,
        A::Yellow => Color::Yellow,
        A::Blue => Color::Blue,
        A::Magenta => Color::Magenta,
        A::Cyan => Color::Cyan,
        A::White => Color::Gray,
        A::BrightBlack => Color::DarkGray,
        A::BrightRed => Color::LightRed,
        A::BrightGreen => Color::LightGreen,
        A::BrightYellow => Color::LightYellow,
        A::BrightBlue => Color::LightBlue,
        A::BrightMagenta => Color::LightMagenta,
        A::BrightCyan => Color::LightCyan,
        A::BrightWhite => Color::White,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// ShortcutBarStyling bridge
// ─────────────────────────────────────────────────────────────────────────

/// Bridges [`ThemeStyles`] to [`ShortcutBarStyling`] without the widgets
/// reaching into a concrete theme type.
struct ThemeShortcutStyles<'a> {
    styles: &'a ThemeStyles,
}

impl ShortcutBarStyling for ThemeShortcutStyles<'_> {
    fn key_style(&self) -> Style {
        Style::default()
            .fg(color_from_anstyle(self.styles.primary.get_fg_color()))
            .add_modifier(Modifier::BOLD)
    }

    fn label_style(&self) -> Style {
        Style::default().fg(color_from_anstyle(Some(self.styles.foreground)))
    }

    fn separator_style(&self) -> Style {
        Style::default().fg(color_from_anstyle(self.styles.secondary.get_fg_color()))
    }

    fn background_style(&self) -> Style {
        Style::default().bg(color_from_anstyle(Some(self.styles.background)))
    }

    fn pending_key_style(&self) -> Style {
        Style::default()
            .fg(color_from_anstyle(self.styles.error.get_fg_color()))
            .add_modifier(Modifier::BOLD)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Keyboard hints (verified against spawn_input_thread key dispatch)
// ─────────────────────────────────────────────────────────────────────────

/// Build the hint list for the shortcuts bar.
///
/// Every hint here corresponds to a real `KeyCode` → `InlineEvent` mapping in
/// `spawn_input_thread`. Do not add a hint without a matching handler.
fn shortcut_hints() -> Vec<HintItem> {
    vec![
        HintItem::new("Tab", "complete"),
        HintItem::new("Enter", "send").pinned(),
        HintItem::new("Esc", "cancel").pinned(),
        HintItem::new("Ctrl+C", "interrupt"),
        HintItem::paired("Up", "Down", "scroll"),
        HintItem::paired("PgUp", "PgDn", "page"),
    ]
}

// ─────────────────────────────────────────────────────────────────────────
// Chrome rendering
// ─────────────────────────────────────────────────────────────────────────

/// Compute the agent view layout and render the bottom shortcuts bar.
/// Returns the layout so the caller places the transcript into
/// `layout.scrollback` and the composer into `layout.prompt`.
pub(super) fn render_chrome(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &RenderState,
) -> AgentViewLayout {
    let styles = active_styles();
    let compact = effective_compact(false, area.height);

    let layout = AgentViewLayout::compute(
        area,
        &CHAT_LAYOUT,
        &ScrollbarConfig {
            enabled: false,
            ..Default::default()
        },
        LayoutInput {
            prompt_height: COMPOSER_HEIGHT,
            shortcuts_height: SHORTCUTS_HEIGHT,
            compact,
            ..LayoutInput::default()
        },
    );

    // ── Shortcuts bar (hints + right-aligned brain health) ──
    let hints = shortcut_hints();
    let shortcut_styles = ThemeShortcutStyles { styles: &styles };
    let mut bar =
        ShortcutsBar::new(&hints, &shortcut_styles).right(shortcuts_right_line(state, &styles));
    if state.pending_quit {
        bar = bar.pending(PendingHint {
            key: "Ctrl+C",
            label: "quit",
        });
    }
    let compact_cfg = CompactConfig::default();
    if compact {
        bar = bar.compact(&compact_cfg);
    }
    frame.render_widget(bar, layout.shortcuts);

    layout
}

/// Transcript (scrollback) area height for a terminal of this size,
/// without rendering. The scrollback-commit path needs the live
/// region's height to decide how many rows to shed into the host
/// terminal's real scrollback.
pub(super) fn scrollback_height(area: Rect) -> u16 {
    let compact = effective_compact(false, area.height);
    AgentViewLayout::compute(
        area,
        &CHAT_LAYOUT,
        &ScrollbarConfig {
            enabled: false,
            ..Default::default()
        },
        LayoutInput {
            prompt_height: COMPOSER_HEIGHT,
            shortcuts_height: SHORTCUTS_HEIGHT,
            compact,
            ..LayoutInput::default()
        },
    )
    .scrollback
    .height
}

/// Right-aligned oxibrain health chip for the shortcuts bar.
///
/// Health is ambient state, so it lives in the always-visible right side —
/// healthy reads as info, unreachable as the theme's error color, and the
/// chip is absent when memory is disabled (Off). Scroll position is
/// deliberately not mirrored here: the scrollbar thumb already encodes it.
fn shortcuts_right_line(state: &RenderState, styles: &ThemeStyles) -> Line<'static> {
    state
        .brain
        .chip_label()
        .map_or_else(Line::default, |(label, healthy)| {
            let chip_color = if healthy {
                color_from_anstyle(styles.info.get_fg_color())
            } else {
                color_from_anstyle(styles.error.get_fg_color())
            };
            Line::from(Span::styled(
                label.to_string(),
                Style::default().fg(chip_color),
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui_vt::main_loop::RenderState;
    use oxicode_vtui::tui::core::InlineHeaderContext;
    use ratatui::{Terminal, backend::TestBackend};

    fn render_to_string(state: &RenderState, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("backend");
        terminal
            .draw(|f| {
                let _ = render_chrome(f, f.area(), state);
            })
            .expect("draw");
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn chrome_has_no_top_status_row() {
        let mut state = RenderState::default();
        state.header_context = InlineHeaderContext::default();
        state.header_context.model = "smoke-model".to_string();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("backend");
        let mut layout = None;
        terminal
            .draw(|f| {
                layout = Some(render_chrome(f, f.area(), &state));
            })
            .expect("draw");
        let layout = layout.expect("layout");

        // The scrollback IS the top of the frame — no chrome row above it.
        assert_eq!(layout.scrollback.y, 0, "scrollback starts at row 0");
        assert_eq!(
            layout.scrollback.height,
            24 - COMPOSER_HEIGHT - SHORTCUTS_HEIGHT,
            "the reclaimed status-bar row belongs to the scrollback"
        );
        // Old chrome content must not render anywhere: no app badge, no
        // workspace/run-status/tools chips (session facts live on the
        // composer border, rendered by main_loop).
        let rendered = render_to_string(&state, 80, 24);
        assert!(
            !rendered.contains("APP") && !rendered.contains("[ready]"),
            "top status bar must be gone: {rendered}"
        );
        assert!(
            !rendered.contains("smoke-model"),
            "model belongs to the composer border, not chrome"
        );
    }

    #[test]
    fn shortcuts_bar_carries_hints_only_by_default() {
        let mut state = RenderState::default();
        state.header_context = InlineHeaderContext::default();

        let rendered = render_to_string(&state, 120, 24);

        // Verified keyboard hints.
        assert!(rendered.contains("send"), "must show Enter:send");
        assert!(rendered.contains("interrupt"), "must show Ctrl+C:interrupt");
        assert!(rendered.contains("cancel"), "must show Esc:cancel");

        // The scroll-position text chip is gone — the scrollbar thumb
        // already encodes position.
        assert!(
            !rendered.contains("line 0/0"),
            "no scroll-position chip: {rendered}"
        );
    }

    #[test]
    fn brain_chip_renders_by_health_on_shortcuts_row() {
        // Off (memory disabled) — the right side is empty.
        let mut state = RenderState::default();
        state.header_context = InlineHeaderContext::default();
        let off = render_to_string(&state, 120, 24);
        assert!(
            !off.contains("brain"),
            "chip hidden when memory is off: {off}"
        );

        // Ok — healthy chip on the right.
        state.brain = crate::tui_vt::main_loop::BrainChip::Ok;
        let ok = render_to_string(&state, 120, 24);
        assert!(ok.contains("brain·ok"), "healthy chip renders: {ok}");

        // Down — degraded chip renders.
        state.brain = crate::tui_vt::main_loop::BrainChip::Down;
        let down = render_to_string(&state, 120, 24);
        assert!(down.contains("brain·down"), "degraded chip renders: {down}");
    }

    #[test]
    fn pending_quit_hint_owns_the_shortcuts_row() {
        let mut state = RenderState::default();
        state.header_context = InlineHeaderContext::default();
        state.pending_quit = true;

        let rendered = render_to_string(&state, 120, 24);
        assert!(
            rendered.contains("press Ctrl+C again to quit"),
            "pending hint replaces hints and the brain chip: {rendered}"
        );
        assert!(
            !rendered.contains("brain·"),
            "the confirmation hint owns the row while pending"
        );
    }

    #[test]
    fn chrome_respects_short_terminal_without_panic() {
        // A short terminal must still render (layout degrades gracefully).
        let mut state = RenderState::default();
        state.header_context = InlineHeaderContext::default();
        let rendered = render_to_string(&state, 60, 12);
        assert!(rendered.contains("send"));
    }
}
