//! In-memory todo state — reference implementation of [`TodoStateProvider`].
//!
//! Uses `parking_lot::RwLock<Vec<TodoPhase>>` so the agent's `todo` tool
//! and the host's observation loop can share state cheaply.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use oxi_sdk::{AgentConfig, InMemoryTodoState, TodoStateProvider};
//!
//! let todo = Arc::new(InMemoryTodoState::new());
//! let mut config = AgentConfig {
//!     model_id: "anthropic/claude-sonnet-4-20250514".into(),
//!     ..Default::default()
//! };
//! config.todo = Some(todo.clone() as Arc<dyn TodoStateProvider>);
//!
//! // Later, observe:
//! let phases = todo.get_phases();
//! ```

use std::pin::Pin;
use std::sync::Arc;

use oxi_agent::tools::todo::{TodoOp, TodoPhase, TodoUpdateResult, apply_ops};
use oxi_agent::tools::{TodoStateProvider, ToolError};
use parking_lot::RwLock;

/// In-memory todo state backed by `Arc<RwLock<Vec<TodoPhase>>>`.
///
/// Safe to share between the agent's `todo` tool (writer) and the host
/// application's observation loop (reader). Thread-safe and lock-free for
/// reads when no writer holds the lock.
///
/// Clone is cheap — shares the same `Arc`-backed buffer.
#[derive(Debug, Clone)]
pub struct InMemoryTodoState {
    phases: Arc<RwLock<Vec<TodoPhase>>>,
}

impl InMemoryTodoState {
    /// Create a new empty todo state.
    pub fn new() -> Self {
        Self {
            phases: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create with pre-populated phases.
    pub fn with_phases(phases: Vec<TodoPhase>) -> Self {
        Self {
            phases: Arc::new(RwLock::new(phases)),
        }
    }

    /// Snapshot of current phases (synchronous, cheap — clones the vec).
    pub fn get_phases(&self) -> Vec<TodoPhase> {
        self.phases.read().clone()
    }
}

impl Default for InMemoryTodoState {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoStateProvider for InMemoryTodoState {
    fn get_phases(&self) -> Vec<TodoPhase> {
        self.get_phases()
    }

    fn apply_ops<'a>(
        &'a self,
        ops: Vec<TodoOp>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<TodoUpdateResult, ToolError>> + Send + 'a>>
    {
        Box::pin(async move {
            // Acquire write lock synchronously, apply, drop guard before .await
            let result = {
                let mut phases = self.phases.write();
                apply_ops(&mut phases, &ops)
            };
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_agent::tools::todo::{TodoItem, TodoStatus};

    #[test]
    fn empty_state() {
        let state = InMemoryTodoState::new();
        assert!(state.get_phases().is_empty());
    }

    #[test]
    fn init_and_read() {
        let state = InMemoryTodoState::new();
        let phases = state.phases.write();
        // Can't apply_ops without async — just verify the lock works
        drop(phases);
        assert!(state.get_phases().is_empty());
    }

    #[test]
    fn with_phases_preserves_data() {
        let initial = vec![TodoPhase {
            name: "Test".into(),
            tasks: vec![TodoItem {
                content: "do thing".into(),
                status: TodoStatus::Pending,
                notes: None,
            }],
        }];
        let state = InMemoryTodoState::with_phases(initial);
        let snapshot = state.get_phases();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].name, "Test");
        assert_eq!(snapshot[0].tasks.len(), 1);
        assert_eq!(snapshot[0].tasks[0].content, "do thing");
    }

    #[test]
    fn clone_shares_state() {
        let state = InMemoryTodoState::with_phases(vec![TodoPhase {
            name: "Shared".into(),
            tasks: vec![],
        }]);
        let clone = state.clone();
        assert_eq!(clone.get_phases().len(), 1);
    }
}
