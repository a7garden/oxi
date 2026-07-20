//! oxi-pager render entry — grok-build quality TUI rendering.
//!
//! Uses vendored grok render primitives (theme, wrapping, glyphs, scrollbar)
//! from `render/grok/` and `render/theme/` to produce grok's visual identity.

pub mod grok;
pub mod markdown_streaming;
pub mod theme;

use crate::render::theme::Theme;
use crate::scrollback::{BlockKind, RenderedBlock};
use crate::state::PagerState;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

// ── Layout constants ────────────────────────────────────────────────────────

const OUTER_HPAD: u16 = 2;
const OUTER_VPAD: u16 = 1;
const ACCENT_WIDTH: u16 = 1;
const BLOCK_PAD_LEFT: u16 = 2;
const BLOCK_PAD_RIGHT: u16 = 1;
#[allow(dead_code)]
const PROMPT_CHROME_LEFT: u16 = 2;
const MIN_SCROLLBACK: u16 = 5;
const STATUS_HEIGHT: u16 = 1;
const PROMPT_HEIGHT: u16 = 3;

// ── Public entry point ──────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, state: &PagerState, theme: &Theme) {
    let area = frame.area();

    let hpad = if area.width >= 40 { OUTER_HPAD } else { 0 };
    let vpad = if area.height >= 16 { OUTER_VPAD } else { 0 };
    let inner = area.inner(Margin::new(hpad, vpad));

    let layout = Layout::vertical([
        Constraint::Length(STATUS_HEIGHT),
        Constraint::Min(MIN_SCROLLBACK),
        Constraint::Length(PROMPT_HEIGHT),
    ])
    .split(inner);

    let status_area = layout[0];
    let scrollback_area = layout[1];
    let prompt_area = layout[2];

    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(theme.bg_base)),
        area,
    );

    render_scrollback(frame, scrollback_area, state, theme);
    render_status(frame, status_area, state, theme);
    render_prompt(frame, prompt_area, state, theme);
}

// ── Scrollback ──────────────────────────────────────────────────────────────

fn render_scrollback(frame: &mut Frame, area: Rect, state: &PagerState, theme: &Theme) {
    let blocks = &state.scrollback.blocks;
    if blocks.is_empty() {
        let welcome = Paragraph::new("Welcome to oxi — type a message to begin.")
            .style(Style::default().fg(theme.gray_dim))
            .alignment(Alignment::Center);
        let center = centered_rect(area, 50, 1);
        frame.render_widget(welcome, center);
        return;
    }

    // Convert blocks to wrapped lines
    let mut lines: Vec<Line<'_>> = Vec::new();
    let content_width = area
        .width
        .saturating_sub(ACCENT_WIDTH + BLOCK_PAD_LEFT + BLOCK_PAD_RIGHT)
        as usize;

    for block in blocks {
        render_block_to_lines(block, content_width, theme, &mut lines);
    }

    let scroll_offset = state
        .scrollback
        .scroll_offset
        .min(lines.len().saturating_sub(area.height as usize));

    let visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll_offset)
        .take(area.height as usize)
        .collect();

    frame.render_widget(
        Paragraph::new(ratatui::text::Text::from(visible))
            .style(Style::default().bg(theme.bg_base)),
        area,
    );
}

fn render_block_to_lines(
    block: &RenderedBlock,
    content_width: usize,
    theme: &Theme,
    out: &mut Vec<Line<'_>>,
) {
    let (accent_color, bg_color) = block_role_colors(&block.kind, theme);

    // Wrap text to content_width and create styled lines
    if !block.text.is_empty() {
        let wrapped = textwrap::wrap(&block.text, content_width);
        for line_text in wrapped {
            let mut spans = Vec::new();

            // Accent column
            let accent_char = if accent_color.is_some() { "┃" } else { " " };
            spans.push(Span::styled(
                accent_char.to_string(),
                Style::default()
                    .fg(accent_color.unwrap_or(theme.bg_base))
                    .bg(bg_color),
            ));

            // Left padding
            spans.push(Span::styled(
                " ".repeat(BLOCK_PAD_LEFT as usize),
                Style::default().bg(bg_color),
            ));

            // Content
            spans.push(Span::styled(
                line_text.to_string(),
                Style::default().fg(theme.text_primary).bg(bg_color),
            ));

            // Fill remaining
            let used: usize = spans.iter().map(|s| s.width()).sum();
            let total = content_width + ACCENT_WIDTH as usize + BLOCK_PAD_LEFT as usize;
            if used < total {
                spans.push(Span::styled(
                    " ".repeat(total - used),
                    Style::default().bg(bg_color),
                ));
            }

            out.push(Line::from(spans));
        }
    }
}

