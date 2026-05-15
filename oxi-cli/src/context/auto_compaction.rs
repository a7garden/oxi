//! Auto-compaction engine for oxi
//!
//! Monitors context usage and automatically triggers compaction when threshold is exceeded.
//! Queues messages during compaction and resumes processing afterward.

use anyhow::{Context, Result};
use oxi_ai::{
    Model, Provider, UserMessage,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Agent message for queued items
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentMessage {
/// pub.
    pub role: String,
/// pub.
    pub content: String,
/// pub.
    pub timestamp: i64,
}

impl AgentMessage {
/// TODO.
    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

/// TODO.
    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// Compaction configuration
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Threshold ratio (0.0 to 1.0) - compact at this percentage of context
    pub threshold: f32,
    /// Keep last N messages always
    pub keep_recent: usize,
    /// Compact N messages at once
    pub max_batch: usize,
    /// Model to use for summarization (None = use same model)
    pub summary_model: Option<String>,
    /// Custom instruction for the summarizer
    pub custom_instructions: Option<String>,
    /// Enable automatic compaction
    pub enabled: bool,
    /// Show user notification during compaction
    pub show_notification: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            keep_recent: 4,
            max_batch: 20,
            summary_model: None,
            custom_instructions: None,
            enabled: true,
            show_notification: true,
        }
    }
}

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct CompactedContext {
    /// Summary of the compacted messages
    pub summary: String,
    /// Number of messages that were compacted
    pub compacted_count: usize,
    /// Estimated tokens saved
    pub tokens_saved: usize,
}

impl CompactedContext {
/// TODO.
    pub fn new(summary: String, compacted_count: usize, tokens_saved: usize) -> Self {
        Self {
            summary,
            compacted_count,
            tokens_saved,
        }
    }
}

/// State of the auto-compactor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactorState {
    /// Idle, not compacting
    Idle,
    /// Currently compacting
    Compacting,
    /// Compaction paused (e.g., during user input)
    Paused,
}

/// Event emitted by the auto-compactor
#[derive(Debug, Clone)]
pub enum CompactorEvent {
    /// Compaction started
    Started {
/// reason.
        reason: CompactionReason,
/// tokens_before.
        tokens_before: usize,
    },
    /// Compaction progress update
    Progress {
/// messages_compacted.
        messages_compacted: usize,
/// total_messages.
        total_messages: usize,
    },
    /// Compaction completed successfully
    Completed {
/// The resulting compacted context.
        result: CompactedContext,
/// Number of tokens after compaction.
        tokens_after: usize,
    },
    /// Compaction was aborted
    Aborted {
        /// Reason for abortion
        reason: String,
    },
    /// Compaction failed
    Failed {
        /// Error message
        error: String,
    },
    /// Context is approaching threshold
    Warning {
        /// Current context size ratio (0.0–1.0)
        current_ratio: f32,
        /// Current token count
        tokens: usize,
        /// Maximum allowed tokens
        max_tokens: usize,
    },
}

/// Reason for compaction trigger
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    /// Manual compaction requested
    Manual,
    /// Automatic due to threshold
    Automatic,
    /// Context overflow detected
    Overflow,
    /// Iteration-based compaction
    Iteration {
        /// Current iteration count
        current: usize,
        /// Compaction interval
        every_n: usize,
    },
}

impl std::fmt::Display for CompactionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactionReason::Manual => write!(f, "manual"),
            CompactionReason::Automatic => write!(f, "automatic"),
            CompactionReason::Overflow => write!(f, "overflow"),
            CompactionReason::Iteration { current, every_n } => {
                write!(f, "iteration {}/{}", current, every_n)
            }
        }
    }
}

/// Notification for UI display
#[derive(Debug, Clone)]
pub struct CompactionNotification {
/// pub.
    pub message: String,
/// pub.
    pub level: NotificationLevel,
/// pub.
    pub can_cancel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// NotificationLevel.
pub enum NotificationLevel {
/// info variant.
    Info,
/// warning variant.
    Warning,
/// error variant.
    Error,
}

impl CompactionNotification {
/// TODO.
    pub fn info(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            level: NotificationLevel::Info,
            can_cancel: false,
        }
    }

/// TODO.
    pub fn warning(msg: impl Into<String>, can_cancel: bool) -> Self {
        Self {
            message: msg.into(),
            level: NotificationLevel::Warning,
            can_cancel,
        }
    }

/// TODO.
    pub fn compacting(tokens: usize, max_tokens: usize, percentage: f32) -> Self {
        Self {
            message: format!(
                "Context at {:.0}% ({}/{} tokens) — summarizing conversation...",
                percentage * 100.0,
                tokens,
                max_tokens
            ),
            level: NotificationLevel::Info,
            can_cancel: true,
        }
    }
}

