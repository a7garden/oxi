//! Footer widget — status bar with model info, tokens, cost, git branch.

use ratatui::{
    widgets::Widget,
    buffer::Buffer,
    layout::Rect,
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
    /// Format token counts as "↑Nk ↓Mk ROk WPk".
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

    /// Format session duration as "Xs", "Xm", or "XhYm".
    pub fn format_duration(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// Build the left-side status string.
    pub fn left_status(&self) -> String {
        let mut parts = Vec::new();

        if !self.model_name.is_empty() {
            if !self.provider_name.is_empty() {
                parts.push(format!("({}) {}", self.provider_name, self.model_name));
            } else {
                parts.push(self.model_name.clone());
            }
        }

        let tokens = Self::format_tokens(
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        );
        if !tokens.is_empty() {
            parts.push(tokens);
        }

        if self.total_cost > 0.0 {
            parts.push(format!("${:.3}", self.total_cost));
        }

        parts.join(" ")
    }

    /// Build the right-side status string.
    pub fn right_status(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref branch) = self.git_branch {
            if !branch.is_empty() {
                parts.push(format!("@{}", branch));
            }
        }

        if self.context_window_pct > 0.0 {
            parts.push(format!("{:.1}%", self.context_window_pct));
        }

        if self.session_duration_secs > 0 {
            parts.push(Self::format_duration(self.session_duration_secs));
        }

        parts.join(" ")
    }
}

/// Footer state — wraps FooterData for stateful rendering.
#[derive(Debug, Default)]
pub struct FooterState {
    pub data: FooterData,
}

/// Footer widget — renders status bar with model, tokens, branch, duration.
pub struct Footer<'a> {
    theme: &'a Theme,
}

impl<'a> Footer<'a> {
    /// Create with the default dark theme (placeholder — use FooterState for real data).
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
        let y = area.y;
        let max_w = area.width as usize;

        // Left section — theme name as placeholder
        let left_text = format!("● {}", self.theme.name);
        let left_len = left_text.chars().count();

        // Right section placeholder (real data comes from FooterState in StatefulWidget version)
        let right_text = "";
        let right_len = right_text.chars().count();

        // Write left text
        for (col, c) in left_text.chars().enumerate() {
            if col < max_w {
                buf.get_mut(area.x + col as u16, y)
                    .set_char(c)
                    .set_style(dim);
            }
        }

        // Write right text right-aligned
        let right_start = max_w.saturating_sub(right_len);
        for (i, c) in right_text.chars().enumerate() {
            let col = right_start + i;
            if col < max_w {
                buf.get_mut(area.x + col as u16, y)
                    .set_char(c)
                    .set_style(dim);
            }
        }

        // Clear remainder
        let used = left_len.max(right_start);
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
        assert_eq!(FooterData::format_tokens(1500, 2500, 500, 100), "↑1.5k ↓2.5k R500 W100");
    }

    #[test]
    fn footer_data_format_duration() {
        assert_eq!(FooterData::format_duration(30), "30s");
        assert_eq!(FooterData::format_duration(90), "1m");
        assert_eq!(FooterData::format_duration(3661), "1h1m");
        assert_eq!(FooterData::format_duration(0), "0s");
    }

    #[test]
    fn footer_data_status_strings() {
        let mut data = FooterData::default();
        data.model_name = "claude-sonnet-4".to_string();
        data.provider_name = "anthropic".to_string();
        data.git_branch = Some("main".to_string());
        data.input_tokens = 1500;
        data.output_tokens = 2500;

        let left = data.left_status();
        assert!(left.contains("anthropic"));
        assert!(left.contains("claude-sonnet-4"));
        assert!(left.contains("↑1.5k"));
        assert!(left.contains("↓2.5k"));

        let right = data.right_status();
        assert!(right.contains("@main"));
    }
}