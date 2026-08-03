//! Width-aware projection of chat messages into memoized ANSI tape rows.

use std::hash::{Hash, Hasher};

use ratatui::text::{Line, Span};

use crate::{
    render::terminal::TerminalCapabilities,
    theme::Theme,
    widgets::{
        chat::{ChatMessage, ContentBlock, MessageRole, StreamingState, ToolCallStatus},
        tool_renderer::{format_tool_call, format_tool_result},
    },
};

use super::{Component, Container, LiveRegion, RenderResult, style::styled_line_to_ansi};

struct MessageComponent {
    message: ChatMessage,
    streaming: bool,
    theme: Theme,
    caps: TerminalCapabilities,
    revision: u64,
}

impl MessageComponent {
    fn new(
        message: ChatMessage,
        streaming: bool,
        theme: &Theme,
        caps: &TerminalCapabilities,
    ) -> Self {
        Self {
            revision: message_revision(&message),
            message,
            streaming,
            theme: theme.clone(),
            caps: caps.clone(),
        }
    }

    fn styled_lines(&self, width: u16) -> Vec<Line<'static>> {
        let styles = self.theme.to_styles();
        let mut lines = Vec::new();
        let role_style = match self.message.role {
            MessageRole::User => styles.primary,
            MessageRole::Assistant => styles.normal,
            MessageRole::System => styles.muted,
        };
        for block in &self.message.content_blocks {
            match block {
                ContentBlock::Text { content } => {
                    let mut rendered =
                        crate::widgets::chat::markdown::md_lines(content, width, &styles);
                    for line in &mut rendered {
                        line.style = role_style.patch(line.style);
                    }
                    lines.extend(rendered);
                }
                ContentBlock::Thinking { content, collapsed } => {
                    let marker = if *collapsed {
                        styles.symbols.nav_expand
                    } else {
                        styles.symbols.nav_collapse
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{marker} Thinking"),
                        styles.muted,
                    )));
                    if !collapsed {
                        lines.extend(crate::widgets::chat::markdown::md_lines(
                            content,
                            width.saturating_sub(2),
                            &styles,
                        ));
                    }
                }
                ContentBlock::ToolCall {
                    name,
                    arguments,
                    result,
                    status,
                    duration,
                    ..
                } => {
                    lines.extend(format_tool_call(name, arguments, width as usize, &styles));
                    if matches!(status, ToolCallStatus::Executing) {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "  {} running{}",
                                styles.symbols.status_running,
                                duration
                                    .as_deref()
                                    .map_or(String::new(), |d| format!(" · {d}"))
                            ),
                            styles.muted,
                        )));
                    }
                    if let Some((output, is_error)) = result {
                        lines.extend(format_tool_result(
                            name,
                            output,
                            *is_error,
                            Some(arguments),
                            width as usize,
                            &styles,
                        ));
                    }
                }
                ContentBlock::ToolResult {
                    tool_name,
                    content,
                    is_error,
                } => {
                    lines.extend(format_tool_result(
                        tool_name,
                        content,
                        *is_error,
                        None,
                        width as usize,
                        &styles,
                    ));
                }
                ContentBlock::Error {
                    title,
                    message,
                    retryable,
                } => {
                    let suffix = if *retryable { " (retryable)" } else { "" };
                    lines.push(Line::from(Span::styled(
                        format!("{} {title}{suffix}", styles.symbols.status_error),
                        styles.error,
                    )));
                    lines.extend(
                        message
                            .lines()
                            .map(|line| Line::from(Span::styled(line.to_string(), styles.error))),
                    );
                }
                ContentBlock::Image {
                    mime_type,
                    base64_data,
                } => {
                    // Tape rows are ANSI-terminated and width-measured as text. Raw
                    // Kitty/iTerm2 escapes here would be counted as cells and their
                    // protocol terminators would collide with LINE_TERMINATOR.
                    // Keep this safe placeholder; the post-paint image writer and
                    // overlay viewer remain the raw-output paths.
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{} image {mime_type} ({} bytes)",
                            styles.symbols.icon_file,
                            base64_data.len()
                        ),
                        styles.muted,
                    )));
                }
                ContentBlock::Dashboard { info } => {
                    lines.extend(crate::widgets::chat::dashboard::dashboard_lines(
                        info, width, &styles,
                    ));
                }
                ContentBlock::Advisory { body, severity, .. } => {
                    // Severity-colored badge card. `AdvisorSeverity` is mirrored
                    // in `oxicode_tui::widgets::chat::types` to keep `oxicode-tui` free
                    // of `oxicode-agent` deps; severity mapping matches the plan:
                    // Nit -> muted, Concern -> warning, Blocker -> error.
                    let (label, badge_style) = match severity {
                        crate::widgets::chat::types::AdvisorSeverity::Nit => ("NIT", styles.muted),
                        crate::widgets::chat::types::AdvisorSeverity::Concern => {
                            ("CONCERN", styles.warning)
                        }
                        crate::widgets::chat::types::AdvisorSeverity::Blocker => {
                            ("BLOCKER", styles.error)
                        }
                    };
                    let prefix = format!("[{}] ", label);
                    let body_style = match severity {
                        crate::widgets::chat::types::AdvisorSeverity::Nit => styles.muted,
                        crate::widgets::chat::types::AdvisorSeverity::Concern => styles.warning,
                        crate::widgets::chat::types::AdvisorSeverity::Blocker => styles.error,
                    };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, badge_style),
                        Span::styled(body.clone(), body_style),
                    ]));
                }
            }
        }
        lines
    }
}

