//! Footer widget — 2-line status bar: model/tokens/duration + path/git.
//!
//! Uses only ASCII-safe characters for broad terminal compatibility.

use crate::Theme;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Sparkline, StatefulWidget, Widget},
};

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
    pub is_compacting: bool,
    /// Optional right-aligned status string (e.g., issue counts from oxicode-cli).
    pub extra_right: String,
    /// Sparkline data: recent token output rate history (tokens/sec, max 60 samples).
    /// Updated by the app layer during streaming.
    pub token_rate_history: Vec<u64>,
    pub version: String,
}

impl Default for FooterData {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            thinking_level: None,
            provider_name: String::new(),
            git_branch: Option::None,
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
            is_compacting: false,
            extra_right: String::new(),
            token_rate_history: Vec::new(),
            version: String::new(),
        }
    }
}

impl FooterData {
    /// Add a token-rate sample, keeping at most 60 entries.
    pub fn push_token_rate(&mut self, rate: u64) {
        self.token_rate_history.push(rate);
        if self.token_rate_history.len() > 60 {
            self.token_rate_history.remove(0);
        }
    }

    pub fn fmt_count(count: u32) -> String {
        if count < 1000 {
            count.to_string()
        } else if count < 1_000_000 {
            format!("{:.1}k", count as f32 / 1000.0)
        } else {
            format!("{:.1}M", count as f32 / 1_000_000.0)
        }
    }

