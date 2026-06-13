//! Integration tests for oxi-sdk.
//!
//! Uses MockProvider to test end-to-end flows without real API calls.

mod common;

use std::sync::Arc;

use oxi_agent::AgentConfig;
use oxi_sdk::prelude::*;
use oxi_sdk::routing::RoutingConfig as SdkRoutingConfig;
use oxi_sdk::{AgentMetrics, InterAgentMessage, MessageBus, ModelRegistry, WorkQueueConfig};

// ── Agent Build + Run ──────────────────────────────────────────────

#[tokio::test]
async fn full_pipeline_build_and_run() {
    let oxi = common::mock_oxi();
    let agent = oxi
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
    let oxi = common::mock_oxi();
    let agent = oxi
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
    let oxi = common::mock_oxi();
    let agent = oxi
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
    let tracer = Tracer::new();
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

// ── Isolation ───────────────────────────────────────────────────────

#[test]
fn oxi_instance_isolation() {
    let oxi1 = OxiBuilder::new().model(common::mock_model()).build();
    let oxi2 = OxiBuilder::new().with_builtins().build();

    assert!(oxi2.resolve_model("mock/model").is_err());
    assert!(
        oxi1.resolve_model("anthropic/claude-sonnet-4-20250514")
            .is_err()
    );
    assert!(oxi1.create_provider("anthropic").is_err());
}
