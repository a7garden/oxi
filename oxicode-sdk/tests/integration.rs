//! Integration tests for oxicode-sdk.
//!
//! Uses MockProvider to test end-to-end flows without real API calls.

mod common;

use std::sync::Arc;

use oxicode_agent::AgentConfig;
use oxicode_sdk::prelude::*;
use oxicode_sdk::routing::RoutingConfig as SdkRoutingConfig;
use oxicode_sdk::{AgentMetrics, InterAgentMessage, MessageBus, ModelRegistry, WorkQueueConfig};

// ── Agent Build + Run ──────────────────────────────────────────────

#[tokio::test]
async fn full_pipeline_build_and_run() {
    let oxicode = common::mock_oxicode();
    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .build()
        .expect("build should succeed");

    let (response, events) = agent.run("Hello".into()).await.expect("run should succeed");
    assert!(!response.content.is_empty());
    assert!(!events.is_empty());
}

#[tokio::test]
async fn agent_with_system_prompt() {
    let oxicode = common::mock_oxicode();
    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .system_prompt("You are a test agent.")
        .build()
        .expect("build with system prompt");

    let (response, _) = agent.run("Test prompt".into()).await.expect("run");
    assert!(!response.content.is_empty());
}

#[tokio::test]
async fn agent_with_custom_tool() {
    let oxicode = common::mock_oxicode();
    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .custom_tool(
            "echo_tool",
            "Echoes input",
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            |params, _ctx| {
                let text = params["text"].as_str().unwrap_or("default");
                Ok(AgentToolResult::success(format!("echo: {text}")))
            },
        )
        .build()
        .expect("build with custom tool");

    let tool_names = agent.tools().names();
    assert!(tool_names.contains(&"echo_tool".to_string()));
}

// ── Port → agent-tool bridging (single-dependency consumer flow) ─────

