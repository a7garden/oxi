//! RBAC — role-based access control with HitL approvals.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// 3-tier role model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Basic — limited tools, workspace only.
    User,
    /// All tools, agent/program/workspace management.
    Superuser,
    /// Full access, RBAC management.
    Admin,
}

impl Role {
    /// Default policy for this role.
    pub fn default_policy(&self) -> RbacPolicy {
        match self {
            Role::Admin => RbacPolicy {
                role: Role::Admin,
                allowed_actions: vec![
                    Action::UseTool("*".into()),
                    Action::AccessPath("*".into()),
                    Action::ManageAgents,
                    Action::ManagePrograms,
                    Action::ManageWorkspaces,
                    Action::ManageRBAC,
                    Action::ViewAuditLog,
                    Action::SystemConfig,
                ]
                .into_iter()
                .collect(),
                resource_patterns: vec!["*".into()],
                max_concurrent_agents: usize::MAX,
            },
            Role::Superuser => RbacPolicy {
                role: Role::Superuser,
                allowed_actions: vec![
                    Action::UseTool("*".into()),
                    Action::AccessPath("/workspace/**".into()),
                    Action::ManageAgents,
                    Action::ManagePrograms,
                    Action::ManageWorkspaces,
                    Action::ViewAuditLog,
                ]
                .into_iter()
                .collect(),
                resource_patterns: vec!["/workspace/**".into(), "/tmp/**".into()],
                max_concurrent_agents: 10,
            },
            Role::User => RbacPolicy {
                role: Role::User,
                allowed_actions: vec![
                    Action::UseTool("read".into()),
                    Action::UseTool("write".into()),
                    Action::UseTool("edit".into()),
                    Action::UseTool("bash".into()),
                    Action::UseTool("grep".into()),
                    Action::UseTool("find".into()),
                    Action::AccessPath("/workspace/**".into()),
                    Action::ManageAgents,
                ]
                .into_iter()
                .collect(),
                resource_patterns: vec!["/workspace/**".into()],
                max_concurrent_agents: 2,
            },
        }
    }
}

/// Who is accessing the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Subject {
    /// Named user.
    User(String),
    /// Agent acting on behalf of a user.
    Agent(Uuid),
    /// System-level (bypasses RBAC).
    System,
}

impl std::fmt::Display for Subject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Subject::User(name) => write!(f, "user:{name}"),
            Subject::Agent(id) => write!(f, "agent:{id}"),
            Subject::System => write!(f, "system"),
        }
    }
}

/// Authorizable actions.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Use a specific tool (name or *).
    UseTool(String),
    /// Access a path pattern.
    AccessPath(String),
    /// Manage agents.
    ManageAgents,
    /// Manage programs.
    ManagePrograms,
    /// Manage workspaces.
    ManageWorkspaces,
    /// Modify RBAC.
    ManageRBAC,
    /// View audit log.
    ViewAuditLog,
    /// System configuration.
    SystemConfig,
}

impl Action {
    /// Whether this action needs HitL approval.
    pub fn requires_approval(&self) -> bool {
        match self {
            Action::ManageRBAC | Action::SystemConfig => true,
            Action::UseTool(t) => t == "*" || t == "osascript" || t == "rm",
            _ => false,
        }
    }
}

/// RBAC policy for a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RbacPolicy {
    /// The role.
    pub role: Role,
    /// Allowed actions.
    pub allowed_actions: HashSet<Action>,
    /// Resource glob patterns.
    pub resource_patterns: Vec<String>,
    /// Max concurrent agents.
    pub max_concurrent_agents: usize,
}

impl RbacPolicy {
    /// Check if this policy allows an action (exact + wildcard).
    pub fn allows(&self, action: &Action) -> bool {
        if self.allowed_actions.contains(action) {
            return true;
        }
        match action {
            Action::UseTool(tool_name) => {
                self.allowed_actions
                    .iter()
                    .any(|a| matches!(a, Action::UseTool(w) if w == "*"))
                    || self
                        .allowed_actions
                        .contains(&Action::UseTool(tool_name.clone()))
            }
            Action::AccessPath(path) => {
                self.allowed_actions
                    .iter()
                    .any(|a| matches!(a, Action::AccessPath(p) if p == "*"))
                    || self
                        .allowed_actions
                        .contains(&Action::AccessPath(path.clone()))
            }
            _ => false,
        }
    }
}

/// RBAC audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RbacAuditEntry {
    /// When the access decision was recorded.
    pub timestamp: DateTime<Utc>,
    /// Who performed the access attempt.
    pub subject: Subject,
    /// Action that was requested.
    pub action: Action,
    /// Resource the action targeted.
    pub resource: String,
    /// Whether the action was permitted.
    pub allowed: bool,
    /// Optional human-readable explanation of the decision.
    pub reason: Option<String>,
}

/// Pending HitL approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    /// Unique identifier for this approval request.
    pub id: Uuid,
    /// Subject requesting the gated action.
    pub subject: Subject,
    /// Action requiring approval.
    pub action: Action,
    /// Resource the action targets.
    pub resource: String,
    /// Human-readable justification for the request.
    pub reason: String,
    /// When the approval was requested.
    pub created_at: DateTime<Utc>,
}

/// Approval status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    /// Awaiting a human decision.
    Pending,
    /// A human reviewer granted the request.
    Approved,
    /// A human reviewer denied the request.
    Rejected,
    /// The request expired before a decision was made.
    Expired,
}

