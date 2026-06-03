//! seL4-style capability types — CSpace, ResourceRef, Rights.
//!
//! Provides a capability-space abstraction where each agent holds a set of
//! typed capability tokens (rights over named resources). Inspired by
//! seL4's capability model, simplified for SDK use.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Access rights for a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Rights {
    /// Read access.
    Read,
    /// Write access.
    Write,
    /// Execute access (e.g., run a tool or command).
    Execute,
    /// Grant (delegate) this capability to another agent.
    Grant,
}

/// Reference to a protected resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceRef {
    /// Kernel-service domain (e.g., a tool name or subsystem).
    KernelDomain { domain: String },
    /// Filesystem path pattern.
    Path { pattern: String },
    /// Network endpoint pattern.
    Network { pattern: String },
    /// Arbitrary named resource.
    Named { name: String },
}

/// A single capability entry: rights over a resource, optionally with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    /// The resource this capability covers.
    pub resource: ResourceRef,
    /// Granted rights.
    pub rights: Vec<Rights>,
    /// Optional human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl CapabilityEntry {
    /// Create a new entry.
    pub fn new(resource: ResourceRef, rights: Vec<Rights>) -> Self {
        Self {
            resource,
            rights,
            label: None,
        }
    }

    /// Check if this entry grants the specified right.
    pub fn has_right(&self, right: Rights) -> bool {
        self.rights.contains(&right)
    }
}

/// Capability Space — an agent's collection of capability tokens.
///
/// Inspired by seL4's CSpace. Each agent has exactly one CSpace that
/// defines what resources it can access and with which rights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CSpace {
    /// The agent that owns this CSpace.
    pub agent_id: Uuid,
    /// Human-readable name (for display).
    #[serde(default)]
    pub name: String,
    /// Capability entries keyed by a stable index.
    entries: HashMap<u32, CapabilityEntry>,
    /// Next free index.
    next_index: u32,
}

impl CSpace {
    /// Create an empty CSpace for an agent.
    pub fn new(agent_id: Uuid) -> Self {
        Self {
            agent_id,
            name: String::new(),
            entries: HashMap::new(),
            next_index: 1,
        }
    }

    /// Create a CSpace with a human-readable name.
    pub fn with_name(agent_id: Uuid, name: &str) -> Self {
        Self {
            agent_id,
            name: name.to_string(),
            entries: HashMap::new(),
            next_index: 1,
        }
    }

    /// Insert a capability entry, returning its index.
    pub fn insert(&mut self, entry: CapabilityEntry) -> u32 {
        let idx = self.next_index;
        self.next_index += 1;
        self.entries.insert(idx, entry);
        idx
    }

    /// Remove a capability by index.
    pub fn remove(&mut self, index: u32) -> Option<CapabilityEntry> {
        self.entries.remove(&index)
    }

    /// Check if this CSpace grants `right` over `resource`.
    ///
    /// Checks for an exact or more-permissive match. For `ResourceRef::Path`
    /// and `ResourceRef::Network`, a wildcard pattern (`*`) matches anything.
    pub fn can(&self, resource: &ResourceRef, right: Rights) -> bool {
        self.entries
            .values()
            .any(|entry| entry.has_right(right) && resource_matches(&entry.resource, resource))
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &CapabilityEntry)> {
        self.entries.iter()
    }

    /// Number of capability entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the CSpace is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Check if a granted resource reference satisfies a required one.
fn resource_matches(granted: &ResourceRef, required: &ResourceRef) -> bool {
    match (granted, required) {
        // Exact match
        _ if granted == required => true,

        // KernelDomain: wildcard matches any domain
        (ResourceRef::KernelDomain { domain: g }, ResourceRef::KernelDomain { domain: _r }) => {
            g == "*"
        }

        // Path: wildcard matches any path
        (ResourceRef::Path { pattern: g }, ResourceRef::Path { pattern: _r }) => {
            if g == "*" || g == "/**" {
                return true;
            }
            // Simple prefix/suffix matching
            if let Some(prefix) = g.strip_suffix("/**") {
                return _r.starts_with(prefix)
                    || _r.starts_with(&format!("{}/", prefix.trim_end_matches('/')));
            }
            false
        }

        // Network: wildcard matches any endpoint
        (ResourceRef::Network { pattern: g }, ResourceRef::Network { pattern: _r }) => g == "*",

        // Named: wildcard matches any name
        (ResourceRef::Named { name: g }, ResourceRef::Named { name: _r }) => g == "*",

        _ => false,
    }
}

/// Builder for constructing a CSpace with standard templates.
#[derive(Debug)]
pub struct CSpaceBuilder {
    agent_id: Uuid,
    name: String,
    entries: Vec<CapabilityEntry>,
}

impl CSpaceBuilder {
    /// Create a builder for the given agent.
    pub fn new(agent_id: Uuid) -> Self {
        Self {
            agent_id,
            name: String::new(),
            entries: Vec::new(),
        }
    }