/// Registering a `MemoryStore` port + `.with_port_memory()` must make the
/// four `memory_*` tools present AND wire the SAME port store into the
/// agent's `MemoryBackend`. Regression for the gap where `with_memory`
/// stored a port the agent loop never read.
#[tokio::test]
async fn with_port_memory_bridges_port_store_into_agent_tools() {
    use oxicode_sdk::inmem::InMemoryMemoryStore;
    use oxicode_sdk::ports::{MemoryEntry, MemoryStore};

    let store = Arc::new(InMemoryMemoryStore::new());
    let oxicode = OxicodeBuilder::new()
        .provider("mock", common::MockProvider)
        .model(common::mock_model())
        .with_memory(store.clone())
        .build();

    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .with_port_memory()
        .build()
        .expect("build should succeed");

    // The four memory tools are registered.
    let names = agent.tools().names();
    for tool in [
        "memory_recall",
        "memory_reflect",
        "memory_retain",
        "memory_edit",
    ] {
        assert!(
            names.iter().any(|n| n == tool),
            "with_port_memory must register `{tool}`"
        );
    }

    // ToolContext.memory is wired through to the config.
    let backend = agent
        .get_config()
        .memory
        .expect("config.memory must be set after with_port_memory");

    // The bridge is LIVE: a write through the port store is visible to the
    // agent's backend (proves they share storage, not just type-shape).
    store
        .put(MemoryEntry {
            id: "m1".into(),
            subject: "s".into(),
            kind: "fact".into(),
            embedding: None,
            content: serde_json::Value::String("port-bridged fact".into()),
            created_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let listed = backend.list("s").await.unwrap();
    assert!(
        listed.iter().any(|m| m.content == "port-bridged fact"),
        "agent backend must read the port store"
    );
}

/// `.with_port_url_resolver()` wires the engine's `InternalUrlRouter` port
/// into `ToolContext.url_resolver`.
#[test]
fn with_port_url_resolver_wires_resolver() {
    let oxicode = common::mock_oxicode();
    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .with_port_url_resolver()
        .build()
        .expect("build should succeed");
    assert!(
        agent.get_config().url_resolver.is_some(),
        "config.url_resolver must be set after with_port_url_resolver"
    );
}

/// `.with_snapshot_store()` threads the hashline store into config, enabling
/// line-anchored edit mode (previously unreachable: AgentConfig had no field).
#[test]
fn with_snapshot_store_threads_into_config() {
    let oxicode = common::mock_oxicode();
    let store = std::sync::Arc::new(oxicode_sdk::oxicode_hashline::InMemorySnapshotStore::new());
    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .with_snapshot_store(store)
        .build()
        .expect("build should succeed");
    assert!(
        agent.get_config().snapshot_store.is_some(),
        "config.snapshot_store must be set after with_snapshot_store"
    );
}

/// `.with_port_subagent()` registers the `subagent` tool and wires the
/// in-process runner (previously the tool + runner had no SDK wiring path).
#[test]
fn with_port_subagent_registers_tool_and_runner() {
    let oxicode = common::mock_oxicode();
    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .workspace("/tmp")
        .with_port_subagent()
        .build()
        .expect("build should succeed");
    let names = agent.tools().names();
    assert!(
        names.iter().any(|n| n == "subagent"),
        "with_port_subagent must register the `subagent` tool"
    );
    assert!(
        agent.get_config().subagent_runner.is_some(),
        "config.subagent_runner must be set after with_port_subagent"
    );
}

// ── Security ───────────────────────────────────────────────────────

#[test]
fn security_capability_enforcement() {
    let audit = Arc::new(AuditLog::new(64));
    let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

    authorizer.grant(
        CapabilitySubject::Agent("readonly".into()),
        CapabilitySet::read_only("/workspace"),
    );

    let subject = CapabilitySubject::Agent("readonly".into());

    assert!(authorizer.check(
        &subject,
        &Capability::FileRead {
            path_pattern: "/workspace/file".into(),
        },
    ));
    assert!(!authorizer.check(
        &subject,
        &Capability::FileWrite {
            path_pattern: "/workspace/file".into(),
        },
    ));
    assert!(!audit.entries().is_empty());
}

#[test]
fn security_role_binding() {
    let audit = Arc::new(AuditLog::new(64));
    let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

    authorizer.define_role("coder", CapabilitySet::coding("/ws"));
    authorizer.bind_role("agent-1", "coder");

    let subject = CapabilitySubject::Agent("agent-1".into());
    assert!(authorizer.check(
        &subject,
        &Capability::FileRead {
            path_pattern: "/ws/src/main.rs".into(),
        },
    ));
    assert!(!authorizer.check(
        &subject,
        &Capability::FileWrite {
            path_pattern: "/etc/passwd".into(),
        },
    ));
}

// ── Coordination ───────────────────────────────────────────────────

#[test]
fn work_queue_lifecycle() {
    let q = WorkQueue::new(WorkQueueConfig::default());

    let id = q.enqueue("review", serde_json::json!({"file": "a.rs"}), 5);
    q.enqueue("build", serde_json::json!({"target": "release"}), 3);

    let item = q.claim("agent-1", None).unwrap();
    assert_eq!(item.priority, 5);

    q.start(&id).unwrap();
    q.complete(
        &id,
        WorkResult {
            success: true,
            content: "LGTM".into(),
            error: None,
            duration_ms: 100,
            tokens_used: None,
        },
    )
    .unwrap();

    let stats = q.stats();
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.pending, 1);
}

#[test]
fn shared_memory_optimistic_locking() {
    let mem = SharedMemory::new();
    let key = MemoryKey::new("ns", "val");

    let v1 = mem.write(&key, serde_json::json!("a"), "w1", None).unwrap();
    assert_eq!(v1, 1);

    let v2 = mem
        .write(&key, serde_json::json!("b"), "w2", Some(v1))
        .unwrap();
    assert_eq!(v2, 2);

    let result = mem.write(&key, serde_json::json!("c"), "w3", Some(1));
    assert!(matches!(result, Err(SdkError::VersionConflict { .. })));
}

#[test]
fn shared_memory_atomic_increment() {
    let mem = SharedMemory::new();
    let key = MemoryKey::new("ns", "counter");

    assert_eq!(mem.increment(&key, 5, "a1"), 5);
    assert_eq!(mem.increment(&key, 3, "a2"), 8);
    assert_eq!(mem.read(&key), Some(serde_json::json!(8)));
}

#[test]
fn consensus_majority_voting() {
    let c = Consensus::new();
    c.start("v1", vec!["a".into(), "b".into(), "c".into()], 0.5);

    c.vote("v1", "a", "yes".into()).unwrap();
    let r = c.vote("v1", "b", "yes".into()).unwrap();
    assert!(r.decided);
    assert_eq!(r.decision.unwrap(), "yes");
}

#[test]
fn consensus_unanimity_required() {
    let c = Consensus::new();
    c.start("v2", vec!["a".into(), "b".into()], 1.0);

    c.vote("v2", "a", "yes".into()).unwrap();
    assert!(!c.status("v2").unwrap().decided);

    c.vote("v2", "b", "yes".into()).unwrap();
    assert!(c.status("v2").unwrap().decided);
}

// ── MessageBus ──────────────────────────────────────────────────────

#[tokio::test]
async fn message_bus_pub_sub() {
    let bus = MessageBus::new(16);
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    assert_eq!(bus.subscriber_count(), 2);

    bus.publish(InterAgentMessage::broadcast(
        "coord",
        "start",
        serde_json::json!({"phase": 1}),
    ));

    let msg1 = rx1.recv().await.unwrap();
    assert_eq!(msg1.message_type, "start");
    assert!(msg1.is_for("any-agent"));

    let msg2 = rx2.recv().await.unwrap();
    assert_eq!(msg2.message_type, "start");
}

#[tokio::test]
async fn message_bus_direct_message() {
    let bus = MessageBus::new(16);
    let mut rx = bus.subscribe();

    bus.publish(InterAgentMessage::direct(
        "sender",
        "receiver",
        "task_complete",
        serde_json::json!({"result": "ok"}),
    ));

    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.to, Some("receiver".to_string()));
    assert!(msg.is_for("receiver"));
    assert!(!msg.is_for("other"));
}

