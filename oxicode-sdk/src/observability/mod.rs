//! Observability layer — tracing, audit, cost tracking, and event sourcing.

mod audit;
pub mod audit_trail;
mod cost;
mod decorator;
mod event_store;
mod trace;

// Re-exports
pub use audit::{AuditEntry, AuditFilter, AuditLog};
pub use audit_trail::{
    AuditAction, AuditError, AuditPersistence, AuditTrail, HashDigest, TrailEntry,
};
pub use cost::{
    CostBreakdown, CostSnapshot, CostTracker, CostTrackerConfig, GlobalCostSnapshot, TokenUsage,
};
pub use decorator::{AgentDecorator, ObservabilityDecorator};
pub use event_store::{EventQuery, EventStore, EventStoreConfig, StoredEvent};
pub use trace::{Span, SpanContext, SpanGuard, SpanId, SpanKind, SpanStatus, TraceId, Tracer};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        // Tracer smoke test
        let tracer = std::sync::Arc::new(Tracer::new());
        let _guard = tracer.start("test", SpanKind::Agent);
        drop(_guard);
        // AuditLog smoke test
        let log = AuditLog::new(64);
        log.log(AuditEntry::lifecycle("test".into(), "smoke".into()));
        // CostTracker smoke test
        let registry = std::sync::Arc::new(oxicode_ai::ModelRegistry::new());
        let tracker = CostTracker::new(registry, CostTrackerConfig::default());
        let model = oxicode_ai::Model::new(
            "dummy/model",
            "Dummy",
            oxicode_ai::Api::AnthropicMessages,
            "dummy",
            "https://dummy.com",
        );
        tracker.record("agent-1", &model, TokenUsage::default());
        let snap = tracker.snapshot("agent-1");
        assert!(snap.is_some());
        // EventStore smoke test
        let store = EventStore::default();
        let seq = store.append("s1", "test", serde_json::json!({"x": 1}));
        assert_eq!(seq, 1);
    }
}
