//! Footer data provider for TUI status display
//!
//! Provides utilities for gathering and formatting footer data
//! such as model info, token usage, and keybinding hints.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Footer data containing all status information
#[derive(Debug, Clone, Default)]
pub struct FooterData {
    /// Current model name
    pub model_name: Option<String>,
    /// Input tokens used
    pub input_tokens: u32,
    /// Output tokens used
    pub output_tokens: u32,
    /// Cached tokens
    pub cached_tokens: Option<u32>,
    /// Estimated cost in USD
    pub estimated_cost: Option<f64>,
    /// Current git branch
    pub git_branch: Option<String>,
    /// Session duration
    pub session_duration: Duration,
    /// Keybinding hints
    pub keybinding_hints: Vec<KeybindingHint>,
    /// Extension status messages
    pub extension_statuses: HashMap<String, String>,
    /// Available provider count
    pub available_providers: usize,
}

impl FooterData {
    /// Create empty footer data
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with model name
    pub fn with_model(mut self, model: &str) -> Self {
        self.model_name = Some(model.to_string());
        self
    }

    /// Create with token usage
    pub fn with_tokens(mut self, input: u32, output: u32) -> Self {
        self.input_tokens = input;
        self.output_tokens = output;
        self
    }

    /// Create with git branch
    pub fn with_git_branch(mut self, branch: Option<String>) -> Self {
        self.git_branch = branch;
        self
    }

    /// Format footer as display string
    pub fn format(&self) -> String {
        let mut parts = Vec::new();

        // Model name
        if let Some(model) = &self.model_name {
            parts.push(format!("Model: {}", model));
        }

        // Token usage
        if self.input_tokens > 0 || self.output_tokens > 0 {
            let mut tokens_str = format!("Tokens: {}/{}", self.input_tokens, self.output_tokens);
            if let Some(cached) = self.cached_tokens {
                tokens_str.push_str(&format!(" (+{} cached)", cached));
            }
            parts.push(tokens_str);
        }

        // Cost
        if let Some(cost) = self.estimated_cost {
            parts.push(format!("Cost: ${:.4}", cost));
        }

        // Git branch
        if let Some(branch) = &self.git_branch {
            parts.push(format!("Branch: {}", branch));
        }

        // Session duration
        if self.session_duration.as_secs() > 0 {
            parts.push(format!("Duration: {}", format_duration(self.session_duration)));
        }

        parts.join(" | ")
    }

    /// Get total tokens
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    /// Check if there is any data to display
    pub fn is_empty(&self) -> bool {
        self.model_name.is_none()
            && self.input_tokens == 0
            && self.output_tokens == 0
            && self.git_branch.is_none()
            && self.session_duration.is_zero()
            && self.extension_statuses.is_empty()
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
    git_branch: Option<String>,
    input_tokens: u32,
    output_tokens: u32,
    cached_tokens: Option<u32>,
    session_timer: SessionTimer,
    keybinding_hints: Vec<KeybindingHint>,
    extension_statuses: HashMap<String, String>,
    available_providers: usize,
}

impl SimpleFooterDataProvider {
    /// Create a new provider
    pub fn new() -> Self {
        Self {
            model_name: None,
            git_branch: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: None,
            session_timer: SessionTimer::new(),
            keybinding_hints: Vec::new(),
            extension_statuses: HashMap::new(),
            available_providers: 0,
        }
    }

    /// Set the model name
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model_name = model;
        self
    }

    /// Set the git branch
    pub fn with_git_branch(mut self, branch: Option<String>) -> Self {
        self.git_branch = branch;
        self
    }

    /// Set token counts
    pub fn with_tokens(mut self, input: u32, output: u32) -> Self {
        self.input_tokens = input;
        self.output_tokens = output;
        self
    }

    /// Add a keybinding hint
    pub fn add_hint(mut self, keys: &str, description: &str) -> Self {
        self.keybinding_hints.push(KeybindingHint::new(keys, description));
        self
    }

    /// Set the available provider count
    pub fn with_providers(mut self, count: usize) -> Self {
        self.available_providers = count;
        self
    }

