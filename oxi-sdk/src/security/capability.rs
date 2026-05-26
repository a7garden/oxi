//! Capability types — fine-grained permission definitions.

use serde::{Deserialize, Serialize};
use std::time::Duration;

// ── Capability ─────────────────────────────────────────────────────────────

/// A fine-grained permission that can be granted to an agent.
///
/// Each variant captures the specific resource and access level.
/// The `Authorizer` checks these at tool execution time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Capability {
    // ── File system ──
    FileRead {
        path_pattern: String,
    },
    FileWrite {
        path_pattern: String,
    },
    FileEdit {
        path_pattern: String,
    },
    FileList {
        path_pattern: String,
    },
    FileFind {
        path_pattern: String,
    },

    // ── Execution ──
    Bash {
        allowed_commands: Vec<StringPattern>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },

    // ── Network ──
    Network {
        allowed_domains: Vec<String>,
    },
    WebBrowse {
        allowed_domains: Vec<String>,
    },

    // ── Agent ──
    Subagent {
        max_children: Option<usize>,
    },
    BusRead {
        channel: Option<String>,
    },
    BusWrite {
        channel: Option<String>,
    },

    // ── Environment ──
    EnvRead {
        allowed_vars: Vec<String>,
    },

    // ── Meta ──
    ToolUse {
        tool_name: String,
    },
    McpAccess {
        resource_patterns: Vec<String>,
    },
}

impl Capability {
    /// Whether this capability satisfies (is at least as permissive as) `required`.
    pub fn satisfies(&self, required: &Capability) -> bool {
        match (self, required) {
            // Same type: check pattern subset
            (
                Capability::FileRead { path_pattern: a },
                Capability::FileRead { path_pattern: b },
            )
            | (
                Capability::FileWrite { path_pattern: a },
                Capability::FileWrite { path_pattern: b },
            )
            | (
                Capability::FileEdit { path_pattern: a },
                Capability::FileEdit { path_pattern: b },
            )
            | (
                Capability::FileList { path_pattern: a },
                Capability::FileList { path_pattern: b },
            )
            | (
                Capability::FileFind { path_pattern: a },
                Capability::FileFind { path_pattern: b },
            ) => pattern_matches(a, b),

            (
                Capability::Bash {
                    allowed_commands: a,
                    ..
                },
                Capability::Bash {
                    allowed_commands: b,
                    ..
                },
            ) => {
                // If `a` contains Wildcard, it satisfies any Bash request
                if a.iter().any(|p| matches!(p, StringPattern::Wildcard)) {
                    return true;
                }
                // Check that every required command is in `a`
                b.iter()
                    .all(|req| a.iter().any(|cap| string_pattern_matches(cap, req)))
            }

            (
                Capability::Network { allowed_domains: a },
                Capability::Network { allowed_domains: b },
            )
            | (
                Capability::WebBrowse { allowed_domains: a },
                Capability::WebBrowse { allowed_domains: b },
            ) => domain_matches(a, b),

            (
                Capability::Subagent { max_children: a },
                Capability::Subagent { max_children: b },
            ) => match (a, b) {
                (None, _) => true, // unlimited
                (Some(a_max), Some(b_max)) => a_max >= b_max,
                (Some(_), None) => false, // required is unlimited but granted is limited
            },

            (Capability::BusRead { channel: a }, Capability::BusRead { channel: b })
            | (Capability::BusWrite { channel: a }, Capability::BusWrite { channel: b }) => {
                match (a, b) {
                    (None, _) => true, // all channels
                    (Some(_), None) => false,
                    (Some(a_ch), Some(b_ch)) => a_ch == b_ch,
                }
            }

            (Capability::EnvRead { allowed_vars: a }, Capability::EnvRead { allowed_vars: b }) => {
                b.iter().all(|req| a.iter().any(|cap| cap == req)) || a.contains(&"*".to_string())
            }

            (Capability::ToolUse { tool_name: a }, Capability::ToolUse { tool_name: b }) => {
                a == b || a == "*"
            }

            (
                Capability::McpAccess {
                    resource_patterns: a,
                },
                Capability::McpAccess {
                    resource_patterns: b,
                },
            ) => a.iter().any(|p| p == "*") || b.iter().all(|req| a.contains(req)),

            _ => false,
        }
    }
}

// ── StringPattern ──────────────────────────────────────────────────────────

/// Pattern for matching strings — either a literal or a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StringPattern {
    Literal(String),
    Wildcard,
}

/// Check if a granted string pattern satisfies a required one.
fn string_pattern_matches(granted: &StringPattern, required: &StringPattern) -> bool {
    match (granted, required) {
        (StringPattern::Wildcard, _) => true,
        (StringPattern::Literal(a), StringPattern::Literal(b)) => a == b,
        (StringPattern::Literal(_), StringPattern::Wildcard) => false,
    }
}

