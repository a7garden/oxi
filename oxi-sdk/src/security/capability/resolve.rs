//! CSpace resolution — determines initial capability space from context.
//!
//! Priority chain:
//! 1. Explicit cspace hint → parse and use it.
//! 2. Persona role → map known roles to templates.
//! 3. Default → fall back to worker template.

use super::template::CapabilityTemplate;
use super::types::CSpace;
use uuid::Uuid;

const ROLE_WORKER: &str = "worker";
const ROLE_STANDARD: &str = "standard";
const ROLE_OPERATOR: &str = "operator";
const ROLE_SUPERVISOR: &str = "supervisor";

/// Resolve an agent's initial CSpace from the available context.
///
/// # Arguments
/// * `cspace_hint` — Optional hint from configuration.
/// * `persona_role` — The role field of the assigned persona, if any.
/// * `default_template` — Override for fallback template name (defaults to "worker").
/// * `agent_id` — The agent that will own the resolved CSpace.
pub fn resolve_cspace(
    cspace_hint: Option<&str>,
    persona_role: Option<&str>,
    default_template: Option<&str>,
    agent_id: Uuid,
) -> CSpace {
    if let Some(hint) = cspace_hint {
        let trimmed = hint.trim();
        if !trimmed.is_empty() {
            return resolve_from_template_name(trimmed, agent_id);
        }
    }

    if let Some(role) = persona_role {
        let trimmed = role.trim().to_lowercase();
        if !trimmed.is_empty() {
            return resolve_from_template_name(&trimmed, agent_id);
        }
    }

    let fallback = default_template.unwrap_or(ROLE_WORKER);
    resolve_from_template_name(fallback, agent_id)
}

fn resolve_from_template_name(name: &str, agent_id: Uuid) -> CSpace {
    match name {
        ROLE_WORKER => CapabilityTemplate::worker().build_for(agent_id),
        ROLE_STANDARD => CapabilityTemplate::standard().build_for(agent_id),
        ROLE_OPERATOR => CapabilityTemplate::operator().build_for(agent_id),
        ROLE_SUPERVISOR => CapabilityTemplate::supervisor().build_for(agent_id),
        _ => {
            if name.starts_with('{') {
                tracing::warn!(
                    "JSON cspace_hint not yet supported, falling back to worker: {}",
                    name
                );
            } else {
                tracing::warn!(
                    "Unknown capability template '{}', falling back to worker",
                    name
                );
            }
            CapabilityTemplate::worker().build_for(agent_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::capability::types::{ResourceRef, Rights};

    #[test]
    fn hint_takes_priority() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("supervisor"), Some("worker"), None, id);
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "security".into()
            },
            Rights::ALL,
        ));
    }

    #[test]
    fn role_used_when_no_hint() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(None, Some("operator"), None, id);
        assert!(cs.can(&ResourceRef::A2a, Rights::EXECUTE));
    }

    #[test]
    fn default_is_worker() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(None, None, None, id);
        assert!(cs.can(
            &ResourceRef::Exec {
                mode: "shell".into()
            },
            Rights::EXECUTE
        ));
        assert!(!cs.can(&ResourceRef::A2a, Rights::READ));
    }

    #[test]
    fn unknown_falls_back_to_worker() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("nonexistent"), None, None, id);
        assert!(cs.can(
            &ResourceRef::Exec {
                mode: "shell".into()
            },
            Rights::EXECUTE
        ));
    }
}
