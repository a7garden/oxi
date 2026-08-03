//! `memory_retain` tool — persist a memory item to the backend.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};

/// Valid memory kinds accepted by [`MemoryRetainTool`].
const VALID_KINDS: [&str; 4] = ["fact", "preference", "context", "summary"];

/// Tool that persists a memory item (content, kind, importance) to the
/// configured `MemoryBackend`.
///
/// Requires `ctx.memory` to be set; otherwise returns an error. The memory
/// is scoped to the current session (`ctx.session_id`), falling back to
/// `"default"` when no session id is available.
pub struct MemoryRetainTool;

#[async_trait]
impl AgentTool for MemoryRetainTool {
    fn name(&self) -> &str {
        "memory_retain"
    }

    fn label(&self) -> &str {
        "Memory Retain"
    }

    fn description(&self) -> &str {
        "Store a piece of information to long-term memory for later recall. \
         Use for facts, preferences, context, or summaries worth remembering \
         across sessions."
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The text to remember."
                },
                "kind": {
                    "type": "string",
                    "enum": ["fact", "preference", "context", "summary"],
                    "default": "fact",
                    "description": "Category of the memory."
                },
                "importance": {
                    "type": "number",
                    "minimum": 0.0,
                    "maximum": 1.0,
                    "default": 0.5,
                    "description": "How important this memory is (0–1)."
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let backend = ctx.memory.as_ref().ok_or("Memory not configured")?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: content")?;

        let kind = params
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("fact");
        if !VALID_KINDS.contains(&kind) {
            return Err(format!(
                "Invalid kind '{}': expected one of {:?}",
                kind, VALID_KINDS
            ));
        }

        // `importance` is validated for forward-compatibility; the current
        // `MemoryBackend::put` signature does not persist it.
        if let Some(importance) = params.get("importance").and_then(|v| v.as_f64())
            && !(0.0..=1.0).contains(&importance)
        {
            return Err(format!(
                "importance must be between 0 and 1, got {}",
                importance
            ));
        }

        let subject = ctx.session_id.as_deref().unwrap_or("default");
        backend.put(content, kind, subject).await?;

        Ok(AgentToolResult::success(format!(
            "Retained [{}] to memory.",
            kind
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::MemoryBackend;
    use parking_lot::Mutex;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    /// Records every `put` call; the remaining trait methods are stubbed.
    #[derive(Debug)]
    struct MockMemory {
        puts: Mutex<Vec<(String, String, String)>>,
    }

    impl MockMemory {
        fn new() -> Self {
            Self {
                puts: Mutex::new(vec![]),
            }
        }
    }

    impl MemoryBackend for MockMemory {
        fn put<'a>(
            &'a self,
            content: &'a str,
            kind: &'a str,
            subject: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
            self.puts
                .lock()
                .push((content.into(), kind.into(), subject.into()));
            Box::pin(async move { Ok("mem-1".to_string()) })
        }

        fn search<'a>(
            &'a self,
            _query: &'a str,
            _k: usize,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<crate::tools::MemoryItem>, ToolError>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(vec![]) })
        }

        fn list<'a>(
            &'a self,
            _subject: &'a str,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<crate::tools::MemoryItem>, ToolError>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(vec![]) })
        }

        fn delete<'a>(
            &'a self,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
    }

    #[tokio::test]
    async fn retain_calls_put_with_correct_args() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default()
            .with_session("sess-42")
            .with_memory(mock.clone());
        let result = MemoryRetainTool
            .execute(
                "c1",
                json!({"content": "hello", "kind": "fact", "importance": 0.9}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "Retained [fact] to memory.");
        let puts = mock.puts.lock();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, "hello");
        assert_eq!(puts[0].1, "fact");
        assert_eq!(puts[0].2, "sess-42");
    }

    #[tokio::test]
    async fn retain_defaults_kind_to_fact() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let result = MemoryRetainTool
            .execute("c1", json!({"content": "x"}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(result.output, "Retained [fact] to memory.");
        assert_eq!(mock.puts.lock()[0].1, "fact");
    }

    #[tokio::test]
    async fn retain_uses_default_subject_without_session() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        MemoryRetainTool
            .execute("c1", json!({"content": "x"}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(mock.puts.lock()[0].2, "default");
    }

    #[tokio::test]
    async fn retain_errors_when_memory_not_configured() {
        let ctx = ToolContext::default();
        let err = MemoryRetainTool
            .execute("c1", json!({"content": "x"}), None, &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "Memory not configured");
    }

    #[tokio::test]
    async fn retain_rejects_invalid_kind() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let err = MemoryRetainTool
            .execute("c1", json!({"content": "x", "kind": "bogus"}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("Invalid kind"));
    }

    #[tokio::test]
    async fn retain_rejects_out_of_range_importance() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let err = MemoryRetainTool
            .execute("c1", json!({"content": "x", "importance": 1.5}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("importance"));
    }

    #[tokio::test]
    async fn retain_rejects_missing_content() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let err = MemoryRetainTool
            .execute("c1", json!({"kind": "fact"}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("content"));
    }
}
