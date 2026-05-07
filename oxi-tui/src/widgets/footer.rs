//! Footer widget — 2줄 상태바: 모델/토큰/시간 + 경로/git

use ratatui::{
    widgets::{StatefulWidget, Widget},
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use crate::Theme;

/// Footer data — shared state for token counts and session info.
#[derive(Debug, Clone)]
pub struct FooterData {
    /// Active model name (full: "provider/model").
    pub model_name: String,
    /// Provider name.
    pub provider_name: String,
    /// Current git branch.
    pub git_branch: Option<String>,
    /// Git working tree dirty.
    pub git_dirty: bool,
    /// Current working directory.
    pub pwd: Option<String>,
    /// Input token count.
    pub input_tokens: u32,
    /// Output token count.
    pub output_tokens: u32,
    /// Cache read token count.
    pub cache_read_tokens: u32,
    /// Cache write token count.
    pub cache_write_tokens: u32,
    /// Context window usage percentage.
    pub context_window_pct: f32,
    /// Context window max tokens.
    pub context_window_max: u32,
    /// Current context tokens used.
    pub context_tokens: u32,
    /// Total session cost.
    pub total_cost: f64,
    /// Session duration in seconds.
    pub session_duration_secs: u64,
    /// Agent busy (streaming).
    pub is_busy: bool,
}

impl Default for FooterData {
    fn default() -> Self {
        Self {
            model_name: String::new(),
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
        }
    }
}

impl FooterData {
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
}

/// Footer state.
#[derive(Debug, Default)]
pub struct FooterState {
    pub data: FooterData,
}

/// Footer widget — 2줄 상태바.
pub struct Footer<'a> {
    theme: &'a Theme,
}

impl<'a> Footer<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl Widget for Footer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut state = FooterState::default();
        StatefulWidget::render(self, area, buf, &mut state);
    }
}

