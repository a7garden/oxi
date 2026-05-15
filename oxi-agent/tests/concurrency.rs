//! Concurrency tests for SharedState and ToolRegistry.
//! Verifies thread-safety of shared mutable state under concurrent access.

use oxi_agent::state::SharedState;
use oxi_agent::tools::ToolRegistry;
use oxi_agent::AgentTool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::thread;
use tokio::sync::oneshot;

// ── Helper: simple tool for testing ──────────────────────────────

struct TestTool {
    name: String,
}

impl TestTool {
    fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }
}

#[async_trait]
impl AgentTool for TestTool {
    fn name(&self) -> &str { &self.name }
    fn label(&self) -> &str { "Test Tool" }
    fn description(&self) -> &str { "A test tool" }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<oxi_agent::AgentToolResult, String> {
        Ok(oxi_agent::AgentToolResult::success("test result"))
    }
}

// ═══════════════════════════════════════════════════════════════════
// SharedState Concurrent Reads
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_shared_state_concurrent_reads() {
    let shared = Arc::new(SharedState::new());

    // Pre-populate with messages
    shared.update(|s| {
        for i in 0..50 {
            s.add_user_message(format!("Message {}", i));
        }
    });

    let mut handles = Vec::new();

    for thread_id in 0..16 {
        let shared_clone = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            // Each thread reads the state multiple times
            for _ in 0..100 {
                let state = shared_clone.get_state();
                assert_eq!(state.messages.len(), 50, "thread {} sees all messages", thread_id);
                assert_eq!(state.iteration, 0);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }
}

#[test]
fn test_shared_state_concurrent_writes() {
    let shared = Arc::new(SharedState::new());
    let message_count = 100;

    let mut handles = Vec::new();

    for thread_id in 0..8 {
        let shared_clone = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for i in 0..message_count {
                shared_clone.update(|s| {
                    s.add_user_message(format!("Thread {} msg {}", thread_id, i));
                });
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    let state = shared.get_state();
    assert_eq!(
        state.messages.len(),
        8 * message_count,
        "all messages from all threads should be present"
    );
}

#[test]
fn test_shared_state_concurrent_read_write() {
    let shared = Arc::new(SharedState::new());

    // Pre-populate
    shared.update(|s| {
        s.add_user_message("Initial".to_string());
    });

    let mut handles = Vec::new();

    // Writers
    for i in 0..4 {
        let shared_clone = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for j in 0..50 {
                shared_clone.update(|s| {
                    s.add_user_message(format!("Writer {} msg {}", i, j));
                });
            }
        }));
    }

    // Readers
    for _ in 0..4 {
        let shared_clone = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let state = shared_clone.get_state();
                // Should always see at least the initial message
                assert!(!state.messages.is_empty());
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    let state = shared.get_state();
    // 1 initial + 4 writers * 50 messages = 201
    assert_eq!(state.messages.len(), 201);
}

#[test]
fn test_shared_state_concurrent_usage_tracking() {
    let shared = Arc::new(SharedState::new());

    let mut handles = Vec::new();
    for _thread_id in 0..8 {
        let shared_clone = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                shared_clone.update(|s| {
                    s.record_usage(10, 5);
                });
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    let state = shared.get_state();
    assert_eq!(state.input_tokens, 8 * 100 * 10);
    assert_eq!(state.output_tokens, 8 * 100 * 5);
    assert_eq!(state.total_tokens, 8 * 100 * 15);
}

#[test]
fn test_shared_state_concurrent_reset() {
    let shared = Arc::new(SharedState::new());

    shared.update(|s| {
        s.add_user_message("Before reset".to_string());
    });

    let shared_clone = Arc::clone(&shared);
    let resetter = thread::spawn(move || {
        shared_clone.reset();
    });

    resetter.join().expect("reset thread");
    let state = shared.get_state();
    assert_eq!(state.messages.len(), 0);
}

// ═══════════════════════════════════════════════════════════════════
// ToolRegistry Concurrent Access
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_tool_registry_concurrent_access() {
    let registry = Arc::new(ToolRegistry::new());
    let mut handles = Vec::new();

    // Concurrent registrations
    for i in 0..20 {
        let reg_clone = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            reg_clone.register(TestTool::new(&format!("tool_{}", i)));
        }));
    }

    for handle in handles {
        handle.join().expect("register thread should not panic");
    }

    // Verify all tools were registered
    let names = registry.names();
    assert_eq!(names.len(), 20, "all 20 tools should be registered");

    // Verify each tool can be retrieved
    for i in 0..20 {
        let tool = registry.get(&format!("tool_{}", i));
        assert!(tool.is_some(), "tool_{} should be registered", i);
    }

    // Now do concurrent lookups
    let mut lookup_handles = Vec::new();
    for i in 0..20 {
        let reg_clone = Arc::clone(&registry);
        lookup_handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let tool = reg_clone.get(&format!("tool_{}", i));
                assert!(tool.is_some(), "concurrent lookup for tool_{}", i);
                assert_eq!(tool.unwrap().name(), format!("tool_{}", i));
            }
        }));
    }

    for handle in lookup_handles {
        handle.join().expect("lookup thread should not panic");
    }
}

