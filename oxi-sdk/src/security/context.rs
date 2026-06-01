//! Agent security context — unforgeable identity token.
//!
//! `AgentContext` is proof that the system has authenticated an agent.
//! It carries agent identity + capability space for access control.

use std::sync::Arc;

use crate::security::capability::types::CSpace;
use uuid::Uuid;

/// Agent security context — unforgeable proof of agent identity.
///
/// Construct this when an agent enters a managed lifecycle.
/// Tools that require access control accept `&AgentContext`.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Unique agent identifier.
    pub agent_id: Uuid,
    /// Human-readable name for permission lookups.
    pub agent_name: String,
    /// Agent's capability space.
    pub cspace: Arc<CSpace>,
}

impl AgentContext {
    /// Create a new context for a named agent with the given CSpace.
    pub fn new(agent_name: String, cspace: CSpace) -> Self {
        let agent_id = cspace.agent_id;
        Self {
            agent_id,
            agent_name,
            cspace: Arc::new(cspace),
        }
    }

    /// Create a context from a template by name.
    pub fn from_template(agent_name: &str, template_name: &str) -> Self {
        let id = Uuid::new_v4();
        let cspace = crate::security::capability::resolve::resolve_cspace(
            Some(template_name),
            None,
            None,
            id,
        );
        Self {
            agent_id: id,
            agent_name: agent_name.to_string(),
            cspace: Arc::new(cspace),
        }
    }
}

impl std::fmt::Display for AgentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent:{}:{}", self.agent_name, self.agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::capability::types::{ResourceRef, Rights};

    #[test]
    fn test_from_template() {
        let ctx = AgentContext::from_template("test", "standard");
        assert_eq!(ctx.agent_name, "test");
        assert!(!ctx.agent_id.is_nil());
        // Standard template has memory read
        assert!(ctx.cspace.can(
            &ResourceRef::KernelDomain {
                domain: "memory".into()
            },
            Rights::READ
        ));
    }

    #[test]
    fn test_display() {
        let ctx = AgentContext::from_template("my-agent", "worker");
        let s = format!("{}", ctx);
        assert!(s.starts_with("agent:my-agent:"));
    }
}
