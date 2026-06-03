//! Layout calculation — LayoutKind, LayoutEntry, compute_layout, measure_*.

use std::collections::HashSet;

use ratatui::style::Style;
use ratatui::text::Line;

use crate::theme::ThemeStyles;
use crate::widgets::chat::dashboard::{measure_dashboard, DashboardInfo};
use crate::widgets::chat::markdown::{filter_tool_json, md_lines};
use crate::widgets::chat::state::ChatViewState;
use crate::widgets::chat::types::{ContentBlock, MessageRole, ToolCallStatus};

// ── Layout calculation ────────────────────────────────────────────────

/// Measure the rendered height of pre-wrapped lines.
///
/// Lines are already wrapped to the correct width by `wrap_lines_styled()`,
/// so we just count them. No need to use `Paragraph::line_count` which
/// would try to re-wrap and produce incorrect results for CJK text.
pub(crate) fn measure_wrapped_height(lines: &[Line<'_>], _width: u16) -> u16 {
    lines.len() as u16
}

/// Calculate the layout: list of (y, height, block_ref) for each piece of content.
#[derive(Clone)]
pub(crate) struct LayoutEntry {
    pub y: u16,
    pub height: u16,
    pub kind: LayoutKind,
}

#[derive(Clone)]
pub(crate) enum LayoutKind {
    Spacer,
    Rule,
    #[allow(dead_code)]
    Label {
        text: String,
        style: Style,
    },
    Text {
        lines: Vec<Line<'static>>,
        is_user: bool,
    },
    ToolBox {
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
        duration: Option<String>,
        expanded: bool,
        key: String,
    },
    ToolResultBox {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    ErrorBox {
        title: String,
        message: String,
        retryable: bool,
    },
    Thinking {
        content: String,
        collapsed: bool,
        key: String,
    },
    Image {
        mime_type: String,
        size_str: String,
    },
    #[allow(dead_code)]
    Spinner {
        frame: usize,
    },
    Dashboard {
        info: DashboardInfo,
    },
}

/// Check if a content block renders as a bordered box (tool calls, errors).
/// Consecutive box blocks need spacers between them for visual separation.
fn is_box_block(block: &ContentBlock) -> bool {
    matches!(
        block,
        ContentBlock::ToolCall { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Error { .. }
    )
}

pub(crate) fn compute_layout(
    state: &ChatViewState,
    width: u16,
    styles: &ThemeStyles,
) -> Vec<LayoutEntry> {
    let mut entries = Vec::new();
    let mut y: u32 = 0;

    let mut rendered_any_message = false;
    let mut msg_idx: usize = 0;

    'outer: for msg in &state.messages {
        // Skip messages that have no visible content; they only create empty spacer rows.
        let has_visible_content = msg.content_blocks.iter().any(|b| match b {
            ContentBlock::Text { content } => !content.trim().is_empty(),
            ContentBlock::Thinking { content, .. } => !content.trim().is_empty(),
            _ => true,
        });
        if !has_visible_content {
            msg_idx += 1;
            continue;
        }

        if rendered_any_message {
            // Gap between messages — use a spacer for breathing room
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: 1,
                    kind: LayoutKind::Spacer,
                });
            }
            y += 1;
        }
        rendered_any_message = true;

        // User messages: left accent border, no label needed (single-user context)
        if msg.role == MessageRole::User {
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: 1,
                    kind: LayoutKind::Rule,
                });
            }
            y += 1;
        }
        let mut prev_was_box = false;
        for (blk_idx, block) in msg.content_blocks.iter().enumerate() {
            // Skip whitespace-only blocks (defensive; finish_streaming also removes them).
            let is_empty = match block {
                ContentBlock::Text { content } => content.trim().is_empty(),
                ContentBlock::Thinking { content, .. } => content.trim().is_empty(),
                _ => false,
            };
            if is_empty {
                continue;
            }

            // Insert spacer between consecutive box-type blocks (tool calls, errors)
            let is_box = is_box_block(block);
            if is_box && prev_was_box {
                if y <= u16::MAX as u32 {
                    entries.push(LayoutEntry {
                        y: y as u16,
                        height: 1,
                        kind: LayoutKind::Spacer,
                    });
                }
                y += 1;
            }
            prev_was_box = is_box;

            let key = format!("{}:{}", msg_idx, blk_idx);
            let mut kind = block_to_layout_kind(block, msg.role, width, &key, styles);
            // Override collapsed/expanded state
            #[allow(clippy::collapsible_match)]
            match &mut kind {
                LayoutKind::Thinking {
                    ref mut collapsed,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_thinking.contains(key) {
                        *collapsed = false;
                    }
                }
                LayoutKind::ToolBox {
                    ref mut expanded,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_tools.contains(key) {
                        *expanded = true;
                    }
                }
                _ => {}
            }
            let h = measure_kind(&kind, width, &state.expanded_thinking, styles);
            if y > u16::MAX as u32 {
                break 'outer;
            }
            entries.push(LayoutEntry {
                y: y as u16,
                height: h,
                kind,
            });
            y += h as u32;
        }
        msg_idx += 1;
    }

    if let Some(ref streaming) = state.streaming {
        // Only add a spacer if we actually rendered any history messages.
        if rendered_any_message {
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: 1,
                    kind: LayoutKind::Spacer,
                });
            }
            y += 1;
        }
        let mut prev_was_box = false;
        for (blk_idx, block) in streaming.message.content_blocks.iter().enumerate() {
            // Skip whitespace-only blocks (prevents large blank gaps during tool-only turns).
            let is_empty = match block {
                ContentBlock::Text { content } => content.trim().is_empty(),
                ContentBlock::Thinking { content, .. } => content.trim().is_empty(),
                _ => false,
            };
            if is_empty {
                continue;
            }

            // Insert spacer between consecutive box-type blocks (tool calls, errors)
            let is_box = is_box_block(block);
            if is_box && prev_was_box {
                if y <= u16::MAX as u32 {
                    entries.push(LayoutEntry {
                        y: y as u16,
                        height: 1,
                        kind: LayoutKind::Spacer,
                    });
                }
                y += 1;
            }
            prev_was_box = is_box;

            let key = format!("s:{}", blk_idx);
            let mut kind = block_to_layout_kind(block, MessageRole::Assistant, width, &key, styles);
            #[allow(clippy::collapsible_match)]
            match &mut kind {
                LayoutKind::Thinking {
                    ref mut collapsed,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_thinking.contains(key) {
                        *collapsed = false;
                    }
                }
                LayoutKind::ToolBox {
                    ref mut expanded,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_tools.contains(key) {
                        *expanded = true;
                    }
                }
                _ => {}
            }
            let h = measure_kind(&kind, width, &state.expanded_thinking, styles);
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: h,
                    kind,
                });
            }
            y += h as u32;
        }
        // Spinner removed: status now shown in the input separator line (render_input_area).
    }

    entries
}

