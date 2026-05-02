//! Context compaction for long conversations
//!
//! This module provides functionality to compact conversation history when it
//! becomes too large, using the LLM itself to summarize older messages.

use crate::error::{Error, ProviderError};
use crate::high_level::complete;
use crate::{
    Api, AssistantMessage, Context, Model, Message, Provider, StreamOptions, 
    TextContent, ContentBlock,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Result of context compaction
#[derive(Debug, Clone)]
pub struct CompactedContext {
    /// Summary of the compacted messages
    pub summary: String,
    /// Messages that were kept (typically recent ones)
    pub kept_messages: Vec<Message>,
    /// Number of messages that were compacted
    pub compacted_count: usize,
}

impl CompactedContext {
    /// Create a new compacted context
    pub fn new(summary: String, kept_messages: Vec<Message>, compacted_count: usize) -> Self {
        Self {
            summary,
            kept_messages,
            compacted_count,
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
        }
    }
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        // Default: compact at 80% context usage
        CompactionStrategy::Threshold(0.8)
    }
}

/// Trait for context compaction implementations
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Compact messages, returning a summary and kept messages
    ///
    /// # Arguments
    /// * `messages` - The messages to compact
    /// * `instruction` - Optional custom instruction for the summarizer
    ///
    /// # Returns
    /// A `CompactedContext` containing the summary and kept messages
    async fn compact(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> Result<CompactedContext>;
}

/// LLM-based compactor that uses the model itself to summarize
pub struct LlmCompactor {
    model: Model,
    #[allow(dead_code)]
    provider: Arc<dyn Provider>,
    /// How many recent messages to always keep (not compacted)
    keep_recent: usize,
    /// Maximum number of old messages to summarize in one batch
    max_batch: usize,
}

