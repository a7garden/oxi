//! `memory_edit` tool — update or delete a memory item.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use crate::tools::typed::TypedTool;
use serde_json::{Value, json};

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};

/// Tool that updates an existing memory item or deletes it when no new content
/// is supplied.
///
/// Requires `ctx.memory` to be set; otherwise returns an error. When `content`
/// is provided the item is updated via `MemoryBackend::put`; when `content`
/// is absent the item is deleted via `MemoryBackend::delete`.
#[derive(Deserialize, JsonSchema)]
pub struct MemoryEditArgs {
    id: String,
    content: Option<String>,
    kind: Option<String>,
    subject: Option<String>,
}

#[allow(missing_docs)]
pub struct MemoryEditTool;

#[async_trait]
impl AgentTool for MemoryEditTool {
    fn name(&self) -> &str {
        "memory_edit"
    }

    fn label(&self) -> &str {
        "Memory Edit"
    }

    fn description(&self) -> &str {
        "Update or delete a memory item. \
         Provide a new `content` to update the item, or omit it to delete."
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The ID of the memory item to edit."
                },
                "content": {
                    "type": "string",
                    "description": "New content for the memory item. If omitted, the item is deleted."
                },
                "kind": {
                    "type": "string",
                    "description": "Category of the memory (only used when updating)."
                },
                "subject": {
                    "type": "string",
                    "description": "Subject scope for the memory (only used when updating)."
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: MemoryEditArgs = serde_json::from_value(params)
            .map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, ctx).await
    }
}

#[async_trait]
impl TypedTool for MemoryEditTool {
    type Args = MemoryEditArgs;

    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let backend = ctx.memory.as_ref().ok_or("Memory not configured")?;
        if let Some(content) = &args.content {
            let kind = args.kind.as_deref().unwrap_or("fact");
            let subject = args.subject.as_deref().or(ctx.session_id.as_deref()).unwrap_or("default");
            backend.put(content, kind, subject).await?;
            Ok(AgentToolResult::success(format!("Updated memory {} (kind: {}).", args.id, kind)))
        } else {
            backend.delete(&args.id).await?;
            Ok(AgentToolResult::success(format!("Deleted memory {}.", args.id)))
        }
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

    /// Records every `put` and `delete` call.
    #[derive(Debug)]
    struct MockMemory {
        puts: Mutex<Vec<(String, String, String)>>,
        deletes: Mutex<Vec<String>>,
    }

    impl MockMemory {
        fn new() -> Self {
            Self {
                puts: Mutex::new(vec![]),
                deletes: Mutex::new(vec![]),
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
            Box::pin(async move { Ok("new-id".to_string()) })
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
            id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
            self.deletes.lock().push(id.into());
            Box::pin(async move { Ok(()) })
        }
    }

    #[tokio::test]
    async fn edit_update_calls_put_with_correct_args() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default()
            .with_session("sess-42")
            .with_memory(mock.clone());
        let result = MemoryEditTool
            .execute(
                "c1",
                json!({"id": "mem-1", "content": "updated", "kind": "fact"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(
            result.output,
            "Updated memory mem-1 (kind: fact)."
        );
        let puts = mock.puts.lock();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, "updated");
        assert_eq!(puts[0].1, "fact");
        assert_eq!(puts[0].2, "sess-42");
        assert_eq!(mock.deletes.lock().len(), 0);
    }

    #[tokio::test]
    async fn edit_delete_calls_delete() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let result = MemoryEditTool
            .execute("c1", json!({"id": "mem-1"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "Deleted memory mem-1.");
        assert_eq!(mock.deletes.lock().len(), 1);
        assert_eq!(mock.deletes.lock()[0], "mem-1");
        assert_eq!(mock.puts.lock().len(), 0);
    }

    #[tokio::test]
    async fn edit_update_defaults_kind_to_fact() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        MemoryEditTool
            .execute("c1", json!({"id": "mem-1", "content": "x"}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(mock.puts.lock()[0].1, "fact");
    }

    #[tokio::test]
    async fn edit_update_uses_default_subject_without_session() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        MemoryEditTool
            .execute("c1", json!({"id": "mem-1", "content": "x"}), None, &ctx)
            .await
            .unwrap();
        assert_eq!(mock.puts.lock()[0].2, "default");
    }

    #[tokio::test]
    async fn edit_update_uses_explicit_subject() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default()
            .with_session("sess-42")
            .with_memory(mock.clone());
        MemoryEditTool
            .execute(
                "c1",
                json!({"id": "mem-1", "content": "x", "subject": "custom-scope"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(mock.puts.lock()[0].2, "custom-scope");
    }

    #[tokio::test]
    async fn edit_errors_when_memory_not_configured() {
        let ctx = ToolContext::default();
        let err = MemoryEditTool
            .execute("c1", json!({"id": "mem-1"}), None, &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "Memory not configured");
    }

    #[tokio::test]
    async fn edit_rejects_missing_id() {
        let mock = Arc::new(MockMemory::new());
        let ctx = ToolContext::default().with_memory(mock.clone());
        let err = MemoryEditTool
            .execute("c1", json!({"content": "x"}), None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("id"));
    }
}
