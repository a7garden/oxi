//! Production bridge from the grok-build-style agent view layout
//! (`oxi_vtui::design::layout`) to the live ratatui render path.
//!
//! `render_chrome` computes the [`AgentViewLayout`] for the current frame and
//! renders the top [`StatusBar`] (header context + footer status) and the
//! bottom [`ShortcutsBar`] (keyboard hints). It returns the layout so the
//! caller can place the transcript and composer into `scrollback` / `prompt`.
//!
//! All keyboard hints advertised by the shortcuts bar are verified against the
//! real key dispatch in `super::main_loop::spawn_input_thread` — a hint that
//! does not match a real handler is a misleading-UI defect.

use oxi_vtui::design::layout::{
    AgentViewLayout, HintItem, LayoutConfig, LayoutInput, ScrollbarConfig, ShortcutBarStyling,
    ShortcutsBar, StatusBar, effective_compact,
};
use oxi_vtui::theme::{ThemeStyles, active_styles};
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

/// Compute the agent view layout and render the top status bar + bottom
/// shortcuts bar. Returns the layout so the caller places the transcript into
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
        &LayoutConfig::default(),
        &ScrollbarConfig::default(),
        LayoutInput {
            prompt_height: COMPOSER_HEIGHT,
            shortcuts_height: SHORTCUTS_HEIGHT,
            compact,
            ..LayoutInput::default()
        },
    );

    // ── Status bar (replaces render_header + render_footer's status line) ──
    let bg = color_from_anstyle(Some(styles.background));
    let status = StatusBar::new(header_line(state, &styles))
        .right(footer_line(state, &styles, layout.scrollback))
        .style(Style::default().bg(bg));
    frame.render_widget(status, layout.status_bar);

    // ── Shortcuts bar ──
    let hints = shortcut_hints();
    let shortcut_styles = ThemeShortcutStyles { styles: &styles };
    let bar = ShortcutsBar::new(&hints, &shortcut_styles);
    frame.render_widget(bar, layout.shortcuts);

    layout
}

/// Header context line: app · provider — model · git · tools.
fn header_line<'a>(state: &'a RenderState, styles: &ThemeStyles) -> Line<'a> {
    let ctx = &state.header_context;
    let fg = color_from_anstyle(Some(styles.foreground));
    let bg = color_from_anstyle(Some(styles.background));
    let secondary = color_from_anstyle(styles.secondary.get_fg_color());
    let info = color_from_anstyle(styles.info.get_fg_color());

    Line::from(vec![
        Span::styled(
            format!(" {} ", ctx.app_name),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {} ", ctx.provider), Style::default().fg(info)),
        Span::raw("  "),
        Span::styled(
            format!("\u{2500} {} ", ctx.model),
            Style::default().fg(secondary),
        ),
        Span::raw("  "),
        Span::styled(
            format!("\u{2387} {} ", ctx.git),
            Style::default().fg(secondary),
        ),
        Span::raw("  "),
        Span::styled(
            format!("\u{2699} {}", ctx.tools),
            Style::default().fg(secondary),
        ),
    ])
}

/// Right-aligned footer status (left status + line position).
fn footer_line<'a>(state: &'a RenderState, styles: &ThemeStyles, area: Rect) -> Line<'a> {
    let left = state.footer_left.clone().unwrap_or_default();
    // `footer_right` is set by the app (SetInputStatus); only fall back to a
    // computed line-position when the app has not supplied one. The viewport
    // is the scrollback height so the position matches what render_transcript
    // actually displays (the old footer used its own 1-row height, which was
    // inconsistent with the transcript scroll).
    let right = state.footer_right.clone().unwrap_or_else(|| {
        let total = state.transcript.len();
        if state.scroll_offset == usize::MAX {
            format!("line {total}/{total}")
        } else {
            let off = super::main_loop::effective_scroll_offset(
                state.scroll_offset,
                total,
                area.height as usize,
            );
            format!("line {}/{}", off.min(total), total)
        }
    });

    Line::from(vec![
        Span::styled(
            left,
            Style::default().fg(color_from_anstyle(Some(styles.foreground))),
        ),
        Span::raw("  "),
        Span::styled(
            right,
            Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui_vt::main_loop::RenderState;
    use oxi_vtui::tui::core::InlineHeaderContext;
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
    fn chrome_paints_status_bar_and_shortcuts_bar() {
        // Default state is enough — we only assert the chrome regions render,
        // not specific header content (theme/header fields are app-supplied).
        let mut state = RenderState::default();
        state.header_context = InlineHeaderContext::default();
        state.header_context.model = "smoke-model".to_string();

        let rendered = render_to_string(&state, 80, 24);

        // StatusBar carries the model name.
        assert!(
            rendered.contains("smoke-model"),
            "status bar must render the header model name"
        );
        // ShortcutsBar carries the verified keyboard hints.
        assert!(
            rendered.contains("send"),
            "shortcuts bar must show Enter:send"
        );
        assert!(
            rendered.contains("interrupt"),
            "shortcuts bar must show Ctrl+C:interrupt"
        );
        assert!(
            rendered.contains("cancel"),
            "shortcuts bar must show Esc:cancel"
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