/// Check if a glob-like path pattern matches a specific path.
/// Supports `**` (recursive), `*` (single segment).
fn pattern_matches(pattern: &str, path: &str) -> bool {
    if pattern == "*" || pattern == "**" || pattern == "/**" {
        return true;
    }
    if pattern == path {
        return true;
    }
    // Simple suffix matching for patterns like "/workspace/**"
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path.starts_with(prefix)
            || path.starts_with(&format!("{}/", prefix.trim_end_matches('/')));
    }
    if let Some(prefix) = pattern.strip_suffix("*") {
        return path.starts_with(prefix);
    }
    false
}

/// Check if granted domains satisfy required domains.
fn domain_matches(granted: &[String], required: &[String]) -> bool {
    if granted.contains(&"*".to_string()) {
        return true;
    }
    required.iter().all(|req| {
        granted.iter().any(|g| {
            if g == "*" {
                return true;
            }
            if g == req {
                return true;
            }
            // Subdomain matching: "*.example.com" matches "sub.example.com"
            if let Some(suffix) = g.strip_prefix("*.") {
                req.ends_with(&format!(".{}", suffix)) || req == suffix
            } else {
                false
            }
        })
    })
}

// ── CapabilitySubject ──────────────────────────────────────────────────────

/// The subject of a capability grant or check.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilitySubject {
    Agent(String),
    Tool(String),
    Group(String),
}

impl std::fmt::Display for CapabilitySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilitySubject::Agent(id) => write!(f, "agent:{}", id),
            CapabilitySubject::Tool(name) => write!(f, "tool:{}", name),
            CapabilitySubject::Group(name) => write!(f, "group:{}", name),
        }
    }
}

// ── CapabilitySet ──────────────────────────────────────────────────────────

/// A set of capabilities with an optional expiration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySet {
    capabilities: Vec<Capability>,
    expires_at_ms: Option<u64>,
}

impl CapabilitySet {
    /// Create a new set from capabilities.
    pub fn new(capabilities: Vec<Capability>) -> Self {
        Self {
            capabilities,
            expires_at_ms: None,
        }
    }

    // ── Presets ──

    /// All capabilities — for system agents.
    pub fn all() -> Self {
        Self::new(vec![
            Capability::FileRead {
                path_pattern: "/**".into(),
            },
            Capability::FileWrite {
                path_pattern: "/**".into(),
            },
            Capability::FileEdit {
                path_pattern: "/**".into(),
            },
            Capability::FileList {
                path_pattern: "/**".into(),
            },
            Capability::FileFind {
                path_pattern: "/**".into(),
            },
            Capability::Bash {
                allowed_commands: vec![StringPattern::Wildcard],
                timeout_secs: None,
            },
            Capability::Network {
                allowed_domains: vec!["*".into()],
            },
            Capability::WebBrowse {
                allowed_domains: vec!["*".into()],
            },
            Capability::Subagent { max_children: None },
            Capability::BusRead { channel: None },
            Capability::BusWrite { channel: None },
            Capability::EnvRead {
                allowed_vars: vec!["*".into()],
            },
            Capability::ToolUse {
                tool_name: "*".into(),
            },
        ])
    }

