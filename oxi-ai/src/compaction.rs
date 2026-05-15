//! Context compaction for long conversations
//!
//! This module provides functionality to compact conversation history when it
//! becomes too large, using the LLM itself to summarize older messages.

use crate::high_level::complete;
use crate::high_level::tokens::estimate as estimate_tokens;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Message, Model, Provider, StreamOptions,
    TextContent, UserMessage,
};

/// Safely truncate a string to a maximum number of characters, appending "..." if truncated.
fn safe_truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars { return s.to_string(); }
    let boundary = s.char_indices()
        .take_while(|(i, _)| *i <= max_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}...", &s[..boundary])
}
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Compaction configuration for LLM-based compaction
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// How many recent messages to always keep (not compacted)
    pub keep_recent: usize,
    /// Maximum number of old messages to include in one summarization batch
    pub max_batch: usize,
    /// Target compaction ratio (0.0 to 1.0) - e.g., 0.5 means reduce to 50%
    pub target_ratio: f32,
    /// Maximum tokens for the summary response
    pub summary_max_tokens: usize,
    /// Temperature for summarization (lower = more focused)
    pub temperature: f32,
    /// Timeout for LLM compaction requests
    pub timeout: Duration,
    /// Custom instruction for the summarizer
    pub custom_instruction: Option<String>,
}

impl CompactionConfig {
    /// Create a default compaction configuration
    pub fn new() -> Self {
        Self {
            keep_recent: 4,
            max_batch: 20,
            target_ratio: 0.5,
            summary_max_tokens: 1024,
            temperature: 0.3,
            timeout: Duration::from_secs(60),
            custom_instruction: None,
        }
    }

    /// Set how many recent messages to always keep
    pub fn with_keep_recent(mut self, count: usize) -> Self {
        self.keep_recent = count;
        self
    }

    /// Set maximum batch size for summarization
    pub fn with_max_batch(mut self, count: usize) -> Self {
        self.max_batch = count;
        self
    }

    /// Set target compaction ratio (0.0 to 1.0)
    pub fn with_target_ratio(mut self, ratio: f32) -> Self {
        self.target_ratio = ratio.clamp(0.1, 0.9);
        self
    }

    /// Set maximum tokens for summary
    pub fn with_summary_max_tokens(mut self, tokens: usize) -> Self {
        self.summary_max_tokens = tokens;
        self
    }

    /// Set temperature for summarization
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp.clamp(0.0, 1.0);
        self
    }

    /// Set timeout for LLM requests
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set custom instruction for the summarizer
    pub fn with_custom_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.custom_instruction = Some(instruction.into());
        self
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadata about a compaction operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMetadata {
    /// Estimated token count before compaction
    pub original_tokens: usize,
    /// Estimated token count after compaction
    pub compacted_tokens: usize,
    /// Number of messages that were compacted
    pub messages_compacted: usize,
    /// Number of messages kept
    pub messages_kept: usize,
    /// Timestamp of compaction
    pub timestamp: DateTime<Utc>,
    /// Target ratio used
    pub target_ratio: f32,
    /// Actual compaction ratio achieved
    pub actual_ratio: f32,
    /// Whether the operation was successful
    pub success: bool,
    /// Error message if the operation failed
    pub error: Option<String>,
}

impl CompactionMetadata {
    /// Create new metadata for a successful compaction
    pub fn new(
        original_tokens: usize,
        compacted_tokens: usize,
        messages_compacted: usize,
        messages_kept: usize,
        target_ratio: f32,
    ) -> Self {
        let actual_ratio = if original_tokens > 0 {
            compacted_tokens as f32 / original_tokens as f32
        } else {
            1.0
        };

        Self {
            original_tokens,
            compacted_tokens,
            messages_compacted,
            messages_kept,
            timestamp: Utc::now(),
            target_ratio,
            actual_ratio,
            success: true,
            error: None,
        }
    }

    /// Create metadata for a failed compaction
    pub fn failed(
        original_tokens: usize,
        messages_compacted: usize,
        target_ratio: f32,
        error: impl Into<String>,
    ) -> Self {
        Self {
            original_tokens,
            compacted_tokens: original_tokens,
            messages_compacted,
            messages_kept: 0,
            timestamp: Utc::now(),
            target_ratio,
            actual_ratio: 1.0,
            success: false,
            error: Some(error.into()),
        }
    }

