//! EntryWidget + Widget impl — renders layout entries into the frame buffer.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::text::truncate_to_width as truncate_str;
use crate::theme::ThemeStyles;
use crate::widgets::chat::dashboard::dashboard_lines;
use crate::widgets::chat::layout::LayoutKind;
use crate::widgets::chat::markdown::{filter_tool_json, md_lines};
use crate::widgets::chat::types::ToolCallStatus;

// ── Rendering into ScrollView ─────────────────────────────────────────

/// A wrapper widget that renders a single content block.
pub(crate) struct EntryWidget<'a> {
    pub entry: &'a LayoutKind,
    pub styles: &'a ThemeStyles,
}

impl<'a> EntryWidget<'a> {
    pub fn new(entry: &'a LayoutKind, styles: &'a ThemeStyles) -> Self {
        Self { entry, styles }
    }
}

impl Widget for EntryWidget<'_> {
    fn render(self, rect: Rect, buf: &mut Buffer) {
        match &self.entry {
            LayoutKind::Spacer => { /* empty line, already cleared */ }
            LayoutKind::Rule => {
                let line = "\u{2500}".repeat(rect.width as usize); // ─
                Line::from(Span::styled(line, self.styles.border)).render(rect, buf);
            }
            LayoutKind::Label { text, style } => {
                Paragraph::new(Line::from(Span::styled(text.clone(), *style))).render(rect, buf);
            }
            LayoutKind::Text { lines, is_user } => {
                if *is_user {
                    let block = Block::default()
                        .borders(Borders::LEFT)
                        .border_style(self.styles.user_border);
                    let inner = block.inner(rect);
                    block.render(rect, buf);
                    // Lines are already pre-wrapped to the correct width
                    // by wrap_lines_styled(). Do NOT use .wrap() here —
                    // ratatui's WordWrapper does not handle CJK line-breaking.
                    //
                    // Use buf.set_line() instead of Paragraph::render() to ensure
                    // multi-cell characters (emojis, wide CJK) have their trailing
                    // cells properly reset. Paragraph::render_line() skips this,
                    // causing Buffer::diff to corrupt wide character output.
                    for (i, line) in lines.iter().enumerate() {
                        let y = inner.y + i as u16;
                        if y >= inner.bottom() {
                            break;
                        }
                        buf.set_line(inner.x, y, line, inner.width);
                    }
                } else {
                    // Don't set .style() here — markdown Spans already carry
                    // their own styling (bold, italic, code, headings).
                    // Paragraph::style() would override all per-Span styles.
                    // Lines are pre-wrapped; no .wrap() needed.
                    //
                    // Use buf.set_line() instead of Paragraph::render() for the
                    // same reason as above: trailing cells of wide characters
                    // must be reset for correct terminal output.
                    for (i, line) in lines.iter().enumerate() {
                        let y = rect.y + i as u16;
                        if y >= rect.bottom() {
                            break;
                        }
                        buf.set_line(rect.x, y, line, rect.width);
                    }
                }
            }
            LayoutKind::ToolBox {
                name,
                arguments,
                result,
                status,
                duration,
                expanded,
                key: _,
            } => {
                use crate::widgets::tool_renderer::{format_tool_call, format_tool_result};

                let (icon, border_style) = match status {
                    ToolCallStatus::Requested => (
                        "\u{25CB}", // ○ (hollow circle — universal)
                        self.styles.muted,
                    ),
                    ToolCallStatus::Executing => (
                        "\u{25CF}", // ● (filled circle — running)
                        self.styles.warning,
                    ),
                    ToolCallStatus::Done => {
                        let is_error = result.as_ref().is_some_and(|(_, e)| *e);
                        if is_error {
                            ("\u{2718}", self.styles.error)
                        } else {
                            ("\u{2713}", self.styles.success)
                        }
                    }
                };

                let has_result = result.is_some();

                // Determine background style based on status
                let bg_style = match status {
                    ToolCallStatus::Requested => self.styles.tool_pending_bg,
                    ToolCallStatus::Executing => self.styles.tool_executing_bg,
                    ToolCallStatus::Done => {
                        let is_error = result.as_ref().is_some_and(|(_, e)| *e);
                        if is_error {
                            self.styles.tool_error_bg
                        } else {
                            self.styles.tool_success_bg
                        }
                    }
                };

                // Box with all borders + status background
                // Use dashed border for pending tool calls to distinguish from active ones
                let border_type = match status {
                    ToolCallStatus::Requested => BorderType::LightDoubleDashed,
                    ToolCallStatus::Executing | ToolCallStatus::Done => BorderType::Plain,
                };

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(border_type)
                    .border_style(border_style)
                    .style(bg_style);
                let inner = block.inner(rect);
                block.render(rect, buf);
                // Do NOT Clear the inner area here — Clear resets cells to
                // default style, which removes the block's background color
                // (tool_pending_bg, tool_success_bg, etc.). The buffer is
                // already clean at the start of each frame (ratatui
                // swap_buffers resets it), so stale content is not an issue.

                let max_w = inner.width as usize;
                let mut content_lines: Vec<Line<'static>> = Vec::new();

                // Calculate reserved space for icon prefix and duration suffix
                // on the header line so format_tool_call truncates correctly.
                let icon_prefix_w = UnicodeWidthStr::width(format!("{} ", icon).as_str());
                let duration_suffix_w = duration
                    .as_ref()
                    .map(|d| UnicodeWidthStr::width(format!("  {}", d).as_str()))
                    .unwrap_or(0);
                let header_avail = max_w
                    .saturating_sub(icon_prefix_w)
                    .saturating_sub(duration_suffix_w)
                    .max(20);

                // Format tool call using new renderer (truncate to header_avail)
                let call_lines = format_tool_call(name, arguments, header_avail, self.styles);
                for (i, line) in call_lines.into_iter().enumerate() {
                    if i == 0 {
                        // Prepend icon to first line, append duration if available
                        let icon_style = border_style.add_modifier(Modifier::BOLD);
                        let name_style = border_style.add_modifier(Modifier::BOLD);
                        let spans = line.spans.into_iter().collect::<Vec<_>>();
                        let mut new_spans = vec![Span::styled(format!("{} ", icon), icon_style)];
                        for span in spans {
                            new_spans.push(Span::styled(
                                span.content.clone(),
                                span.style.patch(name_style),
                            ));
                        }
                        // Append duration to header line
                        if let Some(dur) = duration {
                            new_spans.push(Span::styled(format!("  {}", dur), self.styles.muted));
                        }
                        content_lines.push(Line::from(new_spans));
                    } else {
                        content_lines.push(line);
                    }
                }

                // Separator line between call and result — full width to match borders
                if has_result {
                    content_lines.push(Line::from(Span::styled(
                        "\u{2500}".repeat(max_w),
                        border_style,
                    )));
                }

                // Format result
                if let Some((result_content, is_err)) = result {
                    if *expanded {
                        // Expanded: show full result (up to 80 lines)
                        let all_lines: Vec<&str> = result_content.lines().collect();
                        let total = all_lines.len();
                        let shown = total.min(80);
                        for line in &all_lines[..shown] {
                            let display =
                                crate::text::truncate_to_width(line, max_w.saturating_sub(2));
                            content_lines.push(Line::from(Span::styled(
                                format!("  {}", display),
                                if *is_err {
                                    self.styles.error
                                } else {
                                    self.styles.normal
                                },
                            )));
                        }
                        if total > 80 {
                            content_lines.push(Line::from(Span::styled(
                                crate::text::truncate_to_width(
                                    &format!("  \u{2026} ({} more lines)", total - 80),
                                    max_w,
                                ),
                                self.styles.muted,
                            )));
                        }
                    } else {
                        // Collapsed: use the formatted preview
                        let result_lines =
                            format_tool_result(name, result_content, *is_err, max_w, self.styles);
                        content_lines.extend(result_lines);
                    }

                    // Toggle hint — truncate to max_w to prevent overflow
                    let total_lines = result_content.lines().count();
                    let toggle_hint = if *expanded {
                        " \u{00B7} click to collapse".to_string()
                    } else {
                        format!(" \u{00B7} {} lines \u{00B7} click to expand", total_lines)
                    };
                    content_lines.push(Line::from(Span::styled(
                        crate::text::truncate_to_width(&toggle_hint, max_w),
                        self.styles.muted,
                    )));
                }

                let text: ratatui::text::Text = content_lines.into_iter().collect();
                let para = Paragraph::new(text);
                para.render(inner, buf);
            }
            LayoutKind::ToolResultBox {
                tool_name,
                content,
                is_error,
            } => {
                let (icon, border_style) = if *is_error {
                    ("\u{2718}", self.styles.error)
                } else {
                    ("\u{2713}", self.styles.success)
                };
                let label = if tool_name.is_empty() {
                    icon.to_string()
                } else {
                    format!("{} {}", icon, tool_name)
                };

                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(border_style);
                let inner = block.inner(rect);
                block.render(rect, buf);

                let max_w = inner.width as usize;
                let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
                    format!("  {}", label),
                    border_style.add_modifier(Modifier::BOLD),
                ))];
                for l in content.lines().take(4) {
                    let display = truncate_str(l, max_w.saturating_sub(2));
                    lines.push(Line::from(Span::styled(
                        format!("  {}", display),
                        self.styles.normal,
                    )));
                }
                if content.lines().count() > 4 {
                    lines.push(Line::from(Span::styled("  \u{2026}", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::ErrorBox {
                title,
                message,
                retryable,
            } => {
                let block = Block::bordered()
                    .border_type(BorderType::Double)
                    .border_style(self.styles.error)
                    .title(Span::styled(
                        format!(" error: {} ", title),
                        Style::default()
                            .fg(ratatui::style::Color::White)
                            .add_modifier(Modifier::BOLD),
                    ));
                let inner = block.inner(rect);
                block.render(rect, buf);

                let max_w = inner.width as usize;
                let mut lines: Vec<Line<'static>> = Vec::new();
                for l in message.lines().take(4) {
                    let display = truncate_str(l, max_w);
                    lines.push(Line::from(Span::styled(display, self.styles.normal)));
                }
                if *retryable {
                    lines.push(Line::from(Span::styled(
                        "retry: this error may be temporary",
                        self.styles.muted,
                    )));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                // No wrap — pre-truncated to exact width
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::Thinking {
                content,
                collapsed,
                key: _,
            } => {
                let filtered = filter_tool_json(content);
                let line_count = filtered.lines().count();
                let mut lines: Vec<Line<'static>> = Vec::new();

                // Header with toggle hint
                let header_style = self.styles.accent;
                let count_str = if line_count > 0 {
                    format!(
                        " ({} line{})",
                        line_count,
                        if line_count == 1 { "" } else { "s" }
                    )
                } else {
                    String::new()
                };

                if *collapsed {
                    lines.push(Line::from(vec![
                        Span::styled("\u{2699} ", header_style),
                        Span::styled("Thinking".to_string(), header_style),
                        Span::styled(count_str, self.styles.muted),
                        Span::styled(" \u{00B7} click to expand".to_string(), self.styles.muted),
                    ]));
                    if let Some(first) = filtered.lines().next() {
                        let preview: String = first.chars().take(80).collect();
                        lines.push(Line::from(Span::styled(
                            format!("  {}", preview),
                            self.styles.muted.add_modifier(Modifier::ITALIC),
                        )));
                    }
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("\u{2699} ", header_style),
                        Span::styled("Thinking".to_string(), header_style),
                        Span::styled(count_str, self.styles.muted),
                    ]));
                    let thinking_style = self.styles.muted.add_modifier(Modifier::ITALIC);
                    let md_rendered = md_lines(&filtered, rect.width, self.styles);
                    for md_line in md_rendered {
                        let spans: Vec<Span<'static>> = md_line
                            .spans
                            .into_iter()
                            .map(|s| {
                                let combined = thinking_style.patch(s.style);
                                Span::styled(s.content.into_owned(), combined)
                            })
                            .collect();
                        lines.push(Line::from(spans));
                    }
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(rect, buf);
            }
            LayoutKind::ResponseDivider => {
                let style = self.styles.muted;
                let label = " Response ";
                let total_w = rect.width as usize;
                let label_w = label.len();
                let dash_count = total_w.saturating_sub(label_w) / 2;
                let left_dashes = "\u{2500}".repeat(dash_count);
                let right_dashes = "\u{2500}".repeat(total_w.saturating_sub(dash_count + label_w));
                let line_str = format!("{}{}{}", left_dashes, label, right_dashes);
                Line::from(Span::styled(line_str, style)).render(rect, buf);
            }
            LayoutKind::Image {
                mime_type,
                size_str,
            } => {
                // Try terminal image protocol if supported
                // For now, show placeholder with image info
                // Actual protocol output happens post-render in app.rs
                let lines = vec![
                    Line::from(Span::styled(
                        format!("\u{1F5BC} [image: {}, {}]", mime_type, size_str),
                        self.styles.normal,
                    )),
                    Line::from(Span::styled(
                        "  Ctrl+I -> open in viewer",
                        self.styles.muted,
                    )),
                ];
                let text: ratatui::text::Text = lines.into_iter().collect();
                Clear.render(rect, buf);
                Paragraph::new(text).render(rect, buf);
            }
            LayoutKind::Spinner { frame } => {
                // Moon phase spinner ◐ ◓ ◑ ◒
                let sp = ["\u{25D0}", "\u{25D3}", "\u{25D1}", "\u{25D2}"];
                let ch = sp[frame % sp.len()];
                Paragraph::new(Line::from(Span::styled(
                    format!("  {} Working...", ch),
                    self.styles.accent,
                )))
                .render(rect, buf);
            }
            LayoutKind::Dashboard { info } => {
                let lines = dashboard_lines(info, rect.width, self.styles);
                let block = Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(self.styles.border);
                let inner = block.inner(rect);
                block.render(rect, buf);
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(inner, buf);
            }
        }
    }
}