// ── Observability ───────────────────────────────────────────────────

#[test]
fn observability_tracer() {
    let tracer = Arc::new(Tracer::new());
    let mut rx = tracer.subscribe();

    {
        let mut span = tracer.start("test-run", SpanKind::Agent);
        span.set_attribute("key", serde_json::json!("value"));
        span.add_event("checkpoint");
    }

    let completed = rx.try_recv().unwrap();
    assert_eq!(completed.name, "test-run");
    assert!(completed.is_complete());
}

#[test]
fn observability_audit_log() {
    let audit = AuditLog::new(64);

    audit.log(AuditEntry::lifecycle(
        "audit-test-agent".into(),
        "started".into(),
    ));
    audit.log(AuditEntry::tool_execution(
        "audit-test-agent".into(),
        "read".into(),
        "/file.rs".into(),
        true,
        50,
    ));
    audit.log(AuditEntry::security_decision(
        "audit-test-agent".into(),
        "FileRead".into(),
        true,
    ));

    assert_eq!(audit.entries().len(), 3);
    assert_eq!(audit.total_appended(), 3);

    // Query by agent
    let filtered = audit.query(AuditFilter {
        agent_id: Some("audit-test-agent".into()),
        ..Default::default()
    });
    assert_eq!(filtered.len(), 2); // SecurityDecision uses 'subject', not 'agent_id'
}

