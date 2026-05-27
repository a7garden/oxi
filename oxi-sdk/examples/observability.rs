//! Observability demo — tracing, audit logging, cost tracking, event store.
//!
//! Run: `cargo run -p oxi-sdk --example observability`
//!
//! No API key required — this example demonstrates the observability module only.

use std::sync::Arc;

use oxi_sdk::prelude::*;

fn main() {
    // ── Distributed Tracing ──
    let tracer = Tracer::new();
    let mut rx = tracer.subscribe();
    {
        let mut span = tracer.start("agent-run", SpanKind::Agent);
        span.set_attribute("model", serde_json::json!("claude-sonnet-4"));
        span.add_event("tool-call");
    }
    let completed = rx.try_recv().unwrap();
    println!("Span: {} ({:?})", completed.name, completed.kind);
    println!("Duration: {:?}ms", completed.duration_ms());

    // ── Audit Log ──
    let audit = AuditLog::new(64);
    audit.log(AuditEntry::lifecycle("agent-1".into(), "started".into()));
    audit.log(AuditEntry::tool_execution(
        "agent-1".into(),
        "read".into(),
        "/workspace/main.rs".into(),
        true,
        50,
    ));
    println!("\nAudit entries: {}", audit.entries().len());

    // ── Cost Tracking ──
    let registry = Arc::new(oxi_ai::ModelRegistry::new());
    let cost = Arc::new(CostTracker::new(
        registry,
        CostTrackerConfig {
            per_agent_budget: Some(10.0),
            global_budget: Some(100.0),
            ..Default::default()
        },
    ));
    let model = oxi_ai::Model::new(
        "mock/model",
        "Mock",
        oxi_ai::Api::OpenAiCompletions,
        "mock",
        "http://localhost",
    );
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
    println!(
        "\nCost: input={}, output={}",
        snap.usage.input, snap.usage.output
    );
    println!("Over budget: {}", cost.is_over_budget("agent-1"));

    // ── Event Store ──
    let store = EventStore::default();
    store.append("session-1", "Created", serde_json::json!({"id": 1}));
    store.append(
        "session-1",
        "Updated",
        serde_json::json!({"status": "active"}),
    );
    let events = store.replay("session-1");
    println!("\nEvents: {}", events.len());
    for e in &events {
        println!("  #{} {}", e.sequence, e.event_type);
    }
}