impl Component for MessageComponent {
    fn render(&self, width: u16) -> RenderResult {
        RenderResult::new(
            self.styled_lines(width)
                .iter()
                .map(|line| styled_line_to_ansi(line, &self.caps))
                .collect(),
        )
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn live_region(&self) -> LiveRegion {
        if self.streaming {
            LiveRegion::Mutable { start: 0 }
        } else {
            LiveRegion::None
        }
    }
}

fn message_revision(message: &ChatMessage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{message:?}").hash(&mut hasher);
    hasher.finish()
}

/// Memoized projection of finalized and streaming chat messages.
pub struct TranscriptRenderer {
    container: Container,
    fingerprint: u64,
}

impl TranscriptRenderer {
    /// Create an empty transcript renderer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            container: Container::new(),
            fingerprint: 0,
        }
    }

    /// Synchronize the projection with canonical chat state.
    pub fn sync(
        &mut self,
        messages: &[ChatMessage],
        streaming: Option<&StreamingState>,
        theme: &Theme,
        caps: &TerminalCapabilities,
    ) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{theme:?}").hash(&mut hasher);
        format!("{caps:?}").hash(&mut hasher);
        for message in messages {
            message_revision(message).hash(&mut hasher);
        }
        streaming
            .map(|s| message_revision(&s.message))
            .hash(&mut hasher);
        let fingerprint = hasher.finish();
        if fingerprint == self.fingerprint {
            return;
        }

        self.container.clear();
        for message in messages {
            self.container.add(Box::new(MessageComponent::new(
                message.clone(),
                false,
                theme,
                caps,
            )));
        }
        if let Some(streaming) = streaming {
            self.container.add(Box::new(MessageComponent::new(
                streaming.message.clone(),
                true,
                theme,
                caps,
            )));
        }
        self.fingerprint = fingerprint;
    }

    /// Compose width-aware ANSI rows and live boundary.
    pub fn compose(&mut self, width: u16) -> (&RenderResult, LiveRegion) {
        self.container.compose(width)
    }
}

impl Default for TranscriptRenderer {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn msg(block: ContentBlock) -> ChatMessage {
        ChatMessage {
            role: MessageRole::Assistant,
            content_blocks: vec![block],
            timestamp: 0,
        }
    }

    #[test]
    fn text_wraps_to_width_and_stream_is_live() {
        let mut renderer = TranscriptRenderer::new();
        let streaming = StreamingState {
            message: msg(ContentBlock::Text {
                content: "one two three four".into(),
            }),
        };
        renderer.sync(
            &[],
            Some(&streaming),
            &Theme::dark(),
            &TerminalCapabilities::default(),
        );
        let (result, live) = renderer.compose(8);
        assert!(result.lines.len() > 1);
        assert_eq!(live, LiveRegion::Mutable { start: 0 });
    }

