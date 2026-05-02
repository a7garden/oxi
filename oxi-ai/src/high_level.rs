//! High-level API for oxi-ai
//!
//! Provides convenient functions for common LLM interactions.

use futures::StreamExt;
use crate::{
    Context, Model, StreamOptions, ProviderEvent, AssistantMessage, 
    ContentBlock, TextContent, ToolCall,
};
use crate::error::{Error, ProviderError};

/// High-level complete function that collects all streaming events
/// and returns the final assistant message.
///
/// # Arguments
/// * `model` - The model to use
/// * `context` - The conversation context
/// * `options` - Optional streaming options
///
/// # Returns
/// The final assistant message containing all content blocks
pub async fn complete(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> std::result::Result<AssistantMessage, Error> {
    use crate::providers::stream;
    
    let mut stream = stream(model, context, options).await?;
    
    let mut final_message: Option<AssistantMessage> = None;
    let mut text_buffer = String::new();
    let mut current_text_index: Option<usize> = None;
    let mut tool_calls: Vec<(usize, ToolCall)> = Vec::new();
    
    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::Start { partial } => {
                final_message = Some(partial);
            }
            ProviderEvent::TextStart { content_index, partial } => {
                if final_message.is_none() {
                    final_message = Some(partial);
                }
                current_text_index = Some(content_index);
                text_buffer.clear();
            }
            ProviderEvent::TextDelta { delta, content_index, .. } => {
                text_buffer.push_str(&delta);
                if current_text_index != Some(content_index) {
                    // New text block started
                    if let Some(idx) = current_text_index {
                        // Save previous text block
                        if !text_buffer.is_empty() {
                            push_text_block(&mut final_message, idx, &text_buffer);
                        }
                    }
                    current_text_index = Some(content_index);
                    text_buffer.clear();
                }
                text_buffer.push_str(&delta);
            }
            ProviderEvent::TextEnd { content_index, content, .. } => {
                push_text_block(&mut final_message, content_index, &content);
            }
            ProviderEvent::ThinkingStart { content_index: _, partial } => {
                if final_message.is_none() {
                    final_message = Some(partial);
                }
            }
            ProviderEvent::ThinkingDelta { delta, content_index, .. } => {
                // Append thinking content
                if let Some(ref mut msg) = final_message {
                    // Find or create thinking block
                    let content = ContentBlock::Thinking(crate::ThinkingContent {
                        content_type: crate::ThinkingContentType::Thinking,
                        thinking: delta,
                        thinking_signature: None,
                        redacted: None,
                    });
                    if content_index >= msg.content.len() {
                        msg.content.push(content);
                    }
                }
            }
            ProviderEvent::ThinkingEnd { content_index, content, .. } => {
                if let Some(ref mut msg) = final_message {
                    let thinking = ContentBlock::Thinking(crate::ThinkingContent {
                        content_type: crate::ThinkingContentType::Thinking,
                        thinking: content,
                        thinking_signature: None,
                        redacted: None,
                    });
                    if content_index >= msg.content.len() {
                        msg.content.push(thinking);
                    }
                }
            }
            ProviderEvent::ToolCallStart { content_index, partial } => {
                if final_message.is_none() {
                    final_message = Some(partial);
                }
                // Initialize tool call
                let tc = ToolCall {
                    content_type: crate::ToolCallType::ToolCall,
                    id: format!("tool_call_{}", content_index),
                    name: String::new(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                };
                tool_calls.push((content_index, tc));
            }
            ProviderEvent::ToolCallDelta { delta, content_index, .. } => {
                // Accumulate tool call arguments
                if let Some((_, tc)) = tool_calls.iter_mut().find(|(idx, _)| *idx == content_index) {
                    // Parse the accumulated args
                    let current_args = tc.arguments.to_string() + &delta;
                    if let Ok(parsed) = serde_json::from_str(&current_args) {
                        tc.arguments = parsed;
                    }
                }
            }
            ProviderEvent::ToolCallEnd { content_index, tool_call, .. } => {
                // Update or add tool call
                if let Some((_, tc)) = tool_calls.iter_mut().find(|(idx, _)| *idx == content_index) {
                    *tc = tool_call.clone();
                }
                // Add to final message content
                push_tool_call(&mut final_message, content_index, tool_call.clone());
            }
            ProviderEvent::Done { message, .. } => {
                // Finalize any remaining text
                if let Some(idx) = current_text_index {
                    if !text_buffer.is_empty() {
                        push_text_block(&mut final_message, idx, &text_buffer);
                    }
                }
                
                // Add any pending tool calls
                for (content_index, tc) in &tool_calls {
                    push_tool_call(&mut final_message, *content_index, tc.clone());
                }
                
                final_message = Some(message);
                break;
            }
            ProviderEvent::Error { error, .. } => {
                return Err(Error::Provider(ProviderError::StreamError(error.error_message.unwrap_or_else(|| "Unknown error".to_string()))));
            }
        }
    }
    
    final_message.ok_or_else(|| Error::Provider(ProviderError::StreamError("Stream ended without message".to_string())))
}

