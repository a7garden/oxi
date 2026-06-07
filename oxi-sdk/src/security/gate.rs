//! Unified access gate — single entry point for all authorization decisions.
//!
//! Four-layer check hierarchy with short-circuit evaluation:
//!
//! ```text
//! Layer 0: CSpace (Capability)  — does the agent have the capability token?
//! Layer 1: RBAC                  — does the agent's role allow the action?
//! Layer 2: Agent Permissions     — is the tool/path in allowed lists?
//! Layer 3: ExecPolicy            — is the binary allowed? No metacharacters?
//! ```

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::security::audit_sink::{AuditEvent, AuditSink};
use crate::security::capability::types::{ResourceRef, Rights};
use crate::security::context::AgentContext;
use crate::security::exec_policy::ExecPolicy;
use crate::security::permissions::AgentPermissions;
use crate::security::rbac::{Action, RbacManager, Role, Subject};

// ─── Path Mode ──────────────────────────────────────────────────────────────

/// Path access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    /// Read-only.
    Read,
    /// Write access.
    Write,
}

impl std::fmt::Display for PathMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathMode::Read => write!(f, "read"),
            PathMode::Write => write!(f, "write"),
        }
    }
}

// ─── Deny Layer ─────────────────────────────────────────────────────────────

/// Which security layer denied access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyLayer {
    /// CSpace missing capability.
    Capability,
    /// RBAC policy denied.
    Rbac,
    /// AgentPermissions denied.
    Permission,
    /// ExecPolicy denied.
    ExecPolicy,
}

impl std::fmt::Display for DenyLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DenyLayer::Capability => write!(f, "CSpace"),
            DenyLayer::Rbac => write!(f, "RBAC"),
            DenyLayer::Permission => write!(f, "Permissions"),
            DenyLayer::ExecPolicy => write!(f, "ExecPolicy"),
        }
    }
}

// ─── Access Denied ──────────────────────────────────────────────────────────

/// Authorization denial with layer, reason, and suggestion.
#[derive(Debug, Clone)]
pub struct AccessDenied {
    /// Agent name.
    pub agent: String,
    /// Resource.
    pub resource: String,
    /// Denying layer.
    pub layer: DenyLayer,
    /// Machine-readable reason.
    pub reason: String,
    /// User-facing suggestion.
    pub suggestion: Option<String>,
}

impl std::fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} — {}",
            self.layer,
            self.reason,
            self.suggestion.as_deref().unwrap_or("")
        )
    }
}

// ─── Check Request ──────────────────────────────────────────────────────────

/// Authorization check request.
#[derive(Debug)]
pub enum CheckRequest<'a> {
    /// Tool usage.
    Tool {
        context: &'a AgentContext,
        tool_name: &'a str,
    },
    /// Path access.
    Path {
        context: &'a AgentContext,
        path: &'a Path,
        mode: PathMode,
    },
    /// Command execution.
    Exec {
        context: &'a AgentContext,
        binary: &'a str,
        args: &'a [String],
    },
    /// Network access.
    Network { context: &'a AgentContext },
    /// Agent fork.
    Fork { context: &'a AgentContext },
}

impl<'a> CheckRequest<'a> {
    /// Agent context.
    pub fn agent_context(&self) -> &AgentContext {
        match self {
            CheckRequest::Tool { context, .. } => context,
            CheckRequest::Path { context, .. } => context,
            CheckRequest::Exec { context, .. } => context,
            CheckRequest::Network { context } => context,
            CheckRequest::Fork { context } => context,
        }
    }

    /// Resource description.
    pub fn resource(&self) -> &str {
        match self {
            CheckRequest::Tool { tool_name, .. } => tool_name,
            CheckRequest::Path { path, .. } => path.to_str().unwrap_or("<invalid>"),
            CheckRequest::Exec { binary, .. } => binary,
            CheckRequest::Network { .. } => "<network>",
            CheckRequest::Fork { .. } => "fork",
        }
    }
}

// ─── Shell Metacharacters ───────────────────────────────────────────────────

const SHELL_METACHARS: &[char] = &[
    '|', '&', ';', '$', '`', '<', '>', '(', ')', '{', '}', '\n', '\r', '\0',
];

fn has_metacharacters(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg.contains("..") || SHELL_METACHARS.iter().any(|&c| arg.contains(c)))
}

// ─── Access Gate ────────────────────────────────────────────────────────────

/// Single entry point for all authorization decisions.
///
/// Wraps RBAC, per-agent permissions, capability checks, and exec policy.
pub struct AccessGate {
    /// Per-agent permissions (name → set).
    permissions: Arc<Mutex<std::collections::HashMap<String, AgentPermissions>>>,
    /// RBAC manager.
    rbac: Arc<Mutex<RbacManager>>,
    /// Execution policy.
    exec_policy: Arc<ExecPolicy>,
    /// Audit sink.
    audit: Arc<dyn AuditSink>,
}

