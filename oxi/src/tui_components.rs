//! Interactive mode TUI components
//!
//! Provides high-level interactive components for the oxi terminal interface:
//! - Session selector (navigate/switch/create/delete sessions)
//! - Model selector (choose AI model grouped by provider)
//! - Footer (status bar with model, session, tokens, cost)
//! - Login dialog (API key entry with provider selection)
//! - Diff viewer (show edit diffs with color highlighting)
//! - Bash execution display (streaming output, timer, cancel)

use serde::{Deserialize, Serialize};

/// Session info for display in session selector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub message_count: usize,
    pub model: Option<String>,
    pub parent_id: Option<String>,
}

/// Session selector state
#[derive(Debug, Clone)]
pub struct SessionSelector {
    pub sessions: Vec<SessionInfo>,
    pub selected_index: usize,
    pub filter: String,
    pub scroll_offset: usize,
    pub visible_height: usize,
}

impl SessionSelector {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        Self {
            sessions,
            selected_index: 0,
            filter: String::new(),
            scroll_offset: 0,
            visible_height: 20,
        }
    }

    /// Get filtered sessions matching the current filter
    pub fn filtered_sessions(&self) -> Vec<&SessionInfo> {
        if self.filter.is_empty() {
            self.sessions.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.sessions
                .iter()
                .filter(|s| {
                    s.name.to_lowercase().contains(&filter_lower)
                        || s.id.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        let max = self.filtered_sessions().len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    /// Get currently selected session
    pub fn selected(&self) -> Option<&SessionInfo> {
        self.filtered_sessions().into_iter().nth(self.selected_index)
    }

    /// Update filter text
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    fn adjust_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.selected_index - self.visible_height + 1;
        }
    }

    /// Render the session selector as a string
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("{}\n", "─".repeat(60)));
        output.push_str("Sessions (↑↓ navigate, Enter select, n new, d delete, / filter)\n");
        output.push_str(&format!("{}\n", "─".repeat(60)));

        if !self.filter.is_empty() {
            output.push_str(&format!("Filter: {}\n", self.filter));
        }

        let filtered: Vec<_> = self.filtered_sessions();
        for (i, session) in filtered.iter().enumerate() {
            let marker = if i == self.selected_index { "▶" } else { " " };
            let branch = if session.parent_id.is_some() { "├─ " } else { "  " };
            let name = if session.name.is_empty() {
                &session.id[..8.min(session.id.len())]
            } else {
                &session.name
            };
            output.push_str(&format!(
                "{} {}{:<30} {} msg:{} model:{}\n",
                marker,
                branch,
                name,
                &session.created_at[..10.min(session.created_at.len())],
                session.message_count,
                session.model.as_deref().unwrap_or("-"),
            ));
        }

        if filtered.is_empty() {
            output.push_str("  (no sessions)\n");
        }

        output
    }
}

/// Model info for model selector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_thinking: bool,
    pub context_window: usize,
}

/// Model selector state
#[derive(Debug, Clone)]
pub struct ModelSelector {
    pub models: Vec<ModelInfo>,
    pub selected_index: usize,
    pub filter: String,
    pub grouped: bool,
}