/// Push a text block to the message content
fn push_text_block(msg: &mut Option<AssistantMessage>, index: usize, text: &str) {
    if let Some(ref mut m) = msg {
        let content = ContentBlock::Text(TextContent {
            content_type: crate::TextContentType::Text,
            text: text.to_string(),
        });
        
        // Ensure the content array is large enough
        while m.content.len() <= index {
            m.content.push(ContentBlock::Text(TextContent {
                content_type: crate::TextContentType::Text,
                text: String::new(),
            }));
        }
        
        // Append text to existing block
        if let ContentBlock::Text(t) = &mut m.content[index] {
            if t.text.is_empty() {
                *t = TextContent::new(text);
            } else {
                t.text.push_str(text);
            }
        } else {
            m.content[index] = content;
        }
    }
}

/// Push a tool call block to the message content
fn push_tool_call(msg: &mut Option<AssistantMessage>, index: usize, tool_call: ToolCall) {
    if let Some(ref mut m) = msg {
        while m.content.len() <= index {
            m.content.push(ContentBlock::Text(TextContent::new("")));
        }
        m.content[index] = ContentBlock::ToolCall(tool_call);
    }
}

/// Token estimation utilities
pub mod tokens {
    /// Estimate token count based on character count.
    ///
    /// This is a rough approximation. The actual ratio varies by
    /// model and text content, but typically 1 token ≈ 4 characters
    /// for English text.
    ///
    /// # Arguments
    /// * `text` - The text to estimate tokens for
    ///
    /// # Returns
    /// Estimated token count
    pub fn estimate(text: &str) -> usize {
        text.len() / 4
    }
    
    /// Estimate tokens more accurately based on word count.
    ///
    /// Average English word is ~4.5 characters plus a space = ~5 tokens per word.
    /// English text averages about 1.3 tokens per word.
    ///
    /// # Arguments
    /// * `text` - The text to estimate tokens for
    ///
    /// # Returns
    /// Estimated token count
    pub fn estimate_words(text: &str) -> usize {
        let word_count = text.split_whitespace().count();
        (word_count as f64 * 1.3) as usize
    }
    
    /// Calculate context length usage percentage.
    ///
    /// # Arguments
    /// * `text` - The text to measure
    /// * `context_window` - The model's context window size
    ///
    /// # Returns
    /// Percentage of context window used (0.0 to 1.0)
    pub fn context_usage(text: &str, context_window: usize) -> f64 {
        if context_window == 0 {
            return 0.0;
        }
        (estimate(text) as f64 / context_window as f64).min(1.0)
    }
}

// Re-export estimate_tokens as the main function
pub use tokens::estimate as estimate_tokens;