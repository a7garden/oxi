//! Rendering functions for the TUI.

use super::app::{AppState, NotificationKind};
use oxi_tui_legacy::theme::Theme;
use oxi_tui_legacy::truncate_to_width;
use oxi_tui_legacy::widgets::{chat::ChatView, footer::Footer, input::Input};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Char-safe truncation for the footer indicator. Byte-based slicing would
/// panic on multi-byte UTF-8 (e.g., 28th CJK char starting mid-codepoint).
fn truncate_for_footer(s: &str) -> String {
    const MAX: usize = 40;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(MAX).collect();
        format!("{truncated}…")
    }
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

    // Todo panel height (0 if no phases)
    let todo_height = state.todo_panel.line_count() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),                    // Chat
            Constraint::Length(todo_height),       // Todo sticky panel
            Constraint::Length(input_area_height), // Status + queue + input (dynamic)
            Constraint::Length(3),                 // Footer
        ])
        .split(inner);

    f.render_stateful_widget(ChatView::new(theme), chunks[0], &mut state.chat);

    // Todo sticky panel (only renders if non-empty)
    if todo_height > 0 {
        f.render_stateful_widget(
            oxi_tui_legacy::widgets::todo_panel::TodoPanel::new(theme),
            chunks[1],
            &mut state.todo_panel,
        );
    }

    // Input area (status line + optional queue + input)
    render_input_area(f, chunks[2], state, theme);

    // Slash popup — overlay above the input area
    if state.slash_completion_active {
        render_slash_popup_overlay(f, chunks[2], state, theme);
    }

    // File path completion popup
    if state.file_completion_active {
        render_file_completion_popup(f, chunks[2], state, theme);
    }

    // Status bar
    state.footer_state.data.is_busy = state.is_agent_busy;
    // Issue indicator: shows open count, lock count, top-priority dot, and
    // the most recent open issue title. Cheap (uses the in-memory cache in
    // `FileIssueStore::summary`).
    state.footer_state.data.extra_right = match &state.issue_store {
        Some(store) => {
            let s = store.summary();
            if s.is_empty() {
                String::new()
            } else {
                // Priority uses block-shade density (no color needed — the
                // footer extra_right is a single String rendered with one
                // accent color, so per-priority color is impossible here).
                let prio = match s.top_priority {
                    Some(crate::store::issues::Priority::Critical) => "█",
                    Some(crate::store::issues::Priority::High) => "▓",
                    Some(crate::store::issues::Priority::Medium) => "▒",
                    Some(crate::store::issues::Priority::Low) => "░",
                    None => "",
                };
                let lock = if s.locked_open_count > 0 {
                    format!(" \u{25A6}{}", s.locked_open_count)
                } else {
                    String::new()
                };
                let title_part = match &s.latest_open_title {
                    Some(t) => format!(" · {}", truncate_for_footer(t)),
                    None => String::new(),
                };
                format!("{prio}# {}{lock}{title_part}", s.open_count)
            }
        }
        None => String::new(),
    };
    f.render_stateful_widget(Footer::new(theme), chunks[3], &mut state.footer_state);

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

    let [status_row, queue_area, input_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(queue_lines),
        Constraint::Fill(1),
    ])
    .areas(area);

    render_status_line(f, status_row, state, theme);

    if queue_lines > 0 {
        if is_active {
            render_queue_active(f, queue_area, state, theme);
        } else {
            render_queue_compact(f, queue_area, state, theme);
        }
    }

    state.input.set_placeholder(None);
    f.render_stateful_widget(Input::new(theme), input_row, &mut state.input);
    // Cursor bridge: record the textarea's screen cursor (relative to the
    // rendered content area) translated to absolute terminal coordinates.
    // The v2 pipeline reads `last_input_cursor` and calls `ctx.set_cursor` so
    // CursorState positions the terminal cursor — the legacy Input widget
    // paints into the buffer but never sets the cursor slot. The Input widget
    // insets the textarea by one column of left padding (widgets/input.rs),
    // so the content area origin is `input_row` + (1, 0).
    let (row, col) = state.input.screen_cursor();
    state.last_input_cursor = Some(ratatui::layout::Position {
        x: input_row
            .x
            .saturating_add(1)
            .saturating_add(u16::try_from(col).unwrap_or(u16::MAX)),
        y: input_row
            .y
            .saturating_add(u16::try_from(row).unwrap_or(u16::MAX)),
    });
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
            y: area.y + i as u16,
            width: area.width,
            height: 1,
        };
        let badge = if i == 0 { "next" } else { "" };
        let num = format!("{}. ", i + 1);
        let prefix_w: u16 = (num.len() + badge.len()) as u16;
        let text = msg.text_content().unwrap_or_default();
        let max_chars = area.width.saturating_sub(prefix_w + 3) as usize;
        let truncated: String = text.chars().take(max_chars).collect();
        let ellipsis = if text.chars().count() > max_chars {
            "…"
        } else {
            ""
        };

        let mut spans: Vec<Span<'_>> = Vec::new();
        // Number
        spans.push(Span::styled(num, Style::default().fg(theme.colors.muted)));
        // "next" badge for first item
        if i == 0 {
            spans.push(Span::styled(
                "next ",
                Style::default()
                    .fg(theme.colors.accent)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
        // Message text
        let msg_style = if i == 0 {
            Style::default().fg(theme.colors.foreground)
        } else {
            Style::default().fg(theme.colors.muted)
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
                        .fg(theme.colors.muted)
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
            y: area.y + i as u16,
            width: area.width,
            height: 1,
        };
        let is_sel = i == selected;
        let pointer = if is_sel { "› " } else { "  " };
        let badge = if i == 0 { "next " } else { "" };
        let num = format!("{}. ", i + 1);
        let prefix_w: u16 = (pointer.len() + num.len() + badge.len()) as u16;
        let text = msg.text_content().unwrap_or_default();
        let max_chars = area.width.saturating_sub(prefix_w + 2) as usize;
        let truncated: String = text.chars().take(max_chars).collect();
        let ellipsis = if text.chars().count() > max_chars {
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
                    .fg(theme.colors.accent)
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
                    .fg(theme.colors.background)
                    .bg(theme.colors.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.colors.muted)
            },
        ));
        // "next" badge for first
        if i == 0 {
            spans.push(Span::styled(
                badge,
                if is_sel {
                    Style::default()
                        .fg(theme.colors.background)
                        .bg(theme.colors.accent)
                        .add_modifier(Modifier::ITALIC)
                } else {
                    Style::default()
                        .fg(theme.colors.accent)
                        .add_modifier(Modifier::ITALIC)
                },
            ));
        }
        // Message text
        let msg_style = if is_sel {
            Style::default()
                .fg(theme.colors.background)
                .bg(theme.colors.accent)
                .add_modifier(Modifier::BOLD)
        } else if i == 0 {
            Style::default().fg(theme.colors.foreground)
        } else {
            Style::default().fg(theme.colors.muted)
        };
        spans.push(Span::styled(
            format!("{}{}", truncated, ellipsis),
            msg_style,
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }

    // Hint line at the bottom of allocated space
    let hint_y = area.y + max_items as u16;
    if hint_y < area.y + area.height {
        let row = Rect {
            x: area.x,
            y: hint_y,
            width: area.width,
            height: 1,
        };
        let hint = " ↑↓ nav · d del · e edit · Esc close";
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(theme.colors.muted))),
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
            Paragraph::new(Span::styled(line, Style::default().fg(theme.colors.accent))),
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
            Paragraph::new(Span::styled(line, Style::default().fg(theme.colors.muted))),
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
                        Style::default().fg(theme.colors.accent),
                    ),
                    Span::styled(
                        format!(" {}  ", name_padded),
                        Style::default()
                            .fg(theme.colors.background)
                            .bg(theme.colors.primary)
                            .bold(),
                    ),
                    Span::styled(desc, Style::default().fg(theme.colors.muted)),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", pointer), Style::default()),
                    Span::styled(
                        format!(" {}  ", name_padded),
                        Style::default().fg(theme.colors.foreground),
                    ),
                    Span::styled(desc, Style::default().fg(theme.colors.muted)),
                ]))
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.colors.border));
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
                Style::default().fg(theme.colors.muted),
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
    let bg = theme.colors.background;

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
    // ── Component-based overlay ──
    // All overlays have been migrated to `Box<dyn OverlayComponent>`.
    if let Some(ref mut overlay) = state.overlay_state {
        overlay.render(f, area, theme);
    }
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
    let text_style = Style::default().fg(theme.colors.foreground);

    for notif in &visible {
        let (icon, fg_color, bg_color, border_color) = match notif.kind {
            NotificationKind::Success => (
                theme.symbols.status_success,
                theme.colors.success,
                theme.colors.tool_success_bg,
                theme.colors.success,
            ),
            NotificationKind::Warning => (
                theme.symbols.status_warning,
                theme.colors.warning,
                theme.colors.tool_executing_bg,
                theme.colors.warning,
            ),
            NotificationKind::Error => (
                theme.symbols.status_error,
                theme.colors.error,
                theme.colors.tool_error_bg,
                theme.colors.error,
            ),
            NotificationKind::Info => (
                theme.symbols.status_info,
                theme.colors.primary,
                theme.colors.tool_pending_bg,
                theme.colors.primary,
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
