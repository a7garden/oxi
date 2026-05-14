//! Footer data provider for TUI status display
//!
//! Provides utilities for gathering and formatting footer data
//! such as model info, token usage, and keybinding hints.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::git_utils;

/// Footer data containing all status information for the TUI footer display
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

    /// Create with model information
    pub fn with_model(mut self, model_name: &str, provider_name: &str) -> Self {
        self.model_name = model_name.to_string();
        self.provider_name = provider_name.to_string();
        self
    }

    /// Set the thinking level
    pub fn with_thinking_level(mut self, level: &str) -> Self {
        self.thinking_level = level.to_string();
        self
    }

    /// Set the session name
    pub fn with_session_name(mut self, name: Option<String>) -> Self {
        self.session_name = name;
        self
    }

    /// Set git branch from git_utils
    pub fn with_git_branch(mut self, cwd: &PathBuf) -> Self {
        // Note: get_current_branch already returns Option<String>, no .ok() needed
        self.git_branch = git_utils::get_current_branch(cwd);
        self
    }

    /// Set working directory (abbreviated)
    pub fn with_pwd(mut self, pwd: Option<String>) -> Self {
        self.pwd = pwd;
        self
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
            self.extension_statuses
                .insert(key.to_string(), v.to_string());
        } else {
            self.extension_statuses.remove(key);
        }
    }

    /// Clear all extension statuses
    pub fn clear_extension_statuses(&mut self) {
        self.extension_statuses.clear();
    }

    /// Format tokens for display (e.g., "1.2k", "500")
    pub fn format_tokens(&self) -> String {
        let input = self.get_input_tokens();
        let output = self.get_output_tokens();
        let cache_read = self.get_cache_read_tokens();
        let cache_write = self.get_cache_write_tokens();

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

    /// Format a token count for display
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

    /// Format context window percentage for display
    pub fn format_context_window(&self) -> String {
        if self.context_window_pct > 0.0 {
            format!("{:.1}%", self.context_window_pct)
        } else {
            String::from("0%")
        }
    }

    /// Check if any data is present (excluding defaults)
    pub fn has_data(&self) -> bool {
        self.model_name.is_empty() == false
            || self.get_input_tokens() > 0
            || self.get_output_tokens() > 0
            || self.total_cost > 0.0
    }

    /// Get total tokens
    pub fn total_tokens(&self) -> u32 {
        self.get_input_tokens() + self.get_output_tokens()
    }

    /// Render the footer as display strings for the TUI
    /// Returns up to 2 lines: [pwd_line, stats_line]
    pub fn render_lines(&self, _width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Build pwd line
        let mut pwd_parts = Vec::new();
        if let Some(ref pwd) = self.pwd {
            pwd_parts.push(pwd.clone());
        }
        if let Some(ref branch) = self.git_branch {
            pwd_parts.push(format!("({})", branch));
        }
        if let Some(ref session) = self.session_name {
            pwd_parts.push(format!("• {}", session));
        }
        let pwd_line = if pwd_parts.is_empty() {
            String::new()
        } else {
            pwd_parts.join(" ")
        };
        lines.push(pwd_line);

        // Build stats line
        let mut stats_parts = Vec::new();

        // Token stats
        let token_str = self.format_tokens();
        if !token_str.is_empty() {
            stats_parts.push(token_str);
        }

        // Cost
        if self.total_cost > 0.0 {
            stats_parts.push(format!("${:.3}", self.total_cost));
        }

        // Context window
        if self.context_window_pct > 0.0 {
            stats_parts.push(format!("ctx:{}", self.format_context_window()));
        }

        // Model info on the right
        let mut right_parts = Vec::new();
        if !self.model_name.is_empty() {
            if self.provider_name.is_empty() {
                right_parts.push(self.model_name.clone());
            } else {
                right_parts.push(format!("({}) {}", self.provider_name, self.model_name));
            }
        }

        // Thinking level indicator
        if self.thinking_level != "off" && !self.model_name.is_empty() {
            right_parts.push(format!("thinking:{}", self.thinking_level));
        }

        // Combine stats and model info
        let stats_line = if stats_parts.is_empty() && right_parts.is_empty() {
            String::new()
        } else if stats_parts.is_empty() {
            right_parts.join(" ")
        } else if right_parts.is_empty() {
            stats_parts.join(" ")
        } else {
            format!("{}  {}", stats_parts.join(" "), right_parts.join(" "))
        };
        lines.push(stats_line);

        // Extension statuses (if any) on additional lines
        for (key, value) in &self.extension_statuses {
            lines.push(format!("[{}] {}", key, value));
        }

        lines
    }
}