fn block_role_colors(kind: &BlockKind, theme: &Theme) -> (Option<Color>, Color) {
    match kind {
        BlockKind::User => (None, theme.bg_light),
        BlockKind::Assistant => (None, theme.bg_base),
        BlockKind::ToolCall { .. } => (Some(theme.accent_tool), theme.bg_dark),
        BlockKind::ToolResult { .. } => (Some(theme.accent_success), theme.bg_dark),
        BlockKind::System => (Some(theme.accent_system), theme.bg_dark),
        BlockKind::Thinking => (Some(theme.accent_thinking), theme.bg_dark),
        BlockKind::Error(_) => (Some(theme.accent_error), theme.bg_dark),
    }
}

// ── Status Bar ──────────────────────────────────────────────────────────────

fn render_status(frame: &mut Frame, area: Rect, state: &PagerState, theme: &Theme) {
    let mut spans = Vec::new();

    if state.is_streaming {
        if let Some(model) = &state.status.model {
            spans.push(Span::styled(
                format!(" {} ", model),
                Style::default().fg(theme.accent_assistant).bg(theme.bg_dark),
            ));
        }
    }

    // Spacer fills the bar
    spans.push(Span::styled(
        " ".repeat(area.width as usize),
        Style::default().bg(theme.bg_dark),
    ));

    let tokens = state.status.tokens_in + state.status.tokens_out;
    if tokens > 0 {
        let _token_text = format!("⇣{}", format_tokens(tokens));
        // Right-aligned token count rendered separately
        let line = Line::from(spans);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(theme.bg_dark)),
            area,
        );
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.bg_dark)),
        area,
    );
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ── Prompt ──────────────────────────────────────────────────────────────────

fn render_prompt(frame: &mut Frame, area: Rect, state: &PagerState, theme: &Theme) {
    let focused = !state.waiting_for_input;
    let border_color = if focused {
        theme.accent_user
    } else {
        theme.gray_dim
    };

    let mut spans = Vec::new();

    spans.push(Span::styled(
        "❯ ",
        Style::default()
            .fg(if focused { theme.accent_user } else { theme.gray })
            .bg(theme.bg_dark),
    ));

    let input = if state.prompt.text.is_empty() {
        Span::styled(
            "Type a message...",
            Style::default().fg(theme.gray_dim).bg(theme.bg_dark),
        )
    } else {
        Span::styled(
            &state.prompt.text,
            Style::default().fg(theme.text_primary).bg(theme.bg_dark),
        )
    };
    spans.push(input);

    // Fill remaining width
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let remaining = area.width.saturating_sub(4) as usize;
    if used < remaining {
        spans.push(Span::styled(
            " ".repeat(remaining - used),
            Style::default().bg(theme.bg_dark),
        ));
    }

    // Build bordered prompt
    let inner_width = area.width.saturating_sub(2) as usize;
    let top = format!("╭{}╮", "─".repeat(inner_width));
    let bottom = format!("╰{}╯", "─".repeat(inner_width));

    let lines = vec![
        Line::styled(&top, Style::default().fg(border_color).bg(theme.bg_dark)),
        Line::from(spans),
        Line::styled(&bottom, Style::default().fg(border_color).bg(theme.bg_dark)),
    ];

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg_dark)),
        area,
    );
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn centered_rect(r: Rect, width: u16, height: u16) -> Rect {
    let x = r.x + r.width.saturating_sub(width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(r.width), height.min(r.height))
}