/// RBAC Manager — roles, policies, audit, and HitL approvals.
#[derive(Debug, Clone)]
pub struct RbacManager {
    policies: HashMap<Role, RbacPolicy>,
    subject_roles: HashMap<Subject, Role>,
    audit_log: Vec<RbacAuditEntry>,
    pending_approvals: Vec<(PendingApproval, ApprovalStatus)>,
    max_audit_entries: usize,
}

impl RbacManager {
    /// Create with default policies for all roles.
    pub fn new() -> Self {
        let mut this = Self {
            policies: HashMap::new(),
            subject_roles: HashMap::new(),
            audit_log: Vec::new(),
            pending_approvals: Vec::new(),
            max_audit_entries: 10_000,
        };
        for role in [Role::User, Role::Superuser, Role::Admin] {
            this.policies.insert(role, role.default_policy());
        }
        this
    }

    /// Assign a role.
    pub fn assign_role(&mut self, subject: Subject, role: Role) {
        self.subject_roles.insert(subject, role);
    }

    /// Revoke a role.
    pub fn revoke_role(&mut self, subject: &Subject) {
        self.subject_roles.remove(subject);
    }

    /// Get role for a subject.
    pub fn get_role(&self, subject: &Subject) -> Option<Role> {
        self.subject_roles.get(subject).copied()
    }

    /// Check permission + audit.
    pub fn check_permission(&mut self, subject: &Subject, action: &Action, resource: &str) -> bool {
        if matches!(subject, Subject::System) {
            return true;
        }
        let role = match self.subject_roles.get(subject) {
            Some(r) => *r,
            None => return false,
        };
        let policy = match self.policies.get(&role) {
            Some(p) => p,
            None => return false,
        };
        let allowed = policy.allows(action);
        self.audit_log.push(RbacAuditEntry {
            timestamp: Utc::now(),
            subject: subject.clone(),
            action: action.clone(),
            resource: resource.to_string(),
            allowed,
            reason: if allowed {
                None
            } else {
                Some(format!("role {role:?} does not allow {action:?}"))
            },
        });
        if self.audit_log.len() > self.max_audit_entries {
            self.audit_log
                .drain(0..self.audit_log.len() - self.max_audit_entries);
        }
        allowed
    }

    /// Request HitL approval.
    pub fn request_approval(
        &mut self,
        subject: Subject,
        action: Action,
        resource: String,
        reason: String,
    ) -> Uuid {
        let id = Uuid::new_v4();
        self.pending_approvals.push((
            PendingApproval {
                id,
                subject,
                action,
                resource,
                reason,
                created_at: Utc::now(),
            },
            ApprovalStatus::Pending,
        ));
        id
    }

    /// Approve a request.
    pub fn approve(&mut self, id: Uuid) -> bool {
        if let Some((_, s)) = self
            .pending_approvals
            .iter_mut()
            .find(|(p, s)| p.id == id && *s == ApprovalStatus::Pending)
        {
            *s = ApprovalStatus::Approved;
            return true;
        }
        false
    }

    /// Reject a request.
    pub fn reject(&mut self, id: Uuid) -> bool {
        if let Some((_, s)) = self
            .pending_approvals
            .iter_mut()
            .find(|(p, s)| p.id == id && *s == ApprovalStatus::Pending)
        {
            *s = ApprovalStatus::Rejected;
            return true;
        }
        false
    }

    /// Pending approvals.
    pub fn pending_approvals(&self) -> Vec<&PendingApproval> {
        self.pending_approvals
            .iter()
            .filter(|(_, s)| matches!(s, ApprovalStatus::Pending))
            .map(|(p, _)| p)
            .collect()
    }

    /// All approvals with status.
    pub fn all_approvals(&self) -> &[(PendingApproval, ApprovalStatus)] {
        &self.pending_approvals
    }

    /// Audit log.
    pub fn audit_log(&self) -> &[RbacAuditEntry] {
        &self.audit_log
    }
}

impl Default for RbacManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_assignment() {
        let mut mgr = RbacManager::new();
        let s = Subject::User("alice".into());
        mgr.assign_role(s.clone(), Role::Admin);
        assert_eq!(mgr.get_role(&s), Some(Role::Admin));
        mgr.revoke_role(&s);
        assert_eq!(mgr.get_role(&s), None);
    }

    #[test]
    fn system_bypasses() {
        let mut mgr = RbacManager::new();
        assert!(mgr.check_permission(&Subject::System, &Action::ManageRBAC, "test"));
    }

    #[test]
    fn unknown_denied() {
        let mut mgr = RbacManager::new();
        assert!(!mgr.check_permission(
            &Subject::User("nobody".into()),
            &Action::UseTool("read".into()),
            "test"
        ));
    }

    #[test]
    fn admin_wildcard() {
        let mut mgr = RbacManager::new();
        let s = Subject::User("admin".into());
        mgr.assign_role(s.clone(), Role::Admin);
        assert!(mgr.check_permission(&s, &Action::UseTool("anything".into()), "test"));
    }

    #[test]
    fn approval_lifecycle() {
        let mut mgr = RbacManager::new();
        let id = mgr.request_approval(
            Subject::User("alice".into()),
            Action::ManageRBAC,
            "rbac".into(),
            "need admin".into(),
        );
        assert_eq!(mgr.pending_approvals().len(), 1);
        assert!(mgr.approve(id));
        assert!(mgr.pending_approvals().is_empty());
        assert!(!mgr.approve(id)); // already approved
    }
}