/// Update footer data from agent events
impl FooterData {
    /// Update footer data based on an agent event.
    ///
    /// This method handles common agent events and updates the
    /// corresponding footer data fields accordingly.
    pub fn update_from_event(&mut self, event: &oxi_agent::AgentEvent) {
        use oxi_agent::AgentEvent;

        match event {
            AgentEvent::Start { .. } => {
                // New interaction started
            }
            AgentEvent::Thinking => {
                // Agent is thinking
            }
            AgentEvent::ThinkingDelta { .. } => {
                // Thinking being streamed
            }
            AgentEvent::TextChunk { .. } => {
                // Text being streamed
            }
            AgentEvent::MessageEnd { message } => {
                // Message complete - extract usage if available
                if let oxi_ai::Message::Assistant(a) = message {
                    self.update_tokens(
                        a.usage.input as u32,
                        a.usage.output as u32,
                    );
                }
            }
            AgentEvent::Usage { input_tokens, output_tokens } => {
                self.update_tokens(*input_tokens as u32, *output_tokens as u32);
            }
            AgentEvent::Complete { content: _, stop_reason: _ } => {
                // Message complete
            }
            _ => {
                // Other events not handled by footer
            }
        }
    }

    /// Render a single footer line combining all status information.
    ///
    /// Unlike `render_lines` which returns multiple lines for different info,
    /// this method combines everything into a single compact status line.
    pub fn render_footer_line(&self, width: usize) -> String {
        let mut parts = Vec::new();

        // Left section: model/provider
        if !self.model_name.is_empty() {
            if !self.provider_name.is_empty() {
                parts.push(format!("({}) {}", self.provider_name, self.model_name));
            } else {
                parts.push(self.model_name.clone());
            }
        }

        // Token counts
        let tokens = self.format_tokens();
        if !tokens.is_empty() {
            parts.push(tokens);
        }

        // Cost
        if self.total_cost > 0.0 {
            parts.push(format!("${:.3}", self.total_cost));
        }

        // Git branch
        if let Some(ref branch) = self.git_branch {
            if !branch.is_empty() {
                parts.push(format!("@{}", branch));
            }
        }

        // Context window (color indicator in output)
        if self.context_window_pct > 0.0 {
            parts.push(format!("{:.1}%ctx", self.context_window_pct));
        }

        // Thinking level
        if !self.thinking_level.is_empty() && self.thinking_level != "off" {
            parts.push(format!("thinking:{}", self.thinking_level));
        }

        // Session name and duration
        if let Some(ref session) = self.session_name {
            if !session.is_empty() {
                parts.push(session.clone());
            }
        }
        if self.session_duration_secs > 0 {
            parts.push(Self::format_duration_short(self.session_duration_secs));
        }

        let combined = parts.join(" | ");

        // Truncate if needed
        let visible = Self::visible_width(&combined);
        if visible <= width {
            combined
        } else {
            Self::truncate_to_width(&combined, width)
        }
    }

    /// Calculate visible width of a string (accounts for fullwidth chars).
    fn visible_width(s: &str) -> usize {
        s.chars()
            .map(|c| if Self::is_wide_char(c) { 2 } else { 1 })
            .sum()
    }

    /// Check if a character is wide (typically CJK or emoji).
    fn is_wide_char(c: char) -> bool {
        let code = c as u32;
        (0xFF01..=0xFF5E).contains(&code)
            || (0x4E00..=0x9FFF).contains(&code)
            || (0x3400..=0x4DBF).contains(&code)
            || (0xFE30..=0xFE4F).contains(&code)
            || (0xFF00..=0xFFEF).contains(&code)
            || (0x3000..=0x303F).contains(&code)
    }

    /// Truncate string to visible width.
    fn truncate_to_width(s: &str, max_width: usize) -> String {
        let mut result = String::new();
        let mut width = 0;

        for c in s.chars() {
            let char_width = if Self::is_wide_char(c) { 2 } else { 1 };
            if width + char_width > max_width {
                // Add ellipsis if we have room
                if width >= 3 {
                    result.truncate(result.len() - 3);
                    result.push_str("...");
                }
                break;
            }
            result.push(c);
            width += char_width;
        }

        result
    }