fn block_to_layout_kind(
    block: &ContentBlock,
    role: MessageRole,
    width: u16,
    key: &str,
    styles: &ThemeStyles,
) -> LayoutKind {
    match block {
        ContentBlock::Text { content } => {
            // For user messages, Block::borders(LEFT) takes 1 column,
            // so text must be wrapped to width-1.
            let wrap_w = if role == MessageRole::User {
                width.saturating_sub(1)
            } else {
                width
            };
            let lines = md_lines(content, wrap_w, styles);
            LayoutKind::Text {
                lines,
                is_user: role == MessageRole::User,
            }
        }
        ContentBlock::Thinking { content, collapsed } => {
            // The actual collapsed state is determined by the key lookup later
            // in compute_layout — we store the default here.
            LayoutKind::Thinking {
                content: content.clone(),
                collapsed: *collapsed,
                key: key.to_string(),
            }
        }
        ContentBlock::ToolCall {
            name,
            arguments,
            result,
            status,
            duration,
            ..
        } => LayoutKind::ToolBox {
            name: name.clone(),
            arguments: arguments.clone(),
            result: result.clone(),
            status: *status,
            duration: duration.clone(),
            expanded: false, // overridden in compute_layout
            key: key.to_string(),
        },
        ContentBlock::ToolResult {
            tool_name,
            content,
            is_error,
        } => LayoutKind::ToolResultBox {
            tool_name: tool_name.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
        ContentBlock::Error {
            title,
            message,
            retryable,
        } => LayoutKind::ErrorBox {
            title: title.clone(),
            message: message.clone(),
            retryable: *retryable,
        },
        ContentBlock::Image {
            mime_type,
            base64_data,
        } => {
            let sz = base64_data.len() * 3 / 4;
            let sz_str = if sz >= 1_048_576 {
                format!("{:.1} MB", sz as f64 / 1_048_576.0)
            } else if sz >= 1024 {
                format!("{:.1} KB", sz as f64 / 1024.0)
            } else {
                format!("{} B", sz)
            };
            LayoutKind::Image {
                mime_type: mime_type.clone(),
                size_str: sz_str,
            }
        }
        ContentBlock::Dashboard { info } => LayoutKind::Dashboard { info: info.clone() },
    }
}

pub(crate) fn measure_kind(
    kind: &LayoutKind,
    width: u16,
    expanded_thinking: &HashSet<String>,
    styles: &ThemeStyles,
) -> u16 {
    match kind {
        LayoutKind::Spacer
        | LayoutKind::Rule
        | LayoutKind::Label { .. }
        | LayoutKind::Spinner { .. } => 1,
        LayoutKind::Text { lines, is_user } => {
            // User text: Block::borders(LEFT) takes 1 col, so inner = width-1
            // Assistant text: no block, renders at full width
            let w = if *is_user {
                width.saturating_sub(1)
            } else {
                width
            };
            measure_wrapped_height(lines, w)
        }
        LayoutKind::ToolBox {
            name,
            arguments,
            result,
            duration,
            expanded,
            ..
        } => {
            use crate::widgets::tool_renderer::{measure_call_height, measure_result_height};
            // inner width for the bordered block: borders take 2 cols
            let inner_w = width.saturating_sub(2) as usize;
            let call_h = measure_call_height(name, arguments, inner_w);
            let result_h = result.as_ref().map_or(0, |(r, is_err)| {
                if *expanded {
                    // Full result: show all lines, capped at 80
                    let total = r.lines().count();
                    let shown = total.min(80);
                    let ellipsis = if total > 80 { 1 } else { 0 };
                    shown as u16 + ellipsis
                } else if *is_err {
                    let total = r.lines().count();
                    total.min(4) as u16 + if total > 4 { 1 } else { 0 }
                } else {
                    measure_result_height(name, r, false)
                }
            });
            // Block::ALL adds top + bottom border (2 rows) + separator if result exists
            let separator_h = if result.is_some() { 1 } else { 0 };
            // Toggle hint line when result exists
            let toggle_h = if result.is_some() { 1 } else { 0 };
            let _ = duration;
            2 + call_h + separator_h + result_h + toggle_h
        }
        LayoutKind::ToolResultBox { content, .. } => {
            // 1 header + content lines (max 4) + optional ellipsis
            let n = content.lines().count().min(4);
            1 + n as u16 + if content.lines().count() > 4 { 1 } else { 0 }
        }
        LayoutKind::ErrorBox {
            message, retryable, ..
        } => {
            // Block::bordered() adds top + bottom border (2 rows)
            let n = message.lines().count().min(4);
            2 + n as u16 + if *retryable { 1 } else { 0 }
        }
        LayoutKind::Thinking {
            content,
            collapsed,
            key,
        } => {
            let is_expanded = expanded_thinking.contains(key);
            if *collapsed && !is_expanded {
                // Collapsed: header + one preview line
                let filtered = filter_tool_json(content);
                let line_count = filtered.lines().count();
                1 + if line_count > 0 { 1 } else { 0 }
            } else {
                // Expanded: header + filtered content rendered as markdown
                let filtered = filter_tool_json(content);
                let md = md_lines(&filtered, width, styles);
                1 + md.len() as u16
            }
        }
        LayoutKind::Image { .. } => 2,
        LayoutKind::Dashboard { info } => measure_dashboard(info, width),
    }
}