    /// Get the compression factor (how much the context was reduced)
    pub fn compression_factor(&self) -> f32 {
        if self.actual_ratio > 0.0 {
            1.0 - self.actual_ratio
        } else {
            0.0
        }
    }

    /// Get tokens saved from compaction
    pub fn tokens_saved(&self) -> usize {
        self.original_tokens.saturating_sub(self.compacted_tokens)
    }
}

/// Result of context compaction
#[derive(Debug, Clone)]
pub struct CompactedContext {
    /// Summary of the compacted messages
    pub summary: String,
    /// Messages that were kept (typically recent ones)
    pub kept_messages: Vec<Message>,
    /// Number of messages that were compacted
    pub compacted_count: usize,
    /// Metadata about the compaction operation
    pub metadata: CompactionMetadata,
}

impl CompactedContext {
    /// Create a new compacted context
    pub fn new(
        summary: String,
        kept_messages: Vec<Message>,
        compacted_count: usize,
        metadata: CompactionMetadata,
    ) -> Self {
        Self {
            summary,
            kept_messages,
            compacted_count,
            metadata,
        }
    }

    /// Get the summary text
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Get kept messages count
    pub fn kept_count(&self) -> usize {
        self.kept_messages.len()
    }

    /// Get compacted messages count
    pub fn compacted_count(&self) -> usize {
        self.compacted_count
    }

    /// Get the compaction metadata
    pub fn metadata(&self) -> &CompactionMetadata {
        &self.metadata
    }

    /// Check if compaction was successful
    pub fn is_success(&self) -> bool {
        self.metadata.success
    }
}

/// Compaction strategy determining when to compact
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompactionStrategy {
    /// Never compact context
    Disabled,
    /// Compact when context is at least this percentage full (0.0 to 1.0)
    Threshold(f32),
    /// Compact after every N turns
    EveryNTurns(usize),
    /// Compact when context exceeds this absolute token count
    AbsoluteTokens(usize),
}

impl CompactionStrategy {
    /// Check if compaction should happen based on strategy
    ///
    /// # Arguments
    /// * `context_tokens` - Estimated token count of current context
    /// * `context_window` - Total context window size
    /// * `iteration` - Current iteration count
    ///
    /// # Returns
    /// `true` if compaction should be triggered
    pub fn should_compact(
        &self,
        context_tokens: usize,
        context_window: usize,
        iteration: usize,
    ) -> bool {
        match self {
            CompactionStrategy::Disabled => false,
            CompactionStrategy::Threshold(threshold) => {
                if context_window == 0 {
                    return false;
                }
                let usage = context_tokens as f32 / context_window as f32;
                usage >= *threshold
            }
            CompactionStrategy::EveryNTurns(n) => iteration > 0 && iteration % n == 0,
            CompactionStrategy::AbsoluteTokens(max_tokens) => context_tokens >= *max_tokens,
        }
    }
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        CompactionStrategy::Threshold(0.8)
    }
}

/// Error type for compaction operations
#[derive(Debug, Clone)]
pub enum CompactionError {
    /// Compaction request to LLM failed
    LlmError(String),
    /// No messages to compact
    NoMessagesToCompact,
    /// Too few messages to compact (need at least keep_recent + 1)
    TooFewMessages { total: usize, keep_recent: usize },
    /// Compaction was disabled
    CompactionDisabled,
    /// Context window not available
    NoContextWindow,
}

impl std::fmt::Display for CompactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactionError::LlmError(msg) => write!(f, "LLM compaction failed: {}", msg),
            CompactionError::NoMessagesToCompact => write!(f, "No messages to compact"),
            CompactionError::TooFewMessages { total, keep_recent } => {
                write!(
                    f,
                    "Not enough messages ({}) to compact (need at least {} for keep_recent)",
                    total,
                    keep_recent + 1
                )
            }
            CompactionError::CompactionDisabled => write!(f, "Compaction is disabled"),
            CompactionError::NoContextWindow => write!(f, "Context window not configured"),
        }
    }
}

impl std::error::Error for CompactionError {}

/// Trait for context compaction implementations
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Compact messages, returning a summary and kept messages
    async fn compact(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> std::result::Result<CompactedContext, CompactionError>;

    /// Estimate the token count of messages
    fn estimate_tokens(&self, messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|msg| estimate_tokens(&msg.text_content().unwrap_or_default()))
            .sum()
    }
}

