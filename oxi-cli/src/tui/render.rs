//! Rendering functions for the TUI.

use super::app::{AppOverlay, AppState, NotificationKind, ProviderInfo, SetupStep};
use oxi_tui::theme::Theme;
use oxi_tui::truncate_to_width;
use oxi_tui::widgets::{chat::ChatView, footer::Footer, input::Input};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ── Reusable UI components ──────────────────────────────────────────────

/// Render a centered popup frame: clear + dimmed background + border.
fn render_popup_frame(
    f: &mut Frame,
    area: Rect,
    bg: ratatui::style::Color,
    border: ratatui::style::Color,
) -> Rect {
    f.render_widget(Clear, area);
    let dimmed = Block::default().style(Style::default().bg(bg));
    f.render_widget(dimmed, area);
    let border_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    let inner = border_block.inner(area);
    f.render_widget(border_block, area);
    inner
}

/// Compute a centered popup Rect covering `pct` of the screen.
fn centered_layout(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    let w = (area.width as f32 * width_pct) as u16;
    let h = (area.height as f32 * height_pct) as u16;
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

/// Render a bold title line.
fn render_title(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    f.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )),
        Rect {
            x: area.x,
            y: area.y + y_offset,
            width: area.width,
            height: 1,
        },
    );
}

/// Render a hint line at the bottom of the area.
fn render_hint(f: &mut Frame, area: Rect, text: &str, style: Style) {
    let y = area.y + area.height.saturating_sub(1);
    f.render_widget(
        Paragraph::new(Span::styled(text, style)),
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
    );
}

/// Render a selectable list with pointer, scrolling window, and highlight.
#[allow(clippy::too_many_arguments)]
/// Scroll position info returned by [`render_selectable_list`].
struct ScrollInfo {
    total: usize,
    visible: usize,
    window_start: usize,
}

impl ScrollInfo {
    /// Number of items scrolled off the bottom of the visible window.
    fn below(&self) -> usize {
        self.total.saturating_sub(self.window_start + self.visible)
    }

    /// Number of items scrolled off the top of the visible window.
    #[allow(dead_code)]
    fn above(&self) -> usize {
        self.window_start
    }