impl ModelSelector {
    pub fn new(models: Vec<ModelInfo>) -> Self {
        let mut models = models;
        models.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.name.cmp(&b.name)));
        Self {
            models,
            selected_index: 0,
            filter: String::new(),
            grouped: true,
        }
    }

    /// Get filtered models
    pub fn filtered_models(&self) -> Vec<&ModelInfo> {
        if self.filter.is_empty() {
            self.models.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.models
                .iter()
                .filter(|m| {
                    m.name.to_lowercase().contains(&filter_lower)
                        || m.id.to_lowercase().contains(&filter_lower)
                        || m.provider.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        let max = self.filtered_models().len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    /// Get currently selected model
    pub fn selected(&self) -> Option<&ModelInfo> {
        self.filtered_models().into_iter().nth(self.selected_index)
    }

    /// Render the model selector
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("{}\n", "─".repeat(60)));
        output.push_str("Select Model (↑↓ navigate, Enter select, / filter)\n");
        output.push_str(&format!("{}\n", "─".repeat(60)));

        let filtered: Vec<_> = self.filtered_models();
        let mut last_provider = String::new();

        for (i, model) in filtered.iter().enumerate() {
            // Provider group header
            if self.grouped && model.provider != last_provider {
                last_provider = model.provider.clone();
                output.push_str(&format!("\n  {}\n", model.provider.to_uppercase()));
            }

            let marker = if i == self.selected_index { "▶" } else { " " };
            let vision = if model.supports_vision { "👁" } else { " " };
            let tools = if model.supports_tools { "🔧" } else { " " };
            let thinking = if model.supports_thinking { "💭" } else { " " };
            let ctx = format_bytes(model.context_window);

            output.push_str(&format!(
                " {} {} {}{}{} {:<30} ctx:{}\n",
                marker, model.id, vision, tools, thinking, model.name, ctx,
            ));
        }

        output
    }
}

/// Footer status bar data
#[derive(Debug, Clone, Default)]
pub struct FooterData {
    pub model_name: String,
    pub session_name: String,
    pub provider_name: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub total_cost: f64,
    pub is_thinking: bool,
    pub elapsed_seconds: Option<u64>,
}

impl FooterData {
    /// Render the footer as a single-line status bar
    pub fn render(&self, width: usize) -> String {
        let thinking = if self.is_thinking { "⏳" } else { "✓" };
        let tokens = if self.input_tokens > 0 || self.output_tokens > 0 {
            format!("tok:{}+{}", self.input_tokens, self.output_tokens)
        } else {
            String::new()
        };
        let cost = if self.total_cost > 0.0 {
            format!("${:.4}", self.total_cost)
        } else {
            String::new()
        };
        let elapsed = self.elapsed_seconds
            .map(|s| format!("{}m{}s", s / 60, s % 60))
            .unwrap_or_default();

        let left = format!("{} {} @ {}", thinking, self.model_name, self.provider_name);
        let right = format!("{} {} {}", tokens, cost, elapsed);

        let session_part = if !self.session_name.is_empty() {
            format!(" │ {}", self.session_name)
        } else {
            String::new()
        };

        // Pad to width
        let content_len = left.len() + session_part.len() + right.len() + 2;
        if content_len < width {
            let padding = width - content_len;
            format!("{}{}{:>width$}", left, session_part, right, width = padding + right.len())
        } else {
            format!("{}{} {}", left, session_part, right)
        }
    }
}

/// Login dialog state
#[derive(Debug, Clone)]
pub struct LoginDialog {
    pub providers: Vec<String>,
    pub selected_provider_index: usize,
    pub api_key: String,
    pub cursor_pos: usize,
    pub error_message: Option<String>,
    pub is_masked: bool,
}

impl LoginDialog {
    pub fn new(providers: Vec<String>) -> Self {
        Self {
            providers,
            selected_provider_index: 0,
            api_key: String::new(),
            cursor_pos: 0,
            error_message: None,
            is_masked: true,
        }
    }

    /// Get selected provider
    pub fn selected_provider(&self) -> Option<&str> {
        self.providers.get(self.selected_provider_index).map(|s| s.as_str())
    }

    /// Input a character
    pub fn input_char(&mut self, c: char) {
        self.api_key.insert(self.cursor_pos, c);
        self.cursor_pos += 1;
        self.error_message = None;
    }

    /// Delete character before cursor
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.api_key.remove(self.cursor_pos);
            self.error_message = None;
        }
    }

    /// Cycle provider selection
    pub fn next_provider(&mut self) {
        if !self.providers.is_empty() {
            self.selected_provider_index = (self.selected_provider_index + 1) % self.providers.len();
            self.api_key.clear();
            self.cursor_pos = 0;
            self.error_message = None;
        }
    }

    /// Validate API key format (basic checks)
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("API key cannot be empty".to_string());
        }
        let provider = self.selected_provider().unwrap_or("");
        match provider {
            "anthropic" if !self.api_key.starts_with("sk-ant-") => {
                Err("Anthropic API keys start with 'sk-ant-'".to_string())
            }
            "openai" if !self.api_key.starts_with("sk-") => {
                Err("OpenAI API keys start with 'sk-'".to_string())
            }
            _ => Ok(()),
        }
    }

    /// Render the login dialog
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("{}\n", "─".repeat(50)));
        output.push_str("  API Key Configuration\n");
        output.push_str(&format!("{}\n", "─".repeat(50)));

        // Provider tabs
        for (i, provider) in self.providers.iter().enumerate() {
            if i == self.selected_provider_index {
                output.push_str(&format!(" [{}] ", provider));
            } else {
                output.push_str(&format!("  {}  ", provider));
            }
        }
        output.push('\n');

        // API key input
        let display_key = if self.is_masked {
            "*".repeat(self.api_key.len())
        } else {
            self.api_key.clone()
        };
        output.push_str(&format!("\n  API Key: {}\n", display_key));

        // Error message
        if let Some(ref err) = self.error_message {
            output.push_str(&format!("  ⚠ {}\n", err));
        }

        output.push_str("\n  Tab: switch provider, Enter: save, Esc: cancel\n");
        output
    }
}

