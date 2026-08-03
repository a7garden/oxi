//! Authorizer — capability-based access control with role hierarchy.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::observability::AuditLog;
use crate::security::capability::{Capability, CapabilitySet, CapabilitySubject};

/// Default policy when no explicit grant matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultPolicy {
    /// Deny all unknown subjects.
    DenyAll,
    /// Allow all (backward-compatible with pre-security code).
    AllowAll,
}

/// Capability-based authorizer.
///
/// Maintains:
/// - **Direct grants**: subject → CapabilitySet
/// - **Roles**: role name → CapabilitySet
/// - **Role bindings**: agent → list of role names
///
/// Evaluation order: direct grants → role inheritance → default policy.
pub struct Authorizer {
    /// Direct capability grants.
    grants: Arc<RwLock<HashMap<CapabilitySubject, CapabilitySet>>>,
    /// Role definitions: role name → capability set.
    roles: Arc<RwLock<HashMap<String, CapabilitySet>>>,
    /// Agent → role bindings.
    role_bindings: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Default policy for unmatched subjects.
    default_policy: DefaultPolicy,
    /// Audit log for security decisions.
    audit: Arc<AuditLog>,
}

impl Authorizer {
    /// Create a new authorizer with deny-by-default policy.
    pub fn new(audit: Arc<AuditLog>) -> Self {
        Self {
            grants: Arc::new(RwLock::new(HashMap::new())),
            roles: Arc::new(RwLock::new(HashMap::new())),
            role_bindings: Arc::new(RwLock::new(HashMap::new())),
            default_policy: DefaultPolicy::DenyAll,
            audit,
        }
    }

    /// Create a permissive authorizer (allow all by default).
    pub fn new_permissive(audit: Arc<AuditLog>) -> Self {
        Self {
            grants: Arc::new(RwLock::new(HashMap::new())),
            roles: Arc::new(RwLock::new(HashMap::new())),
            role_bindings: Arc::new(RwLock::new(HashMap::new())),
            default_policy: DefaultPolicy::AllowAll,
            audit,
        }
    }

    // ── Direct grants ──

    /// Grant a full capability set to a subject.
    pub fn grant(&self, subject: CapabilitySubject, caps: CapabilitySet) {
        self.grants.write().insert(subject, caps);
    }

    /// Grant a single capability to a subject.
    pub fn grant_one(&self, subject: CapabilitySubject, cap: Capability) {
        let mut grants = self.grants.write();
        grants
            .entry(subject)
            .and_modify(|set| {
                let mut new_caps = CapabilitySet::new(set.capabilities().to_vec());
                new_caps.add(cap.clone());
                *set = new_caps;
            })
            .or_insert_with(|| CapabilitySet::new(vec![cap]));
    }

    /// Revoke all capabilities from a subject.
    pub fn revoke(&self, subject: &CapabilitySubject) {
        self.grants.write().remove(subject);
    }

    // ── Role management ──

    /// Define a named role with a capability set.
    pub fn define_role(&self, role_name: &str, caps: CapabilitySet) {
        self.roles.write().insert(role_name.to_string(), caps);
    }

    /// Bind a role to an agent.
    pub fn bind_role(&self, agent_id: &str, role_name: &str) {
        self.role_bindings
            .write()
            .entry(agent_id.to_string())
            .or_default()
            .push(role_name.to_string());
    }

    /// Remove a role binding from an agent.
    pub fn unbind_role(&self, agent_id: &str, role_name: &str) {
        if let Some(roles) = self.role_bindings.write().get_mut(agent_id) {
            roles.retain(|r| r != role_name);
        }
    }

    // ── Checks ──

    /// Check if a subject has a required capability.
    pub fn check(&self, subject: &CapabilitySubject, required: &Capability) -> bool {
        let result = self.evaluate(subject, required);
        self.audit
            .log(crate::observability::AuditEntry::security_decision(
                subject.to_string(),
                format!("{:?}", required),
                result,
            ));
        result
    }

    /// Require a capability, returning an error if not granted.
    pub fn require(
        &self,
        subject: &CapabilitySubject,
        required: &Capability,
    ) -> Result<(), crate::error::SdkError> {
        if self.check(subject, required) {
            Ok(())
        } else {
            Err(crate::error::SdkError::PermissionDenied {
                subject: subject.to_string(),
                capability: format!("{:?}", required),
            })
        }
    }

    // ── Internal ──

    fn evaluate(&self, subject: &CapabilitySubject, required: &Capability) -> bool {
        // 1. Direct grants
        let grants = self.grants.read();
        if let Some(set) = grants.get(subject)
            && !set.is_expired()
            && set.satisfies(required)
        {
            return true;
        }
        drop(grants);

        // 2. Role inheritance (only for agents)
        if let CapabilitySubject::Agent(id) = subject {
            let bindings = self.role_bindings.read();
            if let Some(roles) = bindings.get(id) {
                let role_defs = self.roles.read();
                for role_name in roles {
                    if let Some(role_caps) = role_defs.get(role_name)
                        && !role_caps.is_expired()
                        && role_caps.satisfies(required)
                    {
                        return true;
                    }
                }
            }
        }

        // 3. Default policy
        matches!(self.default_policy, DefaultPolicy::AllowAll)
    }
}