    #[test]
    fn every_block_variant_renders() {
        let blocks = vec![
            ContentBlock::Thinking {
                content: "reason".into(),
                collapsed: false,
            },
            ContentBlock::ToolCall {
                id: "1".into(),
                name: "read".into(),
                arguments: "{\"path\":\"a\"}".into(),
                result: Some(("ok".into(), false)),
                status: ToolCallStatus::Done,
                duration: Some("1ms".into()),
            },
            ContentBlock::ToolResult {
                tool_name: "read".into(),
                content: "ok".into(),
                is_error: false,
            },
            ContentBlock::Error {
                title: "bad".into(),
                message: "detail".into(),
                retryable: true,
            },
            ContentBlock::Image {
                mime_type: "image/png".into(),
                base64_data: "YWJj".into(),
            },
            ContentBlock::Advisory {
                body: "consider revisiting tests".into(),
                severity: crate::widgets::chat::types::AdvisorSeverity::Concern,
                timestamp_ms: 1_700_000_000_000,
            },
        ];
        let messages: Vec<_> = blocks.into_iter().map(msg).collect();
        let mut renderer = TranscriptRenderer::new();
        renderer.sync(
            &messages,
            None,
            &Theme::dark(),
            &TerminalCapabilities::default(),
        );
        let (result, live) = renderer.compose(80);
        let text = result.lines.join("\n");
        assert_eq!(live, LiveRegion::None);
        for needle in ["Thinking", "read", "bad", "image/png", "[CONCERN]"] {
            assert!(text.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn transcript_renders_mermaid_fence_as_diagram() {
        let mut renderer = TranscriptRenderer::new();
        renderer.sync(
            &[msg(ContentBlock::Text {
                content: "```mermaid\ngraph TD\n  A --> B\n```".into(),
            })],
            None,
            &Theme::dark(),
            &TerminalCapabilities::default(),
        );
        let (result, _) = renderer.compose(80);
        let text = result.lines.join("\n");
        assert!(text.contains("A"));
        assert!(text.contains("B"));
        assert!(!text.contains("```mermaid"));
    }

    #[test]
    fn transcript_renders_inline_and_display_latex() {
        let mut renderer = TranscriptRenderer::new();
        renderer.sync(
            &[msg(ContentBlock::Text {
                content: "inline $\\alpha^2$\n\n$$\\sum_{i=0}^n$$".into(),
            })],
            None,
            &Theme::dark(),
            &TerminalCapabilities::default(),
        );
        let (result, _) = renderer.compose(80);
        let text = result.lines.join("\n");
        assert!(text.contains("α²"), "rendered: {text}");
        assert!(text.contains("∑ᵢ₌₀ⁿ"), "rendered: {text}");
        assert!(!text.contains("\\alpha"));
    }

    #[test]
    fn transcript_image_is_safe_placeholder_even_when_protocol_is_available() {
        let mut renderer = TranscriptRenderer::new();
        let caps = TerminalCapabilities {
            image_protocol: Some(crate::render::terminal::ImageProtocol::Kitty),
            ..TerminalCapabilities::default()
        };
        renderer.sync(
            &[msg(ContentBlock::Image {
                mime_type: "image/png".into(),
                base64_data: "YWJj".into(),
            })],
            None,
            &Theme::dark(),
            &caps,
        );
        let (result, _) = renderer.compose(80);
        let text = result.lines.join("\n");
        assert!(text.contains("image/png"));
        // The placeholder text IS styled (has SGR ANSI codes from
        // `styled_line_to_ansi`), which is safe. What we must not see
        // is raw Kitty protocol escapes that would corrupt the tape
        // row structure. SGR sequences like `\x1b[38;5;240m` are
        // harmless and expected.
        assert!(
            !text.contains("\x1b_G"),
            "must not contain raw Kitty escapes in tape rows"
        );
        assert!(
            !text.contains("\x1b\\"),
            "must not contain raw image terminators"
        );
    }
}