    /// Update token counts
    pub fn update_tokens(&mut self, input: u32, output: u32) {
        self.input_tokens = input;
        self.output_tokens = output;
    }

    /// Add an extension status
    pub fn set_extension_status(&mut self, key: &str, status: Option<&str>) {
        if let Some(s) = status {
            self.extension_statuses.insert(key.to_string(), s.to_string());
        } else {
            self.extension_statuses.remove(key);
        }
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
            model_name: self.model_name.clone(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_tokens: self.cached_tokens,
            git_branch: self.git_branch.clone(),
            session_duration: self.session_timer.elapsed(),
            keybinding_hints: self.keybinding_hints.clone(),
            extension_statuses: self.extension_statuses.clone(),
            available_providers: self.available_providers,
            estimated_cost: None,
        };

        // Calculate cost if model is available
        if let Some(ref model) = self.model_name {
            let cost_estimator = CostEstimator::new();
            data.estimated_cost = cost_estimator.estimate(
                model,
                self.input_tokens,
                self.output_tokens,
            );
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
        (self.input_tokens, self.output_tokens)
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
    pub fn new() -> Self {
        Self {
            statuses: HashMap::new(),
        }
    }

    pub fn set(&mut self, extension: &str, status: &str) {
        self.statuses.insert(extension.to_string(), status.to_string());
    }

    pub fn clear(&mut self, extension: &str) {
        self.statuses.remove(extension);
    }

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
        assert!(data.is_empty());
    }

    #[test]
    fn test_footer_data_with_model() {
        let data = FooterData::new().with_model("claude-3.5-sonnet");
        assert_eq!(data.model_name, Some("claude-3.5-sonnet".to_string()));
    }

    #[test]
    fn test_footer_data_with_tokens() {
        let data = FooterData::new().with_tokens(1000, 500);
        assert_eq!(data.input_tokens, 1000);
        assert_eq!(data.output_tokens, 500);
    }

    #[test]
    fn test_footer_data_format() {
        let data = FooterData::new()
            .with_model("gpt-4")
            .with_tokens(100, 50);
        let formatted = data.format();
        assert!(formatted.contains("gpt-4"));
        assert!(formatted.contains("100/50"));
    }

    #[test]
    fn test_footer_data_total_tokens() {
        let data = FooterData::new().with_tokens(100, 50);
        assert_eq!(data.total_tokens(), 150);
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
    fn test_keybinding_hint() {
        let hint = KeybindingHint::new("Ctrl+C", "Cancel");
        assert_eq!(hint.keys, "Ctrl+C");
        assert_eq!(hint.description, "Cancel");
    }

    #[test]
    fn test_simple_provider() {
        let provider = SimpleFooterDataProvider::new()
            .with_model(Some("gpt-4".to_string()))
            .with_tokens(100, 50);

        assert_eq!(provider.get_model_name(), Some("gpt-4".to_string()));
        assert_eq!(provider.get_token_counts(), (100, 50));
    }

    #[test]
    fn test_simple_provider_footer_data() {
        let provider = SimpleFooterDataProvider::new()
            .with_model(Some("claude".to_string()))
            .with_tokens(1000, 500);

        let footer = provider.get_footer_data();
        assert_eq!(footer.model_name, Some("claude".to_string()));
        assert!(footer.estimated_cost.is_some());
    }

    #[test]
    fn test_extension_status_tracker() {
        let mut tracker = ExtensionStatusTracker::new();

        tracker.set("my-extension", "Working...");
        assert_eq!(tracker.get_all().get("my-extension"), Some(&"Working...".to_string()));

        tracker.clear("my-extension");
        assert!(tracker.get_all().get("my-extension").is_none());
    }

    #[test]
    fn test_footer_data_with_git_branch() {
        let data = FooterData::new().with_git_branch(Some("main".to_string()));
        assert_eq!(data.git_branch, Some("main".to_string()));
    }

    #[test]
    fn test_footer_data_extension_statuses() {
        let mut data = FooterData::new();
        data.extension_statuses.insert("ext1".to_string(), "status1".to_string());
        assert_eq!(data.extension_statuses.len(), 1);
    }
}