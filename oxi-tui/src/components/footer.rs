//! Footer component for TUI status display.
//!
//! Displays model info, token usage, cost, git branch, context usage,
//! thinking level, and session information in a compact status bar.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::{Cell, Color, Component, Event, Rect, Size, Surface, Theme};
use crate::utils::visible_width;

/// Footer data containing all status information for the TUI footer display.
#[derive(Debug, Clone)]
pub struct FooterData {
    /// Current model name (e.g., "claude-sonnet-4-20250501")
    pub model_name: String,
    /// Provider name (e.g., "anthropic", "openai")
    pub provider_name: String,
    /// Thinking level setting (e.g., "off", "low", "medium", "high")
    pub thinking_level: String,
    /// Current session name (if set)
    pub session_name: Option<String>,
    /// Current git branch (from git_utils)
    pub git_branch: Option<String>,
    /// Current working directory (abbreviated)
    pub pwd: Option<String>,
    /// Input tokens used in current session
    pub input_tokens: Arc<AtomicU32>,
    /// Output tokens used in current session
    pub output_tokens: Arc<AtomicU32>,
    /// Cache read tokens (from model caching)
    pub cache_read_tokens: Arc<AtomicU32>,
    /// Cache write tokens (from model caching)
    pub cache_write_tokens: Arc<AtomicU32>,
    /// Context window usage percentage (0.0 - 100.0)
    pub context_window_pct: f32,
    /// Total estimated cost in USD
    pub total_cost: f64,
    /// Session duration in seconds
    pub session_duration_secs: u64,
    /// Extension status messages (key -> text)
    pub extension_statuses: HashMap<String, String>,
}

impl Default for FooterData {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            provider_name: String::new(),
            thinking_level: "off".to_string(),
            session_name: None,
            git_branch: None,
            pwd: None,
            input_tokens: Arc::new(AtomicU32::new(0)),
            output_tokens: Arc::new(AtomicU32::new(0)),
            cache_read_tokens: Arc::new(AtomicU32::new(0)),
            cache_write_tokens: Arc::new(AtomicU32::new(0)),
            context_window_pct: 0.0,
            total_cost: 0.0,
            session_duration_secs: 0,
            extension_statuses: HashMap::new(),
        }
    }
}

impl FooterData {
    /// Create a new empty FooterData
    pub fn new() -> Self {
        Self::default()
    }

    /// Get current input tokens
    pub fn get_input_tokens(&self) -> u32 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    /// Get current output tokens
    pub fn get_output_tokens(&self) -> u32 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    /// Get cache read tokens
    pub fn get_cache_read_tokens(&self) -> u32 {
        self.cache_read_tokens.load(Ordering::Relaxed)
    }

    /// Get cache write tokens
    pub fn get_cache_write_tokens(&self) -> u32 {
        self.cache_write_tokens.load(Ordering::Relaxed)
    }

    /// Update token counts during streaming
    pub fn update_tokens(&self, input: u32, output: u32) {
        self.input_tokens.store(input, Ordering::Relaxed);
        self.output_tokens.store(output, Ordering::Relaxed);
    }

    /// Update cache tokens during streaming
    pub fn update_cache_tokens(&self, read: u32, write: u32) {
        self.cache_read_tokens.store(read, Ordering::Relaxed);
        self.cache_write_tokens.store(write, Ordering::Relaxed);
    }

    /// Update all token counts at once
    pub fn update_all_tokens(&self, input: u32, output: u32, cache_read: u32, cache_write: u32) {
        self.update_tokens(input, output);
        self.update_cache_tokens(cache_read, cache_write);
    }

    /// Set context window percentage
    pub fn set_context_window_pct(&mut self, pct: f32) {
        self.context_window_pct = pct.clamp(0.0, 100.0);
    }

    /// Set total cost
    pub fn set_total_cost(&mut self, cost: f64) {
        self.total_cost = cost;
    }

    /// Set session duration in seconds
    pub fn set_session_duration(&mut self, secs: u64) {
        self.session_duration_secs = secs;
    }

    /// Add an extension status message
    pub fn set_extension_status(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.extension_statuses.insert(key.to_string(), v.to_string());
        } else {
            self.extension_statuses.remove(key);
        }
    }

    /// Create a footer component from this data.
    pub fn to_footer(&self) -> Footer {
        Footer::new(self.clone())
    }

    /// Create a footer component with theme.
    pub fn to_footer_with_theme(&self, theme: &Theme) -> Footer {
        Footer::with_theme(self.clone(), theme)
    }
}