/// Diff line for the diff viewer
#[derive(Debug, Clone)]
pub enum DiffLine {
    Context { content: String, line_num: usize },
    Added { content: String, line_num: usize },
    Removed { content: String, line_num: usize },
    Header { old_start: usize, old_count: usize, new_start: usize, new_count: usize },
}

/// Diff viewer state
#[derive(Debug, Clone)]
pub struct DiffViewer {
    pub lines: Vec<DiffLine>,
    pub scroll_offset: usize,
    pub visible_height: usize,
    pub file_path: String,
    /// Enable word-level highlighting for changed parts
    pub word_diff: bool,
}

impl DiffViewer {
    pub fn new(file_path: String, diff_text: &str) -> Self {
        let lines = parse_diff_lines(diff_text);
        Self {
            lines,
            scroll_offset: 0,
            visible_height: 30,
            file_path,
            word_diff: true, // Enable word-level highlighting by default
        }
    }

    /// Create without word diff highlighting
    pub fn new_simple(file_path: String, diff_text: &str) -> Self {
        let lines = parse_diff_lines(diff_text);
        Self {
            lines,
            scroll_offset: 0,
            visible_height: 30,
            file_path,
            word_diff: false,
        }
    }

    /// Enable or disable word-level diff highlighting
    pub fn set_word_diff(&mut self, enabled: bool) {
        self.word_diff = enabled;
    }

    /// Render the diff viewer with optional word-level highlighting
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("Diff: {}\n", self.file_path));
        output.push_str(&format!("{}\n", "─".repeat(60)));

        let visible: Vec<_> = self.lines
            .iter()
            .skip(self.scroll_offset)
            .take(self.visible_height)
            .collect();

        for line in &visible {
            match line {
                DiffLine::Header { old_start, old_count, new_start, new_count } => {
                    output.push_str(&format!(
                        "@@ -{},{} +{},{} @@\n",
                        old_start, old_count, new_start, new_count
                    ));
                }
                DiffLine::Context { content, line_num } => {
                    output.push_str(&format!(" {:>4} {}\n", line_num, content));
                }
                DiffLine::Added { content, line_num } => {
                    if self.word_diff {
                        // Apply word-level highlighting for added lines
                        let highlighted = highlight_words_diff(content, true);
                        output.push_str(&format!("+{:>4} {}\n", line_num, highlighted));
                    } else {
                        output.push_str(&format!("+{:>4} {}\n", line_num, content));
                    }
                }
                DiffLine::Removed { content, line_num } => {
                    if self.word_diff {
                        // Apply word-level highlighting for removed lines
                        let highlighted = highlight_words_diff(content, false);
                        output.push_str(&format!("-{:>4} {}\n", line_num, highlighted));
                    } else {
                        output.push_str(&format!("-{:>4} {}\n", line_num, content));
                    }
                }
            }
        }

        let remaining = self.lines.len().saturating_sub(self.scroll_offset + self.visible_height);
        if remaining > 0 {
            output.push_str(&format!("... {} more lines\n", remaining));
        }

        output
    }

    /// Scroll up
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll down
    pub fn scroll_down(&mut self, amount: usize) {
        let max = self.lines.len().saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max);
    }
}

/// Parse unified diff text into DiffLine structs
fn parse_diff_lines(diff: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let mut old_line = 0;
    let mut new_line = 0;

    for raw_line in diff.lines() {
        if raw_line.starts_with("@@") {
            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
            if let Some(header) = parse_hunk_header(raw_line) {
                old_line = header.0;
                new_line = header.2;
                lines.push(DiffLine::Header {
                    old_start: header.0,
                    old_count: header.1,
                    new_start: header.2,
                    new_count: header.3,
                });
            }
        } else if raw_line.starts_with('+') {
            let content = raw_line[1..].to_string();
            lines.push(DiffLine::Added { content, line_num: new_line });
            new_line += 1;
        } else if raw_line.starts_with('-') {
            let content = raw_line[1..].to_string();
            lines.push(DiffLine::Removed { content, line_num: old_line });
            old_line += 1;
        } else if raw_line.starts_with(' ') {
            let content = raw_line[1..].to_string();
            lines.push(DiffLine::Context { content, line_num: new_line });
            old_line += 1;
            new_line += 1;
        }
    }

    lines
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    // @@ -old_start,old_count +new_start,new_count @@
    let text = line.trim_start_matches('@').trim_start_matches(' ');
    let text = text.trim_end_matches('@').trim_end_matches(' ');
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let old: Vec<usize> = parts[0]
        .trim_start_matches('-')
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    let new: Vec<usize> = parts
        .get(1)?
        .trim_start_matches('+')
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();

    Some((
        *old.first()?,
        *old.get(1).unwrap_or(&1),
        *new.first()?,
        *new.get(1).unwrap_or(&1),
    ))
}