/// Auto-compaction engine
pub struct AutoCompactor {
    /// Keep: LLM provider stored for future background compaction execution.
    #[allow(dead_code)]
    llm: Arc<dyn Provider>,
    model: Model,
    config: CompactionConfig,
    pending_queue: RwLock<Vec<AgentMessage>>,
    state: RwLock<CompactorState>,
    last_check: RwLock<Instant>,
    last_tokens: RwLock<usize>,
    compaction_count: RwLock<usize>,
}

impl AutoCompactor {
    /// Create a new auto-compactor
    pub fn new(llm: Arc<dyn Provider>, model: Model, config: CompactionConfig) -> Self {
        Self {
            llm,
            model,
            config,
            pending_queue: RwLock::new(Vec::new()),
            state: RwLock::new(CompactorState::Idle),
            last_check: RwLock::new(Instant::now()),
            last_tokens: RwLock::new(0),
            compaction_count: RwLock::new(0),
        }
    }

    /// Create with default configuration
    pub fn with_defaults(llm: Arc<dyn Provider>, model: Model) -> Self {
        Self::new(llm, model, CompactionConfig::default())
    }

    /// Get current state
    pub fn state(&self) -> CompactorState {
        *self.state.read()
    }

    /// Check if compaction should be triggered
    pub fn should_compact(&self, context_tokens: u32, max_tokens: u32) -> bool {
        if !self.config.enabled {
            return false;
        }
        if max_tokens == 0 {
            return false;
        }
        let ratio = context_tokens as f32 / max_tokens as f32;
        *self.last_tokens.write() = context_tokens as usize;
        ratio >= self.config.threshold
    }

    /// Get the context ratio (0.0 to 1.0)
    pub fn context_ratio(&self, context_tokens: u32, max_tokens: u32) -> f32 {
        if max_tokens == 0 {
            return 0.0;
        }
        context_tokens as f32 / max_tokens as f32
    }

    /// Queue a message for processing after compaction
    pub fn queue_for_compaction(&self, msg: AgentMessage) {
        debug!(
            "Queueing message for post-compaction processing: {}",
            msg.role
        );
        self.pending_queue.write().push(msg);
    }

    /// Flush the pending queue, returning all queued messages
    pub fn flush_queue(&self) -> Vec<AgentMessage> {
        let mut queue = self.pending_queue.write();
        let messages: Vec<AgentMessage> = queue.drain(..).collect();
        debug!("Flushed {} queued messages", messages.len());
        messages
    }

    /// Get number of pending messages
    pub fn pending_count(&self) -> usize {
        self.pending_queue.read().len()
    }