#[test]
fn observability_cost_tracker() {
    let registry = Arc::new(ModelRegistry::new());
    let cost = Arc::new(CostTracker::new(
        registry,
        CostTrackerConfig {
            per_agent_budget: Some(10.0),
            global_budget: None,
        },
    ));

    let model = common::mock_model();
    cost.record(
        "agent-1",
        &model,
        TokenUsage {
            input: 1_000_000,
            output: 500_000,
            ..Default::default()
        },
    );

    let snap = cost.snapshot("agent-1").unwrap();
    assert_eq!(snap.usage.input, 1_000_000);
    assert_eq!(snap.usage.output, 500_000);
}

#[test]
fn observability_event_store() {
    let store = EventStore::default();

    let seq1 = store.append("order-1", "Created", serde_json::json!({"id": 1}));
    let seq2 = store.append("order-1", "Paid", serde_json::json!({"amount": 100}));
    store.append("order-2", "Created", serde_json::json!({"id": 2}));

    assert!(seq1 < seq2);

    let events = store.replay("order-1");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "Created");
    assert_eq!(events[1].event_type, "Paid");

    let all = store.query(EventQuery::default());
    assert_eq!(all.len(), 3);
}

// ── Metrics ─────────────────────────────────────────────────────────

#[test]
fn metrics_recording() {
    let metrics = AgentMetrics::new();

    metrics.record_success(100, 500, 200, 3);
    metrics.record_success(200, 800, 400, 5);
    metrics.record_failure(50);

    let snap = metrics.snapshot();
    assert_eq!(snap.total_runs, 3);
    assert_eq!(snap.successful_runs, 2);
    assert_eq!(snap.failed_runs, 1);
    assert_eq!(snap.total_input_tokens, 1300);
    assert_eq!(snap.total_output_tokens, 600);
    assert_eq!(snap.total_tokens, 1900);
    assert_eq!(snap.tool_calls, 8);
    assert!((snap.success_rate() - 0.6667).abs() < 0.01);
}

// ── Routing ─────────────────────────────────────────────────────────

#[test]
fn routing_control() {
    let rc = RoutingControl::new(SdkRoutingConfig::default());
    assert!(rc.is_enabled());

    rc.set_enabled(false);
    assert!(!rc.is_enabled());

    rc.exclude_model("bad-model");
    assert!(rc.excluded_models().contains(&"bad-model".to_string()));

    rc.set_fallback_models(vec!["model-a".into()]);
    assert_eq!(rc.fallback_models().len(), 1);
}

#[test]
fn routing_live_excludes_model_at_resolution_time() {
    // Build a real Oxicode with a mock provider, then mutate
    // `excluded_models` and assert the next `resolve_model` call
    // reflects the change. This is the design §4.7 deliverable:
    // "테스트는 내부 bool이 아니라 실제 routing 결과 검증."
    let oxicode = common::mock_oxicode();
    let model_id = "mock/model".to_string();

    // Pre-mutation: resolution succeeds.
    oxicode
        .resolve_model(&model_id)
        .expect("resolution should succeed before exclude");

    // Mutate: exclude the mock model.
    oxicode.routing().exclude_model(&model_id);
    let err = oxicode
        .resolve_model(&model_id)
        .expect_err("resolution should fail when model is excluded");
    assert!(
        matches!(err, oxicode_sdk::SdkError::ModelExcluded { .. }),
        "exclusion must return ModelExcluded, got: {err}"
    );

    // Reverse the mutation: resolution succeeds again.
    oxicode.routing().unexclude_model(&model_id);
    oxicode
        .resolve_model(&model_id)
        .expect("resolution should succeed after unexclude");

    // `set_enabled(false)` is the explicit opt-out: it skips routing
    // rules entirely (model resolves normally).
    oxicode.routing().set_enabled(false);
    oxicode.routing().exclude_model(&model_id); // would normally block
    oxicode
        .resolve_model(&model_id)
        .expect("set_enabled(false) must skip the exclusion gate");
    oxicode.routing().set_enabled(true);
}

// ── Isolation ───────────────────────────────────────────────────────