impl std::fmt::Debug for Authorizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authorizer")
            .field("default_policy", &self.default_policy)
            .field("grant_count", &self.grants.read().len())
            .field("role_count", &self.roles.read().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_authorizer() -> Authorizer {
        Authorizer::new(Arc::new(AuditLog::new(64)))
    }

    #[test]
    fn direct_grant_check() {
        let auth = test_authorizer();
        auth.grant(
            CapabilitySubject::Agent("a1".into()),
            CapabilitySet::coding("/workspace"),
        );
        assert!(auth.check(
            &CapabilitySubject::Agent("a1".into()),
            &Capability::FileRead {
                path_pattern: "/workspace/src/main.rs".into()
            },
        ));
        assert!(!auth.check(
            &CapabilitySubject::Agent("a1".into()),
            &Capability::FileWrite {
                path_pattern: "/etc/passwd".into()
            },
        ));
    }

    #[test]
    fn grant_one_adds_capability() {
        let auth = test_authorizer();
        auth.grant_one(
            CapabilitySubject::Agent("a1".into()),
            Capability::FileRead {
                path_pattern: "/ws/**".into(),
            },
        );
        assert!(auth.check(
            &CapabilitySubject::Agent("a1".into()),
            &Capability::FileRead {
                path_pattern: "/ws/file".into()
            },
        ));
    }

    #[test]
    fn revoke_removes_all() {
        let auth = test_authorizer();
        let subject = CapabilitySubject::Agent("a1".into());
        auth.grant(subject.clone(), CapabilitySet::coding("/ws"));
        auth.revoke(&subject);
        assert!(!auth.check(
            &subject,
            &Capability::FileRead {
                path_pattern: "/ws/file".into()
            },
        ));
    }

    #[test]
    fn role_based_access() {
        let auth = test_authorizer();
        auth.define_role("coder", CapabilitySet::coding("/workspace"));
        auth.bind_role("agent-001", "coder");
        assert!(auth.check(
            &CapabilitySubject::Agent("agent-001".into()),
            &Capability::FileRead {
                path_pattern: "/workspace/any".into()
            },
        ));
        assert!(!auth.check(
            &CapabilitySubject::Agent("agent-001".into()),
            &Capability::FileWrite {
                path_pattern: "/etc/passwd".into()
            },
        ));
    }

    #[test]
    fn multi_role_binding() {
        let auth = test_authorizer();
        auth.define_role("coder", CapabilitySet::coding("/ws"));
        auth.define_role("browser", CapabilitySet::browser("/ws"));
        auth.bind_role("agent-001", "coder");
        auth.bind_role("agent-001", "browser");
        // Coder gives file write
        assert!(auth.check(
            &CapabilitySubject::Agent("agent-001".into()),
            &Capability::FileWrite {
                path_pattern: "/ws/src/main.rs".into()
            },
        ));
        // Browser gives web browse
        assert!(auth.check(
            &CapabilitySubject::Agent("agent-001".into()),
            &Capability::WebBrowse {
                allowed_domains: vec!["*".into()]
            },
        ));
    }

    #[test]
    fn unbind_role() {
        let auth = test_authorizer();
        auth.define_role("coder", CapabilitySet::coding("/ws"));
        auth.bind_role("a1", "coder");
        auth.unbind_role("a1", "coder");
        assert!(!auth.check(
            &CapabilitySubject::Agent("a1".into()),
            &Capability::FileRead {
                path_pattern: "/ws/any".into()
            },
        ));
    }

    #[test]
    fn deny_by_default() {
        let auth = test_authorizer();
        assert!(!auth.check(
            &CapabilitySubject::Agent("unknown".into()),
            &Capability::FileRead {
                path_pattern: "/any".into()
            },
        ));
    }

    #[test]
    fn permissive_allows_all() {
        let auth = Authorizer::new_permissive(Arc::new(AuditLog::new(64)));
        assert!(auth.check(
            &CapabilitySubject::Agent("anyone".into()),
            &Capability::FileRead {
                path_pattern: "/any".into()
            },
        ));
    }

    #[test]
    fn require_success() {
        let auth = test_authorizer();
        auth.grant(
            CapabilitySubject::Agent("a1".into()),
            CapabilitySet::read_only("/ws"),
        );
        assert!(
            auth.require(
                &CapabilitySubject::Agent("a1".into()),
                &Capability::FileRead {
                    path_pattern: "/ws/file".into()
                },
            )
            .is_ok()
        );
    }

    #[test]
    fn require_failure() {
        let auth = test_authorizer();
        let result = auth.require(
            &CapabilitySubject::Agent("a1".into()),
            &Capability::FileRead {
                path_pattern: "/ws/file".into(),
            },
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("permission denied"));
    }
}