impl StatefulWidget for Footer<'_> {
    type State = FooterState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height < 2 || area.width < 4 {
            return;
        }

        let styles = self.theme.to_styles();
        let max_w = area.width as usize;
        let d = &state.data;

        // Top separator line
        let row0 = area.y;
        for col in 0..max_w {
            buf[(area.x + col as u16, row0)].set_char('─').set_style(styles.border);
        }

        // Line 1 row starts at y+1
        let row1 = area.y + 1;
        let row2 = area.y + 2;

        // Build left-side content: tokens + duration
        let mut left_parts: Vec<String> = Vec::new();

        // Token display: in/out/cache + context usage
        if d.input_tokens > 0 || d.output_tokens > 0 {
            let mut token_parts: Vec<String> = Vec::new();
            if d.input_tokens > 0 {
                token_parts.push(format!("in:{}", FooterData::fmt_count(d.input_tokens)));
            }
            if d.output_tokens > 0 {
                token_parts.push(format!("out:{}", FooterData::fmt_count(d.output_tokens)));
            }
            if d.cache_read_tokens > 0 {
                token_parts.push(format!("cache:{}", FooterData::fmt_count(d.cache_read_tokens)));
            }
            left_parts.push(token_parts.join("  "));
        }

        // Context window usage
        if d.context_tokens > 0 && d.context_window_max > 0 {
            let max = FooterData::fmt_count(d.context_window_max);
            let pct = d.context_window_pct;
            left_parts.push(format!("{} ({:.1}%)", max, pct));
        }

        // Duration
        if d.session_duration_secs > 0 {
            left_parts.push(FooterData::format_duration(d.session_duration_secs));
        }

        let left_text = left_parts.join("  ");

        // Build right-side content: ● model_name
        let model_short = if d.model_name.is_empty() {
            "[no model]".to_string()
        } else {
            d.model_name.split('/').last().unwrap_or(&d.model_name).to_string()
        };

        // Status indicator color
        let indicator_color = if d.is_busy {
            self.theme.colors.accent.to_ratatui()
        } else {
            self.theme.colors.success.to_ratatui()
        };

        let model_display = format!("● {}", model_short);
        let model_width = model_display.chars().count();

        // Left-aligned content (tokens, duration) — muted
        // Side padding: 1 char on each side
        let side_pad = 1;
        let mut col: usize = side_pad;
        // Leading space
        if col < max_w {
            buf[(area.x + col as u16, row1)].set_char(' ').set_style(styles.muted);
            col += 1;
        }
        for c in left_text.chars() {
            if col >= max_w { break; }
            buf[(area.x + col as u16, row1)].set_char(c).set_style(styles.muted);
            col += 1;
        }

        // Fill gap between left and right
        let right_start = max_w.saturating_sub(model_width + 1); // +1 for trailing space
        while col < right_start {
            if col >= max_w { break; }
            buf[(area.x + col as u16, row1)].set_char(' ').set_style(styles.normal);
            col += 1;
        }

        // Right-aligned: ● model_name (primary, bold)
        for (i, c) in model_display.chars().enumerate() {
            if col >= max_w { break; }
            let style = if i == 0 {
                // ● indicator
                Style::default()
                    .fg(indicator_color)
                    .bg(self.theme.colors.background.to_ratatui())
            } else if i == 1 {
                // space after ●
                Style::default()
                    .fg(self.theme.colors.primary.to_ratatui())
                    .bg(self.theme.colors.background.to_ratatui())
                    .add_modifier(Modifier::BOLD)
            } else {
                // model name chars
                Style::default()
                    .fg(self.theme.colors.primary.to_ratatui())
                    .bg(self.theme.colors.background.to_ratatui())
                    .add_modifier(Modifier::BOLD)
            };
            buf[(area.x + col as u16, row1)].set_char(c).set_style(style);
            col += 1;
        }

        // ═══════════════════════════════════════════════════════
        // Line 2: path (branch)  (left-aligned, muted)
        // ═══════════════════════════════════════════════════════
        col = 0;

        // Indent
        if col < max_w {
            buf[(area.x + col as u16, row2)].set_char(' ').set_style(styles.muted);
            col += 1;
        }

        // Full path
        let home = std::env::var("HOME").unwrap_or_default();
        let pwd_display = if let Some(ref pwd) = d.pwd {
            if !home.is_empty() && pwd.starts_with(&home) {
                format!("~{}", &pwd[home.len()..])
            } else {
                pwd.clone()
            }
        } else {
            String::new()
        };
        for c in pwd_display.chars() {
            if col >= max_w { break; }
            buf[(area.x + col as u16, row2)].set_char(c).set_style(styles.muted);
            col += 1;
        }

        // Git branch (accent)
        if let Some(ref branch) = d.git_branch {
            if !branch.is_empty() {
                let dirty_marker = if d.git_dirty { "*" } else { "" };
                let git_str = format!(" ({}){}", branch, dirty_marker);
                for c in git_str.chars() {
                    if col >= max_w { break; }
                    buf[(area.x + col as u16, row2)].set_char(c).set_style(
                        Style::default()
                            .fg(self.theme.colors.accent.to_ratatui())
                            .bg(self.theme.colors.background.to_ratatui())
                    );
                    col += 1;
                }

                // ✓ or ✗ for clean/dirty
                let status_char = if d.git_dirty { " ✗" } else { " ✓" };
                let status_style = if d.git_dirty {
                    Style::default().fg(self.theme.colors.error.to_ratatui())
                } else {
                    Style::default().fg(self.theme.colors.success.to_ratatui())
                };
                for c in status_char.chars() {
                    if col >= max_w { break; }
                    buf[(area.x + col as u16, row2)].set_char(c).set_style(
                        status_style.bg(self.theme.colors.background.to_ratatui())
                    );
                    col += 1;
                }
            }
        }

        // Clear remainder
        while col < max_w {
            buf[(area.x + col as u16, row2)].set_char(' ').set_style(styles.muted);
            col += 1;
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
