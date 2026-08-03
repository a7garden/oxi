//! CSpace resolution — resolve a template name to a concrete CSpace.
//!
//! Provides `resolve_cspace()` which maps template names like "standard",
//! "worker", "admin" to pre-built CSpace configurations.

use uuid::Uuid;

use super::types::CSpaceBuilder;

/// Resolve a CSpace from a template name and optional overrides.
///
/// # Arguments
/// * `template` - Template name: "standard", "worker", "admin", or None for empty.
/// * `workspace` - Optional workspace path override for path capabilities.
/// * `extra_domains` - Optional extra kernel domains to grant execute on.
/// * `agent_id` - UUID of the owning agent.
pub fn resolve_cspace(
    template: Option<&str>,
    workspace: Option<&str>,
    extra_domains: Option<&[String]>,
    agent_id: Uuid,
) -> super::types::CSpace {
    let name = template.unwrap_or("empty");
    let mut builder = CSpaceBuilder::new(agent_id).name(name);

    match name {
        "admin" | "superuser" => {
            builder = builder.all_access();
        }
        "standard" | "default" => {
            builder = builder.standard();
            // Add workspace path if provided
            if let Some(ws) = workspace {
                use super::types::{ResourceRef, Rights};
                builder = builder.grant(
                    ResourceRef::Path {
                        pattern: format!("{ws}/**"),
                    },
                    vec![Rights::Read, Rights::Write, Rights::Execute],
                );
            }
        }
        "worker" => {
            builder = builder.worker();
            if let Some(ws) = workspace {
                use super::types::{ResourceRef, Rights};
                builder = builder.grant(
                    ResourceRef::Path {
                        pattern: format!("{ws}/**"),
                    },
                    vec![Rights::Read, Rights::Write, Rights::Execute],
                );
            }
        }
        _ => {
            // Unknown template → empty CSpace (least privilege)
        }
    }

    // Grant extra domains if provided
    if let Some(domains) = extra_domains {
        use super::types::{ResourceRef, Rights};
        for domain in domains {
            builder = builder.grant(
                ResourceRef::KernelDomain {
                    domain: domain.clone(),
                },
                vec![Rights::Read, Rights::Write, Rights::Execute],
            );
        }
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::capability::types::{ResourceRef, Rights};

    #[test]
    fn test_resolve_standard() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("standard"), None, None, id);
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "read".into()
            },
            Rights::Execute
        ));
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "memory".into()
            },
            Rights::Read
        ));
    }

    #[test]
    fn test_resolve_admin() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("admin"), None, None, id);
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "anything".into()
            },
            Rights::Grant
        ));
    }

    #[test]
    fn test_resolve_worker() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("worker"), None, None, id);
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "subagent".into()
            },
            Rights::Execute
        ));
    }

    #[test]
    fn test_resolve_unknown_is_empty() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("nonexistent"), None, None, id);
        assert!(cs.is_empty());
    }

    #[test]
    fn test_resolve_with_workspace() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(Some("standard"), Some("/my/project"), None, id);
        assert!(cs.can(
            &ResourceRef::Path {
                pattern: "/my/project/src/lib.rs".into()
            },
            Rights::Read
        ));
    }

    #[test]
    fn test_resolve_with_extra_domains() {
        let id = Uuid::new_v4();
        let cs = resolve_cspace(
            Some("standard"),
            None,
            Some(&["custom_tool".to_string()]),
            id,
        );
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "custom_tool".into()
            },
            Rights::Execute
        ));
    }
}
