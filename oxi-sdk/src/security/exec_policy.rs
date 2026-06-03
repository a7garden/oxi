//! Execution policy configuration for command allowlisting.
//!
//! Defines which binaries agents can execute and how arguments are validated.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// How the execution allowlist is enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AllowlistMode {
    /// All binaries allowed (default for development).
    #[default]
    Permissive,
    /// Only explicitly listed binaries allowed.
    Enforced,
}

/// Execution policy — controls which binaries agents can run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecPolicy {
    /// Allowlist enforcement mode.
    #[serde(default)]
    pub allowlist_mode: AllowlistMode,
    /// Explicitly allowed binary names.
    #[serde(default)]
    pub allowed_commands: HashSet<String>,
    /// Default safe commands always allowed.
    #[serde(default)]
    pub default_safe_commands: HashSet<String>,
}

impl ExecPolicy {
    /// Create a permissive policy (all allowed).
    pub fn permissive() -> Self {
        Self {
            allowlist_mode: AllowlistMode::Permissive,
            allowed_commands: HashSet::new(),
            default_safe_commands: Self::safe_defaults(),
        }
    }

    /// Create an enforced policy with only the given commands.
    pub fn enforced(commands: Vec<&str>) -> Self {
        Self {
            allowlist_mode: AllowlistMode::Enforced,
            allowed_commands: commands.into_iter().map(String::from).collect(),
            default_safe_commands: Self::safe_defaults(),
        }
    }

    /// Check if a binary is allowed.
    pub fn is_binary_allowed(&self, binary: &str) -> bool {
        match self.allowlist_mode {
            AllowlistMode::Permissive => true,
            AllowlistMode::Enforced => {
                self.allowed_commands.contains(binary)
                    || self.default_safe_commands.contains(binary)
            }
        }
    }

    fn safe_defaults() -> HashSet<String> {
        [
            "git", "grep", "find", "cat", "ls", "head", "tail", "wc", "sort", "uniq",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self::permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissive_allows_all() {
        let policy = ExecPolicy::permissive();
        assert!(policy.is_binary_allowed("rm"));
        assert!(policy.is_binary_allowed("anything"));
    }

    #[test]
    fn enforced_allows_listed() {
        let policy = ExecPolicy::enforced(vec!["echo", "git"]);
        assert!(policy.is_binary_allowed("echo"));
        assert!(policy.is_binary_allowed("git"));
        assert!(!policy.is_binary_allowed("rm"));
    }

    #[test]
    fn enforced_safe_defaults() {
        let policy = ExecPolicy::enforced(vec![]);
        // Safe defaults should always be allowed
        assert!(policy.is_binary_allowed("git"));
        assert!(policy.is_binary_allowed("grep"));
        assert!(policy.is_binary_allowed("cat"));
    }
}