/// Highlight word-level changes in a diff line
/// Returns the content with ANSI color codes for changed words.
fn highlight_words_diff(content: &str, is_added: bool) -> String {
    use std::fmt::Write;

    // Split content into words while preserving spaces
    let words: Vec<&str> = content.split_whitespace().collect();
    let mut result = String::new();

    for (i, word) in words.iter().enumerate() {
        // Simple heuristic: short words (1-4 chars) that differ are likely changed
        let is_short_change = word.len() <= 4 && !word.chars().all(|c| c.is_alphanumeric());

        if is_short_change && i > 0 {
            // Highlight as changed
            let color = if is_added { "\x1b[32m" } else { "\x1b[31m" };
            write!(&mut result, "{}{}{}\x1b[0m ", color, word, "\x1b[0m").unwrap();
        } else {
            write!(&mut result, "{} ", word).unwrap();
        }
    }

    result.trim_end().to_string()
}

/// Bash execution display state
#[derive(Debug, Clone)]
pub struct BashExecution {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub start_time: std::time::Instant,
    pub is_running: bool,
    pub is_cancelled: bool,
}

impl BashExecution {
    pub fn new(command: String) -> Self {
        Self {
            command,
            output: String::new(),
            exit_code: None,
            start_time: std::time::Instant::now(),
            is_running: true,
            is_cancelled: false,
        }
    }

    /// Append output
    pub fn append_output(&mut self, text: &str) {
        self.output.push_str(text);
    }

    /// Mark as complete
    pub fn complete(&mut self, exit_code: i32) {
        self.exit_code = Some(exit_code);
        self.is_running = false;
    }