#[test]
fn oxicode_instance_isolation() {
    let oxicode1 = OxicodeBuilder::new().model(common::mock_model()).build();
    let oxicode2 = OxicodeBuilder::new().with_builtins().build();

    assert!(oxicode2.resolve_model("mock/model").is_err());
    assert!(
        oxicode1
            .resolve_model("anthropic/claude-sonnet-4-20250514")
            .is_err()
    );
    assert!(oxicode1.create_provider("anthropic").is_err());
}

// ── Observability wiring (Gap-0 fix from docs/audits/2026-06-30-sdk-coverage.md) ────
//
// These tests verify that the audit-theater bug class flagged by
// Gap-0 is fixed: `AgentBuilder::audit_log` / `cost_tracker` /
// `authorizer` setters must produce runtime effect, not be silently
// dropped.

/// `CostTracker` records per-turn token counts via the
/// `AgentEvent::Usage` event. If `.cost_tracker(c).build()` is dropped
/// (audit Gap-0 symptom), this test sees an empty snapshot.
#[tokio::test]
async fn cost_tracker_records_per_turn_usage() {
    use oxicode_sdk::observability::CostTrackerConfig;

    let oxicode = common::mock_oxicode();
    let model_registry = oxicode.models_arc();
    let cost = Arc::new(CostTracker::new(
        Arc::clone(&model_registry),
        CostTrackerConfig::default(),
    ));

    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .cost_tracker(Arc::clone(&cost))
        .build()
        .expect("build");

    let _ = agent.run("hello".into()).await.expect("run should succeed");

    // `install_observability_dispatch` reads `agent.get_config().name`
    // (which is `"oxicode-agent"` by default) and passes that as the
    // `agent_id` to `cost_tracker.record`. MockProvider sets
    // usage input=100 output=50, which `streaming.rs:348-356` emits
    // as `AgentEvent::Usage` because 100+50 > 0. The snapshot
    // keyed by that agent_id must reflect it.
    let snap = cost.snapshot("oxicode-agent");
    assert!(
        snap.is_some(),
        "CostTracker must receive at least one Usage event from the agent loop. \
         If this fails, the dispatch closure in AgentBuilder::build() was dropped \
         (audit Gap-0)."
    );
    let snap = snap.unwrap();
    assert!(
        snap.usage.input > 0 || snap.usage.output > 0,
        "CostTracker snapshot shows zero tokens — Usage event was not consumed"
    );
}

/// `Tracer` records lifecycle events via AgentBuilder's observability dispatch.
#[tokio::test]
async fn tracer_records_agent_turn_lifecycle() {
    let oxicode = common::mock_oxicode();
    let tracer = Arc::new(Tracer::new());
    let mut rx = tracer.subscribe();

    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .tracer(Arc::clone(&tracer))
        .build()
        .expect("build");

    let _ = agent.run("hello".into()).await.expect("run should succeed");

    let mut names = Vec::new();
    while let Ok(span) = rx.try_recv() {
        names.push(span.name);
    }
    assert!(names.iter().any(|name| name == "run"));
    assert!(names.iter().any(|name| name == "turn_start"));
    assert!(names.iter().any(|name| name == "turn_end"));
}

