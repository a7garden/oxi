//! Footer widget — 2-line status bar: model/tokens/duration + path/git.
//!
//! Uses only ASCII-safe characters for broad terminal compatibility.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, LineGauge, Paragraph, StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthStr;
use crate::Theme;

/// Footer data — shared state for token counts and session info.
#[derive(Debug, Clone)]
pub struct FooterData {
    pub model_name: String,
    pub thinking_level: Option<String>, // e.g., Some("high"), None means "off"
    pub provider_name: String,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub pwd: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub context_window_pct: f32,
    pub context_window_max: u32,
    pub context_tokens: u32,
    pub total_cost: f64,
    pub session_duration_secs: u64,
    pub is_busy: bool,
    pub version: String,
}

impl Default for FooterData {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            thinking_level: None,
            provider_name: String::new(),
            git_branch: None,
            git_dirty: false,
            pwd: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            context_window_pct: 0.0,
            context_window_max: 200_000,
            context_tokens: 0,
            total_cost: 0.0,
            session_duration_secs: 0,
            is_busy: false,
            version: String::new(),
        }
    }
}

impl FooterData {
    pub fn fmt_count(count: u32) -> String {
        if count < 1000 { count.to_string() }
        else if count < 1_000_000 { format!("{:.1}k", count as f32 / 1000.0) }
        else { format!("{:.1}M", count as f32 / 1_000_000.0) }
    }

    pub fn format_duration(secs: u64) -> String {
        if secs < 60 { format!("{}s", secs) }
        else if secs < 3600 { format!("{}m", secs / 60) }
        else { format!("{}h{}m", secs / 3600, (secs % 3600) / 60) }
    }
}

#[derive(Debug, Default)]
pub struct FooterState {
    pub data: FooterData,
}

pub struct Footer<'a> {
    theme: &'a Theme,
}

impl<'a> Footer<'a> {
    pub fn new(theme: &'a Theme) -> Self { Self { theme } }
}

impl StatefulWidget for Footer<'_> {
    type State = FooterState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height < 2 || area.width < 4 { return; }