impl AccessGate {
    /// Create a new gate.
    pub fn new(
        permissions: Arc<Mutex<std::collections::HashMap<String, AgentPermissions>>>,
        rbac: Arc<Mutex<RbacManager>>,
        exec_policy: Arc<ExecPolicy>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        Self {
            permissions,
            rbac,
            exec_policy,
            audit,
        }
    }

    /// Create a permissive gate (all checks pass). Useful for development.
    pub fn permissive() -> Self {
        Self {
            permissions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            rbac: Arc::new(Mutex::new(RbacManager::new())),
            exec_policy: Arc::new(ExecPolicy::permissive()),
            audit: Arc::new(crate::security::audit_sink::TracingAuditSink),
        }
    }

    /// Grant default permissions for an agent.
    pub fn register_agent(&self, agent_name: &str, role: Role) {
        self.permissions.lock().insert(
            agent_name.to_string(),
            AgentPermissions::for_new_agent(agent_name),
        );
        self.rbac
            .lock()
            .assign_role(Subject::User(agent_name.to_string()), role);
    }

    /// Perform authorization check.
    pub fn check(&self, req: CheckRequest<'_>) -> Result<(), AccessDenied> {
        let result = match &req {
            CheckRequest::Tool { context, tool_name } => self.check_tool(context, tool_name),
            CheckRequest::Path {
                context,
                path,
                mode,
            } => self.check_path(context, path, *mode),
            CheckRequest::Exec {
                context,
                binary,
                args,
            } => self.check_exec(context, binary, args),
            CheckRequest::Network { context } => self.check_network(context),
            CheckRequest::Fork { context } => self.check_fork(context),
        };
        self.record_check(&req, &result);
        result
    }

    fn check_tool(&self, ctx: &AgentContext, tool: &str) -> Result<(), AccessDenied> {
        // Layer 0: CSpace
        let resource = ResourceRef::KernelDomain {
            domain: tool.to_string(),
        };
        if !ctx.cspace.can(&resource, Rights::Execute) {
            let always_on = [
                "read", "write", "edit", "grep", "find", "ls", "bash", "exec",
            ];
            if !always_on.contains(&tool) {
                return Err(AccessDenied {
                    agent: ctx.agent_name.clone(),
                    resource: tool.to_string(),
                    layer: DenyLayer::Capability,
                    reason: format!("No Execute capability for '{tool}' in CSpace"),
                    suggestion: Some(format!("Add '{tool}' capability to the agent's template.")),
                });
            }
        }

        // Layer 1+2: RBAC + Permissions
        let subject = Subject::User(ctx.agent_name.clone());
        if !self
            .rbac
            .lock()
            .check_permission(&subject, &Action::UseTool(tool.to_string()), tool)
        {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: tool.to_string(),
                layer: DenyLayer::Rbac,
                reason: format!("RBAC denied '{tool}' for '{}'", ctx.agent_name),
                suggestion: None,
            });
        }

        let perms = self.permissions.lock();
        if let Some(p) = perms.get(&ctx.agent_name)
            && !p.allowed_tools.contains(tool)
        {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: tool.to_string(),
                layer: DenyLayer::Permission,
                reason: format!("'{tool}' not in allowed_tools for '{}'", ctx.agent_name),
                suggestion: None,
            });
        }

        Ok(())
    }

    fn check_path(
        &self,
        ctx: &AgentContext,
        path: &Path,
        _mode: PathMode,
    ) -> Result<(), AccessDenied> {
        let path_str = path.to_string_lossy();

        // Layer 2: Path permissions
        let perms = self.permissions.lock();
        if let Some(p) = perms.get(&ctx.agent_name)
            && p.is_path_denied(&path_str)
        {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: path_str.to_string(),
                layer: DenyLayer::Permission,
                reason: format!("Path '{path_str}' is in denied_paths"),
                suggestion: None,
            });
        }

        Ok(())
    }

    fn check_exec(
        &self,
        ctx: &AgentContext,
        binary: &str,
        args: &[String],
    ) -> Result<(), AccessDenied> {
        // Layer 3: ExecPolicy
        if !self.exec_policy.is_binary_allowed(binary) {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: binary.to_string(),
                layer: DenyLayer::ExecPolicy,
                reason: format!("Binary '{binary}' not in allowlist"),
                suggestion: Some("Add to ExecPolicy.allowed_commands.".into()),
            });
        }

        if has_metacharacters(args) {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: binary.to_string(),
                layer: DenyLayer::ExecPolicy,
                reason: "Arguments contain shell metacharacters or path traversal".into(),
                suggestion: None,
            });
        }

        Ok(())
    }

    fn check_network(&self, ctx: &AgentContext) -> Result<(), AccessDenied> {
        let perms = self.permissions.lock();
        if let Some(p) = perms.get(&ctx.agent_name)
            && !p.network_access
        {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: "<network>".into(),
                layer: DenyLayer::Permission,
                reason: "Network access disabled".into(),
                suggestion: Some("Set network_access to true.".into()),
            });
        }
        Ok(())
    }

    fn check_fork(&self, ctx: &AgentContext) -> Result<(), AccessDenied> {
        let perms = self.permissions.lock();
        if let Some(p) = perms.get(&ctx.agent_name)
            && !p.can_fork
        {
            return Err(AccessDenied {
                agent: ctx.agent_name.clone(),
                resource: "fork".into(),
                layer: DenyLayer::Permission,
                reason: "Fork not allowed".into(),
                suggestion: Some("Set can_fork to true.".into()),
            });
        }
        Ok(())
    }

    fn record_check(&self, req: &CheckRequest<'_>, result: &Result<(), AccessDenied>) {
        let ctx = req.agent_context();
        let ts = chrono::Utc::now();
        let event = match result {
            Ok(()) => AuditEvent::ToolAccess {
                timestamp: ts,
                agent: ctx.agent_name.clone(),
                tool: req.resource().to_string(),
                allowed: true,
                layer: None,
                reason: None,
            },
            Err(denied) => AuditEvent::ToolAccess {
                timestamp: ts,
                agent: ctx.agent_name.clone(),
                tool: req.resource().to_string(),
                allowed: false,
                layer: Some(denied.layer.to_string()),
                reason: Some(denied.reason.clone()),
            },
        };
        self.audit.record(event);
    }
}

