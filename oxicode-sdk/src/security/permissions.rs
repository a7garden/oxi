//! Agent permissions — per-agent permission sets and audit entries.

use chrono::{DateTime, Utc};
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Permissions for a single agent (least-privilege defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPermissions {
    /// Agent name.
    pub agent_name: String,
    /// Allowed tool names.
    #[serde(default)]
    pub allowed_tools: HashSet<String>,
    /// Allowed path glob patterns.
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// Denied path glob patterns (takes precedence).
    #[serde(default)]
    pub denied_paths: Vec<String>,
    /// Network access.
    #[serde(default)]
    pub network_access: bool,
    /// Max execution time in seconds (0 = unlimited).
    #[serde(default)]
    pub max_execution_time_secs: u64,
    /// Max memory in MB (0 = unlimited).
    #[serde(default)]
    pub max_memory_mb: u64,
    /// Can spawn sub-agents.
    #[serde(default)]
    pub can_fork: bool,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            agent_name: String::new(),
            allowed_tools: ["read", "write", "edit", "bash", "grep", "find", "exec"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            allowed_paths: vec!["/workspace/**".to_string()],
            denied_paths: vec![
                "/etc/**".to_string(),
                "/root/**".to_string(),
                "/sys/**".to_string(),
                "/proc/**".to_string(),
                ".oxios/**".to_string(),
            ],
            network_access: false,
            max_execution_time_secs: 300,
            max_memory_mb: 512,
            can_fork: false,
        }
    }
}

impl AgentPermissions {
    /// Create default permissions for a new agent.
    pub fn for_new_agent(agent_name: &str) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            ..Default::default()
        }
    }

    /// Allow a tool.
    pub fn allow_tool(&mut self, tool: &str) {
        self.allowed_tools.insert(tool.to_string());
    }

    /// Deny a tool.
    pub fn deny_tool(&mut self, tool: &str) {
        self.allowed_tools.remove(tool);
    }

    /// Allow a path pattern (dedup).
    pub fn allow_path(&mut self, path: &str) {
        if !self.allowed_paths.contains(&path.to_string()) {
            self.allowed_paths.push(path.to_string());
        }
    }

    /// Deny a path pattern (dedup).
    pub fn deny_path(&mut self, path: &str) {
        if !self.denied_paths.contains(&path.to_string()) {
            self.denied_paths.push(path.to_string());
        }
    }

    /// Enable network access.
    pub fn enable_network(&mut self) {
        self.network_access = true;
    }

    /// Enable agent forking.
    pub fn enable_forking(&mut self) {
        self.can_fork = true;
    }

    /// Check if a path matches any denied pattern.
    pub fn is_path_denied(&self, path: &str) -> bool {
        for pattern in &self.denied_paths {
            if let Ok(p) = Pattern::new(pattern)
                && p.matches(path)
            {
                return true;
            }
        }
        false
    }

    /// Check if a path matches any allowed pattern.
    pub fn is_path_allowed(&self, path: &str) -> bool {
        for pattern in &self.allowed_paths {
            if let Ok(p) = Pattern::new(pattern)
                && p.matches(path)
            {
                return true;
            }
        }
        false
    }
}

/// Partial update for permissions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionUpdate {
    /// Override allowed tools.
    pub allowed_tools: Option<HashSet<String>>,
    /// Override allowed paths.
    pub allowed_paths: Option<Vec<String>>,
    /// Override denied paths.
    pub denied_paths: Option<Vec<String>>,
    /// Override network access.
    pub network_access: Option<bool>,
    /// Override max execution time.
    pub max_execution_time_secs: Option<u64>,
    /// Override max memory.
    pub max_memory_mb: Option<u64>,
    /// Override can_fork.
    pub can_fork: Option<bool>,
}

impl PermissionUpdate {
    /// Apply non-None fields to target.
    pub fn apply(&self, perms: &mut AgentPermissions) {
        if let Some(tools) = &self.allowed_tools {
            perms.allowed_tools = tools.clone();
        }
        if let Some(paths) = &self.allowed_paths {
            perms.allowed_paths = paths.clone();
        }
        if let Some(paths) = &self.denied_paths {
            perms.denied_paths = paths.clone();
        }
        if let Some(v) = self.network_access {
            perms.network_access = v;
        }
        if let Some(v) = self.max_execution_time_secs {
            perms.max_execution_time_secs = v;
        }
        if let Some(v) = self.max_memory_mb {
            perms.max_memory_mb = v;
        }
        if let Some(v) = self.can_fork {
            perms.can_fork = v;
        }
    }
}

/// Security audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermAuditEntry {
    /// When.
    pub timestamp: DateTime<Utc>,
    /// Agent.
    pub agent_name: String,
    /// Action type.
    pub action: String,
    /// Resource.
    pub resource: String,
    /// Allowed?
    pub allowed: bool,
    /// Reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl PermAuditEntry {
    /// Create a new audit entry.
    pub fn new(
        agent_name: &str,
        action: &str,
        resource: &str,
        allowed: bool,
        reason: Option<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            agent_name: agent_name.to_string(),
            action: action.to_string(),
            resource: resource.to_string(),
            allowed,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_basic_tools() {
        let p = AgentPermissions::default();
        assert!(p.allowed_tools.contains("read"));
        assert!(p.allowed_tools.contains("bash"));
        assert!(!p.network_access);
    }

    #[test]
    fn denies_sensitive_paths() {
        let p = AgentPermissions::default();
        assert!(p.is_path_denied("/etc/passwd"));
        assert!(p.is_path_denied("/root/.ssh/id_rsa"));
    }

    #[test]
    fn allows_workspace() {
        let p = AgentPermissions::default();
        assert!(p.is_path_allowed("/workspace/src/main.rs"));
        assert!(!p.is_path_allowed("/tmp/evil"));
    }

    #[test]
    fn partial_update() {
        let mut p = AgentPermissions::for_new_agent("a");
        let update = PermissionUpdate {
            network_access: Some(true),
            ..Default::default()
        };
        update.apply(&mut p);
        assert!(p.network_access);
        assert!(!p.can_fork);
    }
}