    /// Set a human-readable name.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Grant rights over a resource.
    pub fn grant(mut self, resource: ResourceRef, rights: Vec<Rights>) -> Self {
        self.entries.push(CapabilityEntry::new(resource, rights));
        self
    }

    /// Grant all rights over all resources (admin/superuser).
    pub fn all_access(self) -> Self {
        self.grant(
            ResourceRef::KernelDomain { domain: "*".into() },
            vec![Rights::Read, Rights::Write, Rights::Execute, Rights::Grant],
        )
        .grant(
            ResourceRef::Path {
                pattern: "/**".into(),
            },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        )
        .grant(
            ResourceRef::Network {
                pattern: "*".into(),
            },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        )
    }

    /// Standard template: read/write/execute on workspace tools.
    pub fn standard(self) -> Self {
        // Essential tools
        self.grant(
            ResourceRef::KernelDomain {
                domain: "read".into(),
            },
            vec![Rights::Read, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "write".into(),
            },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "edit".into(),
            },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "bash".into(),
            },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "grep".into(),
            },
            vec![Rights::Read, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "find".into(),
            },
            vec![Rights::Read, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "ls".into(),
            },
            vec![Rights::Read, Rights::Execute],
        )
        .grant(
            ResourceRef::KernelDomain {
                domain: "memory".into(),
            },
            vec![Rights::Read, Rights::Write],
        )
        .grant(
            ResourceRef::Path {
                pattern: "/workspace/**".into(),
            },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        )
    }

    /// Worker template: standard + subagent + network.
    pub fn worker(self) -> Self {
        self.standard()
            .grant(
                ResourceRef::KernelDomain {
                    domain: "subagent".into(),
                },
                vec![Rights::Execute],
            )
            .grant(
                ResourceRef::Network {
                    pattern: "*".into(),
                },
                vec![Rights::Read, Rights::Write],
            )
    }

    /// Build the CSpace.
    pub fn build(self) -> CSpace {
        let mut cspace = CSpace::with_name(self.agent_id, &self.name);
        for entry in self.entries {
            cspace.insert(entry);
        }
        cspace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cspace_insert_and_check() {
        let id = Uuid::new_v4();
        let mut cs = CSpace::new(id);
        cs.insert(CapabilityEntry::new(
            ResourceRef::KernelDomain {
                domain: "read".into(),
            },
            vec![Rights::Read, Rights::Execute],
        ));

        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "read".into()
            },
            Rights::Execute
        ));
        assert!(!cs.can(
            &ResourceRef::KernelDomain {
                domain: "write".into()
            },
            Rights::Execute
        ));
    }

    #[test]
    fn test_cspace_wildcard() {
        let id = Uuid::new_v4();
        let mut cs = CSpace::new(id);
        cs.insert(CapabilityEntry::new(
            ResourceRef::KernelDomain { domain: "*".into() },
            vec![Rights::Read, Rights::Write, Rights::Execute],
        ));

        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "anything".into()
            },
            Rights::Execute
        ));
    }

    #[test]
    fn test_cspace_path_glob() {
        let id = Uuid::new_v4();
        let mut cs = CSpace::new(id);
        cs.insert(CapabilityEntry::new(
            ResourceRef::Path {
                pattern: "/workspace/**".into(),
            },
            vec![Rights::Read, Rights::Write],
        ));

        assert!(cs.can(
            &ResourceRef::Path {
                pattern: "/workspace/src/main.rs".into()
            },
            Rights::Read
        ));
        assert!(!cs.can(
            &ResourceRef::Path {
                pattern: "/etc/passwd".into()
            },
            Rights::Read
        ));
    }

    #[test]
    fn test_builder_standard() {
        let id = Uuid::new_v4();
        let cs = CSpaceBuilder::new(id).standard().build();
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
    fn test_builder_all_access() {
        let id = Uuid::new_v4();
        let cs = CSpaceBuilder::new(id).all_access().build();
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "anything".into()
            },
            Rights::Grant
        ));
        assert!(cs.can(
            &ResourceRef::Path {
                pattern: "/any/path".into()
            },
            Rights::Write
        ));
    }

    #[test]
    fn test_builder_worker() {
        let id = Uuid::new_v4();
        let cs = CSpaceBuilder::new(id).worker().build();
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "subagent".into()
            },
            Rights::Execute
        ));
        assert!(cs.can(
            &ResourceRef::Network {
                pattern: "example.com".into()
            },
            Rights::Read
        ));
    }

    #[test]
    fn test_remove_capability() {
        let id = Uuid::new_v4();
        let mut cs = CSpace::new(id);
        let idx = cs.insert(CapabilityEntry::new(
            ResourceRef::KernelDomain {
                domain: "read".into(),
            },
            vec![Rights::Read],
        ));
        assert!(cs.remove(idx).is_some());
        assert!(!cs.can(
            &ResourceRef::KernelDomain {
                domain: "read".into()
            },
            Rights::Read
        ));
    }
}
