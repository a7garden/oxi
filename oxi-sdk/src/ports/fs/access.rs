//! Simple rule-based `AccessGate` — TOML allow/deny list per tool.
//!
//! Reads `<path>/access.toml` with this schema:
//!
//! ```toml
//! [rules.bash]
//! # Patterns are substring matches on `request.action` (the command line).
//! deny = ["rm -rf /", "rm -rf ~", ":(){:|:&};"]    # catastrophic commands
//! require_approval = ["sudo ", "apt ", "brew "]
//!
//! [rules.write]
//! deny = ["/etc/", "/usr/"]
//! require_approval = [".ssh/", ".aws/credentials"]
//!
//! [rules.edit]
//! require_approval = [".git/"]
//! ```
//!
//! Resolution order per request: `deny` → `require_approval` → `Allow`.
//! Patterns are matched as substrings on `request.action`.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use crate::SdkError;
use crate::ports::{AccessDecision, AccessGate, ToolCallRequest};

/// Rule-based gate. Pure sync, no I/O at request time.
pub struct SimpleAccessGate {
    rules: parking_lot::RwLock<Rules>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for SimpleAccessGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleAccessGate")
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
struct Rules {
    #[serde(default)]
    rules: std::collections::BTreeMap<String, ToolRule>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct ToolRule {
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    require_approval: Vec<String>,
}

impl SimpleAccessGate {
    /// Create a gate that allows everything.
    pub fn permissive() -> Self {
        Self {
            rules: parking_lot::RwLock::new(Rules::default()),
            path: None,
        }
    }

    /// Load rules from a TOML file. If the file does not exist, the gate
    /// is permissive (allows all).
    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let rules = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Rules::default()
        };
        Self {
            rules: parking_lot::RwLock::new(rules),
            path: Some(path),
        }
    }

    /// Reload rules from disk. Replaces the current in-memory rules.
    pub fn reload(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = std::fs::read_to_string(path)?;
        let parsed: Rules = toml::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        *self.rules.write() = parsed;
        Ok(())
    }
}

#[async_trait]
impl AccessGate for SimpleAccessGate {
    async fn check(&self, request: &ToolCallRequest) -> Result<AccessDecision, SdkError> {
        let rules = self.rules.read();
        if let Some(rule) = rules.rules.get(&request.tool) {
            for pat in &rule.deny {
                if request.action.contains(pat) {
                    return Ok(AccessDecision::Deny {
                        reason: format!("matches deny pattern: {pat}"),
                    });
                }
            }
            for pat in &rule.require_approval {
                if request.action.contains(pat) {
                    return Ok(AccessDecision::RequireApproval {
                        reason: format!("matches approval pattern: {pat}"),
                    });
                }
            }
        }
        Ok(AccessDecision::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn req(tool: &str, action: &str) -> ToolCallRequest {
        ToolCallRequest {
            tool: tool.into(),
            action: action.into(),
            cwd: std::path::PathBuf::from("/tmp"),
            subject: "test".into(),
        }
    }

    #[tokio::test]
    async fn permissive_allows_all() {
        let g = SimpleAccessGate::permissive();
        let d = g.check(&req("bash", "ls -la")).await.unwrap();
        assert_eq!(d, AccessDecision::Allow);
    }

    #[tokio::test]
    async fn deny_pattern_blocks() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("access.toml");
        fs::write(
            &p,
            r#"[rules.bash]
deny = ["rm -rf /"]
"#,
        )
        .unwrap();
        let g = SimpleAccessGate::from_file(&p);
        let d = g.check(&req("bash", "rm -rf /")).await.unwrap();
        assert!(matches!(d, AccessDecision::Deny { .. }));
    }

    #[tokio::test]
    async fn approval_pattern_pauses() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("access.toml");
        fs::write(
            &p,
            r#"[rules.bash]
require_approval = ["sudo "]
"#,
        )
        .unwrap();
        let g = SimpleAccessGate::from_file(&p);
        let d = g.check(&req("bash", "sudo apt update")).await.unwrap();
        assert!(matches!(d, AccessDecision::RequireApproval { .. }));
    }

    #[tokio::test]
    async fn unmatched_action_allows() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("access.toml");
        fs::write(
            &p,
            r#"[rules.bash]
deny = ["rm -rf /"]
require_approval = ["sudo "]
"#,
        )
        .unwrap();
        let g = SimpleAccessGate::from_file(&p);
        let d = g.check(&req("bash", "ls -la")).await.unwrap();
        assert_eq!(d, AccessDecision::Allow);
    }

    #[tokio::test]
    async fn reload_picks_up_changes() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("access.toml");
        fs::write(&p, "[rules.bash]\ndeny = [\"old-pattern\"]\n").unwrap();
        let g = SimpleAccessGate::from_file(&p);
        // First: old pattern denied.
        let d1 = g.check(&req("bash", "old-pattern")).await.unwrap();
        assert!(matches!(d1, AccessDecision::Deny { .. }));
        // Update file.
        fs::write(&p, "[rules.bash]\ndeny = [\"new-pattern\"]\n").unwrap();
        g.reload().unwrap();
        // Old pattern now allowed.
        let d2 = g.check(&req("bash", "old-pattern")).await.unwrap();
        assert_eq!(d2, AccessDecision::Allow);
        // New pattern denied.
        let d3 = g.check(&req("bash", "new-pattern")).await.unwrap();
        assert!(matches!(d3, AccessDecision::Deny { .. }));
    }
}