/// Theme colors for footer component.
#[derive(Debug, Clone)]
pub struct FooterTheme {
    /// Normal text color.
    pub normal: Color,
    /// Dimmed text color.
    pub dim: Color,
    /// Primary accent color.
    pub primary: Color,
    /// Success/positive color (green).
    pub success: Color,
    /// Warning color (yellow).
    pub warning: Color,
    /// Error/danger color (red).
    pub error: Color,
    /// Separator character.
    pub separator: char,
}

impl Default for FooterTheme {
    fn default() -> Self {
        Self {
            normal: Color::Default,
            dim: Color::Indexed(245),
            primary: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            separator: '|',
        }
    }
}

impl FooterTheme {
    /// Create theme from global theme.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            normal: theme.colors.foreground,
            dim: theme.colors.muted,
            primary: theme.colors.primary,
            success: theme.colors.success,
            warning: theme.colors.warning,
            error: theme.colors.error,
            separator: '|',
        }
    }
}

/// Footer component for displaying status information.
pub struct Footer {
    /// Footer data to display.
    data: FooterData,
    /// Current width of the footer.
    width: usize,
    /// Theme colors for the footer.
    theme: FooterTheme,
    /// Whether to show extension statuses.
    show_extension_statuses: bool,
    /// Minimum width before truncation.
    min_width: usize,
    /// Dirty flag for render requests.
    dirty: bool,
}

impl Footer {
    /// Create a new footer with the given data.
    pub fn new(data: FooterData) -> Self {
        Self {
            data,
            width: 80,
            theme: FooterTheme::default(),
            show_extension_statuses: true,
            min_width: 40,
            dirty: true,
        }
    }

    /// Create with theme.
    pub fn with_theme(data: FooterData, theme: &Theme) -> Self {
        Self {
            data,
            width: 80,
            theme: FooterTheme::from_theme(theme),
            show_extension_statuses: true,
            min_width: 40,
            dirty: true,
        }
    }

    /// Update the footer data.
    pub fn set_data(&mut self, data: FooterData) {
        self.data = data;
        self.dirty = true;
    }

    /// Get a reference to the footer data.
    pub fn data(&self) -> &FooterData {
        &self.data
    }

    /// Get a mutable reference to the footer data.
    pub fn data_mut(&mut self) -> &mut FooterData {
        &mut self.data
    }

    /// Set the width.
    pub fn set_width(&mut self, width: usize) {
        self.width = width;
    }

    /// Set whether to show extension statuses.
    pub fn set_show_extension_statuses(&mut self, show: bool) {
        self.show_extension_statuses = show;
        self.dirty = true;
    }

    /// Get the number of rows this footer requires.
    pub fn height(&self) -> usize {
        let mut rows = 1; // Main footer row

        if self.show_extension_statuses && !self.data.extension_statuses.is_empty() {
            // Extension statuses may wrap to multiple lines
            let status_line = self.render_extension_status_line();
            let line_width = visible_width(&status_line);
            if line_width > 0 {
                let wrapped_lines = (line_width + self.width.saturating_sub(1)) / self.width;
                rows += wrapped_lines;
            }
        }

        rows
    }

    /// Render the extension statuses line.
    fn render_extension_status_line(&self) -> String {
        let mut parts: Vec<String> = self
            .data
            .extension_statuses
            .values()
            .cloned()
            .collect();
        parts.sort();
        parts.join(" ")
    }

    /// Format tokens for display.
    fn format_tokens(input: u32, output: u32, cache_read: u32, cache_write: u32) -> String {
        let mut parts = Vec::new();

        if input > 0 {
            parts.push(format!("↑{}", Self::format_token_count(input)));
        }
        if output > 0 {
            parts.push(format!("↓{}", Self::format_token_count(output)));
        }
        if cache_read > 0 {
            parts.push(format!("R{}", Self::format_token_count(cache_read)));
        }
        if cache_write > 0 {
            parts.push(format!("W{}", Self::format_token_count(cache_write)));
        }

        parts.join(" ")
    }

    /// Format a token count for display.
    fn format_token_count(count: u32) -> String {
        if count < 1000 {
            count.to_string()
        } else if count < 10000 {
            format!("{:.1}k", count as f32 / 1000.0)
        } else if count < 1_000_000 {
            format!("{}k", count / 1000)
        } else {
            format!("{:.1}M", count as f32 / 1_000_000.0)
        }
    }

    /// Get the color for context window percentage.
    #[allow(dead_code)]
    fn context_color(pct: f32) -> Color {
        if pct > 90.0 {
            Color::Red
        } else if pct > 70.0 {
            Color::Yellow
        } else {
            Color::Green
        }
    }