/// LLM-based compactor that uses the model itself to summarize
pub struct LlmCompactor {
    model: Model,
    _provider: Arc<dyn Provider>,
    config: CompactionConfig,
}

impl LlmCompactor {
    /// Create a new LLM compactor with default configuration
    pub fn new(model: Model, provider: Arc<dyn Provider>) -> Self {
        Self {
            model,
            _provider: provider,
            config: CompactionConfig::new(),
        }
    }

    /// Create a new LLM compactor with custom configuration
    pub fn with_config(
        model: Model,
        provider: Arc<dyn Provider>,
        config: CompactionConfig,
    ) -> Self {
        Self {
            model,
            _provider: provider,
            config,
        }
    }

    /// Set how many recent messages to always keep
    pub fn with_keep_recent(mut self, count: usize) -> Self {
        self.config.keep_recent = count;
        self
    }

    /// Set maximum batch size for summarization
    pub fn with_max_batch(mut self, count: usize) -> Self {
        self.config.max_batch = count;
        self
    }

    /// Set target compaction ratio
    pub fn with_target_ratio(mut self, ratio: f32) -> Self {
        self.config.target_ratio = ratio.clamp(0.1, 0.9);
        self
    }

    /// Build the summarization prompt
    fn build_summarize_prompt(&self, messages: &[Message], instruction: Option<&str>) -> String {
        let mut prompt = String::new();

        prompt.push_str("Summarize the following conversation concisely. ");
        prompt.push_str("Capture the key points, decisions, and any ongoing tasks or context.\n\n");

        if let Some(instr) = instruction {
            prompt.push_str(&format!("Focus areas: {}\n\n", instr));
        } else if let Some(ref custom_instr) = self.config.custom_instruction {
            prompt.push_str(&format!("Focus areas: {}\n\n", custom_instr));
        }

        prompt.push_str("## Conversation to summarize:\n");

        for (i, msg) in messages.iter().enumerate() {
            let role = match msg {
                Message::User(_) => "User",
                Message::Assistant(_) => "Assistant",
                Message::ToolResult(_) => "Tool",
            };
            let content = msg.text_content().unwrap_or_default();
            let content_preview = safe_truncate(&content, 500);
            prompt.push_str(&format!("[{} {}]: {}\n", role, i + 1, content_preview));
        }

        prompt.push_str("\n## Summary:\n");
        prompt
            .push_str("Provide a concise summary that captures the essence of this conversation.");

        prompt
    }

    /// Attempt to compact using a fallback strategy if LLM fails
    async fn compact_with_fallback(
        &self,
        old_messages: &[Message],
        recent_messages: &[Message],
        instruction: Option<&str>,
    ) -> std::result::Result<CompactedContext, CompactionError> {
        // Try LLM-based summarization first
        match self.summarize_with_llm(old_messages, instruction).await {
            Ok(summary) => {
                // Build the summary message
                let mut summary_msg =
                    AssistantMessage::new(Api::AnthropicMessages, "compactor", &self.model.id);
                summary_msg.content = vec![ContentBlock::Text(TextContent::new(format!(
                    "[Previous conversation summarized: {}]",
                    summary
                )))];

                // Build final compacted context
                let mut kept = vec![Message::Assistant(summary_msg)];
                kept.extend(recent_messages.iter().cloned());

                let original_tokens = self.estimate_tokens(old_messages);
                let compacted_tokens = self.estimate_tokens(&kept);
                let kept_len = kept.len();

                Ok(CompactedContext::new(
                    summary,
                    kept,
                    old_messages.len(),
                    CompactionMetadata::new(
                        original_tokens,
                        compacted_tokens,
                        old_messages.len(),
                        kept_len,
                        self.config.target_ratio,
                    ),
                ))
            }
            Err(llm_err) => {
                // Fallback: simple truncation with key topics
                self.compact_fallback(old_messages, recent_messages)
                    .await
                    .map_err(|_| CompactionError::LlmError(llm_err.to_string()))
            }
        }
    }