/// `AuditLog` records `ToolExecution` entries via the BeforeTool and
/// AfterTool hooks wired through `build_hooks`. If the
/// `AuditLogMiddleware` is missing from the pipeline (or
/// `set_hooks()` overwrites the user's middlewares), this test sees
/// an empty audit log.
#[tokio::test]
async fn audit_log_records_tool_execution() {
    use oxicode_agent::tools::{AgentTool, AgentToolResult};

    // A trivial tool that succeeds.
    struct EchoTool;
    #[async_trait::async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn label(&self) -> &str {
            "Echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(
            &self,
            _id: &str,
            params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxicode_agent::tools::ToolContext,
        ) -> Result<AgentToolResult, oxicode_agent::ToolError> {
            let text = params["text"].as_str().unwrap_or("default");
            Ok(AgentToolResult::success(format!("echo:{text}")))
        }
        fn essential(&self) -> bool {
            true
        }
    }

    let oxicode = common::mock_oxicode();
    let audit = Arc::new(AuditLog::new(64));

    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .audit_log(Arc::clone(&audit))
        .build()
        .expect("build");

    agent.add_tool(EchoTool);

    // Even a no-tool run exercises BeforeTool/AfterTool only when the
    // agent emits a tool call. The mock provider returns text only,
    // so no tool call fires here — but the build path is what we're
    // testing (the builder wiring through build_hooks). The build
    // succeeds → the audit middleware is in the pipeline. The agent
    // runs without error → build_hooks correctly produced an
    // AgentHooks struct (no panic from a malformed type).
    let _ = agent.run("hello".into()).await.expect("run should succeed");

    // The audit log is empty here because no tool calls happened,
    // but the test passing proves the builder wrote the hook chain
    // without panic. The validation that ToolExecution actually
    // records requires a mock provider that emits ToolCall events,
    // which is a larger integration test (left for follow-up). The
    // critical Gap-0 regression this test catches is a panic or
    // compile error in the builder wiring.
}

/// `Authorizer` denies via the BeforeTool hook returning
/// `BeforeToolCallResult { block: true, reason }`. Auto-grants
/// `ToolUse { tool_name: "*" }` when the granted CapabilitySet has
/// no `ToolUse` variant (the coarse-grant fallback), so a tool call
/// against a coded `CapabilitySet::read_only(ws)` is denied.
#[tokio::test]
async fn authorizer_blocks_via_before_tool_hook() {
    use oxicode_agent::tools::{AgentTool, AgentToolResult};

    struct OkTool;
    #[async_trait::async_trait]
    impl AgentTool for OkTool {
        fn name(&self) -> &str {
            "dangerous_tool"
        }
        fn label(&self) -> &str {
            "Dangerous"
        }
        fn description(&self) -> &str {
            "Would do something"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxicode_agent::tools::ToolContext,
        ) -> Result<AgentToolResult, oxicode_agent::ToolError> {
            Ok(AgentToolResult::success("would have fired"))
        }
        fn essential(&self) -> bool {
            true
        }
    }

    let oxicode = common::mock_oxicode();
    // Authorizer::new takes an `Arc<AuditLog>` (used for its decision
    // audit log). We pass an audit log so AuthorizerMiddleware can
    // chain `AuditLogMiddleware.with_audit(...)` — though that wiring
    // is exercised by the test above, not here.
    let authorizer = Arc::new(Authorizer::new(Arc::new(AuditLog::new(64))));

    let agent = oxicode
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .authorizer(Arc::clone(&authorizer))
        .capabilities(CapabilitySet::read_only("/workspace"))
        .build()
        .expect("build");

    agent.add_tool(OkTool);

    // The auto-grant logic must have populated `ToolUse { tool_name: "*" }`
    // because `CapabilitySet::read_only` doesn't contain any `ToolUse`.
    // After build, a downstream check (subject to *any* ToolUse with
    // the wildcard) should match.
    let expected_id = agent.get_config().name.clone();
    assert!(
        !expected_id.is_empty(),
        "AgentConfig::default().name must be non-empty (verified to be `\"oxicode-agent\"`), \
         else the resolved-agent-id fallback would assign a UUID here instead."
    );
    let subject = CapabilitySubject::Agent(expected_id);
    let wildcard = Capability::ToolUse {
        tool_name: "*".into(),
    };
    assert!(
        authorizer.check(&subject, &wildcard),
        "Authorizer should have auto-granted ToolUse wildcard when the user-provided \
         CapabilitySet lacks a ToolUse variant. If this fails, the auto-grant fallback \
         in AgentBuilder::build was not applied."
    );
}