#[test]
fn test_tool_registry_concurrent_register_and_lookup() {
    let registry = Arc::new(ToolRegistry::new());

    // Start with a few tools
    registry.register(TestTool::new("initial_1"));
    registry.register(TestTool::new("initial_2"));

    let mut handles = Vec::new();

    // Some threads register new tools
    for i in 0..10 {
        let reg_clone = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            reg_clone.register(TestTool::new(&format!("concurrent_{}", i)));
        }));
    }

    // Some threads look up existing tools
    for _ in 0..10 {
        let reg_clone = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            // Should always be able to find initial tools
            let tool = reg_clone.get("initial_1");
            assert!(tool.is_some());
            let tool = reg_clone.get("initial_2");
            assert!(tool.is_some());
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // Final check: all tools should be present
    let names = registry.names();
    assert!(names.contains(&"initial_1".to_string()));
    assert!(names.contains(&"initial_2".to_string()));
    for i in 0..10 {
        assert!(
            names.contains(&format!("concurrent_{}", i)),
            "concurrent_{} should be registered",
            i
        );
    }
}

#[test]
fn test_tool_registry_concurrent_unregister() {
    let registry = Arc::new(ToolRegistry::new());

    // Register tools
    for i in 0..20 {
        registry.register(TestTool::new(&format!("tool_{}", i)));
    }

    let mut handles = Vec::new();

    // Concurrent unregistrations
    for i in 0..10 {
        let reg_clone = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            let removed = reg_clone.unregister(&format!("tool_{}", i));
            assert!(removed, "tool_{} should be removed", i);
        }));
    }

    // Concurrent lookups for non-removed tools
    for i in 10..20 {
        let reg_clone = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            let tool = reg_clone.get(&format!("tool_{}", i));
            assert!(tool.is_some(), "tool_{} should still exist", i);
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // Verify final state
    let names = registry.names();
    for i in 0..10 {
        assert!(!names.contains(&format!("tool_{}", i)), "tool_{} should be gone", i);
    }
    for i in 10..20 {
        assert!(names.contains(&format!("tool_{}", i)), "tool_{} should remain", i);
    }
}

#[test]
fn test_tool_registry_definitions_concurrent() {
    let registry = Arc::new(ToolRegistry::new());

    for i in 0..10 {
        registry.register(TestTool::new(&format!("def_tool_{}", i)));
    }

    let mut handles = Vec::new();
    for _ in 0..8 {
        let reg_clone = Arc::clone(&registry);
        handles.push(thread::spawn(move || {
            let defs = reg_clone.definitions();
            assert!(defs.len() >= 10);
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }
}

// ═══════════════════════════════════════════════════════════════════
// AgentState Thread Safety
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_agent_state_iteration_concurrent() {
    let shared = Arc::new(SharedState::new());

    let mut handles = Vec::new();
    for _ in 0..8 {
        let shared_clone = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                shared_clone.update(|s| {
                    s.increment_iteration();
                });
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    let state = shared.get_state();
    assert_eq!(state.iteration, 800, "8 threads × 100 increments = 800");
}

#[test]
fn test_agent_state_clear_and_build() {
    let shared = Arc::new(SharedState::new());

    shared.update(|s| {
        s.add_user_message("msg1".to_string());
        s.add_assistant_message("resp1".to_string());
        s.add_user_message("msg2".to_string());
    });

    assert_eq!(shared.get_state().messages.len(), 3);

    shared.reset();
    assert_eq!(shared.get_state().messages.len(), 0);

    shared.update(|s| {
        s.add_user_message("after reset".to_string());
    });
    assert_eq!(shared.get_state().messages.len(), 1);
}

#[test]
fn test_agent_state_is_complete_concurrent() {
    let shared = Arc::new(SharedState::new());

    assert!(!shared.get_state().is_complete());

    shared.update(|s| {
        s.set_stop_reason(oxi_agent::types::StopReason::Stop);
    });

    assert!(shared.get_state().is_complete());

    // Reset should clear it
    shared.reset();
    assert!(!shared.get_state().is_complete());
}