    /// Format duration in short form.
    fn format_duration_short(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

/// A keybinding hint for display
#[derive(Debug, Clone)]
pub struct KeybindingHint {
    /// The key sequence
    pub keys: String,
    /// Description of the action
    pub description: String,
}

impl KeybindingHint {
/// TODO.
    pub fn new(keys: &str, description: &str) -> Self {
        Self {
            keys: keys.to_string(),
            description: description.to_string(),
        }
    }
}

/// Session timer for tracking duration
pub struct SessionTimer {
    start: Instant,
}

impl SessionTimer {
    /// Create a new timer starting now
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get the elapsed duration
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Reset the timer
    pub fn reset(&mut self) {
        self.start = Instant::now();
    }
}

impl Default for SessionTimer {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a duration in a human-readable format
pub fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();

    if total_secs < 60 {
        return format!("{}s", total_secs);
    }

    let minutes = total_secs / 60;
    if minutes < 60 {
        let seconds = total_secs % 60;
        return format!("{}m {}s", minutes, seconds);
    }

    let hours = minutes / 60;
    let mins = minutes % 60;
    if hours < 24 {
        return format!("{}h {}m", hours, mins);
    }

    let days = hours / 24;
    let hrs = hours % 24;
    format!("{}d {}h", days, hrs)
}

/// Cost estimation for different models
/// Note: These are rough estimates based on public pricing
pub struct CostEstimator {
    /// Price per 1M input tokens
    input_price_per_m: HashMap<String, f64>,
    /// Price per 1M output tokens
    output_price_per_m: HashMap<String, f64>,
}

impl CostEstimator {
    /// Create a new cost estimator with default prices
    pub fn new() -> Self {
        let mut input_price_per_m = HashMap::new();
        let mut output_price_per_m = HashMap::new();

        // Anthropic models (approximate)
        input_price_per_m.insert("claude".to_string(), 3.0);
        output_price_per_m.insert("claude".to_string(), 15.0);

        // OpenAI models
        input_price_per_m.insert("gpt-4".to_string(), 30.0);
        output_price_per_m.insert("gpt-4".to_string(), 60.0);
        input_price_per_m.insert("gpt-3.5".to_string(), 0.5);
        output_price_per_m.insert("gpt-3.5".to_string(), 1.5);

        // Google models
        input_price_per_m.insert("gemini".to_string(), 0.125);
        output_price_per_m.insert("gemini".to_string(), 0.5);

        Self {
            input_price_per_m,
            output_price_per_m,
        }
    }

    /// Estimate cost for a given model and token usage
    pub fn estimate(&self, model: &str, input_tokens: u32, output_tokens: u32) -> Option<f64> {
        // Find the matching price tier
        let model_lower = model.to_lowercase();

        let input_price = self
            .input_price_per_m
            .iter()
            .find(|(name, _)| model_lower.contains(&name.to_lowercase()))
            .map(|(_, price)| *price);

        let output_price = self
            .output_price_per_m
            .iter()
            .find(|(name, _)| model_lower.contains(&name.to_lowercase()))
            .map(|(_, price)| *price);

        match (input_price, output_price) {
            (Some(inp), Some(outp)) => {
                let input_cost = (input_tokens as f64 / 1_000_000.0) * inp;
                let output_cost = (output_tokens as f64 / 1_000_000.0) * outp;
                Some(input_cost + output_cost)
            }
            _ => None,
        }
    }
}

impl Default for CostEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// Footer data provider trait
pub trait FooterDataProvider: Send + Sync {
    /// Get the current footer data
    fn get_footer_data(&self) -> FooterData;

    /// Get the model name
    fn get_model_name(&self) -> Option<String>;

    /// Get git branch
    fn get_git_branch(&self) -> Option<String>;

    /// Get token counts
    fn get_token_counts(&self) -> (u32, u32);

    /// Get session duration
    fn get_session_duration(&self) -> Duration;

    /// Get keybinding hints
    fn get_keybinding_hints(&self) -> Vec<KeybindingHint>;
}

/// Simple footer data provider implementation
pub struct SimpleFooterDataProvider {
    model_name: Option<String>,
    provider_name: Option<String>,
    git_branch: Option<String>,
    pwd: Option<String>,
    input_tokens: Arc<AtomicU32>,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    session_timer: SessionTimer,
    keybinding_hints: Vec<KeybindingHint>,
    extension_statuses: HashMap<String, String>,
    available_providers: usize,
    thinking_level: String,
    session_name: Option<String>,
}

impl SimpleFooterDataProvider {
    /// Create a new provider
    pub fn new() -> Self {
        Self {
            model_name: None,
            provider_name: None,
            git_branch: None,
            pwd: None,
            input_tokens: Arc::new(AtomicU32::new(0)),
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            session_timer: SessionTimer::new(),
            keybinding_hints: Vec::new(),
            extension_statuses: HashMap::new(),
            available_providers: 0,
            thinking_level: "off".to_string(),
            session_name: None,
        }
    }

    /// Set the model name
    pub fn with_model(mut self, model: Option<String>, provider: Option<String>) -> Self {
        self.model_name = model;
        self.provider_name = provider;
        self
    }

    /// Set the git branch
    pub fn with_git_branch(mut self, branch: Option<String>) -> Self {
        self.git_branch = branch;
        self
    }

    /// Set working directory
    pub fn with_pwd(mut self, pwd: Option<String>) -> Self {
        self.pwd = pwd;
        self
    }

    /// Set token counts
    pub fn with_tokens(mut self, input: u32, output: u32) -> Self {
        self.input_tokens = Arc::new(AtomicU32::new(input));
        self.output_tokens = output;
        self
    }

    /// Add a keybinding hint
    pub fn add_hint(mut self, keys: &str, description: &str) -> Self {
        self.keybinding_hints
            .push(KeybindingHint::new(keys, description));
        self
    }

    /// Set the available provider count
    pub fn with_providers(mut self, count: usize) -> Self {
        self.available_providers = count;
        self
    }

    /// Update token counts
    pub fn update_tokens(&mut self, input: u32, output: u32) {
        self.input_tokens = Arc::new(AtomicU32::new(input));
        self.output_tokens = output;
    }

    /// Update cache tokens
    pub fn update_cache_tokens(&mut self, read: u32, write: u32) {
        self.cache_read_tokens = read;
        self.cache_write_tokens = write;
    }

    /// Add an extension status
    pub fn set_extension_status(&mut self, key: &str, status: Option<&str>) {
        if let Some(s) = status {
            self.extension_statuses
                .insert(key.to_string(), s.to_string());
        } else {
            self.extension_statuses.remove(key);
        }
    }

    /// Set thinking level
    pub fn set_thinking_level(&mut self, level: &str) {
        self.thinking_level = level.to_string();
    }

    /// Set session name
    pub fn set_session_name(&mut self, name: Option<String>) {
        self.session_name = name;
    }

    /// Set provider count
    pub fn set_available_providers(&mut self, count: usize) {
        self.available_providers = count;
    }
}

impl Default for SimpleFooterDataProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FooterDataProvider for SimpleFooterDataProvider {
    fn get_footer_data(&self) -> FooterData {
        let mut data = FooterData {
            model_name: self.model_name.clone().unwrap_or_default(),
            provider_name: self.provider_name.clone().unwrap_or_default(),
            thinking_level: self.thinking_level.clone(),
            session_name: self.session_name.clone(),
            git_branch: self.git_branch.clone(),
            pwd: self.pwd.clone(),
            input_tokens: Arc::clone(&self.input_tokens),
            output_tokens: Arc::new(AtomicU32::new(self.output_tokens)),
            cache_read_tokens: Arc::new(AtomicU32::new(self.cache_read_tokens)),
            cache_write_tokens: Arc::new(AtomicU32::new(self.cache_write_tokens)),
            context_window_pct: 0.0,
            total_cost: 0.0,
            session_duration_secs: self.session_timer.elapsed().as_secs(),
            extension_statuses: self.extension_statuses.clone(),
        };

        // Calculate cost if model is available
        if let Some(ref model) = self.model_name {
            let cost_estimator = CostEstimator::new();
            if let Some(cost) = cost_estimator.estimate(
                model,
                self.input_tokens.load(Ordering::Relaxed),
                self.output_tokens,
            ) {
                data.total_cost = cost;
            }
        }

        data
    }

    fn get_model_name(&self) -> Option<String> {
        self.model_name.clone()
    }

    fn get_git_branch(&self) -> Option<String> {
        self.git_branch.clone()
    }

    fn get_token_counts(&self) -> (u32, u32) {
        (
            self.input_tokens.load(Ordering::Relaxed),
            self.output_tokens,
        )
    }

    fn get_session_duration(&self) -> Duration {
        self.session_timer.elapsed()
    }

    fn get_keybinding_hints(&self) -> Vec<KeybindingHint> {
        self.keybinding_hints.clone()
    }
}

/// Extension status helper
pub struct ExtensionStatusTracker {
    statuses: HashMap<String, String>,
}

impl ExtensionStatusTracker {
/// TODO.
    pub fn new() -> Self {
        Self {
            statuses: HashMap::new(),
        }
    }

/// TODO: document this function.
    pub fn set(&mut self, extension: &str, status: &str) {
        self.statuses
            .insert(extension.to_string(), status.to_string());
    }

/// TODO: document this function.
    pub fn clear(&mut self, extension: &str) {
        self.statuses.remove(extension);
    }

/// TODO: document this function.
    pub fn get_all(&self) -> &HashMap<String, String> {
        &self.statuses
    }
}

impl Default for ExtensionStatusTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_footer_data_new() {
        let data = FooterData::new();
        assert!(data.model_name.is_empty());
        assert!(data.provider_name.is_empty());
        assert_eq!(data.get_input_tokens(), 0);
        assert_eq!(data.get_output_tokens(), 0);
    }

    #[test]
    fn test_footer_data_with_model() {
        let data = FooterData::new().with_model("claude-3.5-sonnet", "anthropic");
        assert_eq!(data.model_name, "claude-3.5-sonnet");
        assert_eq!(data.provider_name, "anthropic");
    }

    #[test]
    fn test_footer_data_with_thinking_level() {
        let data = FooterData::new().with_thinking_level("medium");
        assert_eq!(data.thinking_level, "medium");
    }

    #[test]
    fn test_footer_data_with_session_name() {
        let data = FooterData::new().with_session_name(Some("my-session".to_string()));
        assert_eq!(data.session_name, Some("my-session".to_string()));
    }

    #[test]
    fn test_footer_data_with_pwd() {
        let data = FooterData::new().with_pwd(Some("~/projects/oxi".to_string()));
        assert_eq!(data.pwd, Some("~/projects/oxi".to_string()));
    }

    #[test]
    fn test_footer_data_update_tokens() {
        let data = FooterData::new();
        data.update_tokens(1000, 500);
        assert_eq!(data.get_input_tokens(), 1000);
        assert_eq!(data.get_output_tokens(), 500);
    }

    #[test]
    fn test_footer_data_update_cache_tokens() {
        let data = FooterData::new();
        data.update_cache_tokens(2000, 100);
        assert_eq!(data.get_cache_read_tokens(), 2000);
        assert_eq!(data.get_cache_write_tokens(), 100);
    }

    #[test]
    fn test_footer_data_update_all_tokens() {
        let data = FooterData::new();
        data.update_all_tokens(1000, 500, 2000, 100);
        assert_eq!(data.get_input_tokens(), 1000);
        assert_eq!(data.get_output_tokens(), 500);
        assert_eq!(data.get_cache_read_tokens(), 2000);
        assert_eq!(data.get_cache_write_tokens(), 100);
    }

    #[test]
    fn test_footer_data_total_tokens() {
        let data = FooterData::new();
        data.update_tokens(100, 50);
        assert_eq!(data.total_tokens(), 150);
    }

    #[test]
    fn test_footer_data_format_tokens() {
        let data = FooterData::new();
        data.update_tokens(1500, 2500);
        let formatted = data.format_tokens();
        assert!(formatted.contains("↑1.5k") || formatted.contains("↑1500"));
        assert!(formatted.contains("↓2.5k") || formatted.contains("↓2500"));
    }

    #[test]
    fn test_footer_data_format_context_window() {
        let mut data = FooterData::new();
        data.set_context_window_pct(75.5);
        assert_eq!(data.format_context_window(), "75.5%");
    }

    #[test]
    fn test_footer_data_has_data() {
        let mut data = FooterData::new();
        assert!(!data.has_data());
        data.model_name = "gpt-4".to_string();
        assert!(data.has_data());
    }

    #[test]
    fn test_footer_data_render_lines() {
        let mut data = FooterData::new();
        data.model_name = "gpt-4".to_string();
        data.provider_name = "openai".to_string();
        data.update_tokens(100, 50);
        data.set_total_cost(0.01);
        data.pwd = Some("~/projects".to_string());

        let lines = data.render_lines(80);
        assert!(lines.len() >= 2);
        assert!(lines[0].contains("~/projects"));
        assert!(lines[1].contains("gpt-4"));
    }

    #[test]
    fn test_footer_data_extension_status() {
        let mut data = FooterData::new();
        data.set_extension_status("ext1", Some("working"));
        assert_eq!(
            data.extension_statuses.get("ext1"),
            Some(&"working".to_string())
        );
        data.set_extension_status("ext1", None);
        assert!(data.extension_statuses.get("ext1").is_none());
    }

    #[test]
    fn test_footer_data_set_context_window_pct() {
        let mut data = FooterData::new();
        data.set_context_window_pct(150.0); // Should be clamped to 100
        assert_eq!(data.context_window_pct, 100.0);
        data.set_context_window_pct(-10.0); // Should be clamped to 0
        assert_eq!(data.context_window_pct, 0.0);
    }

    #[test]
    fn test_footer_data_set_total_cost() {
        let mut data = FooterData::new();
        data.set_total_cost(1.234);
        assert_eq!(data.total_cost, 1.234);
    }

    #[test]
    fn test_footer_data_set_session_duration() {
        let mut data = FooterData::new();
        data.set_session_duration(3600);
        assert_eq!(data.session_duration_secs, 3600);
    }

    #[test]
    fn test_session_timer() {
        let timer = SessionTimer::new();
        std::thread::sleep(Duration::from_millis(10));
        let elapsed = timer.elapsed();
        assert!(elapsed.as_millis() >= 10);
    }

    #[test]
    fn test_session_timer_reset() {
        let mut timer = SessionTimer::new();
        std::thread::sleep(Duration::from_millis(10));
        timer.reset();
        let elapsed = timer.elapsed();
        assert!(elapsed.as_millis() < 10);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m");
        assert_eq!(format_duration(Duration::from_secs(86401)), "1d 0h");
    }

    #[test]
    fn test_cost_estimator() {
        let estimator = CostEstimator::new();

        // Test with Claude model
        let cost = estimator.estimate("claude-3.5-sonnet", 1_000_000, 1_000_000);
        assert!(cost.is_some());
        assert!(cost.unwrap() > 0.0);

        // Test with unknown model
        let cost = estimator.estimate("unknown-model", 1000, 500);
        assert!(cost.is_none());
    }

    #[test]
    fn test_cost_estimator_gpt4() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("gpt-4-turbo", 500_000, 200_000);
        assert!(cost.is_some());
        let val = cost.unwrap();
        // gpt-4: $30/M input, $60/M output
        // 0.5M * $30 + 0.2M * $60 = $15 + $12 = $27
        assert!((val - 27.0).abs() < 0.1);
    }