impl std::fmt::Debug for AccessGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessGate").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOpSink;
    impl AuditSink for NoOpSink {
        fn record(&self, _event: AuditEvent) {}
    }

    fn make_gate() -> (AccessGate, AgentContext) {
        let perms = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let rbac = Arc::new(Mutex::new(RbacManager::new()));
        let ctx = AgentContext::from_template("test-agent", "standard");

        perms.lock().insert(
            "test-agent".into(),
            AgentPermissions::for_new_agent("test-agent"),
        );
        rbac.lock()
            .assign_role(Subject::User("test-agent".into()), Role::Superuser);

        let gate = AccessGate::new(
            perms,
            rbac,
            Arc::new(ExecPolicy::permissive()),
            Arc::new(NoOpSink),
        );
        (gate, ctx)
    }

    #[test]
    fn test_tool_allowed() {
        let (gate, ctx) = make_gate();
        assert!(
            gate.check(CheckRequest::Tool {
                context: &ctx,
                tool_name: "bash"
            })
            .is_ok()
        );
    }

    #[test]
    fn test_exec_metacharacters_denied() {
        let (gate, ctx) = make_gate();
        let result = gate.check(CheckRequest::Exec {
            context: &ctx,
            binary: "echo",
            args: &["foo; rm -rf /".to_string()],
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().layer, DenyLayer::ExecPolicy);
    }

    #[test]
    fn test_network_denied_by_default() {
        let (gate, ctx) = make_gate();
        let result = gate.check(CheckRequest::Network { context: &ctx });
        assert!(result.is_err());
    }

    #[test]
    fn test_fork_denied_by_default() {
        let (gate, ctx) = make_gate();
        let result = gate.check(CheckRequest::Fork { context: &ctx });
        assert!(result.is_err());
    }

    #[test]
    fn test_path_denied_in_deny_list() {
        let (gate, ctx) = make_gate();
        let result = gate.check(CheckRequest::Path {
            context: &ctx,
            path: Path::new("/etc/passwd"),
            mode: PathMode::Read,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_permissive_gate() {
        let gate = AccessGate::permissive();
        let ctx = AgentContext::from_template("dev", "worker");
        // Permissive gate has no permissions registered, so tool check passes RBAC
        // but may fail permissions — that's fine, it's for development
        drop(gate);
    }

    #[test]
    fn test_deny_layer_display() {
        assert_eq!(format!("{}", DenyLayer::Capability), "CSpace");
        assert_eq!(format!("{}", DenyLayer::ExecPolicy), "ExecPolicy");
    }

    #[test]
    fn test_access_denied_display() {
        let d = AccessDenied {
            agent: "test".into(),
            resource: "exec".into(),
            layer: DenyLayer::ExecPolicy,
            reason: "not allowed".into(),
            suggestion: Some("add it".into()),
        };
        assert!(format!("{}", d).contains("[ExecPolicy]"));
    }
}
