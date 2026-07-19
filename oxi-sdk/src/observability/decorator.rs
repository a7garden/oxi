//! Agent decorator — pluggable transform applied to every agent a
//! supervisor spawns.
//!
//! Built-in [`ObservabilityDecorator`] bundles the four observability
//! pieces (audit, authorizer, tracer, cost tracker) that previously had
//! silent no-op setters on [`crate::SupervisorBuilder`]. The decorator
//! is applied to the [`crate::AgentBuilder`] *before* `.build()` runs,
//! so the agent's middleware pipeline + event-tap dispatch are wired
//! exactly as if the caller had set each piece per-agent via
//! `Oxi::agent(...).audit_log(...)`.
//!
//! See `docs/designs/2026-07-18-stub-completion.md` §4.7.

use std::sync::Arc;

use crate::agent_builder::AgentBuilder;
use crate::observability::{AuditLog, CostTracker, Tracer};
use crate::security::Authorizer;

/// Pluggable transform applied to every agent a supervisor spawns.
///
/// Implementations receive an [`AgentBuilder`] (already bound to the
/// supervisor's `Oxi` instance and seeded with the spawn config) and
/// return a (possibly modified) builder. The supervisor then calls
/// `.build()` on the returned builder.
///
/// This is the integration point for cross-cutting concerns that
/// should apply to *every* supervisor-spawned agent — observability,
/// capability enforcement, middleware injection, etc.
///
/// # Example
///
/// ```no_run
/// use std::sync::Arc;
/// use oxi_sdk::{
///     observability::{AgentDecorator, ObservabilityDecorator, AuditLog, Tracer},
///     OxiBuilder,
/// };
///
/// let decorator = ObservabilityDecorator::new()
///     .with_audit(Arc::new(AuditLog::new(256)))
///     .with_tracer(Arc::new(Tracer::new()));
///
/// let (oxi, supervisor) = OxiBuilder::new()
///     .with_builtins()
///     .supervisor()
///     .with_agent_decorator(Arc::new(oxi_sdk::Oxi::clone), Arc::new(decorator))
///     .build()
///     .unwrap();
/// # let _ = (oxi, supervisor);
/// ```
pub trait AgentDecorator: Send + Sync {
    /// Apply this decorator's transforms to the builder. The
    /// implementor may chain `audit_log`, `tracer`, `authorizer`,
    /// `cost_tracker`, `capabilities`, middleware, etc., then return
    /// the builder for the caller to `.build()`.
    fn decorate<'a>(&self, builder: AgentBuilder<'a>) -> AgentBuilder<'a>;
}
/// Built-in decorator that bundles audit, authorizer, tracer, and cost
/// tracker observability and applies them to every spawned agent.
///
/// Construct with [`ObservabilityDecorator::new`] (all fields
/// `None`) then attach the pieces you want via the `with_*` builders.
/// Fields left `None` are simply not applied — no warning, no
/// silent-drop. This replaces the four deprecated no-op setters on
/// `SupervisorBuilder` (`with_audit`, `with_authorizer`,
/// `with_tracer`, `with_cost_tracker`).
#[derive(Default, Clone)]
pub struct ObservabilityDecorator {
    audit: Option<Arc<AuditLog>>,
    authorizer: Option<Arc<Authorizer>>,
    tracer: Option<Arc<Tracer>>,
    cost_tracker: Option<Arc<CostTracker>>,
}

impl ObservabilityDecorator {
    /// Create an empty decorator (all observability pieces `None`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach an audit log. Replaces any previously-set audit log.
    pub fn with_audit(mut self, audit: Arc<AuditLog>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Attach an authorizer. Replaces any previously-set authorizer.
    pub fn with_authorizer(mut self, authorizer: Arc<Authorizer>) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    /// Attach a tracer. Replaces any previously-set tracer.
    pub fn with_tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attach a cost tracker. Replaces any previously-set tracker.
    pub fn with_cost_tracker(mut self, tracker: Arc<CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }

    /// Borrow the audit log, if set.
    pub fn audit(&self) -> Option<&Arc<AuditLog>> {
        self.audit.as_ref()
    }

    /// Borrow the authorizer, if set.
    pub fn authorizer(&self) -> Option<&Arc<Authorizer>> {
        self.authorizer.as_ref()
    }

    /// Borrow the tracer, if set.
    pub fn tracer(&self) -> Option<&Arc<Tracer>> {
        self.tracer.as_ref()
    }

    /// Borrow the cost tracker, if set.
    pub fn cost_tracker(&self) -> Option<&Arc<CostTracker>> {
        self.cost_tracker.as_ref()
    }
}

impl AgentDecorator for ObservabilityDecorator {
    fn decorate<'a>(&self, builder: AgentBuilder<'a>) -> AgentBuilder<'a> {
        let b = builder;
        let b = if let Some(a) = self.audit.as_ref() {
            b.audit_log(a.clone())
        } else {
            b
        };
        let b = if let Some(a) = self.authorizer.as_ref() {
            b.authorizer(a.clone())
        } else {
            b
        };
        let b = if let Some(t) = self.tracer.as_ref() {
            b.tracer(t.clone())
        } else {
            b
        };
        if let Some(c) = self.cost_tracker.as_ref() { b.cost_tracker(c.clone()) } else { b }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::{AuditLog, CostTrackerConfig, Tracer};
    use crate::security::Authorizer;
    use oxi_ai::ModelRegistry;

    #[test]
    fn empty_decorator_is_noop_on_fields() {
        let d = ObservabilityDecorator::new();
        assert!(d.audit().is_none());
        assert!(d.authorizer().is_none());
        assert!(d.tracer().is_none());
        assert!(d.cost_tracker().is_none());
    }

    #[test]
    fn builders_set_fields() {
        let audit = Arc::new(AuditLog::new(64));
        let tracer = Arc::new(Tracer::new());
        let authorizer = Arc::new(Authorizer::new(audit.clone()));
        let registry = Arc::new(ModelRegistry::new());
        let cost = Arc::new(CostTracker::new(registry, CostTrackerConfig::default()));

        let d = ObservabilityDecorator::new()
            .with_audit(audit.clone())
            .with_tracer(tracer.clone())
            .with_authorizer(authorizer.clone())
            .with_cost_tracker(cost.clone());

        assert!(Arc::ptr_eq(d.audit().unwrap(), &audit));
        assert!(Arc::ptr_eq(d.tracer().unwrap(), &tracer));
        assert!(Arc::ptr_eq(d.authorizer().unwrap(), &authorizer));
        assert!(Arc::ptr_eq(d.cost_tracker().unwrap(), &cost));
    }

    #[test]
    fn decorator_implements_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ObservabilityDecorator>();
        assert_send_sync::<Arc<dyn AgentDecorator>>();
    }
}