    /// Truncate text to fit within width.
    fn truncate_to_width(text: &str, max_width: usize) -> String {
        let mut result = String::new();
        let mut width = 0;

        for c in text.chars() {
            // Check if character is likely fullwidth (CJK characters, emojis, etc.)
            let char_width = if unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) == 2 { 2 } else { 1 };
            if width + char_width > max_width {
                break;
            }
            result.push(c);
            width += char_width;
        }

        if width < max_width && result.len() < text.len() {
            // Add ellipsis
            if width >= 3 {
                result.truncate(result.len().saturating_sub(3));
                result.push_str("...");
            }
        }

        result
    }

    /// Render the main footer line.
    fn render_main_line(&self) -> String {
        let sep = format!(" {} ", self.theme.separator);

        // Left section: model/provider | tokens | cost
        let mut left_parts = Vec::new();

        // Model info
        if !self.data.model_name.is_empty() {
            if !self.data.provider_name.is_empty() {
                left_parts.push(format!("({}) {}", self.data.provider_name, self.data.model_name));
            } else {
                left_parts.push(self.data.model_name.clone());
            }
        }

        // Tokens
        let tokens = Self::format_tokens(
            self.data.get_input_tokens(),
            self.data.get_output_tokens(),
            self.data.get_cache_read_tokens(),
            self.data.get_cache_write_tokens(),
        );
        if !tokens.is_empty() {
            left_parts.push(tokens);
        }

        // Cost
        if self.data.total_cost > 0.0 {
            left_parts.push(format!("${:.3}", self.data.total_cost));
        }

        // Right section: git branch | context % | thinking | session
        let mut right_parts = Vec::new();

        if let Some(ref branch) = self.data.git_branch {
            if !branch.is_empty() {
                right_parts.push(format!("@{}", branch));
            }
        }

        // Context window with color coding
        if self.data.context_window_pct > 0.0 {
            let ctx_str = format!("{:.1}%", self.data.context_window_pct);
            right_parts.push(ctx_str);
        }

        // Thinking level
        if !self.data.thinking_level.is_empty() && self.data.thinking_level != "off" {
            right_parts.push(format!("thinking:{}", self.data.thinking_level));
        }

        // Session name
        if let Some(ref session) = self.data.session_name {
            if !session.is_empty() {
                right_parts.push(session.clone());
            }
        }

        // Session duration
        if self.data.session_duration_secs > 0 {
            right_parts.push(Self::format_duration(self.data.session_duration_secs));
        }

        // Build the line
        let left_str = left_parts.join(" ");
        let right_str = right_parts.join(" ");

        if left_str.is_empty() && right_str.is_empty() {
            return String::new();
        }

        if left_str.is_empty() {
            return Self::truncate_to_width(&right_str, self.width);
        }

        if right_str.is_empty() {
            return Self::truncate_to_width(&left_str, self.width);
        }

        let separator_str = sep.clone();
        let combined = format!("{}{}{}", left_str, separator_str, right_str);

        if visible_width(&combined) <= self.width {
            combined
        } else {
            // Need to truncate
            let left_width = visible_width(&left_str);
            let sep_width = visible_width(&separator_str);
            let min_right = 10;

            if left_width + sep_width + min_right > self.width {
                // Truncate left side
                let available = self.width.saturating_sub(sep_width + min_right);
                format!("{}{}{}",
                    Self::truncate_to_width(&left_str, available),
                    separator_str,
                    Self::truncate_to_width(&right_str, min_right)
                )
            } else {
                // Truncate right side
                let available = self.width.saturating_sub(left_width + sep_width);
                format!("{}{}{}",
                    left_str,
                    separator_str,
                    Self::truncate_to_width(&right_str, available)
                )
            }
        }
    }

    /// Format duration in human-readable format.
    fn format_duration(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

impl Component for Footer {
    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, _event: &Event) -> bool {
        // Footer is not interactive
        false
    }

    fn render(&mut self, surface: &mut Surface, rect: Rect) {
        self.width = rect.width as usize;

        let dim = self.theme.dim;
        let mut row = rect.y;

        // Main footer line
        let main_line = self.render_main_line();
        let main_width = visible_width(&main_line);

        // Draw the main line
        for (col, c) in main_line.chars().enumerate() {
            if col >= rect.width as usize {
                break;
            }
            let color = dim;

            surface.set(
                row,
                col as u16,
                Cell::new(c).with_fg(color),
            );
        }

        // Fill remaining width with dim spaces
        for col in main_width..rect.width as usize {
            surface.set(row, col as u16, Cell::new(' ').with_fg(dim));
        }

        row += 1;

        // Extension statuses line (if any)
        if self.show_extension_statuses && !self.data.extension_statuses.is_empty() {
            let status_line = self.render_extension_status_line();
            let status_width = visible_width(&status_line);

            for (col, c) in status_line.chars().enumerate() {
                if col >= rect.width as usize {
                    break;
                }
                surface.set(
                    row,
                    col as u16,
                    Cell::new(c).with_fg(dim),
                );
            }

            // Fill remaining
            for col in status_width..rect.width as usize {
                surface.set(row, col as u16, Cell::new(' ').with_fg(dim));
            }
        }

        self.dirty = false;
    }

    fn min_size(&self) -> Size {
        Size {
            width: self.min_width as u16,
            height: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn create_test_footer_data() -> FooterData {
        FooterData {
            model_name: "claude-sonnet-4".to_string(),
            provider_name: "anthropic".to_string(),
            thinking_level: "medium".to_string(),
            session_name: Some("test-session".to_string()),
            git_branch: Some("main".to_string()),
            pwd: Some("~/projects".to_string()),
            input_tokens: Arc::new(AtomicU32::new(1500)),
            output_tokens: Arc::new(AtomicU32::new(2500)),
            cache_read_tokens: Arc::new(AtomicU32::new(500)),
            cache_write_tokens: Arc::new(AtomicU32::new(100)),
            context_window_pct: 65.0,
            total_cost: 0.025,
            session_duration_secs: 125,
            extension_statuses: HashMap::new(),
        }
    }

    #[test]
    fn test_footer_new() {
        let data = FooterData::new();
        let footer = Footer::new(data);
        assert!(footer.data.model_name.is_empty());
    }

    #[test]
    fn test_footer_with_theme() {
        let data = create_test_footer_data();
        let theme = Theme::dark();
        let footer = Footer::with_theme(data, &theme);
        // Dark theme muted color is Indexed(8)
        assert_eq!(footer.theme.dim, Color::Indexed(8));
    }

    #[test]
    fn test_footer_data_mutation() {
        let mut data = create_test_footer_data();
        let mut footer = Footer::new(data.clone());

        data.set_total_cost(0.050);
        footer.set_data(data);

        assert_eq!(footer.data().total_cost, 0.050);
    }

    #[test]
    fn test_footer_data_mut() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);
        footer.data_mut().set_total_cost(0.100);
        assert_eq!(footer.data().total_cost, 0.100);
    }

    #[test]
    fn test_footer_set_width() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);
        footer.set_width(120);
        assert_eq!(footer.width, 120);
    }

    #[test]
    fn test_footer_show_extension_statuses() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);

        footer.set_show_extension_statuses(false);
        assert!(!footer.show_extension_statuses);

        footer.set_show_extension_statuses(true);
        assert!(footer.show_extension_statuses);
    }

    #[test]
    fn test_footer_height_basic() {
        let data = create_test_footer_data();
        let footer = Footer::new(data);
        assert_eq!(footer.height(), 1);
    }

    #[test]
    fn test_footer_height_with_extensions() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);

        let mut ext_statuses = HashMap::new();
        ext_statuses.insert("ext1".to_string(), "Working...".to_string());
        ext_statuses.insert("ext2".to_string(), "Done".to_string());

        footer.data_mut().extension_statuses = ext_statuses;
        footer.set_show_extension_statuses(true);

        // Height should be 2 when extension statuses are shown
        assert_eq!(footer.height(), 2);
    }

    #[test]
    fn test_footer_render_main_line() {
        let data = create_test_footer_data();
        let footer = Footer::new(data);

        let line = footer.render_main_line();

        // Should contain model info
        assert!(line.contains("claude-sonnet-4") || line.contains("anthropic"));
    }

    #[test]
    fn test_footer_render_main_line_context_color() {
        let mut data = create_test_footer_data();
        data.context_window_pct = 85.0;
        let footer = Footer::new(data);

        // Should render without panic
        let line = footer.render_main_line();
        assert!(!line.is_empty());
    }

    #[test]
    fn test_footer_render_main_line_high_context() {
        let mut data = create_test_footer_data();
        data.context_window_pct = 95.0;
        let footer = Footer::new(data);

        let line = footer.render_main_line();
        assert!(!line.is_empty());
    }

    #[test]
    fn test_footer_render_main_line_empty_data() {
        let data = FooterData::new();
        let footer = Footer::new(data);

        let line = footer.render_main_line();
        // Empty data should produce empty line
        assert!(line.is_empty());
    }

    #[test]
    fn test_footer_truncate_to_width() {
        let _data = create_test_footer_data();
        let _footer = Footer::new(FooterData::new());

        let truncated = Footer::truncate_to_width("hello world", 5);
        // Should truncate or fit
        assert!(truncated.len() <= 8); // "hello" or "he..."
    }

    #[test]
    fn test_footer_format_duration() {
        // 30 seconds
        assert_eq!(Footer::format_duration(30), "30s");
        // 90 seconds = 1m 30s (shows just "1m")
        assert_eq!(Footer::format_duration(90), "1m");
        // 3661 seconds = 1h 1m
        assert_eq!(Footer::format_duration(3661), "1h1m");
    }

    #[test]
    fn test_footer_context_color() {
        // Low usage - green
        assert_eq!(Footer::context_color(50.0), Color::Green);
        // Medium usage - yellow
        assert_eq!(Footer::context_color(75.0), Color::Yellow);
        // High usage - red
        assert_eq!(Footer::context_color(95.0), Color::Red);
    }

    #[test]
    fn test_footer_format_tokens() {
        let tokens = Footer::format_tokens(1500, 2500, 500, 100);
        assert!(tokens.contains("↑1.5k"));
        assert!(tokens.contains("↓2.5k"));
        assert!(tokens.contains("R"));
        assert!(tokens.contains("W"));
    }

    #[test]
    fn test_footer_format_token_count() {
        assert_eq!(Footer::format_token_count(500), "500");
        assert_eq!(Footer::format_token_count(1500), "1.5k");
        assert_eq!(Footer::format_token_count(10000), "10k");
        assert_eq!(Footer::format_token_count(1500000), "1.5M");
    }

    #[test]
    fn test_footer_to_footer() {
        let data = create_test_footer_data();
        let footer = data.to_footer();
        assert_eq!(footer.data().model_name, "claude-sonnet-4");
    }

    #[test]
    fn test_footer_to_footer_with_theme() {
        let data = create_test_footer_data();
        let theme = Theme::dark();
        let footer = data.to_footer_with_theme(&theme);
        assert_eq!(footer.data().model_name, "claude-sonnet-4");
    }

    #[test]
    fn test_footer_height_with_wide_extensions() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);

        // Add wide extension statuses
        let mut ext_statuses = HashMap::new();
        for i in 0..5 {
            ext_statuses.insert(format!("ext{}", i), "A".repeat(50));
        }
        footer.data_mut().extension_statuses = ext_statuses;
        footer.set_show_extension_statuses(true);
        footer.set_width(40);

        // Height should be > 1 when extension statuses wrap
        let h = footer.height();
        assert!(h >= 2);
    }

    #[test]
    fn test_footer_render_empty_surface() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);

        let mut surface = Surface::new(80, 1);
        let rect = Rect::new(0, 0, 80, 1);
        footer.render(&mut surface, rect);
    }

    #[test]
    fn test_footer_render_wide() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);

        let mut surface = Surface::new(120, 1);
        let rect = Rect::new(0, 0, 120, 1);
        footer.render(&mut surface, rect);
    }

    #[test]
    fn test_footer_theme_from_theme() {
        let theme = Theme::dark();
        let footer_theme = FooterTheme::from_theme(&theme);
        assert_eq!(footer_theme.separator, '|');
        assert_eq!(footer_theme.dim, Color::Indexed(8)); // muted color in dark theme
    }

    #[test]
    fn test_footer_handle_event() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);
        let event = Event::Key(crate::KeyEvent::new(crate::KeyCode::Char('a')));
        assert!(!footer.handle_event(&event));
    }

    #[test]
    fn test_footer_render_with_all_fields() {
        let mut data = create_test_footer_data();
        data.pwd = Some("/home/user/project".to_string());
        data.git_branch = Some("feature/xyz".to_string());
        data.session_name = Some("my-session".to_string());

        let mut footer = Footer::new(data);

        let mut surface = Surface::new(120, 1);
        let rect = Rect::new(0, 0, 120, 1);
        footer.render(&mut surface, rect);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_footer_min_size() {
        let footer = Footer::new(FooterData::new());
        let min = footer.min_size();
        assert_eq!(min.height, 1);
        assert_eq!(min.width, 40);
    }

    #[test]
    fn test_footer_dirty_flag() {
        let data = create_test_footer_data();
        let mut footer = Footer::new(data);
        
        assert!(footer.is_dirty()); // Initial state is dirty
        
        footer.clear_dirty();
        assert!(!footer.is_dirty());
        
        footer.request_render();
        assert!(footer.is_dirty());
    }
}