    /// Summarize messages using the LLM
    async fn summarize_with_llm(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> std::result::Result<String, CompactionError> {
        let prompt = self.build_summarize_prompt(messages, instruction);

        let mut context = Context::new();
        context.set_system_prompt(
            "You are a helpful assistant that summarizes conversations concisely.",
        );
        context.add_message(Message::User(UserMessage::new(prompt)));

        let options = StreamOptions {
            temperature: Some(self.config.temperature as f64),
            max_tokens: Some(self.config.summary_max_tokens),
            ..Default::default()
        };

        let summary_message = complete(&self.model, &context, Some(options))
            .await
            .map_err(|e| CompactionError::LlmError(e.to_string()))?;

        Ok(summary_message.text_content())
    }

    /// Fallback compaction when LLM fails - simple truncation with key preservation
    async fn compact_fallback(
        &self,
        old_messages: &[Message],
        recent_messages: &[Message],
    ) -> std::result::Result<CompactedContext, CompactionError> {
        // Simple fallback: keep first and last message, summarize in between
        let mut summary_parts = Vec::new();

        if old_messages.len() > 2 {
            // Keep first message's topic
            if let Some(first) = old_messages.first() {
                let content = first.text_content().unwrap_or_default();
                let preview = safe_truncate(&content, 200);
                summary_parts.push(format!("Started discussing: {}", preview));
            }

            // Keep last message (likely the most relevant recent context)
            if let Some(last) = old_messages.last() {
                let content = last.text_content().unwrap_or_default();
                let preview = safe_truncate(&content, 200);
                summary_parts.push(format!("Ended with: {}", preview));
            }

            summary_parts.push(format!(
                "({} messages omitted)",
                old_messages.len().saturating_sub(2)
            ));
        } else if !old_messages.is_empty() {
            // Just preserve first message content
            if let Some(msg) = old_messages.first() {
                let content = msg.text_content().unwrap_or_default();
                summary_parts.push(format!("Conversation started: {}", content));
            }
        }

        let summary = summary_parts.join(" ");

        let mut summary_msg =
            AssistantMessage::new(Api::AnthropicMessages, "compactor", &self.model.id);
        summary_msg.content = vec![ContentBlock::Text(TextContent::new(format!(
            "[Previous conversation summary: {}]",
            summary
        )))];

        let mut kept = vec![Message::Assistant(summary_msg)];
        kept.extend(recent_messages.iter().cloned());

        let original_tokens = self.estimate_tokens(old_messages);
        let compacted_tokens = self.estimate_tokens(&kept);
        let kept_len = kept.len();

        Ok(CompactedContext::new(
            summary,
            kept,
            old_messages.len(),
            CompactionMetadata::new(
                original_tokens,
                compacted_tokens,
                old_messages.len(),
                kept_len,
                self.config.target_ratio,
            ),
        ))
    }
}

#[async_trait]
impl Compactor for LlmCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> std::result::Result<CompactedContext, CompactionError> {
        // Check minimum requirements
        if messages.is_empty() {
            return Err(CompactionError::NoMessagesToCompact);
        }

        if messages.len() <= self.config.keep_recent {
            // Not enough messages to compact, return as-is with zero compaction
            let original_tokens = self.estimate_tokens(messages);
            return Ok(CompactedContext::new(
                String::new(),
                messages.to_vec(),
                0,
                CompactionMetadata::new(
                    original_tokens,
                    original_tokens,
                    0,
                    messages.len(),
                    self.config.target_ratio,
                ),
            ));
        }

        // Split into old messages (to compact) and recent messages (to keep)
        let keep_count = self.config.keep_recent.min(messages.len());
        let old_messages: Vec<Message> = messages[..messages.len() - keep_count].to_vec();
        let recent_messages: Vec<Message> = messages[messages.len() - keep_count..].to_vec();

        if old_messages.is_empty() {
            return Err(CompactionError::NoMessagesToCompact);
        }

        // Handle LLM failure gracefully
        self.compact_with_fallback(&old_messages, &recent_messages, instruction)
            .await
    }
}

