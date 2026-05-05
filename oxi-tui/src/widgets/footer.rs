//! Footer widget — status bar with model info, tokens, cost, git branch.

use ratatui::{
    widgets::Widget,
    buffer::Buffer,
    layout::Rect,
    style::{Style, Modifier},
};
use crate::Theme;

/// Footer data — shared state for token counts and session info.
#[derive(Debug, Clone)]
pub struct FooterData {
    pub model_name: String,
    pub provider_name: String,
    pub git_branch: Option<String>,
    pub pwd: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub context_window_pct: f32,
    pub total_cost: f64,
    pub session_duration_secs: u64,
}

impl Default for FooterData {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            provider_name: String::new(),
            git_branch: None,
            pwd: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            context_window_pct: 0.0,
            total_cost: 0.0,
            session_duration_secs: 0,
        }
    }
}

impl FooterData {
    pub fn format_tokens(input: u32, output: u32, cache_read: u32, cache_write: u32) -> String {
        let mut parts = Vec::new();
        if input > 0 {
            parts.push(format!("↑{}", Self::fmt_count(input)));
        }
        if output > 0 {
            parts.push(format!("↓{}", Self::fmt_count(output)));
        }
        if cache_read > 0 {
            parts.push(format!("R{}", Self::fmt_count(cache_read)));
        }
        if cache_write > 0 {
            parts.push(format!("W{}", Self::fmt_count(cache_write)));
        }
        parts.join(" ")
    }

    fn fmt_count(count: u32) -> String {
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
}

/// Footer state — wraps shared FooterData with rendering position.
#[derive(Debug, Default)]
pub struct FooterState {
    pub data: FooterData,
}

/// Footer widget.
pub struct Footer<'a> {
    theme: &'a Theme,
}

impl<'a> Footer<'a> {
    pub fn new() -> Self {
        Self { theme: &crate::Theme::dark() }
    }

    pub fn with_theme(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl Default for Footer<'static> {
    fn default() -> Self {
        Self { theme: &Theme::dark() }
    }
}

impl Widget for Footer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 {
            return;
        }

        let styles = self.theme.to_styles();
        let dim = styles.muted;

        // Build left and right sections
        let mut left_parts: Vec<String> = Vec::new();
        let mut right_parts: Vec<String> = Vec::new();

        // Left: model, tokens, cost
        if !self.theme.colors.name.is_empty() {
            // Actually use data from state if available - for now build from data
            // Note: this widget doesn't hold state, so we use theme-based defaults
            // The actual data comes from the call site
        }

        // Write content row by row
        let y = area.y;
        let max_w = area.width as usize;

        // Left section
        let left_text = "".to_string();
        let right_text = "".to_string();

        for (col, c) in left_text.chars().enumerate() {
            if col < max_w {
                buf.get_mut(area.x + col as u16, y)
                    .set_char(c)
                    .set_style(dim);
            }
        }

        for (col, c) in right_text.chars().enumerate() {
            let col_from_right = area.width as usize - 1 - col;
            if col_from_right < max_w {
                buf.get_mut(col_from_right as u16, y)
                    .set_char(c)
                    .set_style(dim);
            }
        }

        // Clear remainder
        let used = left_text.chars().count();
        for col in used..max_w {
            buf.get_mut(area.x + col as u16, y)
                .set_char(' ')
                .set_style(dim);
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
    fn footer_data_format_tokens() {
        assert_eq!(FooterData::format_tokens(0, 0, 0, 0), "");
        assert_eq!(FooterData::format_tokens(1500, 0, 0, 0), "↑1.5k");
        assert_eq!(FooterData::format_tokens(0, 2500, 0, 0), "↓2.5k");
    }

    #[test]
    fn footer_data_format_duration() {
        assert_eq!(FooterData::format_duration(30), "30s");
        assert_eq!(FooterData::format_duration(90), "1m");
        assert_eq!(FooterData::format_duration(3661), "1h1m");
    }
}