    #[test]
    fn test_keybinding_hint() {
        let hint = KeybindingHint::new("Ctrl+C", "Cancel");
        assert_eq!(hint.keys, "Ctrl+C");
        assert_eq!(hint.description, "Cancel");
    }

    #[test]
    fn test_simple_provider() {
        let provider = SimpleFooterDataProvider::new()
            .with_model(Some("gpt-4".to_string()), Some("openai".to_string()))
            .with_tokens(100, 50);

        assert_eq!(provider.get_model_name(), Some("gpt-4".to_string()));
        assert_eq!(provider.get_token_counts(), (100, 50));
    }

    #[test]
    fn test_simple_provider_update() {
        let mut provider = SimpleFooterDataProvider::new();
        provider.update_tokens(200, 100);
        provider.update_cache_tokens(500, 50);
        assert_eq!(provider.get_token_counts(), (200, 100));
    }

    #[test]
    fn test_simple_provider_footer_data() {
        let provider = SimpleFooterDataProvider::new()
            .with_model(Some("claude".to_string()), Some("anthropic".to_string()))
            .with_tokens(1000, 500);

        let footer = provider.get_footer_data();
        assert_eq!(footer.model_name, "claude");
        assert!(footer.total_cost > 0.0);
    }

    #[test]
    fn test_extension_status_tracker() {
        let mut tracker = ExtensionStatusTracker::new();

        tracker.set("my-extension", "Working...");
        assert_eq!(
            tracker.get_all().get("my-extension"),
            Some(&"Working...".to_string())
        );

        tracker.clear("my-extension");
        assert!(tracker.get_all().get("my-extension").is_none());
    }

    #[test]
    fn test_simple_provider_thinking_level() {
        let mut provider = SimpleFooterDataProvider::new();
        provider.set_thinking_level("high");
        let footer = provider.get_footer_data();
        assert_eq!(footer.thinking_level, "high");
    }

    #[test]
    fn test_simple_provider_session_name() {
        let mut provider = SimpleFooterDataProvider::new();
        provider.set_session_name(Some("my-session".to_string()));
        let footer = provider.get_footer_data();
        assert_eq!(footer.session_name, Some("my-session".to_string()));
    }
}