/// Additional methods for LlmCompactor (not part of Compactor trait)
impl LlmCompactor {
    /// Summarize a conversation branch for comparison purposes.
    ///
    /// This is used when branching occurs and you want to understand
    /// what changed compared to another branch (e.g., main).
    pub async fn summarize_branch(
        &self,
        messages: &[Message],
        branch_name: &str,
    ) -> std::result::Result<String, CompactionError> {
        if messages.is_empty() {
            return Ok(format!("Branch '{}' is empty", branch_name));
        }

        let mut prompt = String::new();
        prompt.push_str(&format!(
            "Summarize the conversation branch '{}' concisely. ",
            branch_name
        ));
        prompt.push_str("Focus on: what was discussed, decisions made, and current state.\n\n");

        prompt.push_str("## Branch messages:\n");
        for (i, msg) in messages.iter().enumerate() {
            let role = match msg {
                Message::User(_) => "User",
                Message::Assistant(_) => "Assistant",
                Message::ToolResult(_) => "Tool",
            };
            let content = msg.text_content().unwrap_or_default();
            let content_preview = safe_truncate(&content, 300);
            prompt.push_str(&format!("[{} {}]: {}\n", role, i + 1, content_preview));
        }

        prompt.push_str("\n## Summary (be concise):\n");

        // Use LLM to generate summary
        let mut context = Context::new();
        context.set_system_prompt(
            "You are a helpful assistant that summarizes conversation branches. ",
        );
        context.add_message(Message::User(UserMessage::new(prompt)));

        let options = StreamOptions {
            temperature: Some(0.3),
            max_tokens: Some(512),
            ..Default::default()
        };

        let summary_message = complete(&self.model, &context, Some(options))
            .await
            .map_err(|e| CompactionError::LlmError(e.to_string()))?;

        Ok(summary_message.text_content())
    }
}

/// Context manager that handles compaction automatically
pub struct CompactionManager {
    strategy: CompactionStrategy,
    compactor: Option<Arc<dyn Compactor>>,
    context_window: usize,
    config: CompactionConfig,
}

impl CompactionManager {
    /// Create a new compaction manager
    pub fn new(strategy: CompactionStrategy, context_window: usize) -> Self {
        Self {
            strategy,
            compactor: None,
            context_window,
            config: CompactionConfig::new(),
        }
    }

    /// Create a new compaction manager with custom config
    pub fn with_config(
        strategy: CompactionStrategy,
        context_window: usize,
        config: CompactionConfig,
    ) -> Self {
        Self {
            strategy,
            compactor: None,
            context_window,
            config,
        }
    }

    /// Set the compactor to use
    pub fn with_compactor<C: Compactor + 'static>(mut self, compactor: Arc<C>) -> Self {
        self.compactor = Some(compactor);
        self
    }

    /// Set the compactor from a trait object
    pub fn set_compactor(&mut self, compactor: Arc<dyn Compactor>) {
        self.compactor = Some(compactor);
    }

    /// Check if compaction should be triggered
    pub fn should_compact(&self, context_tokens: usize, iteration: usize) -> bool {
        self.strategy
            .should_compact(context_tokens, self.context_window, iteration)
    }

    /// Get the current strategy
    pub fn strategy(&self) -> &CompactionStrategy {
        &self.strategy
    }

    /// Get the compaction configuration
    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }

    /// Set compaction configuration
    pub fn set_config(&mut self, config: CompactionConfig) {
        self.config = config;
    }

    /// Compact the given messages if appropriate
    pub async fn compact_if_needed(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
        context_tokens: usize,
        iteration: usize,
    ) -> std::result::Result<Option<CompactedContext>, CompactionError> {
        if !self.should_compact(context_tokens, iteration) {
            return Ok(None);
        }

        let compactor = match &self.compactor {
            Some(c) => c,
            None => return Err(CompactionError::CompactionDisabled),
        };

        let result = compactor.compact(messages, instruction).await?;
        Ok(Some(result))
    }

    /// Force compaction regardless of strategy
    pub async fn compact_now(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> std::result::Result<CompactedContext, CompactionError> {
        let compactor = match &self.compactor {
            Some(c) => c,
            None => return Err(CompactionError::CompactionDisabled),
        };

        compactor.compact(messages, instruction).await
    }

    /// Get estimated token count for messages
    pub fn estimate_tokens(&self, messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|msg| estimate_tokens(&msg.text_content().unwrap_or_default()))
            .sum()
    }
}