        let styles = self.theme.to_styles();
        let d = &state.data;

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // separator
                Constraint::Length(1), // line 1
                Constraint::Length(1), // line 2
            ])
            .split(area);

        // Separator
        Block::default()
            .borders(Borders::TOP)
            .border_style(styles.border)
            .render(rows[0], buf);

        // ── Row 1: tokens + gauge ... model ──
        {
            let total_tokens = d.input_tokens + d.output_tokens + d.cache_read_tokens + d.cache_write_tokens;
            let pct = if d.context_window_max > 0 {
                (total_tokens as f64 / d.context_window_max as f64).min(1.0)
            } else { 0.0 };
            let pct_display = (pct * 100.0) as f32;
            let has_tokens = total_tokens > 0;

            let mut left_parts: Vec<String> = Vec::new();
            if has_tokens {
                let max = FooterData::fmt_count(d.context_window_max);
                left_parts.push(format!("{:.1}% / {}", pct_display, max));
            }
            if d.session_duration_secs > 0 {
                left_parts.push(FooterData::format_duration(d.session_duration_secs));
            }
            let left_text = left_parts.join("  ");

            // Build model display string with optional thinking level
            // Format: "(provider) model • thinking" or "(provider) model" or "model • thinking" or just "model"
            let model_display = if d.model_name.is_empty() {
                "[no model]".to_string()
            } else {
                let model_part = d.model_name.split('/').last().unwrap_or(&d.model_name);
                let provider_part = d.model_name.split('/').next().unwrap_or("");
                let thinking_part = d.thinking_level.as_ref().map(|l| format!(" • {}", l));

                if provider_part.is_empty() {
                    format!("{}{}", model_part, thinking_part.unwrap_or_default())
                } else {
                    format!("({}) {}{}", provider_part, model_part, thinking_part.unwrap_or_default())
                }
            };

            let indicator_color = if d.is_busy {
                self.theme.colors.accent.to_ratatui()
            } else {
                self.theme.colors.success.to_ratatui()
            };

            // Build right_span with indicator + model name
            let mut spans = vec![
                Span::styled("*", Style::default().fg(indicator_color)),
            ];

            if d.model_name.is_empty() {
                spans.push(Span::styled(" [no model]", Style::default()
                    .fg(self.theme.colors.primary.to_ratatui())
                    .add_modifier(Modifier::BOLD)));
            } else {
                let thinking_style = if d.thinking_level.is_some() {
                    // Style thinking level as muted (secondary info)
                    Style::default().fg(self.theme.colors.muted.to_ratatui())
                } else {
                    Style::default()
                        .fg(self.theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD)
                };

                let model_part = d.model_name.split('/').last().unwrap_or(&d.model_name);
                let provider_part = d.model_name.split('/').next().unwrap_or("");

                // Provider
                if !provider_part.is_empty() {
                    spans.push(Span::styled(format!(" ({})", provider_part),
                        Style::default()
                            .fg(self.theme.colors.primary.to_ratatui())
                            .add_modifier(Modifier::BOLD)));
                }
                // Model name
                spans.push(Span::styled(format!(" {}", model_part),
                    Style::default()
                        .fg(self.theme.colors.primary.to_ratatui())
                        .add_modifier(Modifier::BOLD)));
                // Thinking level
                if let Some(ref level) = d.thinking_level {
                    spans.push(Span::styled(format!(" • {}", level), thinking_style));
                }
            }

            let right_span = Line::from(spans);

            let text_w = UnicodeWidthStr::width(left_text.as_str()) as u16 + 2;
            let model_display_w = UnicodeWidthStr::width(model_display.as_str()) as u16 + 4;
            let gauge_w = rows[1].width.saturating_sub(text_w).saturating_sub(model_display_w);

            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(text_w.min(rows[1].width)),
                    Constraint::Length(gauge_w),
                    Constraint::Min(model_display_w),
                ])
                .split(rows[1]);

            Paragraph::new(Line::from(Span::styled(format!(" {}", left_text), styles.muted)))
                .alignment(Alignment::Left)
                .render(cols[0], buf);

            if has_tokens && gauge_w > 4 {
                let label = format!("{:.0}%", pct_display);
                let gauge_color = if pct < 0.7 {
                    self.theme.colors.success.to_ratatui()
                } else if pct < 0.9 {
                    self.theme.colors.warning.to_ratatui()
                } else {
                    self.theme.colors.error.to_ratatui()
                };
                LineGauge::default()
                    .ratio(pct)
                    .label(label)
                    .style(Style::default().fg(gauge_color).bg(self.theme.colors.background.to_ratatui()))
                    .render(cols[1], buf);
            }

            Paragraph::new(right_span)
                .alignment(Alignment::Right)
                .render(cols[2], buf);
        }

        // ── Row 2: path + git ... version ──
        {
            let mut left_spans: Vec<Span> = Vec::new();

            let home = dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let pwd_display = if let Some(ref pwd) = d.pwd {
                if !home.is_empty() && pwd.starts_with(&home) {
                    format!(" ~{}", &pwd[home.len()..])
                } else {
                    format!(" {}", pwd)
                }
            } else { String::new() };
            left_spans.push(Span::styled(pwd_display, styles.muted));

            if let Some(ref branch) = d.git_branch {
                if !branch.is_empty() {
                    let dirty_marker = if d.git_dirty { "*" } else { "" };
                    left_spans.push(Span::styled(
                        format!(" ({}){}", branch, dirty_marker),
                        Style::default().fg(self.theme.colors.accent.to_ratatui()),
                    ));
                    let (status_char, status_style) = if d.git_dirty {
                        (" X", Style::default().fg(self.theme.colors.error.to_ratatui()))
                    } else {
                        (" ok", Style::default().fg(self.theme.colors.success.to_ratatui()))
                    };
                    left_spans.push(Span::styled(status_char, status_style));
                }
            }

            let version_tag = if !d.version.is_empty() {
                format!(" v{} ", d.version)
            } else { String::new() };

            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Min(1)])
                .split(rows[2]);

            Paragraph::new(Line::from(left_spans))
                .alignment(Alignment::Left)
                .render(cols[0], buf);

            Paragraph::new(Line::from(Span::styled(version_tag, Style::default().fg(self.theme.colors.muted.to_ratatui()))))
                .alignment(Alignment::Right)
                .render(cols[1], buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_data_default() {
        let data = FooterData::default();
        assert!(data.model_name.is_empty());
        assert_eq!(data.input_tokens, 0);
    }

    #[test]
    fn footer_data_format_duration() {
        assert_eq!(FooterData::format_duration(30), "30s");
        assert_eq!(FooterData::format_duration(90), "1m");
        assert_eq!(FooterData::format_duration(3661), "1h1m");
    }

    #[test]
    fn footer_data_fmt_count() {
        assert_eq!(FooterData::fmt_count(500), "500");
        assert_eq!(FooterData::fmt_count(1500), "1.5k");
        assert_eq!(FooterData::fmt_count(1_500_000), "1.5M");
    }
}
