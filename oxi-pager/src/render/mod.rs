//! oxi-pager render entry. The `grok/` and `theme/` subtrees are vendored
//! verbatim from grok-build (Apache-2.0, © 2023-2026 SpaceXAI — see
//! NOTICE-vendored.md). This module is the oxi-side adapter that drives
//! those primitives from `PagerState`.

pub mod grok;
pub mod markdown_streaming;
pub mod theme;

use crate::render::theme::Theme as GrokTheme;
use crate::render::theme::md_style;
use crate::scrollback::{BlockKind, RenderedBlock};
use crate::state::PagerState;
use crate::theme_bridge;
use oxi_tui::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

/// Render the full TUI frame from `PagerState`.
///
/// Syncs the live oxi theme into grok's global, then lays out:
/// - chat scrollback (user / assistant / tool blocks)
/// - token bar (1 row)
/// - prompt input (3 rows)
///
/// Markdown flows through the vendored pipeline
/// (`oxi_vendor_grok_markdown::render_markdown_ratatui_full` +
/// `grok::wrapping::word_wrap_lines_with_joiners`) so styled spans, OSC8
/// hyperlinks, and code-block syntax highlighting all work end-to-end.
pub fn render(frame: &mut Frame, state: &PagerState, theme: &Theme) {
    // Sync oxi theme → grok global so vendored render sees the right palette.
    theme_bridge::apply_oxi_theme(theme);
    let grok = GrokTheme::current();

    let area = frame.area();
    if area.width < 16 || area.height < 4 {
        return;
    }

    let inner = area.inner(Margin::new(1, 0));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // scrollback
            Constraint::Length(1), // token bar
            Constraint::Length(3), // prompt
        ])
        .split(inner);

    render_scrollback(frame, chunks[0], state, &grok);
    render_token_bar(frame, chunks[1], state, &grok);
    render_prompt(frame, chunks[2], state, &grok);
}

fn render_scrollback(frame: &mut Frame, area: Rect, state: &PagerState, grok: &GrokTheme) {
    if state.scrollback.blocks.is_empty() {
        let welcome = format!(
            " {} {} — waiting",
            spinner_glyph(state.status.spinner_phase),
            state.status.model.as_deref().unwrap_or("oxi"),
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                welcome,
                Style::new().fg(grok.gray_dim),
            ))),
            area,
        );
        return;
    }

    let width = area.width.max(8) as usize;
    let mut items: Vec<ListItem> = Vec::new();
    let block_count = state.scrollback.blocks.len();
    for (idx, block) in state.scrollback.blocks.iter().enumerate() {
        for line in render_block_lines(block, width, grok) {
            items.push(ListItem::new(line));
        }
        if idx + 1 < block_count {
            items.push(ListItem::new(Line::from(Span::styled(
                " ",
                Style::new().fg(grok.gray_dim),
            ))));
        }
    }

    let mut list_state = state.list_state;
    if state.scrollback.follow_tail {
        list_state.select(Some(items.len().saturating_sub(1)));
    }
    frame.render_stateful_widget(
        List::new(items).block(Block::default()),
        area,
        &mut list_state,
    );
}

fn render_block_lines(block: &RenderedBlock, width: usize, grok: &GrokTheme) -> Vec<Line<'static>> {
    match &block.kind {
        BlockKind::ToolCall { name, .. } => {
            let label = format!(" {} {}", spinner_glyph(0), name);
            vec![Line::from(Span::styled(
                label,
                Style::new()
                    .fg(grok.accent_tool)
                    .add_modifier(Modifier::DIM),
            ))]
        }
        BlockKind::ToolResult { .. } => {
            vec![Line::from(Span::styled(
                block.text.clone(),
                Style::new().fg(grok.gray),
            ))]
        }
        BlockKind::System => {
            vec![Line::from(Span::styled(
                block.text.clone(),
                Style::new().fg(grok.gray_dim).add_modifier(Modifier::DIM),
            ))]
        }
        BlockKind::User => {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                " You",
                Style::new()
                    .fg(grok.accent_user)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(wrap_plain(&block.text, width, grok.text_primary));
            lines
        }
        BlockKind::Assistant => {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                " Assistant",
                Style::new()
                    .fg(grok.accent_assistant)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(render_markdown_wrapped(&block.text, width));
            lines
        }
    }
}

fn render_markdown_wrapped(text: &str, width: usize) -> Vec<Line<'static>> {
    let ms = md_style::style();
    let (out, _checkpoint) =
        oxi_vendor_grok_markdown::render_markdown_ratatui_full(text, ms, true, None);
    let (wrapped, _joiners) =
        crate::render::grok::wrapping::word_wrap_lines_with_joiners(out.lines, width);
    wrapped
}

fn wrap_plain(text: &str, width: usize, fg: Color) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut buf = String::new();
        let mut col: usize = 0;
        for word in raw.split(' ') {
            let w = word.chars().count();
            if col == 0 {
                buf.push_str(word);
                col = w;
            } else if col + 1 + w <= width {
                buf.push(' ');
                buf.push_str(word);
                col += 1 + w;
            } else {
                out.push(Line::from(Span::styled(
                    std::mem::take(&mut buf),
                    Style::new().fg(fg),
                )));
                buf.push_str(word);
                col = w;
            }
        }
        out.push(Line::from(Span::styled(buf, Style::new().fg(fg))));
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled(String::new(), Style::new().fg(fg))));
    }
    out
}

fn render_token_bar(frame: &mut Frame, area: Rect, state: &PagerState, grok: &GrokTheme) {
    let model = state.status.model.as_deref().unwrap_or("oxi");
    let line = format!(
        " {} {}  in {} · out {}{}",
        spinner_glyph(state.status.spinner_phase),
        model,
        state.status.tokens_in,
        state.status.tokens_out,
        if state.status.cost > 0.0 {
            format!(" · ${:.4}", state.status.cost)
        } else {
            String::new()
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(line, Style::new().fg(grok.gray)))),
        area,
    );
}

fn render_prompt(frame: &mut Frame, area: Rect, state: &PagerState, grok: &GrokTheme) {
    let text = if state.prompt.text.is_empty() {
        Span::styled(" Type a message…", Style::new().fg(grok.gray_dim))
    } else {
        Span::styled(
            format!(" {}", state.prompt.text),
            Style::new().fg(grok.text_primary),
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(text))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(grok.prompt_border_active))
                    .title(" Input "),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn spinner_glyph(phase: u8) -> &'static str {
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "⠟", "⠻"];
    FRAMES[(phase as usize) % FRAMES.len()]
}