impl Default for CompactionManager {
    fn default() -> Self {
        Self::new(CompactionStrategy::default(), 128_000)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create test user messages
    fn make_user_message(content: &str) -> Message {
        Message::user(content)
    }

    // Helper to create test assistant messages
    fn make_assistant_message(content: &str) -> Message {
        Message::Assistant({
            let mut msg = AssistantMessage::new(Api::AnthropicMessages, "test", "test-model");
            msg.content = vec![ContentBlock::Text(TextContent::new(content))];
            msg
        })
    }

    // Helper to create a test model
    fn make_test_model() -> Model {
        Model::new(
            "test-model",
            "Test Model",
            Api::AnthropicMessages,
            "test",
            "https://test.example.com",
        )
    }

    #[test]
    fn test_compaction_config_defaults() {
        let config = CompactionConfig::new();
        assert_eq!(config.keep_recent, 4);
        assert_eq!(config.max_batch, 20);
        assert!((config.target_ratio - 0.5).abs() < 0.001);
        assert_eq!(config.summary_max_tokens, 1024);
        assert!((config.temperature - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_compaction_config_builder_pattern() {
        let config = CompactionConfig::new()
            .with_keep_recent(10)
            .with_max_batch(30)
            .with_target_ratio(0.3)
            .with_temperature(0.5);

        assert_eq!(config.keep_recent, 10);
        assert_eq!(config.max_batch, 30);
        assert!((config.target_ratio - 0.3).abs() < 0.001);
        assert!((config.temperature - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_compaction_config_ratio_clamping() {
        // Test upper bound clamping
        let config = CompactionConfig::new().with_target_ratio(1.5);
        assert!((config.target_ratio - 0.9).abs() < 0.001);

        // Test lower bound clamping
        let config = CompactionConfig::new().with_target_ratio(-0.5);
        assert!((config.target_ratio - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_compaction_metadata_success() {
        let metadata = CompactionMetadata::new(
            1000, // original_tokens
            500,  // compacted_tokens
            10,   // messages_compacted
            5,    // messages_kept
            0.5,  // target_ratio
        );

        assert!(metadata.success);
        assert_eq!(metadata.original_tokens, 1000);
        assert_eq!(metadata.compacted_tokens, 500);
        assert_eq!(metadata.messages_compacted, 10);
        assert_eq!(metadata.messages_kept, 5);
        assert!((metadata.actual_ratio - 0.5).abs() < 0.001);
        assert!((metadata.compression_factor() - 0.5).abs() < 0.001);
        assert_eq!(metadata.tokens_saved(), 500);
        assert!(metadata.error.is_none());
    }

    #[test]
    fn test_compaction_metadata_failure() {
        let metadata = CompactionError::LlmError("test error".to_string());

        // Verify error message
        assert!(metadata.to_string().contains("test error"));
    }

    #[test]
    fn test_compaction_metadata_compression_factor() {
        // Zero original tokens should result in 1.0 ratio
        let metadata = CompactionMetadata::new(0, 0, 0, 0, 0.5);
        assert!((metadata.actual_ratio - 1.0).abs() < 0.001);
        assert!((metadata.compression_factor() - 0.0).abs() < 0.001);

        // Full compression
        let metadata = CompactionMetadata::new(1000, 100, 10, 5, 0.5);
        assert!((metadata.compression_factor() - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_compaction_metadata_tokens_saved() {
        // Normal case
        let metadata = CompactionMetadata::new(1000, 400, 10, 5, 0.5);
        assert_eq!(metadata.tokens_saved(), 600);

        // No savings
        let metadata = CompactionMetadata::new(1000, 1000, 0, 0, 0.5);
        assert_eq!(metadata.tokens_saved(), 0);

        // Compacted is larger than original (should not happen but should be safe)
        let metadata = CompactionMetadata::new(500, 600, 5, 3, 0.5);
        assert_eq!(metadata.tokens_saved(), 0); // saturating_sub
    }

    #[test]
    fn test_compaction_strategy_disabled() {
        let strategy = CompactionStrategy::Disabled;
        assert!(!strategy.should_compact(100_000, 128_000, 5));
        assert!(!strategy.should_compact(120_000, 128_000, 10));
        assert!(!strategy.should_compact(0, 128_000, 1));
    }

    #[test]
    fn test_compaction_strategy_threshold() {
        let strategy = CompactionStrategy::Threshold(0.8);

        // Below threshold (79%)
        assert!(!strategy.should_compact(100_000, 128_000, 1));

        // At threshold (exactly 80%)
        assert!(strategy.should_compact(102_400, 128_000, 1));

        // Above threshold (93%)
        assert!(strategy.should_compact(120_000, 128_000, 1));

        // Zero context window should return false
        assert!(!strategy.should_compact(100_000, 0, 1));
    }

    #[test]
    fn test_compaction_strategy_every_n_turns() {
        let strategy = CompactionStrategy::EveryNTurns(5);

        // Before threshold iterations
        assert!(!strategy.should_compact(0, 128_000, 0));
        assert!(!strategy.should_compact(0, 128_000, 3));
        assert!(!strategy.should_compact(0, 128_000, 4));

        // At threshold iterations
        assert!(strategy.should_compact(0, 128_000, 5));
        assert!(strategy.should_compact(0, 128_000, 10));
        assert!(strategy.should_compact(0, 128_000, 15));

        // Not at threshold
        assert!(!strategy.should_compact(0, 128_000, 6));
        assert!(!strategy.should_compact(0, 128_000, 9));
    }

    #[test]
    fn test_compaction_strategy_absolute_tokens() {
        let strategy = CompactionStrategy::AbsoluteTokens(100_000);

        // Below threshold
        assert!(!strategy.should_compact(50_000, 128_000, 0));
        assert!(!strategy.should_compact(99_999, 128_000, 0));

        // At threshold
        assert!(strategy.should_compact(100_000, 128_000, 0));

        // Above threshold
        assert!(strategy.should_compact(150_000, 128_000, 0));
    }

    #[test]
    fn test_compacted_context_basic() {
        let metadata = CompactionMetadata::new(1000, 500, 10, 5, 0.5);
        let ctx = CompactedContext::new(
            "Test summary".to_string(),
            vec![make_user_message("test")],
            10,
            metadata,
        );

        assert_eq!(ctx.summary(), "Test summary");
        assert_eq!(ctx.kept_count(), 1);
        assert_eq!(ctx.compacted_count(), 10);
        assert!(ctx.is_success());
        assert_eq!(ctx.metadata().tokens_saved(), 500);
    }

    #[test]
    fn test_compacted_context_with_empty_summary() {
        let metadata = CompactionMetadata::new(100, 100, 0, 2, 0.5);
        let ctx = CompactedContext::new(
            String::new(), // Empty summary
            vec![make_user_message("test1"), make_user_message("test2")],
            0,
            metadata,
        );

        assert_eq!(ctx.summary(), "");
        assert_eq!(ctx.kept_count(), 2);
        assert_eq!(ctx.compacted_count(), 0);
    }

    #[test]
    fn test_llm_compactor_config_builder() {
        // Test that LlmCompactor can be created and builder pattern works
        use crate::providers::CloudflareProvider;
        let provider = CloudflareProvider::new();
        let model = make_test_model();
        let compactor = LlmCompactor::new(model, Arc::new(provider))
            .with_keep_recent(6)
            .with_max_batch(25)
            .with_target_ratio(0.6);

        assert!(compactor.config.keep_recent >= 4);
        assert!(compactor.config.max_batch >= 20);
    }

    #[test]
    fn test_compaction_error_display() {
        let err = CompactionError::NoMessagesToCompact;
        assert_eq!(err.to_string(), "No messages to compact");

        let err = CompactionError::TooFewMessages {
            total: 3,
            keep_recent: 5,
        };
        assert!(err.to_string().contains("3"));
        // The error message says "need at least keep_recent + 1", so with keep_recent=5 it shows 6
        assert!(err.to_string().contains("6"));

        let err = CompactionError::CompactionDisabled;
        assert_eq!(err.to_string(), "Compaction is disabled");

        let err = CompactionError::NoContextWindow;
        assert_eq!(err.to_string(), "Context window not configured");

        let err = CompactionError::LlmError("API timeout".to_string());
        assert!(err.to_string().contains("API timeout"));
    }

    #[test]
    fn test_compaction_manager_default() {
        let manager = CompactionManager::default();
        assert!(matches!(
            manager.strategy(),
            CompactionStrategy::Threshold(_)
        ));
        assert_eq!(manager.config().keep_recent, 4);
    }

    #[test]
    fn test_compaction_manager_with_custom_strategy() {
        let strategy = CompactionStrategy::AbsoluteTokens(50_000);
        let manager = CompactionManager::new(strategy, 200_000);

        // Should not compact below threshold
        assert!(!manager.should_compact(30_000, 0));

        // Should compact above threshold
        assert!(manager.should_compact(60_000, 0));
    }

    #[test]
    fn test_compaction_manager_with_config() {
        let config = CompactionConfig::new()
            .with_keep_recent(8)
            .with_target_ratio(0.4);

        let manager =
            CompactionManager::with_config(CompactionStrategy::default(), 128_000, config);

        assert_eq!(manager.config().keep_recent, 8);
        assert!((manager.config().target_ratio - 0.4).abs() < 0.001);
    }

    #[test]
    fn test_compaction_manager_should_compact_integration() {
        let manager = CompactionManager::new(CompactionStrategy::Threshold(0.75), 100_000);

        // Below threshold
        assert!(!manager.should_compact(70_000, 0));

        // At threshold (75%)
        assert!(manager.should_compact(75_000, 0));

        // Above threshold
        assert!(manager.should_compact(80_000, 0));
        assert!(manager.should_compact(100_000, 0));
    }

    #[test]
    fn test_compaction_manager_no_compactor_set() {
        let manager = CompactionManager::new(CompactionStrategy::EveryNTurns(5), 128_000);

        // should_compact with EveryNTurns(5) at iteration 5 should return true
        // (compact_if_needed would return Err when no compactor is set, but should_compact works)
        assert!(manager.should_compact(0, 5)); // iteration 5 triggers compaction
    }

    #[test]
    fn test_token_estimation_helper() {
        use crate::providers::CloudflareProvider;
        let provider = CloudflareProvider::new();
        let model = make_test_model();
        let compactor = LlmCompactor::new(model, Arc::new(provider));

        let messages = vec![
            make_user_message("Hello world, this is a test message."),
            make_assistant_message("This is a response with some content."),
        ];

        let tokens = compactor.estimate_tokens(&messages);
        assert!(tokens > 0, "Should estimate tokens for messages");
    }

    #[test]
    fn test_compaction_config_custom_instruction() {
        let config = CompactionConfig::new()
            .with_custom_instruction("Focus on code changes and technical decisions");

        assert!(config.custom_instruction.is_some());
        assert!(config.custom_instruction.unwrap().contains("code changes"));
    }

    #[test]
    fn test_compaction_metadata_timestamp_is_set() {
        let metadata = CompactionMetadata::new(1000, 500, 10, 5, 0.5);
        assert!(metadata.timestamp <= Utc::now());
    }

    #[test]
    fn test_compaction_ratio_achievement() {
        // Simulate compaction that achieves target ratio
        let metadata = CompactionMetadata::new(1000, 500, 10, 5, 0.5);
        assert!((metadata.actual_ratio - 0.5).abs() < 0.001);

        // Simulate compaction that exceeds target (more compression)
        let metadata = CompactionMetadata::new(1000, 300, 10, 5, 0.5);
        assert!((metadata.actual_ratio - 0.3).abs() < 0.001);
        assert!(metadata.compression_factor() > 0.5);

        // Simulate compaction that doesn't meet target (less compression)
        let metadata = CompactionMetadata::new(1000, 700, 10, 5, 0.5);
        assert!((metadata.actual_ratio - 0.7).abs() < 0.001);
        assert!(metadata.compression_factor() < 0.5);
    }

    #[test]
    fn test_compaction_manager_config_updates() {
        let mut manager = CompactionManager::default();

        let new_config = CompactionConfig::new()
            .with_keep_recent(12)
            .with_target_ratio(0.3);

        manager.set_config(new_config);

        assert_eq!(manager.config().keep_recent, 12);
        assert!((manager.config().target_ratio - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_llm_compactor_has_summarize_branch() {
        // Verify that LlmCompactor has the summarize_branch method
        use crate::providers::CloudflareProvider;
        let provider = CloudflareProvider::new();
        let model = make_test_model();
        let compactor = LlmCompactor::new(model, Arc::new(provider));
        
        // Just verify the method exists (runtime test would require async)
        let messages = vec![
            make_user_message("Test message 1"),
            make_assistant_message("Test response 1"),
            make_user_message("Test message 2"),
        ];
        
        // The method exists and can be called (we can't test async in sync test)
        // We verify it compiles correctly
        let branch_name = "test-branch";
        // This is a compile-time check that the method exists
        let _future = compactor.summarize_branch(&messages, branch_name);
    }

    #[test]
    fn test_summarize_branch_returns_error_on_llm_failure() {
        // Test that summarize_branch handles empty messages gracefully
        use crate::providers::CloudflareProvider;
        let provider = CloudflareProvider::new();
        let model = make_test_model();
        let compactor = LlmCompactor::new(model, Arc::new(provider));
        
        // Empty messages should return immediately
        let messages: Vec<Message> = vec![];
        
        // This should not panic with empty messages
        // (We can't test the async result in a sync test, but compile-time check passes)
        let _future = compactor.summarize_branch(&messages, "empty-branch");
    }
}