    /// Human-readable position string like "(5/28, 12 below)" or empty if everything fits.
    fn hint(&self) -> String {
        if self.total <= self.visible {
            return String::new();
        }
        let below = self.below();
        let above = self.window_start;
        match (above, below) {
            (0, 0) => String::new(),
            (0, b) => format!(" ({} below)", b),
            (a, 0) => format!(" ({} above)", a),
            (a, b) => format!(" ({} above, {} below)", a, b),
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[must_use]
fn render_selectable_list(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    items: &[String],
    selected: usize,
    styles: &oxi_tui::theme::ThemeStyles,
    theme: &Theme,
    highlight_color: Option<ratatui::style::Color>,
) -> Option<ScrollInfo> {
    if items.is_empty() {
        return None;
    }

    let list_area = Rect {
        x: area.x,
        y: area.y + y_offset,
        width: area.width,
        height: area.height.saturating_sub(y_offset).saturating_sub(1),
    };

    let max_show = list_area.height as usize;
    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .map(|(i, text)| {
            let is_sel = i == selected;
            let pointer = if is_sel { "-> " } else { "   " };
            let content = format!("{}{}", pointer, text);

            let style = if is_sel {
                let hl = highlight_color.unwrap_or_else(|| theme.colors.primary.to_ratatui());
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(hl)
                    .bold()
            } else {
                styles.normal
            };

            ListItem::new(Span::styled(content, style))
        })
        .collect();

    f.render_widget(List::new(list_items), list_area);

    Some(ScrollInfo {
        total: items.len(),
        visible: max_show.min(items.len()),
        window_start,
    })
}

/// Render a list with status indicators (filled/empty circle).
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn render_status_list(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    items: &[(String, bool)],
    selected: usize,
    styles: &oxi_tui::theme::ThemeStyles,
    theme: &Theme,
    highlight_color: Option<ratatui::style::Color>,
) {
    let strings: Vec<String> = items
        .iter()
        .map(|(name, has_key)| {
            let status = if *has_key { "*" } else { "o" };
            format!("{} {}", status, name)
        })
        .collect();
    let _ = render_selectable_list(
        f,
        area,
        y_offset,
        &strings,
        selected,
        styles,
        theme,
        highlight_color,
    );
}

/// A single row in the provider list — either a category header or an item.
enum ProviderRow {
    /// Category header line.
    Header { label: String },
    /// Provider item. `provider_idx` is the index into the `providers` slice.
    Item { provider_idx: usize },
}

/// Render provider selection list grouped by category with descriptions.
///
/// Uses `List` widget with scroll window — fully scrollable.
#[must_use]
fn render_provider_list(
    f: &mut Frame,
    area: Rect,
    providers: &[ProviderInfo],
    selected: usize,
    _styles: &oxi_tui::theme::ThemeStyles,
    theme: &Theme,
) -> Option<ScrollInfo> {
    if providers.is_empty() {
        return None;
    }

    // Category display order and labels
    let category_order = [
        ("primary", "Primary Providers"),
        ("chinese", "Chinese AI"),
        ("open", "Open Providers"),
        ("cloud", "Cloud"),
        ("enterprise", "Enterprise"),
        ("specialized", "Other"),
    ];

    // Build flat row list and index mapping
    let mut rows: Vec<ProviderRow> = Vec::new();
    // Map: provider index → row index (for scroll calculation)
    let mut provider_to_row: Vec<usize> = vec![0; providers.len()];

    for (cat_key, cat_label) in &category_order {
        let cat_providers: Vec<(usize, &ProviderInfo)> = providers
            .iter()
            .enumerate()
            .filter(|(_, p)| p.category == *cat_key)
            .collect();
        if cat_providers.is_empty() {
            continue;
        }

        // Category header
        rows.push(ProviderRow::Header {
            label: cat_label.to_string(),
        });

        for (pi, _p) in &cat_providers {
            provider_to_row[*pi] = rows.len();
            rows.push(ProviderRow::Item { provider_idx: *pi });
        }
    }

    // Calculate list area (leave 1 row for hint at bottom)
    let list_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(1),
    };

    let max_show = list_area.height as usize;
    let selected_line = provider_to_row[selected];
    let window_start = if selected_line >= max_show {
        selected_line.saturating_sub(max_show - 1)
    } else {
        0
    };

    let accent_color = theme.colors.accent.to_ratatui();
    let fg_color = theme.colors.foreground.to_ratatui();
    let muted_color = theme.colors.muted.to_ratatui();
    let bg_color = theme.colors.background.to_ratatui();
    let success_color = theme.colors.success.to_ratatui();

    let list_items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .map(|(_row_idx, row)| match row {
            ProviderRow::Header { label } => {
                // ── Category Header ────────── style
                let dash_count = area.width as usize;
                let header_text = format!(" {:} ", label);
                let header_len = header_text.chars().count();
                let trailing = dash_count.saturating_sub(header_len + 4); // "── " + " ─"
                let line = format!("── {} {}", header_text, "─".repeat(trailing));
                ListItem::new(Span::styled(
                    line,
                    Style::default()
                        .fg(accent_color)
                        .add_modifier(Modifier::BOLD),
                ))
            }
            ProviderRow::Item { provider_idx } => {
                let is_sel = *provider_idx == selected;
                let p = &providers[*provider_idx];
                let check = if p.has_key { "✓" } else { "○" };

                let mut spans: Vec<Span<'_>> = Vec::new();

                // Pointer
                let pointer = if is_sel { "› " } else { "  " };
                spans.push(Span::styled(
                    pointer,
                    if is_sel {
                        Style::default()
                            .fg(accent_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ));

                // Check + name
                let name_str = format!("{} {}", check, p.display_name);
                spans.push(Span::styled(
                    format!(" {:<16}", name_str),
                    if is_sel {
                        Style::default()
                            .fg(bg_color)
                            .bg(accent_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(fg_color)
                    },
                ));

                // Description
                if !p.description.is_empty() {
                    let desc_style = if is_sel {
                        Style::default().fg(bg_color).bg(accent_color)
                    } else {
                        Style::default().fg(muted_color)
                    };
                    spans.push(Span::styled(format!(" {}", p.description), desc_style));
                }

                // Key badge
                if p.has_key {
                    let badge_style = if is_sel {
                        Style::default().fg(bg_color).bg(accent_color)
                    } else {
                        Style::default().fg(success_color)
                    };
                    spans.push(Span::styled(" [key]", badge_style));
                }

                ListItem::new(Line::from(spans))
            }
        })
        .collect();

    f.render_widget(List::new(list_items), list_area);

    Some(ScrollInfo {
        total: providers.len(),
        visible: max_show.min(rows.len()),
        window_start,
    })
}

/// Render a text input field with label and cursor.
#[allow(dead_code)]
fn render_input_field(
    f: &mut Frame,
    area: Rect,
    y_offset: u16,
    label: &str,
    value: &str,
    theme: &Theme,
    styles: &oxi_tui::theme::ThemeStyles,
) {
    let field_y = area.y + y_offset;
    let field_w = area.width.saturating_sub(4);
    let display: String = value.chars().take(field_w as usize).collect();
    let input_line = format!(" {}: {}", label, display);

    f.render_widget(
        Paragraph::new(Span::styled(input_line, styles.normal)),
        Rect {
            x: area.x + 2,
            y: field_y,
            width: field_w,
            height: 1,
        },
    );

    // Cursor
    let cursor_col = (label.len() as u16) + 3 + (display.len().min(field_w as usize) as u16);
    f.render_widget(
        Paragraph::new(Span::styled(
            " ",
            Style::default()
                .fg(theme.colors.cursor_fg.to_ratatui())
                .bg(theme.colors.cursor_bg.to_ratatui()),
        )),
        Rect {
            x: area.x + 2 + cursor_col,
            y: field_y,
            width: 1,
            height: 1,
        },
    );
}

// ── Main draw function ──────────────────────────────────────────────────

/// Main draw function — renders the full TUI frame.
pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let size = f.area();

    // Overlay takes over the entire screen
    if state.overlay.is_some() || state.overlay_state.is_some() {
        render_overlay(f, size, state, theme);
        return;
    }

    // Horizontal: 1 col left, 1 col right. Vertical: 0 top/bottom.
    let inner = size.inner(Margin::new(1, 0));

    // Calculate queue area height
    let queue_count = state.steering_messages_snapshot.len();
    let is_active = state.queue_panel_visible && queue_count > 0;
    let queue_lines = if queue_count > 0 {
        if is_active {
            // Active: selected highlight, up to 5 items + hint line
            (queue_count.min(5) as u16 + 1).min(6) // items + hint
        } else {
            // Compact: up to 3 message previews
            queue_count.min(3) as u16
        }
    } else {
        0
    };

    // Calculate dynamic input height based on content
    let content_width = inner.width.saturating_sub(2); // 1 left + 1 right padding
    let input_height = state.input.required_height(content_width, INPUT_MAX_HEIGHT);
    let input_area_height = 1 + queue_lines + input_height; // status + queue + input

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),                    // Chat
            Constraint::Length(input_area_height), // Status + queue + input (dynamic)
            Constraint::Length(3),                 // Footer
        ])
        .split(inner);

    f.render_stateful_widget(ChatView::new(theme), chunks[0], &mut state.chat);

    // Input area (status line + optional queue + input)
    render_input_area(f, chunks[1], state, theme);

    // Slash popup — overlay above the input area
    if state.slash_completion_active {
        render_slash_popup_overlay(f, chunks[1], state, theme);
    }

    // File path completion popup
    if state.file_completion_active {
        render_file_completion_popup(f, chunks[1], state, theme);
    }

    // Status bar
    state.footer_state.data.is_busy = state.is_agent_busy;
    f.render_stateful_widget(Footer::new(theme), chunks[2], &mut state.footer_state);

    // Notifications (toasts) — rendered on top of everything
    render_notifications(f, size, state, theme);
}

// ── Input area ──────────────────────────────────────────────────────────

/// Maximum height for the input text area (in rows).
const INPUT_MAX_HEIGHT: u16 = 8;

fn render_input_area(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len();
    let is_active = state.queue_panel_visible && queue_count > 0;
    let queue_lines = if queue_count > 0 {
        if is_active {
            (queue_count.min(5) as u16 + 1).min(6)
        } else {
            queue_count.min(3) as u16
        }
    } else {
        0
    };

    // Row 0: status line (Working/Idle + queue count)
    let status_row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    render_status_line(f, status_row, state, theme);

    // Queue preview / active panel rows
    if is_active {
        render_queue_active(f, area, state, theme);
    } else {
        render_queue_compact(f, area, state, theme);
    }

    // Input: height is whatever remains after status + queue
    let input_y = area.y + 1 + queue_lines;
    let input_height = area.height.saturating_sub(1 + queue_lines);
    let input_row = Rect {
        x: area.x,
        y: input_y,
        width: area.width,
        height: input_height,
    };
    state.input.set_placeholder(None);
    f.render_stateful_widget(Input::new(theme), input_row, &mut state.input);
}

/// Compact queue preview: dim numbered lines with message text.
fn render_queue_compact(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len().min(3);
    let hint_text = " Ctrl+Q ";
    let hint_width = hint_text.len() as u16;

    for (i, msg) in state
        .steering_messages_snapshot
        .iter()
        .take(queue_count)
        .enumerate()
    {
        let row = Rect {
            x: area.x,
            y: area.y + 1 + i as u16,
            width: area.width,
            height: 1,
        };
        let badge = if i == 0 { "next" } else { "" };
        let num = format!("{}. ", i + 1);
        let prefix_w: u16 = (num.len() + badge.len()) as u16;
        let max_chars = area.width.saturating_sub(prefix_w + 3) as usize;
        let truncated: String = msg.chars().take(max_chars).collect();
        let ellipsis = if msg.chars().count() > max_chars {
            "…"
        } else {
            ""
        };

        let mut spans: Vec<Span<'_>> = Vec::new();
        // Number
        spans.push(Span::styled(
            num,
            Style::default().fg(theme.colors.muted.to_ratatui()),
        ));
        // "next" badge for first item
        if i == 0 {
            spans.push(Span::styled(
                "next ",
                Style::default()
                    .fg(theme.colors.accent.to_ratatui())
                    .add_modifier(Modifier::ITALIC),
            ));
        }
        // Message text
        let msg_style = if i == 0 {
            Style::default().fg(theme.colors.foreground.to_ratatui())
        } else {
            Style::default().fg(theme.colors.muted.to_ratatui())
        };
        spans.push(Span::styled(
            format!("{}{}", truncated, ellipsis),
            msg_style,
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), row);

        // Ctrl+Q hint on the right side of first line
        if i == 0 {
            f.render_widget(
                Paragraph::new(Span::styled(
                    hint_text,
                    Style::default()
                        .fg(theme.colors.muted.to_ratatui())
                        .add_modifier(Modifier::ITALIC),
                )),
                Rect {
                    x: area.x + area.width.saturating_sub(hint_width),
                    y: row.y,
                    width: hint_width,
                    height: 1,
                },
            );
        }
    }
}

/// Active queue panel: selected highlight, navigation, delete/edit.
fn render_queue_active(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len();
    let max_items = queue_count.min(5);
    let selected = state.queue_panel_selected;

    // Item rows
    for (i, msg) in state
        .steering_messages_snapshot
        .iter()
        .take(max_items)
        .enumerate()
    {
        let row = Rect {
            x: area.x,
            y: area.y + 1 + i as u16,
            width: area.width,
            height: 1,
        };
        let is_sel = i == selected;
        let pointer = if is_sel { "› " } else { "  " };
        let badge = if i == 0 { "next " } else { "" };
        let num = format!("{}. ", i + 1);
        let prefix_w: u16 = (pointer.len() + num.len() + badge.len()) as u16;
        let max_chars = area.width.saturating_sub(prefix_w + 2) as usize;
        let truncated: String = msg.chars().take(max_chars).collect();
        let ellipsis = if msg.chars().count() > max_chars {
            "…"
        } else {
            ""
        };

        let mut spans: Vec<Span<'_>> = Vec::new();
        // Pointer
        spans.push(Span::styled(
            pointer,
            if is_sel {
                Style::default()
                    .fg(theme.colors.accent.to_ratatui())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ));
        // Number
        spans.push(Span::styled(
            num,
            if is_sel {
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(theme.colors.accent.to_ratatui())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.colors.muted.to_ratatui())
            },
        ));
        // "next" badge for first
        if i == 0 {
            spans.push(Span::styled(
                badge,
                if is_sel {
                    Style::default()
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.accent.to_ratatui())
                        .add_modifier(Modifier::ITALIC)
                } else {
                    Style::default()
                        .fg(theme.colors.accent.to_ratatui())
                        .add_modifier(Modifier::ITALIC)
                },
            ));
        }
        // Message text
        let msg_style = if is_sel {
            Style::default()
                .fg(theme.colors.background.to_ratatui())
                .bg(theme.colors.accent.to_ratatui())
                .add_modifier(Modifier::BOLD)
        } else if i == 0 {
            Style::default().fg(theme.colors.foreground.to_ratatui())
        } else {
            Style::default().fg(theme.colors.muted.to_ratatui())
        };
        spans.push(Span::styled(
            format!("{}{}", truncated, ellipsis),
            msg_style,
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }

    // Hint line at the bottom of allocated space
    let hint_y = area.y + 1 + max_items as u16;
    if hint_y < area.y + area.height {
        let row = Rect {
            x: area.x,
            y: hint_y,
            width: area.width,
            height: 1,
        };
        let hint = " ↑↓ nav · d del · e edit · Esc close";
        f.render_widget(
            Paragraph::new(Span::styled(
                hint,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            row,
        );
    }
}

/// Render the status/separator line with Working/Idle indicator and queue count.
fn render_status_line(f: &mut Frame, row: Rect, state: &AppState, theme: &Theme) {
    let queue_count = state.steering_messages_snapshot.len();

    if state.is_agent_busy {
        // Moon phase spinner ◐ ◓ ◑ ◒
        let sp = ["\u{25D0}", "\u{25D3}", "\u{25D1}", "\u{25D2}"];
        let ch = sp[state.spinner_frame % sp.len()];
        let label = if queue_count > 0 {
            format!(" {} Working  ⏳{} queued", ch, queue_count)
        } else {
            format!(" {} Working", ch)
        };
        let label_len: u16 = label
            .chars()
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16)
            .sum();
        let dash_count = row.width.saturating_sub(label_len + 2) as usize;
        let line = format!("── {} {}", label, "─".repeat(dash_count));
        f.render_widget(
            Paragraph::new(Span::styled(
                line,
                Style::default().fg(theme.colors.accent.to_ratatui()),
            )),
            row,
        );
    } else {
        let label = if queue_count > 0 {
            format!(" ○ Idle  ⏳{} queued", queue_count)
        } else {
            " ○ Idle".to_string()
        };
        let label_len: u16 = label
            .chars()
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16)
            .sum();
        let dash_count = row.width.saturating_sub(label_len + 2) as usize;
        let line = format!("── {} {}", label, "─".repeat(dash_count));
        f.render_widget(
            Paragraph::new(Span::styled(
                line,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            row,
        );
    }
}

// ── Slash popup overlay ─────────────────────────────────────────────────

fn render_slash_popup_overlay(f: &mut Frame, input_area: Rect, state: &AppState, theme: &Theme) {
    if state.slash_completions.is_empty() {
        return;
    }
    let selected = state.slash_completion_index;
    let total = state.slash_completions.len();
    let max_show = 8usize.min(total);

    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    // Popup positioned above the input area
    let popup_width = input_area.width;
    let popup_height = max_show as u16 + 2;
    let popup_x = input_area.x;
    let popup_y = input_area.y.saturating_sub(popup_height);

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    f.render_widget(Clear, popup_area);

    let visible: Vec<_> = state
        .slash_completions
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .collect();

    let name_width = state
        .slash_completions
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(10)
        .max(10);

    let list_items: Vec<ListItem> = visible
        .iter()
        .map(|(i, comp)| {
            let is_selected = *i == selected;
            let pointer = if is_selected { "->" } else { "  " };
            let name_padded = format!("{:<width$}", comp.name, width = name_width);
            let desc_space = (popup_width as usize).saturating_sub(name_width + 8);
            let desc: String = comp.description.chars().take(desc_space).collect();

            if is_selected {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {} ", pointer),
                        Style::default().fg(theme.colors.accent.to_ratatui()),
                    ),
                    Span::styled(
                        format!(" {}  ", name_padded),
                        Style::default()
                            .fg(theme.colors.background.to_ratatui())
                            .bg(theme.colors.primary.to_ratatui())
                            .bold(),
                    ),
                    Span::styled(desc, Style::default().fg(theme.colors.muted.to_ratatui())),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", pointer), Style::default()),
                    Span::styled(
                        format!(" {}  ", name_padded),
                        Style::default().fg(theme.colors.foreground.to_ratatui()),
                    ),
                    Span::styled(desc, Style::default().fg(theme.colors.muted.to_ratatui())),
                ]))
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
    let popup_inner = block.inner(popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(List::new(list_items), popup_inner);

    // Page indicator
    let page = window_start / max_show + 1;
    let total_pages = total.div_ceil(max_show);
    if total_pages > 1 {
        let indicator = format!("({}/{})", page, total_pages);
        let indicator_area = Rect {
            x: popup_area.x
                + popup_area
                    .width
                    .saturating_sub(indicator.chars().count() as u16 + 2),
            y: popup_area.y + popup_area.height.saturating_sub(1),
            width: indicator.chars().count() as u16 + 2,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                indicator,
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            indicator_area,
        );
    }
}

// ── File completion popup ───────────────────────────────────────────────

fn render_file_completion_popup(f: &mut Frame, input_area: Rect, state: &AppState, theme: &Theme) {
    if state.file_completions.is_empty() {
        return;
    }
    let selected = state.file_completion_index;
    let total = state.file_completions.len();
    let max_show = 8usize.min(total);

    let window_start = if selected >= max_show {
        selected - max_show + 1
    } else {
        0
    };

    let popup_width = input_area.width;
    let popup_height = max_show as u16 + 2; // border + items
    let popup_x = input_area.x;
    let popup_y = input_area.y.saturating_sub(popup_height);

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    f.render_widget(Clear, popup_area);

    let visible: Vec<_> = state
        .file_completions
        .iter()
        .enumerate()
        .skip(window_start)
        .take(max_show)
        .collect();

    let styles = theme.to_styles();
    let bg = theme.colors.background.to_ratatui();

    let items: Vec<Line> = visible
        .iter()
        .map(|(idx, item)| {
            let is_selected = *idx == selected;
            let style = if is_selected {
                styles.primary
            } else {
                styles.muted
            };
            let label = if let Some(desc) = &item.description {
                format!(" {} {}", item.label, desc)
            } else {
                format!(" {}", item.label)
            };
            Line::from(Span::styled(label, style))
        })
        .collect();

    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(styles.muted)
        .style(ratatui::style::Style::default().bg(bg));

    let list = ratatui::widgets::List::new(items).block(block);
    f.render_widget(list, popup_area);
}

// ── Overlay dispatch ────────────────────────────────────────────────────

fn render_overlay(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    // ── Component-based overlay (takes priority) ──
    if let Some(ref mut overlay) = state.overlay_state {
        overlay.render(f, area, theme);
        return;
    }
    // ── Legacy AppOverlay enum ──
    match &state.overlay {
        Some(AppOverlay::Setup(step)) | Some(AppOverlay::ProviderConfig(step)) => {
            let step = step.clone();
            render_setup_step(f, area, state, theme, &step);
        }
        Some(AppOverlay::ModelSelect { .. }) => {
            render_model_select(f, area, state, theme);
        }
        Some(AppOverlay::LogoutSelect { .. }) => {
            render_logout_select(f, area, state, theme);
        }
        Some(AppOverlay::ResumeSelect { .. }) => {
            render_resume_select(f, area, state, theme);
        }
        // RoutingStatus uses component-based overlay (overlay_state), handled above
        Some(AppOverlay::RoutingStatus { .. }) | None => {}
    }
}

// ── Setup wizard ────────────────────────────────────────────────────────

#[allow(dead_code)]
fn render_setup_step(
    f: &mut Frame,
    area: Rect,
    _state: &mut AppState,
    theme: &Theme,
    step: &SetupStep,
) {
    let styles = theme.to_styles();
    let bg = theme.colors.background.to_ratatui();
    let fg = theme.colors.primary.to_ratatui();

    match step {
        SetupStep::SelectAuthType { selected, .. } => {
            render_title(f, area, 2, " How would you like to authenticate?", fg, bg);
            let items = vec![
                " API Key    Enter an API key manually".to_string(),
                // OAuth — not yet implemented. Uncomment when browser redirect → callback → token exchange is built.
                // " OAuth      Sign in with your account".to_string(),
            ];
            let _ = render_selectable_list(f, area, 4, &items, *selected, &styles, theme, None);
            render_hint(
                f,
                area,
                " Up/Down select  |  Enter confirm  |  Esc cancel",
                styles.muted,
            );
        }

        SetupStep::SelectProvider {
            providers,
            selected,
            ..
        } => {
            render_title(f, area, 2, " Select a provider to get started", fg, bg);
            // Provider list starts below the title (y_offset 3: title at 2 + 1 gap)
            let list_area = Rect {
                x: area.x,
                y: area.y + 3,
                width: area.width,
                height: area.height.saturating_sub(3),
            };
            let scroll = render_provider_list(f, list_area, providers, *selected, &styles, theme);
            let pos = scroll.as_ref().map_or(String::new(), |s| s.hint());
            let hint = format!(
                " Up/Down select  |  Enter confirm  |  q quit  ({} providers){}",
                providers.len(),
                pos
            );
            render_hint(f, area, &hint, styles.muted);
        }

        SetupStep::EnterApiKey { provider, key, .. } => {
            let title = format!(" Enter API key for {}", provider);
            render_title(f, area, 3, &title, fg, bg);
            render_input_field(f, area, 5, "API Key", key, theme, &styles);
            render_hint(
                f,
                area,
                " Type your key  |  Enter save  |  Esc back",
                styles.muted,
            );
        }

        SetupStep::SelectModel {
            provider,
            models,
            selected,
        } => {
            let title = format!(" Select a model for {}", provider);
            render_title(f, area, 2, &title, fg, bg);
            let scroll =
                render_selectable_list(f, area, 4, models, *selected, &styles, theme, None);
            let pos = scroll.as_ref().map_or(String::new(), |s| s.hint());
            let hint = format!(
                " Up/Down select  |  Enter confirm  |  Esc back  ({} models){}",
                models.len(),
                pos
            );
            render_hint(f, area, &hint, styles.muted);
        }

        SetupStep::Done { provider, model } => {
            let msg = format!(" {} is ready!", provider);
            let msg_y = area.y + 4;
            f.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default()
                        .fg(theme.colors.success.to_ratatui())
                        .bg(bg)
                        .bold(),
                )),
                Rect {
                    x: area.x + 2,
                    y: msg_y,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
            let model_line = format!(" Model: {}", model);
            f.render_widget(
                Paragraph::new(Span::styled(model_line, styles.normal)),
                Rect {
                    x: area.x + 2,
                    y: msg_y + 1,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
            f.render_widget(
                Paragraph::new(Span::styled(" Press Enter to start chatting", styles.muted)),
                Rect {
                    x: area.x + 2,
                    y: msg_y + 3,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
        }
    }
}

// ── Model select overlay ────────────────────────────────────────────────

fn render_model_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let (_provider, models, filter, selected) = match &state.overlay {
        Some(AppOverlay::ModelSelect {
            provider,
            models,
            filter,
            selected,
        }) => (provider.clone(), models.clone(), filter.clone(), *selected),
        _ => return,
    };

    // Compute filtered models
    let filtered: Vec<String> = if filter.is_empty() {
        models.clone()
    } else {
        let lower = filter.to_lowercase();
        models
            .iter()
            .filter(|m| m.to_lowercase().contains(&lower))
            .cloned()
            .collect()
    };

    // Map selected index to filtered list
    let selected_in_filtered = if filter.is_empty() {
        selected
    } else {
        filtered
            .iter()
            .position(|m| m == &models[selected])
            .unwrap_or(0)
    };

    let popup = centered_layout(area, 0.7, 0.7);
    let inner = render_popup_frame(
        f,
        popup,
        theme.colors.background.to_ratatui(),
        theme.colors.border.to_ratatui(),
    );

    // Title + filter
    let title_line = if filter.is_empty() {
        " Select a model ".to_string()
    } else {
        format!(" Filter: {} ", filter)
    };
    render_title(
        f,
        inner,
        0,
        &title_line,
        theme.colors.primary.to_ratatui(),
        theme.colors.background.to_ratatui(),
    );

    // List
    let scroll = render_selectable_list(
        f,
        inner,
        2,
        &filtered,
        selected_in_filtered,
        &styles,
        theme,
        None,
    );

    // Hint
    let pos = scroll.as_ref().map_or(String::new(), |s| s.hint());
    let hint = format!(
        " {} models  |  Up/Down  |  type to filter  |  Enter select  |  Esc cancel{}",
        filtered.len(),
        pos
    );
    render_hint(f, inner, &hint, styles.muted);
}

// ── Logout select overlay ───────────────────────────────────────────────

fn render_logout_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let (providers, selected) = match &state.overlay {
        Some(AppOverlay::LogoutSelect {
            providers,
            selected,
        }) => (providers.clone(), *selected),
        _ => return,
    };

    let popup = centered_layout(area, 0.5, 0.5);
    let inner = render_popup_frame(
        f,
        popup,
        theme.colors.background.to_ratatui(),
        theme.colors.border.to_ratatui(),
    );

    render_title(
        f,
        inner,
        0,
        " Select provider to logout",
        theme.colors.error.to_ratatui(),
        theme.colors.background.to_ratatui(),
    );
    let _ = render_selectable_list(
        f,
        inner,
        2,
        &providers,
        selected,
        &styles,
        theme,
        Some(theme.colors.error.to_ratatui()),
    );
    render_hint(
        f,
        inner,
        " Up/Down select  |  Enter remove  |  Esc cancel",
        styles.muted,
    );
}

fn render_resume_select(f: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let styles = theme.to_styles();

    let sessions = match &state.overlay {
        Some(AppOverlay::ResumeSelect { sessions, .. }) => sessions.clone(),
        _ => return,
    };

    if sessions.is_empty() {
        return;
    }

    let selected = match &state.overlay {
        Some(AppOverlay::ResumeSelect { selected, .. }) => *selected,
        _ => 0,
    };

    // Build session labels
    let items: Vec<String> = sessions
        .iter()
        .map(|s| {
            let name_or_id = s
                .name
                .clone()
                .unwrap_or_else(|| s.id[..8.min(s.id.len())].to_string());
            format!("{} — {} ({} messages)", name_or_id, s.cwd, s.message_count)
        })
        .collect();

    let popup = centered_layout(area, 0.6, 0.6);
    let inner = render_popup_frame(
        f,
        popup,
        theme.colors.background.to_ratatui(),
        theme.colors.border.to_ratatui(),
    );

    render_title(
        f,
        inner,
        0,
        " Resume session",
        theme.colors.primary.to_ratatui(),
        theme.colors.background.to_ratatui(),
    );
    let scroll = render_selectable_list(f, inner, 2, &items, selected, &styles, theme, None);
    let pos = scroll.as_ref().map_or(String::new(), |s| s.hint());
    let hint = format!(" Up/Down select  |  Enter resume  |  Esc cancel{}", pos);
    render_hint(f, inner, &hint, styles.muted);
}

// ── Notifications (Toasts) ───────────────────────────────────────────────

const NOTIF_MIN_WIDTH: u16 = 36;
const NOTIF_MAX_WIDTH: u16 = 60;
const NOTIF_PADDING: u16 = 2;
const NOTIF_GAP: u16 = 1;
const NOTIF_RIGHT_MARGIN: u16 = 2;
const NOTIF_BOTTOM_OFFSET: u16 = 5;

fn split_line_by_width(text: &str, max_width: usize) -> (String, String) {
    if max_width == 0 {
        return (String::new(), text.to_string());
    }

    let mut width = 0usize;
    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            break;
        }
        width += cw;
        end = idx + ch.len_utf8();
    }

    let line = text[..end].to_string();
    let rest = text[end..].trim_start().to_string();
    (line, rest)
}

fn render_notifications(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if state.notifications.is_empty() {
        return;
    }

    let available_width = area.width.saturating_sub(NOTIF_RIGHT_MARGIN);
    if area.height < 4 || available_width < 8 {
        return;
    }

    // Stack notifications from bottom to top (most recent first)
    let visible: Vec<_> = state.notifications.iter().rev().take(3).collect();
    let max_inner_width = NOTIF_MAX_WIDTH.min(available_width.saturating_sub(1));
    let min_inner_width = NOTIF_MIN_WIDTH.min(max_inner_width);

    if max_inner_width == 0 {
        return;
    }

    let mut y_offset = 0u16;
    let base_y = area.y + area.height.saturating_sub(NOTIF_BOTTOM_OFFSET);
    let text_style = Style::default().fg(theme.colors.foreground.to_ratatui());

    for notif in &visible {
        let (icon, fg_color, bg_color, border_color) = match notif.kind {
            NotificationKind::Success => (
                "\u{2713}", // ✓
                theme.colors.success.to_ratatui(),
                theme.colors.tool_success_bg.to_ratatui(),
                theme.colors.success.to_ratatui(),
            ),
            NotificationKind::Warning => (
                "\u{26A0}", // ⚠
                theme.colors.warning.to_ratatui(),
                theme.colors.tool_executing_bg.to_ratatui(),
                theme.colors.warning.to_ratatui(),
            ),
            NotificationKind::Error => (
                "\u{2717}", // ✗
                theme.colors.error.to_ratatui(),
                theme.colors.tool_error_bg.to_ratatui(),
                theme.colors.error.to_ratatui(),
            ),
            NotificationKind::Info => (
                "\u{2139}", // ℹ
                theme.colors.primary.to_ratatui(),
                theme.colors.tool_pending_bg.to_ratatui(),
                theme.colors.primary.to_ratatui(),
            ),
        };

        let icon_label = icon.to_string();
        let icon_width = UnicodeWidthStr::width(icon_label.as_str()) as u16;
        let icon_gap_width = icon_width.saturating_add(1);
        let message_width = UnicodeWidthStr::width(notif.message.as_str()) as u16;

        let desired_inner_width = message_width
            .saturating_add(icon_gap_width)
            .saturating_add(NOTIF_PADDING * 2)
            .max(min_inner_width)
            .min(max_inner_width);
        let notif_width = desired_inner_width.saturating_add(1).min(available_width);

        let inner_width = notif_width.saturating_sub(1);
        let text_area_width = inner_width.saturating_sub(NOTIF_PADDING * 2);
        if text_area_width == 0 {
            continue;
        }
        let line1_width = text_area_width.saturating_sub(icon_gap_width);
        let line2_width = text_area_width;

        let padding = " ".repeat(NOTIF_PADDING as usize);
        let icon_pad = " ".repeat(icon_gap_width as usize);

        let (line1_text, remainder) = split_line_by_width(&notif.message, line1_width as usize);

        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::raw(padding.clone()),
            Span::styled(
                icon_label.clone(),
                Style::default().fg(fg_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(line1_text, text_style),
        ]));

        if !remainder.is_empty() && line2_width > 0 {
            let line2_text = truncate_to_width(&remainder, line2_width as usize);
            lines.push(Line::from(vec![
                Span::raw(padding.clone()),
                Span::raw(icon_pad.clone()),
                Span::styled(line2_text, text_style),
            ]));
        }

        let card_height = 2 + lines.len() as u16;
        let notif_y = base_y.saturating_sub(y_offset + card_height);
        if notif_y < area.y {
            break;
        }

        let notif_x = area.x
            + area
                .width
                .saturating_sub(notif_width)
                .saturating_sub(NOTIF_RIGHT_MARGIN);

        let notif_area = Rect {
            x: notif_x,
            y: notif_y,
            width: notif_width,
            height: card_height,
        };

        // Clear behind the notification
        f.render_widget(ratatui::widgets::Clear, notif_area);

        // Background card with all borders
        let card_block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(bg_color));
        let inner = card_block.inner(notif_area);
        f.render_widget(
            ratatui::widgets::Paragraph::new("")
                .style(Style::default().bg(bg_color))
                .block(card_block),
            notif_area,
        );

        f.render_widget(ratatui::widgets::Paragraph::new(lines), inner);

        y_offset += card_height + NOTIF_GAP;
    }
}
