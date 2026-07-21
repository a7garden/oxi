//! `memory_reflect` tool — persist a session summary to memory.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};

/// Tool that stores a session reflection/summary to the configured
/// `MemoryBackend`.
///
/// When a `summary` is supplied it is persisted as a `summary` memory scoped
/// to the current session. Without one the tool returns a placeholder —
/// automatic LLM summarisation is a future enhancement.
///
/// Requires `ctx.memory` to be set; otherwise returns an error.
pub struct MemoryReflectTool;

#[async_trait]
impl AgentTool for MemoryReflectTool {
    fn name(&self) -> &str {
        "memory_reflect"
    }

    fn label(&self) -> &str {
        "Memory Reflect"
    }

    fn description(&self) -> &str {
        "Save a session summary or reflection to long-term memory. \
         A non-empty `summary` is required."
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Session summary to persist to long-term memory."
                }
            },
            "required": ["summary"]
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

        let subject = ctx.session_id.as_deref().unwrap_or("default");

        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or("missing required parameter: summary")?;
        if summary.trim().is_empty() {
            return Err("summary must not be empty".into());
        }
        backend.put(summary, "summary", subject).await?;
        Ok(AgentToolResult::success(format!(
            "Reflected session summary to memory (subject: {}).",
            subject
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
    async fn reflect_with_summary_persists() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default()
            .with_session("s1")
            .with_memory(mock.clone());
        let result = MemoryReflectTool
            .execute("c1", json!({"summary": "did X"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Reflected session summary"));
        let puts = mock.puts.lock();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, "did X");
        assert_eq!(puts[0].1, "summary");
        assert_eq!(puts[0].2, "s1");
    }

    #[tokio::test]
    async fn reflect_without_summary_errors() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let err = MemoryReflectTool
            .execute("c1", json!({}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("summary"));
        assert!(mock.puts.lock().is_empty());
    }

    #[tokio::test]
    async fn reflect_rejects_empty_summary() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let err = MemoryReflectTool
            .execute("c1", json!({"summary": "   "}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("empty"));
        assert!(mock.puts.lock().is_empty());
    }

    #[tokio::test]
    async fn reflect_errors_when_memory_not_configured() {
        let ctx = ToolContext::default();
        let err = MemoryReflectTool
            .execute("c1", json!({"summary": "x"}), None, &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "Memory not configured");
    }
}
