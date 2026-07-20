//! `memory_recall` tool — search the memory backend for relevant items.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use crate::tools::typed::TypedTool;
use super::{AgentTool, AgentToolResult, MemoryItem, ToolContext, ToolError};

/// Default number of results returned by [`MemoryRecallTool`] when `limit` is omitted.
#[allow(dead_code)]
const DEFAULT_LIMIT: usize = 5;
/// Maximum number of results [`MemoryRecallTool`] will return.
const MAX_LIMIT: usize = 20;

/// Tool that searches the configured `MemoryBackend` for memories matching
/// a query and returns the matches in a compact, model-friendly format.
#[derive(Deserialize, JsonSchema)]
pub struct MemoryRecallArgs {
    query: String,
    #[serde(default = "default_mem_limit")]
    limit: u64,
}

fn default_mem_limit() -> u64 { 5 }

///
/// Requires `ctx.memory` to be set; otherwise returns an error.
pub struct MemoryRecallTool;

#[async_trait]
impl AgentTool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }

    fn label(&self) -> &str {
        "Memory Recall"
    }

    fn description(&self) -> &str {
        "Search long-term memory for information relevant to a query. \
         Returns the most relevant stored memories (facts, preferences, \
         context, summaries)."
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for in memory."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "default": 5,
                    "description": "Maximum number of results to return."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: MemoryRecallArgs = serde_json::from_value(params)
            .map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, ctx).await
    }
}

#[async_trait]
impl TypedTool for MemoryRecallTool {
    type Args = MemoryRecallArgs;

    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let backend = ctx.memory.as_ref().ok_or("Memory not configured")?;
        let limit = (args.limit as usize).clamp(1, MAX_LIMIT);
        let results = backend.search(&args.query, limit).await?;
        Ok(AgentToolResult::success(format_results(&results)))
    }
}

/// Format memory search results into a compact, model-friendly string.
fn format_results(items: &[MemoryItem]) -> String {
    if items.is_empty() {
        return "No matching memories found.".to_string();
    }
    let mut out = format!(
        "Found {} memor{}:\n\n",
        items.len(),
        if items.len() == 1 { "y" } else { "ies" }
    );
    for (i, item) in items.iter().enumerate() {
        out.push_str(&format!("{}. [{}] {}\n", i + 1, item.kind, item.content));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::MemoryBackend;
    use parking_lot::Mutex;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    /// Returns canned search results and records the requested `k`.
    #[derive(Debug)]
    struct MockMemory {
        items: Vec<MemoryItem>,
        last_k: Mutex<Option<usize>>,
    }

    impl MemoryBackend for MockMemory {
        fn put<'a>(
            &'a self,
            _content: &'a str,
            _kind: &'a str,
            _subject: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
            Box::pin(async move { Ok("mem-1".to_string()) })
        }

        fn search<'a>(
            &'a self,
            _query: &'a str,
            k: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
            *self.last_k.lock() = Some(k);
            let items: Vec<MemoryItem> = self.items.iter().take(k).cloned().collect();
            Box::pin(async move { Ok(items) })
        }

        fn list<'a>(
            &'a self,
            _subject: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
            Box::pin(async move { Ok(vec![]) })
        }

        fn delete<'a>(
            &'a self,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
    }

    fn make_item(id: &str, kind: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.into(),
            kind: kind.into(),
            content: content.into(),
            subject: "s".into(),
        }
    }

    #[tokio::test]
    async fn recall_returns_formatted_results() {
        let mock = Arc::new(MockMemory {
            items: vec![
                make_item("1", "fact", "Rust is fast"),
                make_item("2", "preference", "Likes dark mode"),
            ],
            last_k: Mutex::new(None),
        });
        let ctx = ToolContext::default().with_memory(mock.clone());
        let result = MemoryRecallTool
            .execute("c1", json!({"query": "rust", "limit": 5}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("[fact] Rust is fast"));
        assert!(result.output.contains("[preference] Likes dark mode"));
        assert_eq!(*mock.last_k.lock(), Some(5));
    }

    #[tokio::test]
    async fn recall_reports_empty_results() {
        let mock = Arc::new(MockMemory {
            items: vec![],
            last_k: Mutex::new(None),
        });
        let ctx = ToolContext::default().with_memory(mock);
        let result = MemoryRecallTool
            .execute("c1", json!({"query": "nothing"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "No matching memories found.");
    }

    #[tokio::test]
    async fn recall_uses_default_limit() {
        let mock = Arc::new(MockMemory {
            items: vec![],
            last_k: Mutex::new(None),
        });
        let ctx = ToolContext::default().with_memory(mock.clone());
        MemoryRecallTool
            .execute("c1", json!({"query": "x"}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(*mock.last_k.lock(), Some(DEFAULT_LIMIT));
    }

    #[tokio::test]
    async fn recall_clamps_oversized_limit() {
        let mock = Arc::new(MockMemory {
            items: vec![],
            last_k: Mutex::new(None),
        });
        let ctx = ToolContext::default().with_memory(mock.clone());
        MemoryRecallTool
            .execute("c1", json!({"query": "x", "limit": 100}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(*mock.last_k.lock(), Some(MAX_LIMIT));
    }

    #[tokio::test]
    async fn recall_clamps_zero_limit() {
        let mock = Arc::new(MockMemory {
            items: vec![],
            last_k: Mutex::new(None),
        });
        let ctx = ToolContext::default().with_memory(mock.clone());
        MemoryRecallTool
            .execute("c1", json!({"query": "x", "limit": 0}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(*mock.last_k.lock(), Some(1));
    }

    #[tokio::test]
    async fn recall_errors_when_memory_not_configured() {
        let ctx = ToolContext::default();
        let err = MemoryRecallTool
            .execute("c1", json!({"query": "x"}), None, &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "Memory not configured");
    }

    #[tokio::test]
    async fn recall_rejects_missing_query() {
        let mock = Arc::new(MockMemory {
            items: vec![],
            last_k: Mutex::new(None),
        });
        let ctx = ToolContext::default().with_memory(mock);
        let err = MemoryRecallTool
            .execute("c1", json!({"limit": 3}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("query"));
    }
}