    /// Read-only access — for research agents.
    pub fn read_only(workspace: &str) -> Self {
        let ws = workspace.to_string();
        Self::new(vec![
            Capability::FileRead {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileList {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileFind {
                path_pattern: format!("{}/**", ws),
            },
            Capability::BusRead { channel: None },
        ])
    }

    /// Standard coding agent permissions.
    pub fn coding(workspace: &str) -> Self {
        let ws = workspace.to_string();
        Self::new(vec![
            Capability::FileRead {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileWrite {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileEdit {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileList {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileFind {
                path_pattern: format!("{}/**", ws),
            },
            Capability::Bash {
                allowed_commands: vec![
                    StringPattern::Literal("git".into()),
                    StringPattern::Literal("cargo".into()),
                    StringPattern::Literal("npm".into()),
                    StringPattern::Literal("node".into()),
                    StringPattern::Literal("python3".into()),
                    StringPattern::Literal("ls".into()),
                    StringPattern::Literal("cat".into()),
                    StringPattern::Literal("grep".into()),
                    StringPattern::Literal("rg".into()),
                    StringPattern::Literal("find".into()),
                    StringPattern::Literal("mkdir".into()),
                    StringPattern::Literal("cp".into()),
                    StringPattern::Literal("mv".into()),
                ],
                timeout_secs: Some(30),
            },
            Capability::Subagent {
                max_children: Some(2),
            },
            Capability::BusRead { channel: None },
        ])
    }

    /// Research agent — read + web browsing, no writes.
    pub fn research(workspace: &str) -> Self {
        let ws = workspace.to_string();
        Self::new(vec![
            Capability::FileRead {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileList {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileFind {
                path_pattern: format!("{}/**", ws),
            },
            Capability::Network {
                allowed_domains: vec!["*".into()],
            },
            Capability::WebBrowse {
                allowed_domains: vec!["*".into()],
            },
            Capability::BusRead { channel: None },
        ])
    }

    /// Browser agent — read + browse + output-only writes.
    pub fn browser(workspace: &str) -> Self {
        let ws = workspace.to_string();
        Self::new(vec![
            Capability::FileRead {
                path_pattern: format!("{}/**", ws),
            },
            Capability::FileWrite {
                path_pattern: format!("{}/output/**", ws),
            },
            Capability::Network {
                allowed_domains: vec!["*".into()],
            },
            Capability::WebBrowse {
                allowed_domains: vec!["*".into()],
            },
        ])
    }

    // ── Builder ──

    /// Add a capability.
    pub fn add(&mut self, cap: Capability) -> &mut Self {
        self.capabilities.push(cap);
        self
    }

    /// Set TTL for this capability set.
    pub fn with_ttl(mut self, duration: Duration) -> Self {
        let expires = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64 + duration.as_millis() as u64)
            .unwrap_or(u64::MAX);
        self.expires_at_ms = Some(expires);
        self
    }

    /// Check if the set has expired.
    pub fn is_expired(&self) -> bool {
        match self.expires_at_ms {
            Some(expires) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                now > expires
            }
            None => false,
        }
    }

    /// Access the capabilities.
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Check if any capability in this set satisfies `required`.
    pub fn satisfies(&self, required: &Capability) -> bool {
        self.capabilities.iter().any(|cap| cap.satisfies(required))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_capability_satisfies() {
        let cap = Capability::FileRead {
            path_pattern: "/workspace/**".into(),
        };
        let req = Capability::FileRead {
            path_pattern: "/workspace/src/main.rs".into(),
        };
        assert!(cap.satisfies(&req));

        let denied = Capability::FileRead {
            path_pattern: "/etc/passwd".into(),
        };
        assert!(!cap.satisfies(&denied));
    }

    #[test]
    fn bash_wildcard_satisfies() {
        let cap = Capability::Bash {
            allowed_commands: vec![StringPattern::Wildcard],
            timeout_secs: None,
        };
        let req = Capability::Bash {
            allowed_commands: vec![StringPattern::Literal("rm".into())],
            timeout_secs: None,
        };
        assert!(cap.satisfies(&req));
    }

    #[test]
    fn domain_matching() {
        let granted = vec!["*.example.com".to_string()];
        let required = vec!["sub.example.com".to_string()];
        assert!(domain_matches(&granted, &required));

        let denied = vec!["other.com".to_string()];
        assert!(!domain_matches(&granted, &denied));
    }

    #[test]
    fn capability_set_coding_satisfies() {
        let set = CapabilitySet::coding("/workspace");
        assert!(set.satisfies(&Capability::FileRead {
            path_pattern: "/workspace/src/main.rs".into()
        }));
        assert!(!set.satisfies(&Capability::FileWrite {
            path_pattern: "/etc/passwd".into()
        }));
    }

    #[test]
    fn capability_set_read_only() {
        let set = CapabilitySet::read_only("/ws");
        assert!(set.satisfies(&Capability::FileRead {
            path_pattern: "/ws/any".into()
        }));
        assert!(!set.satisfies(&Capability::FileWrite {
            path_pattern: "/ws/any".into()
        }));
    }

    #[test]
    fn capability_set_all() {
        let set = CapabilitySet::all();
        assert!(set.satisfies(&Capability::FileRead {
            path_pattern: "/anything".into()
        }));
        assert!(set.satisfies(&Capability::FileWrite {
            path_pattern: "/anything".into()
        }));
        assert!(set.satisfies(&Capability::Bash {
            allowed_commands: vec![StringPattern::Literal("anything".into())],
            timeout_secs: None,
        }));
    }

    #[test]
    fn capability_set_with_ttl_not_expired() {
        let set = CapabilitySet::coding("/ws").with_ttl(Duration::from_secs(3600));
        assert!(!set.is_expired());
    }

    #[test]
    fn capability_set_expired() {
        let mut set = CapabilitySet::coding("/ws");
        set.expires_at_ms = Some(1); // long ago
        assert!(set.is_expired());
    }

    #[test]
    fn capability_set_add() {
        let mut set = CapabilitySet::new(vec![]);
        set.add(Capability::FileRead {
            path_pattern: "/ws".into(),
        });
        assert_eq!(set.capabilities().len(), 1);
    }

    #[test]
    fn subject_display() {
        assert_eq!(
            CapabilitySubject::Agent("a1".into()).to_string(),
            "agent:a1"
        );
        assert_eq!(
            CapabilitySubject::Tool("read".into()).to_string(),
            "tool:read"
        );
        assert_eq!(
            CapabilitySubject::Group("coders".into()).to_string(),
            "group:coders"
        );
    }
}