impl LlmCompactor {
    /// Create a new LLM compactor
    pub fn new(model: Model, provider: Arc<dyn Provider>) -> Self {
        Self {
            model,
            provider,
            keep_recent: 4,
            max_batch: 20,
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

    /// Build the summarization prompt
    fn build_summarize_prompt(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> String {
        let mut prompt = String::new();
        
        prompt.push_str("Summarize the following conversation concisely. ");
        prompt.push_str("Capture the key points, decisions, and any ongoing tasks or context.\n\n");
        
        if let Some(instr) = instruction {
            prompt.push_str(&format!("Focus areas: {}\n\n", instr));
        }
        
        prompt.push_str("## Conversation to summarize:\n");
        
        for (i, msg) in messages.iter().enumerate() {
            let role = match msg {
                Message::User(_) => "User",
                Message::Assistant(_) => "Assistant",
                Message::ToolResult(_) => "Tool",
            };
            let content = msg.text_content().unwrap_or_default();
            let content_preview = if content.len() > 500 {
                format!("{}...", &content[..500])
            } else {
                content
            };
            prompt.push_str(&format!("[{} {}]: {}\n", role, i + 1, content_preview));
        }
        
        prompt.push_str("\n## Summary:\n");
        prompt.push_str("Provide a concise summary that captures the essence of this conversation.");
        
        prompt
    }
}

#[async_trait]
impl Compactor for LlmCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> Result<CompactedContext> {
        if messages.len() <= self.keep_recent {
            return Ok(CompactedContext::new(
                String::new(),
                messages.to_vec(),
                0,
            ));
        }

        // Split into old messages (to compact) and recent messages (to keep)
        let keep_count = self.keep_recent.min(messages.len());
        let old_messages = &messages[..messages.len() - keep_count];
        let recent_messages: Vec<Message> = messages[messages.len() - keep_count..].to_vec();

        if old_messages.is_empty() {
            return Ok(CompactedContext::new(
                String::new(),
                messages.to_vec(),
                0,
            ));
        }

        // Build summarization context
        let prompt = self.build_summarize_prompt(old_messages, instruction);
        
        let mut context = Context::new();
        context.set_system_prompt(
            "You are a helpful assistant that summarizes conversations concisely."
        );
        context.add_message(Message::User(crate::UserMessage::new(prompt)));
        
        let options = StreamOptions {
            temperature: Some(0.3), // Lower temperature for summarization
            max_tokens: Some(1024),
            ..Default::default()
        };

        // Call LLM to get summary
        let summary_message = complete(&self.model, &context, Some(options))
            .await
            .map_err(|e| Error::Provider(ProviderError::StreamError(e.to_string())))?;
        
        let summary = summary_message.text_content();
        
        // Create summary message to insert before kept messages
        let mut summary_msg = AssistantMessage::new(
            Api::AnthropicMessages,
            "compactor",
            &self.model.id,
        );
        summary_msg.content = vec![
            ContentBlock::Text(TextContent::new(format!(
                "[Previous conversation summarized: {}]",
                summary
            ))),
        ];

        // Build final compacted context
        let mut kept = vec![Message::Assistant(summary_msg)];
        kept.extend(recent_messages);

        Ok(CompactedContext::new(
            summary,
            kept,
            old_messages.len(),
        ))
    }
}

/// Context manager that handles compaction automatically
pub struct CompactionManager {
    strategy: CompactionStrategy,
    compactor: Option<Arc<dyn Compactor>>,
    context_window: usize,
}

impl CompactionManager {
    /// Create a new compaction manager
    pub fn new(strategy: CompactionStrategy, context_window: usize) -> Self {
        Self {
            strategy,
            compactor: None,
            context_window,
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
        self.strategy.should_compact(context_tokens, self.context_window, iteration)
    }

    /// Get the current strategy
    pub fn strategy(&self) -> &CompactionStrategy {
        &self.strategy
    }

    /// Compact the given messages if appropriate
    pub async fn compact_if_needed(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
        context_tokens: usize,
        iteration: usize,
    ) -> Result<Option<CompactedContext>> {
        if !self.should_compact(context_tokens, iteration) {
            return Ok(None);
        }

        let compactor = match &self.compactor {
            Some(c) => c,
            None => return Ok(None),
        };

        let result = compactor.compact(messages, instruction).await?;
        Ok(Some(result))
    }
}

impl Default for CompactionManager {
    fn default() -> Self {
        Self::new(CompactionStrategy::default(), 128_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_strategy_disabled() {
        let strategy = CompactionStrategy::Disabled;
        assert!(!strategy.should_compact(100_000, 128_000, 5));
        assert!(!strategy.should_compact(120_000, 128_000, 10));
    }

    #[test]
    fn test_compaction_strategy_threshold() {
        let strategy = CompactionStrategy::Threshold(0.8);
        
        // Below threshold
        assert!(!strategy.should_compact(100_000, 128_000, 1));
        
        // At threshold
        assert!(strategy.should_compact(102_400, 128_000, 1));
        
        // Above threshold
        assert!(strategy.should_compact(120_000, 128_000, 1));
    }

    #[test]
    fn test_compaction_strategy_every_n_turns() {
        let strategy = CompactionStrategy::EveryNTurns(5);
        
        // Before threshold iterations
        assert!(!strategy.should_compact(0, 128_000, 3));
        assert!(!strategy.should_compact(0, 128_000, 4));
        
        // At threshold iterations
        assert!(strategy.should_compact(0, 128_000, 5));
        assert!(strategy.should_compact(0, 128_000, 10));
        assert!(strategy.should_compact(0, 128_000, 15));
        
        // Not at threshold
        assert!(!strategy.should_compact(0, 128_000, 6));
    }

    #[test]
    fn test_compacted_context() {
        let ctx = CompactedContext::new(
            "Test summary".to_string(),
            vec![],
            10,
        );
        
        assert_eq!(ctx.summary(), "Test summary");
        assert_eq!(ctx.kept_count(), 0);
        assert_eq!(ctx.compacted_count(), 10);
    }
}
