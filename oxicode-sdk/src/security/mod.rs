//! Capability-based security module.
//!
//! Provides fine-grained, deny-by-default permissions for agents:
//! - **Capability**: individual permission (FileRead, Bash, etc.)
//! - **CapabilitySet**: named preset bundles (coding, read_only, etc.)
//! - **Authorizer**: grant/check/revoke with role hierarchy
//! - **SecurityMiddleware**: tool execution guard via Middleware trait

mod audit_sink;
mod authorizer;
pub mod capability;
mod context;
mod exec_policy;
mod gate;
pub mod middleware;
mod permissions;
mod rbac;

pub use audit_sink::{AuditEvent, AuditSink, TracingAuditSink, TrailAuditSink};
pub use authorizer::{Authorizer, DefaultPolicy};
pub use capability::{Capability, CapabilitySet, CapabilitySubject, StringPattern};
pub use context::AgentContext;
pub use exec_policy::{AllowlistMode, ExecPolicy};
pub use gate::{AccessDenied, AccessGate, CheckRequest, DenyLayer, PathMode};
pub use middleware::SecurityMiddleware;
pub use permissions::{AgentPermissions, PermAuditEntry, PermissionUpdate};
pub use rbac::{
    Action, ApprovalStatus, PendingApproval, RbacAuditEntry, RbacManager, RbacPolicy, Role, Subject,
};