    pub fn format_duration(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// Shorten a path for display: ~ replacement, then take last component if still long.
    fn display_path(pwd: &str) -> String {
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let shortened = if !home.is_empty() && pwd.starts_with(&home) {
            format!("~{}", &pwd[home.len()..])
        } else {
            pwd.to_string()
        };
        // If still long, just show the last directory name
        if shortened.len() > 20 {
            std::path::Path::new(&shortened)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(shortened)
        } else {
            shortened
        }
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
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl StatefulWidget for Footer<'_> {
    type State = FooterState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height < 2 || area.width < 4 {
            return;
        }
        // Paint the footer area with surface_bg for a distinct status bar.
        buf.set_style(
            area,
            ratatui::style::Style::default().bg(self.theme.colors.surface_bg),
        );

        let styles = self.theme.to_styles();
        let d = &state.data;

        let [sep_row, row1, row2] = area.layout(&Layout::vertical([
            Constraint::Length(1), // separator
            Constraint::Length(1), // line 1: tokens + duration
            Constraint::Length(1), // line 2: path + git ... model
        ]));

        // Separator — match the input status line (accent when busy, muted when idle)
        let separator_style = if d.is_busy {
            styles.accent
        } else {
            styles.muted
        };
        Block::default()
            .borders(Borders::TOP)
            .border_style(separator_style)
            .render(sep_row, buf);

        // ── Row 1: tokens + duration ──
        {
            let total_tokens =
                d.input_tokens + d.output_tokens + d.cache_read_tokens + d.cache_write_tokens;
            let pct = if d.context_window_max > 0 {
                (total_tokens as f64 / d.context_window_max as f64).min(1.0)
            } else {
                0.0
            };
            let pct_display = (pct * 100.0) as f32;
            let has_tokens = total_tokens > 0;
            let token_style = if has_tokens {
                styles.normal
            } else {
                styles.muted
            };

            let in_fmt = FooterData::fmt_count(d.input_tokens + d.cache_read_tokens);
            let out_fmt = FooterData::fmt_count(d.output_tokens);

            let mut left_spans: Vec<Span<'_>> = vec![
                Span::styled(
                    format!(
                        " {}{} {}{}",
                        styles.symbols.arrow_up, in_fmt, styles.symbols.arrow_down, out_fmt
                    ),
                    token_style,
                ),
                Span::styled(
                    format!(
                        "  {:.1}%/{}",
                        pct_display,
                        FooterData::fmt_count(d.context_window_max)
                    ),
                    token_style,
                ),
            ];

            if d.is_compacting {
                left_spans.push(Span::styled(
                    "  Compacting...",
                    Style::default()
                        .fg(self.theme.colors.warning)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            if d.session_duration_secs > 0 {
                left_spans.push(Span::styled(
                    format!("  {}", FooterData::format_duration(d.session_duration_secs)),
                    styles.muted,
                ));
            }

            // Show sparkline if we have enough samples during streaming
            // Optional right-aligned extra (e.g., issue counts injected by oxicode-cli).
            if !d.extra_right.is_empty() {
                left_spans.push(Span::styled(
                    format!("  {} {}", styles.symbols.sep_pipe.trim(), d.extra_right),
                    Style::default().fg(self.theme.colors.accent),
                ));
            }

            let show_sparkline = has_tokens && d.token_rate_history.len() >= 3;

            if show_sparkline {
                let [info_area, spark_area] = row1.layout(&Layout::horizontal([
                    Constraint::Min(30), // token text (minimum 30 chars)
                    Constraint::Min(10), // sparkline (remaining space)
                ]));

                Paragraph::new(Line::from(left_spans))
                    .alignment(Alignment::Left)
                    .render(info_area, buf);

                let sparkline_style = if pct > 0.8 {
                    self.theme.colors.warning
                } else {
                    self.theme.colors.primary
                };

                Sparkline::default()
                    .data(&d.token_rate_history)
                    .style(sparkline_style)
                    .render(spark_area, buf);
            } else {
                Paragraph::new(Line::from(left_spans))
                    .alignment(Alignment::Left)
                    .render(row1, buf);
            }
        }

        // ── Row 2: path + git ... model + thinking ──
        {
            let mut left_spans: Vec<Span> = Vec::new();

            if let Some(ref pwd) = d.pwd {
                let display = FooterData::display_path(pwd);
                left_spans.push(Span::styled(format!(" {}", display), styles.muted));
            }

            if let Some(ref branch) = d.git_branch
                && !branch.is_empty()
            {
                left_spans.push(Span::styled(
                    format!(" ({})", branch),
                    Style::default().fg(self.theme.colors.accent),
                ));
                if d.git_dirty {
                    left_spans.push(Span::styled(
                        " *",
                        Style::default().fg(self.theme.colors.warning),
                    ));
                }
            }

            // Right: model + thinking (moved from row 1 to row 2, where version was)
            let mut right_spans: Vec<Span<'_>> = vec![];

            if d.model_name.is_empty() {
                right_spans.push(Span::styled(
                    "[no model]".to_string(),
                    Style::default()
                        .fg(self.theme.colors.primary)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                let model_bold = Style::default()
                    .fg(self.theme.colors.primary)
                    .add_modifier(Modifier::BOLD);
                let thinking_style = Style::default().fg(self.theme.colors.muted);

                let model_part = d.model_name.split('/').next_back().unwrap_or(&d.model_name);
                let provider_part = d.model_name.split('/').next().unwrap_or("");

                if !provider_part.is_empty() {
                    right_spans.push(Span::styled(format!("({}) ", provider_part), model_bold));
                }
                right_spans.push(Span::styled(model_part.to_string(), model_bold));
                if let Some(ref level) = d.thinking_level {
                    right_spans.push(Span::styled(
                        format!("  {}", styles.symbols.thinking_level_icon(level)),
                        thinking_style,
                    ));
                }
            }

            let [left_col, right_col] = row2.layout(&Layout::horizontal([
                Constraint::Min(1),
                Constraint::Min(1),
            ]));

            Paragraph::new(Line::from(left_spans))
                .alignment(Alignment::Left)
                .render(left_col, buf);

            Paragraph::new(Line::from(right_spans))
                .alignment(Alignment::Right)
                .render(right_col, buf);
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

    #[test]
    fn display_path_short() {
        let path = format!("{}/projects/myapp", dirs::home_dir().unwrap().display());
        assert!(FooterData::display_path(&path).contains("~/projects/myapp"));
    }

    #[test]
    fn display_path_long_truncates_to_basename() {
        let path = "/Volumes/MERCURY/PROJECTS/oxicode";
        let displayed = FooterData::display_path(path);
        assert_eq!(displayed, "oxicode");
    }

    #[test]
    fn display_path_home_short() {
        let home = dirs::home_dir().unwrap();
        let path = format!("{}/src", home.display());
        let displayed = FooterData::display_path(&path);
        assert_eq!(displayed, "~/src");
    }
}