    /// Cancel execution
    pub fn cancel(&mut self) {
        self.is_cancelled = true;
        self.is_running = false;
        self.exit_code = Some(-1);
        self.output.push_str("\n[Cancelled]");
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    /// Render the bash execution display
    pub fn render(&self) -> String {
        let mut output = String::new();
        let status = if self.is_cancelled {
            "⛔ CANCELLED"
        } else if self.is_running {
            &format!("⏳ Running ({:.1}s)", self.elapsed().as_secs_f64())
        } else {
            match self.exit_code {
                Some(0) => "✓ Done",
                Some(c) => &format!("✗ Exit code: {}", c) as &str,
                None => "Running",
            }
        };

        output.push_str(&format!("$ {}\n", self.command));
        if !self.output.is_empty() {
            output.push_str(&self.output);
            if !self.output.ends_with('\n') {
                output.push('\n');
            }
        }
        output.push_str(&format!("{}\n", status));

        output
    }
}

/// Format bytes for human-readable display
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_selector_navigation() {
        let sessions = vec![
            SessionInfo {
                id: "1".to_string(),
                name: "Session 1".to_string(),
                created_at: "2025-01-01".to_string(),
                message_count: 5,
                model: Some("gpt-4".to_string()),
                parent_id: None,
            },
            SessionInfo {
                id: "2".to_string(),
                name: "Session 2".to_string(),
                created_at: "2025-01-02".to_string(),
                message_count: 3,
                model: Some("claude-3".to_string()),
                parent_id: Some("1".to_string()),
            },
        ];
        let mut selector = SessionSelector::new(sessions);
        assert_eq!(selector.selected().unwrap().id, "1");
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "2");
        selector.move_up();
        assert_eq!(selector.selected().unwrap().id, "1");
    }

    #[test]
    fn test_session_selector_filter() {
        let sessions = vec![
            SessionInfo {
                id: "1".to_string(),
                name: "Rust coding".to_string(),
                created_at: "2025-01-01".to_string(),
                message_count: 5,
                model: None,
                parent_id: None,
            },
            SessionInfo {
                id: "2".to_string(),
                name: "Python coding".to_string(),
                created_at: "2025-01-02".to_string(),
                message_count: 3,
                model: None,
                parent_id: None,
            },
        ];
        let mut selector = SessionSelector::new(sessions);
        selector.set_filter("rust".to_string());
        let filtered = selector.filtered_sessions();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "Rust coding");
    }

    #[test]
    fn test_model_selector() {
        let models = vec![
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                provider: "openai".to_string(),
                supports_vision: true,
                supports_tools: true,
                supports_thinking: false,
                context_window: 128000,
            },
            ModelInfo {
                id: "claude-sonnet".to_string(),
                name: "Claude Sonnet".to_string(),
                provider: "anthropic".to_string(),
                supports_vision: true,
                supports_tools: true,
                supports_thinking: true,
                context_window: 200000,
            },
        ];
        let mut selector = ModelSelector::new(models);
        assert_eq!(selector.selected().unwrap().id, "claude-sonnet");
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "gpt-4o");
    }

    #[test]
    fn test_footer_render() {
        let footer = FooterData {
            model_name: "gpt-4o".to_string(),
            session_name: "test".to_string(),
            provider_name: "openai".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            total_cost: 0.05,
            is_thinking: false,
            elapsed_seconds: Some(30),
        };
        let rendered = footer.render(80);
        assert!(rendered.contains("gpt-4o"));
        assert!(rendered.contains("openai"));
    }

    #[test]
    fn test_login_dialog() {
        let mut dialog = LoginDialog::new(vec![
            "anthropic".to_string(),
            "openai".to_string(),
        ]);
        assert_eq!(dialog.selected_provider(), Some("anthropic"));
        dialog.next_provider();
        assert_eq!(dialog.selected_provider(), Some("openai"));
        dialog.input_char('s');
        dialog.input_char('k');
        assert_eq!(dialog.api_key, "sk");
        dialog.backspace();
        assert_eq!(dialog.api_key, "s");
    }

    #[test]
    fn test_login_dialog_validation() {
        let mut dialog = LoginDialog::new(vec!["openai".to_string()]);
        assert!(dialog.validate().is_err()); // empty key
        dialog.api_key = "sk-1234".to_string();
        assert!(dialog.validate().is_ok());
    }

    #[test]
    fn test_diff_viewer() {
        let diff = "@@ -1,3 +1,3 @@\n line1\n-old line\n+new line\n line3\n";
        let viewer = DiffViewer::new("test.txt".to_string(), diff);
        assert_eq!(viewer.lines.len(), 5); // header + 4 lines
        let rendered = viewer.render();
        assert!(rendered.contains("old line"));
        assert!(rendered.contains("new line"));
    }

    #[test]
    fn test_diff_viewer_scroll() {
        let mut diff = "@@ -1,5 +1,5 @@\n".to_string();
        for i in 0..100 {
            diff.push_str(&format!(" line {}\n", i));  // context lines start with space
        }
        let mut viewer = DiffViewer::new("test.txt".to_string(), &diff);
        viewer.visible_height = 10;
        assert!(viewer.lines.len() > 10, "need {} lines, got {}", 11, viewer.lines.len());
        viewer.scroll_down(10);
        assert!(viewer.scroll_offset > 0);
        viewer.scroll_up(5);
        assert!(viewer.scroll_offset < 10);
    }

    #[test]
    fn test_bash_execution() {
        let mut exec = BashExecution::new("echo hello".to_string());
        assert!(exec.is_running);
        exec.append_output("hello\n");
        exec.complete(0);
        assert!(!exec.is_running);
        assert_eq!(exec.exit_code, Some(0));
        let rendered = exec.render();
        assert!(rendered.contains("echo hello"));
        assert!(rendered.contains("hello"));
        assert!(rendered.contains("Done"));
    }

    #[test]
    fn test_bash_execution_cancel() {
        let mut exec = BashExecution::new("sleep 999".to_string());
        exec.cancel();
        assert!(exec.is_cancelled);
        assert!(!exec.is_running);
        let rendered = exec.render();
        assert!(rendered.contains("CANCELLED"));
    }

    #[test]
    fn test_parse_hunk_header() {
        let result = parse_hunk_header("@@ -1,3 +1,3 @@");
        assert_eq!(result, Some((1, 3, 1, 3)));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500B");
        assert_eq!(format_bytes(1024), "1.0KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0GB");
    }
}