    /// Get the compaction configuration
    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }

    /// Update configuration
    pub fn set_config(&mut self, config: CompactionConfig) {
        self.config = config;
    }

    /// Get total compaction count
    pub fn compaction_count(&self) -> usize {
        *self.compaction_count.read()
    }

    /// Get notification for UI display
    pub fn get_notification(
        &self,
        context_tokens: usize,
        max_tokens: usize,
    ) -> Option<CompactionNotification> {
        if !self.config.show_notification {
            return None;
        }
        let ratio = if max_tokens > 0 {
            context_tokens as f32 / max_tokens as f32
        } else {
            0.0
        };

        if ratio >= self.config.threshold {
            Some(CompactionNotification::compacting(
                context_tokens,
                max_tokens,
                ratio,
            ))
        } else if ratio >= self.config.threshold * 0.9 {
            let warning = format!(
                "Context at {:.0}% — consider compacting soon",
                ratio * 100.0
            );
            Some(CompactionNotification::warning(warning, false))
        } else {
            None
        }
    }

    /// Perform compaction on a list of messages
    pub async fn compact(&self, messages: &[AgentMessage]) -> Result<CompactedContext> {
        if messages.is_empty() {
            return Ok(CompactedContext::new(String::new(), 0, 0));
        }

        *self.state.write() = CompactorState::Compacting;
        let tokens_before = self.estimate_tokens(messages);

        info!(
            "Starting compaction: {} messages, ~{} tokens",
            messages.len(),
            tokens_before
        );

        // Build the summary prompt
        let prompt = self.build_summarization_prompt(messages);

        // Convert to LLM messages
        let _llm_messages: Vec<oxi_ai::Message> =
            vec![oxi_ai::Message::User(UserMessage::new(prompt))];

        // Create a context for the completion
        let mut context = oxi_ai::Context::new();
        context.set_system_prompt(
            "You are a helpful assistant that summarizes conversations concisely. \
             Capture key points, decisions, and ongoing tasks. Be precise with file names and error messages.",
        );

        let options = oxi_ai::StreamOptions {
            temperature: Some(0.3),
            max_tokens: Some(1024),
            ..Default::default()
        };

        // Perform the summarization
        let summary = oxi_ai::complete(&self.model, &context, Some(options))
            .await
            .context("LLM summarization failed")?;

        let summary_text = summary.text_content();

        // Calculate estimated tokens saved
        let summary_tokens = self.estimate_tokens_string(&summary_text);
        let tokens_saved = tokens_before.saturating_sub(summary_tokens);

        let result = CompactedContext::new(summary_text, messages.len(), tokens_saved);

        *self.compaction_count.write() += 1;
        *self.state.write() = CompactorState::Idle;

        info!(
            "Compaction completed: {} messages summarized, ~{} tokens saved",
            messages.len(),
            tokens_saved
        );

        Ok(result)
    }

    /// Build summarization prompt from messages
    fn build_summarization_prompt(&self, messages: &[AgentMessage]) -> String {
        let mut prompt = String::new();

        prompt.push_str("Summarize the following conversation concisely. ");
        prompt.push_str("Capture the key points, decisions, and any ongoing tasks or context.\n\n");

        if let Some(ref instruction) = self.config.custom_instructions {
            prompt.push_str(&format!("Focus areas: {}\n\n", instruction));
        }

        prompt.push_str("## Conversation to summarize:\n");

        for (i, msg) in messages.iter().enumerate() {
            let role = match msg.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => "System",
            };
            let content = if msg.content.len() > 500 {
                let boundary = msg.content.char_indices()
                    .take_while(|(i, _)| *i <= 500)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0);
                format!("{}...", &msg.content[..boundary])
            } else {
                msg.content.clone()
            };
            prompt.push_str(&format!("[{} {}]: {}\n", role, i + 1, content));
        }

        prompt.push_str("\n## Summary:\n");
        prompt
            .push_str("Provide a concise summary that captures the essence of this conversation.");

        prompt
    }

    /// Estimate tokens in messages (rough approximation)
    fn estimate_tokens(&self, messages: &[AgentMessage]) -> usize {
        messages
            .iter()
            .map(|msg| {
                let chars = msg.content.chars().count();
                // Rough approximation: chars / 3
                (chars / 3).max(1)
            })
            .sum()
    }

    /// Estimate tokens in a single string
    fn estimate_tokens_string(&self, text: &str) -> usize {
        (text.len() / 4).max(1)
    }

    /// Abort any ongoing compaction
    pub fn abort(&self) {
        let state = self.state.read();
        if *state == CompactorState::Compacting {
            warn!("Compaction abort requested");
            *self.state.write() = CompactorState::Idle;
        }
    }

    /// Reset the compactor state
    pub fn reset(&self) {
        self.pending_queue.write().clear();
        *self.state.write() = CompactorState::Idle;
        *self.compaction_count.write() = 0;
    }

    /// Update last check timestamp
    pub fn mark_checked(&self) {
        *self.last_check.write() = Instant::now();
    }

    /// Get time since last check
    pub fn time_since_check(&self) -> Duration {
        self.last_check.read().elapsed()
    }

    /// Get last measured tokens
    pub fn last_tokens(&self) -> usize {
        *self.last_tokens.read()
    }

    /// Check if currently compacting
    pub fn is_compacting(&self) -> bool {
        *self.state.read() == CompactorState::Compacting
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oxi_ai::Api;

    fn create_test_provider() -> Arc<dyn Provider> {
        // Create a mock provider for testing
        struct MockProvider;
        #[async_trait]
        impl oxi_ai::Provider for MockProvider {
            async fn stream(
                &self,
                _model: &oxi_ai::Model,
                _context: &oxi_ai::Context,
                _options: Option<oxi_ai::StreamOptions>,
            ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = oxi_ai::ProviderEvent> + Send>>, oxi_ai::ProviderError> {
                use futures::StreamExt;
                let stream = futures::stream::empty::<oxi_ai::ProviderEvent>();
                Ok(Box::pin(stream))
            }
            fn name(&self) -> &str { "mock" }
        }
        Arc::new(MockProvider)
    }

    fn create_test_model() -> Model {
        Model::new(
            "test-model",
            "Test Model",
            Api::AnthropicMessages,
            "test",
            "https://test.example.com",
        )
    }

    fn create_test_messages(count: usize) -> Vec<AgentMessage> {
        (0..count)
            .map(|i| AgentMessage::user(format!("Test message {}", i)))
            .collect()
    }

    #[test]
    fn test_compaction_config_defaults() {
        let config = CompactionConfig::default();
        assert!((config.threshold - 0.8).abs() < 0.001);
        assert_eq!(config.keep_recent, 4);
        assert_eq!(config.max_batch, 20);
        assert!(config.enabled);
        assert!(config.show_notification);
    }

    #[test]
    fn test_compaction_config_builder() {
        let config = CompactionConfig {
            threshold: 0.7,
            keep_recent: 8,
            max_batch: 30,
            summary_model: Some("claude-3".to_string()),
            custom_instructions: Some("Focus on code".to_string()),
            enabled: true,
            show_notification: true,
        };

        assert!((config.threshold - 0.7).abs() < 0.001);
        assert_eq!(config.keep_recent, 8);
        assert_eq!(config.max_batch, 30);
        assert_eq!(config.summary_model, Some("claude-3".to_string()));
    }

    #[test]
    fn test_should_compact_disabled() {
        let provider = create_test_provider();
        let model = create_test_model();
        let config = CompactionConfig {
            enabled: false,
            threshold: 0.8,
            ..Default::default()
        };
        let compactor = AutoCompactor::new(provider, model, config);

        assert!(!compactor.should_compact(100_000, 128_000));
        assert!(!compactor.should_compact(120_000, 128_000));
    }

    #[test]
    fn test_should_compact_threshold() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        // Below threshold (79%)
        assert!(!compactor.should_compact(100_000, 128_000));

        // At threshold (exactly 80%)
        assert!(compactor.should_compact(102_400, 128_000));

        // Above threshold (90%)
        assert!(compactor.should_compact(115_000, 128_000));
    }

    #[test]
    fn test_should_compact_zero_max() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        assert!(!compactor.should_compact(100_000, 0));
    }

    #[test]
    fn test_context_ratio() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        assert!((compactor.context_ratio(64000, 128000) - 0.5).abs() < 0.001);
        assert!((compactor.context_ratio(102400, 128000) - 0.8).abs() < 0.001);
        assert!((compactor.context_ratio(128000, 128000) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_queue_and_flush() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        assert_eq!(compactor.pending_count(), 0);

        compactor.queue_for_compaction(AgentMessage::user("Hello".to_string()));
        compactor.queue_for_compaction(AgentMessage::assistant("Hi there".to_string()));

        assert_eq!(compactor.pending_count(), 2);

        let messages = compactor.flush_queue();
        assert_eq!(messages.len(), 2);
        assert_eq!(compactor.pending_count(), 0);
    }

    #[test]
    fn test_queue_message_order() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        compactor.queue_for_compaction(AgentMessage::user("First".to_string()));
        compactor.queue_for_compaction(AgentMessage::user("Second".to_string()));
        compactor.queue_for_compaction(AgentMessage::user("Third".to_string()));

        let messages = compactor.flush_queue();
        assert_eq!(messages[0].content, "First");
        assert_eq!(messages[1].content, "Second");
        assert_eq!(messages[2].content, "Third");
    }

    #[test]
    fn test_get_notification_warning() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        // Default threshold is 0.8 (80%), warning threshold is 0.72 (72%)
        // Below warning threshold
        let notif = compactor.get_notification(90000, 128000); // 70.3% - below 72% warning
        assert!(notif.is_none());

        // Above warning threshold (72%) but below compaction threshold (80%)
        let notif = compactor.get_notification(95000, 128000); // 74.2% - above 72% warning
        assert!(notif.is_some());

        // Above compaction threshold (80%)
        let notif = compactor.get_notification(110000, 128000); // 85.9%
        assert!(notif.is_some());
        assert!(notif.unwrap().message.contains("86"));
    }

    #[test]
    fn test_get_notification_disabled() {
        let provider = create_test_provider();
        let model = create_test_model();
        let config = CompactionConfig {
            show_notification: false,
            ..Default::default()
        };
        let compactor = AutoCompactor::new(provider, model, config);

        // Should return None even when above threshold
        let notif = compactor.get_notification(110000, 128000);
        assert!(notif.is_none());
    }

    #[test]
    fn test_compactor_state() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        assert_eq!(compactor.state(), CompactorState::Idle);
        assert!(!compactor.is_compacting());
    }

    #[test]
    fn test_compaction_count() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        assert_eq!(compactor.compaction_count(), 0);

        // Simulate incrementing count
        *compactor.compaction_count.write() += 1;
        assert_eq!(compactor.compaction_count(), 1);
    }

    #[test]
    fn test_reset() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        // Queue some messages
        compactor.queue_for_compaction(AgentMessage::user("Test".to_string()));
        compactor.queue_for_compaction(AgentMessage::assistant("Response".to_string()));

        // Reset
        compactor.reset();

        assert_eq!(compactor.pending_count(), 0);
        assert_eq!(compactor.state(), CompactorState::Idle);
        assert_eq!(compactor.compaction_count(), 0);
    }

    #[test]
    fn test_compacted_context() {
        let ctx = CompactedContext::new("Test summary".to_string(), 10, 500);

        assert_eq!(ctx.compacted_count, 10);
        assert_eq!(ctx.tokens_saved, 500);
        assert_eq!(ctx.summary, "Test summary");
    }

    #[test]
    fn test_compaction_reason_display() {
        assert_eq!(CompactionReason::Manual.to_string(), "manual");
        assert_eq!(CompactionReason::Automatic.to_string(), "automatic");
        assert_eq!(CompactionReason::Overflow.to_string(), "overflow");
        assert_eq!(
            CompactionReason::Iteration {
                current: 5,
                every_n: 10
            }
            .to_string(),
            "iteration 5/10"
        );
    }

    #[test]
    fn test_notification_levels() {
        let info = CompactionNotification::info("Test info");
        assert_eq!(info.level, NotificationLevel::Info);
        assert!(!info.can_cancel);

        let warning = CompactionNotification::warning("Test warning", true);
        assert_eq!(warning.level, NotificationLevel::Warning);
        assert!(warning.can_cancel);

        let compacting = CompactionNotification::compacting(100000, 128000, 0.78);
        assert_eq!(compacting.level, NotificationLevel::Info);
        assert!(compacting.can_cancel);
        assert!(compacting.message.contains("78%"));
    }

    #[test]
    fn test_agent_message_creation() {
        let user_msg = AgentMessage::user("Hello".to_string());
        assert_eq!(user_msg.role, "user");
        assert_eq!(user_msg.content, "Hello");
        assert!(user_msg.timestamp > 0);

        let assistant_msg = AgentMessage::assistant("Hi there".to_string());
        assert_eq!(assistant_msg.role, "assistant");
        assert_eq!(assistant_msg.content, "Hi there");
    }

    #[test]
    fn test_agent_message_serialization() {
        let msg = AgentMessage::user("Test content".to_string());
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, "Test content");
        assert_eq!(parsed.role, "user");
    }

    #[test]
    fn test_multiple_flush_is_empty() {
        let provider = create_test_provider();
        let model = create_test_model();
        let compactor = AutoCompactor::with_defaults(provider, model);

        // First flush should be empty
        let messages = compactor.flush_queue();
        assert!(messages.is_empty());

        // Second flush should also be empty
        let messages = compactor.flush_queue();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_config_custom_threshold() {
        let provider = create_test_provider();
        let model = create_test_model();
        let config = CompactionConfig {
            threshold: 0.6, // 60% threshold
            ..Default::default()
        };
        let compactor = AutoCompactor::new(provider, model, config);

        // At 60%, should trigger
        assert!(compactor.should_compact(76800, 128000)); // 60%

        // At 59%, should not trigger
        assert!(!compactor.should_compact(75500, 128000)); // 59%
    }
}
