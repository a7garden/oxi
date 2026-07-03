//! Observability adapters — bridges for SDK observability types into the
//! [`Middleware`] pipeline so they cooperate with user-added middleware
//! instead of overwriting it.
//!
//! Background: [`crate::agent_builder::AgentBuilder`] stores `AuditLog`,
//! `Authorizer`, `Tracer`, and `CostTracker` setters that used to be
//! silently dropped. The audit report `docs/audits/2026-06-30-sdk-coverage.md`
//! flagged this as **API theater**. The fix is split into two layers:
//!
//! 1. The hook-slot observations (AuditLog, Authorizer) use the
//!    [`Middleware`] trait — they push into the SAME pipeline that
//!    `build_hooks()` consumes in
//!    `crate::agent_builder::AgentBuilder::build`. This way user-added
//!    middlewares (rate limit, logging, ...) and audit/auth all fire
//!    through the same unified `BeforeTool` / `AfterTool` chain.
//!    `set_hooks()` (which REPLACES the whole `AgentHooks`) is still
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::middleware::{
    Middleware, MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewareResult,
};
use crate::observability::{AuditEntry, AuditLog};
use crate::security::Authorizer;

/// Audit-log middleware — emits an `AuditEntry::lifecycle` (tool_start)
/// on `BeforeTool` and an `AuditEntry::tool_execution` on `AfterTool`,
/// plus records `SecurityDecision` entries supplied by [`AuthorizerMiddleware`]
/// when wrapped together via [`crate::middleware::build_hooks`] chain.
pub struct AuditLogMiddleware {
    audit: Arc<AuditLog>,
    agent_id: String,
}

impl AuditLogMiddleware {
    /// Create a new audit-log middleware bound to a specific agent id.
    pub fn new(audit: Arc<AuditLog>, agent_id: impl Into<String>) -> Self {
        Self {
            audit,
            agent_id: agent_id.into(),
        }
    }

    fn handle_before(&self, ctx: &MiddlewareContext) -> MiddlewareResult {
        let tool_name = match &ctx.data {
            MiddlewareData::BeforeTool { tool_name, .. } => tool_name.clone(),
            _ => return MiddlewareResult::pass(),
        };
        self.audit.log(AuditEntry::lifecycle(
            self.agent_id.clone(),
            format!("tool_start:{}", tool_name),
        ));
        MiddlewareResult::pass()
    }

    fn handle_after(&self, ctx: &MiddlewareContext) -> MiddlewareResult {
        let tool_name = match &ctx.data {
            MiddlewareData::AfterTool {
                tool_name, result, ..
            } => {
                let success = !result.is_empty() && !result.starts_with("error:");
                (tool_name.clone(), success)
            }
            _ => return MiddlewareResult::pass(),
        };
        let (name, success) = tool_name;
        self.audit.log(AuditEntry::tool_execution(
            self.agent_id.clone(),
            name,
            "{}".into(),
            success,
            0,
        ));
        MiddlewareResult::pass()
    }
}

impl Middleware for AuditLogMiddleware {
    fn name(&self) -> &str {
        "AuditLogMiddleware"
    }

    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::BeforeTool, MiddlewarePhase::AfterTool]
    }

    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        let result = match ctx.phase {
            MiddlewarePhase::BeforeTool => self.handle_before(ctx),
            MiddlewarePhase::AfterTool => self.handle_after(ctx),
            _ => MiddlewareResult::pass(),
        };
        Box::pin(async move { result })
    }
}

/// Authorizer middleware — denies a tool call when the agent lacks the
/// required capability. Uses `crate::security::Authorizer::check`.
pub struct AuthorizerMiddleware {
    authorizer: Arc<Authorizer>,
    audit: Option<Arc<AuditLog>>,
    agent_id: String,
}

impl AuthorizerMiddleware {
    /// Create a new authorizer middleware bound to a specific agent id.
    pub fn new(authorizer: Arc<Authorizer>, agent_id: impl Into<String>) -> Self {
        Self {
            authorizer,
            audit: None,
            agent_id: agent_id.into(),
        }
    }

    /// Attach an audit log so denials are recorded as `SecurityDecision`
    /// entries. Without this, denials are silent on the audit trail.
    pub fn with_audit(mut self, audit: Arc<AuditLog>) -> Self {
        self.audit = Some(audit);
        self
    }
}

impl Middleware for AuthorizerMiddleware {
    fn name(&self) -> &str {
        "AuthorizerMiddleware"
    }

    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::BeforeTool]
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        let tool_name = match &ctx.data {
            MiddlewareData::BeforeTool { tool_name, .. } => tool_name.clone(),
            _ => {
                return Box::pin(async move { MiddlewareResult::pass() });
            }
        };

        let authorizer = Arc::clone(&self.authorizer);
        let audit = self.audit.clone();
        let agent_id = self.agent_id.clone();
        let cap = crate::security::Capability::ToolUse {
            tool_name: tool_name.clone(),
        };

        Box::pin(async move {
            let subject = crate::security::CapabilitySubject::Agent(agent_id.clone());
            let granted = authorizer.check(&subject, &cap);
            if let Some(audit) = audit {
                audit.log(AuditEntry::security_decision(
                    agent_id,
                    format!("tool:{}", tool_name),
                    granted,
                ));
            }
            if granted {
                MiddlewareResult::pass()
            } else {
                MiddlewareResult::block(format!(
                    "authorizer denied tool `{}` for agent `{}`",
                    tool_name, subject
                ))
            }
        })
    }
}

/// Helper used by `AgentBuilder::build()`: synthesize an agent id for
/// observability dispatch if `AgentConfig::name` is empty, matching the
/// existing pattern at agent_builder.rs:443-447.
pub fn resolved_agent_id(config: &oxi_agent::AgentConfig) -> String {
    if config.name.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        config.name.clone()
    }
}
