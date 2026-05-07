//! Footer widget — 싱글 상태바: 모델 · 경로 · 토큰 · 시간

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
    fn fmt_count(count: u32) -> String {
        if count < 1000 {
            count.to_string()
        } else if count < 1_000_000 {
            format!("{:.1}k", count as f32 / 1000.0)
        } else {
            format!("{:.1}M", count as f32 / 1_000_000.0)
        }
    }

    /// Format duration as compact string.
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

/// Footer state — wraps FooterData for stateful rendering.
#[derive(Debug, Default)]
pub struct FooterState {
    /// The footer data.
    pub data: FooterData,
}

/// Footer widget — renders single-line status bar.
pub struct Footer<'a> {
    theme: &'a Theme,
}

impl<'a> Footer<'a> {
    /// Create with a theme reference.
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
        if area.width < 4 {
            return;
        }

        let styles = self.theme.to_styles();
        let y = area.y;
        let max_w = area.width as usize;
        let d = &state.data;

        // ── Build segments ──

        // 1. Status indicator + model
        let indicator_color = if d.is_busy {
            self.theme.colors.accent.to_ratatui()
        } else {
            self.theme.colors.success.to_ratatui()
        };
        let model_short = d.model_name.split('/').last().unwrap_or(&d.model_name);

        // 2. Full path + git branch + dirty
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
        let git_str = match (&d.git_branch, d.git_dirty) {
            (Some(b), true) if !b.is_empty() => format!(" {}* ", b),
            (Some(b), false) if !b.is_empty() => format!(" {} ✓", b),
            _ => String::new(),
        };

        // 3. Token usage: cur/max (pct)
        let token_str = if d.context_tokens > 0 && d.context_window_max > 0 {
            let cur = FooterData::fmt_count(d.context_tokens);
            let max = FooterData::fmt_count(d.context_window_max);
            let pct = d.context_window_pct;
            format!(" {}/{} ({:.1}%)", cur, max, pct)
        } else {
            String::new()
        };

        // 4. Duration
        let dur_str = if d.session_duration_secs > 0 {
            format!(" {}", FooterData::format_duration(d.session_duration_secs))
        } else {
            String::new()
        };

        // ── Render left side: ● model  path (branch) ──
        let mut col: usize = 0;

        // Indicator ●
        if col < max_w {
            buf[(area.x, y)].set_char('●').set_style(
                Style::default().fg(indicator_color).bg(self.theme.colors.background.to_ratatui())
            );
            col += 1;
        }

        // Space
        if col < max_w {
            buf[(area.x + col as u16, y)].set_char(' ').set_style(styles.normal);
            col += 1;
        }

        // Model name (primary color, bold)
        for c in model_short.chars() {
            if col >= max_w { break; }
            buf[(area.x + col as u16, y)].set_char(c).set_style(
                Style::default()
                    .fg(self.theme.colors.primary.to_ratatui())
                    .bg(self.theme.colors.background.to_ratatui())
                    .add_modifier(Modifier::BOLD)
            );
            col += 1;
        }

        // Separator space
        if col < max_w {
            buf[(area.x + col as u16, y)].set_char(' ').set_style(styles.normal);
            col += 1;
        }

        // Path (muted)
        for c in pwd_display.chars() {
            if col >= max_w { break; }
            buf[(area.x + col as u16, y)].set_char(c).set_style(styles.muted);
            col += 1;
        }

        // Git branch (accent)
        for c in git_str.chars() {
            if col >= max_w { break; }
            buf[(area.x + col as u16, y)].set_char(c).set_style(
                Style::default().fg(self.theme.colors.accent.to_ratatui()).bg(self.theme.colors.background.to_ratatui())
            );
            col += 1;
        }

        // ── Render right side: tokens (duration) — right-aligned ──
        let right_content = format!("{}{}", token_str, dur_str);
        let right_len = right_content.chars().count();

        // Fill gap with spaces
        let right_start = max_w.saturating_sub(right_len);
        while col < right_start {
            if col >= max_w { break; }
            buf[(area.x + col as u16, y)].set_char(' ').set_style(styles.normal);
            col += 1;
        }

        // Write right content
        for c in right_content.chars() {
            if col >= max_w { break; }
            buf[(area.x + col as u16, y)].set_char(c).set_style(styles.muted);
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
}
