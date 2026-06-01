//! Core capability types — unforgeable tokens binding rights to resources.
//!
//! Inspired by capability-based security (seL4, Capsicum). Each capability
//! binds a set of rights to a specific resource. Capabilities cannot be
//! forged — they are issued by the kernel or by agents with DELEGATE rights.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use uuid::Uuid;

/// Unique identifier for a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapabilityId(pub Uuid);

impl CapabilityId {
    /// Generate a new random capability ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cap:{}", self.0)
    }
}

impl Default for CapabilityId {
    fn default() -> Self {
        Self::new()
    }
}

/// Who issued a capability.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "issuer", content = "id")]
pub enum Issuer {
    /// Root authority.
    Kernel,
    /// An agent that delegated a subset of its own authority.
    Agent(Uuid),
}

impl fmt::Display for Issuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issuer::Kernel => write!(f, "kernel"),
            Issuer::Agent(id) => write!(f, "agent:{id}"),
        }
    }
}

/// Bit-flag rights encoded in a capability.
///
/// | Flag     | Value |
/// |----------|-------|
/// | NONE     | 0x00  |
/// | READ     | 0x01  |
/// | WRITE    | 0x02  |
/// | EXECUTE  | 0x04  |
/// | DELEGATE | 0x08  |
/// | ALL      | 0x0F  |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Rights(pub u8);

impl Rights {
    /// No rights at all.
    pub const NONE: Rights = Rights(0x00);
    /// Read access.
    pub const READ: Rights = Rights(0x01);
    /// Write / mutate access.
    pub const WRITE: Rights = Rights(0x02);
    /// Execute a resource.
    pub const EXECUTE: Rights = Rights(0x04);
    /// Right to delegate.
    pub const DELEGATE: Rights = Rights(0x08);
    /// All rights combined.
    pub const ALL: Rights = Rights(0x0F);

    /// Returns `true` if this set contains all the given rights.
    pub fn contains(self, other: Rights) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns a new Rights set with the given flags added.
    pub fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }

    /// Returns a new Rights set with only the flags present in both.
    pub fn intersect(self, other: Rights) -> Rights {
        Rights(self.0 & other.0)
    }

    /// Returns true if no rights are set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == Rights::ALL.0 {
            return write!(f, "ALL");
        }
        if self.0 == Rights::NONE.0 {
            return write!(f, "NONE");
        }
        let mut parts = Vec::new();
        if self.contains(Rights::READ) {
            parts.push("R");
        }
        if self.contains(Rights::WRITE) {
            parts.push("W");
        }
        if self.contains(Rights::EXECUTE) {
            parts.push("X");
        }
        if self.contains(Rights::DELEGATE) {
            parts.push("D");
        }
        write!(f, "{}", parts.join("|"))
    }
}

impl std::ops::BitOr for Rights {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Rights(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for Rights {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Rights(self.0 & rhs.0)
    }
}

/// A reference to a protected resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "ref")]
pub enum ResourceRef {
    /// A kernel domain (e.g., "memory", "scheduler").
    KernelDomain { domain: String },
    /// An installed skill.
    Skill { name: String },
    /// A workspace space.
    Space { id: Uuid },
    /// Another agent.
    Agent { id: Uuid },
    /// Command execution (shell or structured).
    Exec { mode: String },
    /// Headless browser access.
    Browser,
    /// Agent-to-agent communication channel.
    A2a,
    /// MCP server bridge.
    Mcp { server: String },
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceRef::KernelDomain { domain } => write!(f, "kernel:{domain}"),
            ResourceRef::Skill { name } => write!(f, "skill:{name}"),
            ResourceRef::Space { id } => write!(f, "space:{id}"),
            ResourceRef::Agent { id } => write!(f, "agent:{id}"),
            ResourceRef::Exec { mode } => write!(f, "exec:{mode}"),
            ResourceRef::Browser => write!(f, "browser"),
            ResourceRef::A2a => write!(f, "a2a"),
            ResourceRef::Mcp { server } => write!(f, "mcp:{server}"),
        }
    }
}

/// An unforgeable token encoding authority over a specific resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Unique identifier.
    pub id: CapabilityId,
    /// The resource this capability governs.
    pub resource: ResourceRef,
    /// The rights granted.
    pub rights: Rights,
    /// Who issued this capability.
    pub issuer: Issuer,
}

impl Capability {
    /// Kernel-issued capability.
    pub fn kernel(resource: ResourceRef, rights: Rights) -> Self {
        Self {
            id: CapabilityId::new(),
            resource,
            rights,
            issuer: Issuer::Kernel,
        }
    }

    /// Agent-delegated capability.
    pub fn delegated(resource: ResourceRef, rights: Rights, issuer: Uuid) -> Self {
        Self {
            id: CapabilityId::new(),
            resource,
            rights,
            issuer: Issuer::Agent(issuer),
        }
    }

    /// Returns true if this capability grants the requested rights.
    pub fn grants(&self, required: Rights) -> bool {
        self.rights.contains(required)
    }
}

/// An agent's capability space: the complete set of capabilities it holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CSpace {
    /// The agent that owns this capability space.
    pub agent_id: Uuid,
    caps: HashMap<CapabilityId, Capability>,
}

impl CSpace {
    /// Creates an empty CSpace.
    pub fn new(agent_id: Uuid) -> Self {
        Self {
            agent_id,
            caps: HashMap::new(),
        }
    }

    /// Insert a capability.
    pub fn insert(&mut self, cap: Capability) -> Option<Capability> {
        self.caps.insert(cap.id, cap)
    }

    /// Remove a capability by ID.
    pub fn remove(&mut self, id: &CapabilityId) -> Option<Capability> {
        self.caps.remove(id)
    }

    /// Look up a capability by ID.
    pub fn get(&self, id: &CapabilityId) -> Option<&Capability> {
        self.caps.get(id)
    }

    /// Returns true if any capability matches the resource and grants the rights.
    pub fn can(&self, resource: &ResourceRef, required: Rights) -> bool {
        self.caps
            .values()
            .any(|cap| &cap.resource == resource && cap.grants(required))
    }

    /// Iterator over all capabilities.
    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.caps.values()
    }

    /// Number of capabilities.
    pub fn len(&self) -> usize {
        self.caps.len()
    }

    /// Whether the space is empty.
    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }

    /// Retain only matching capabilities.
    pub fn retain(&mut self, pred: impl Fn(&Capability) -> bool) {
        self.caps.retain(|_, cap| pred(cap));
    }

    /// Filter capabilities by resource type.
    pub fn filter_resource(&self, f: impl Fn(&ResourceRef) -> bool) -> Vec<&Capability> {
        self.caps.values().filter(|c| f(&c.resource)).collect()
    }

    /// Unique kernel domain names active in this CSpace.
    pub fn active_domains(&self) -> Vec<&str> {
        let mut domains: Vec<&str> = self
            .caps
            .values()
            .filter_map(|cap| match &cap.resource {
                ResourceRef::KernelDomain { domain } => Some(domain.as_str()),
                ResourceRef::Exec { .. } => Some("exec"),
                ResourceRef::Browser => Some("browser"),
                _ => None,
            })
            .collect();
        domains.sort();
        domains.dedup();
        domains
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_bit_ops() {
        let rw = Rights::READ | Rights::WRITE;
        assert!(rw.contains(Rights::READ));
        assert!(rw.contains(Rights::WRITE));
        assert!(!rw.contains(Rights::EXECUTE));
        assert_eq!(rw.intersect(Rights::READ), Rights::READ);
    }

    #[test]
    fn rights_display() {
        assert_eq!(format!("{}", Rights::ALL), "ALL");
        assert_eq!(format!("{}", Rights::NONE), "NONE");
        assert_eq!(format!("{}", Rights::READ | Rights::EXECUTE), "R|X");
    }

    #[test]
    fn cspace_can_check() {
        let agent = Uuid::new_v4();
        let mut cs = CSpace::new(agent);
        cs.insert(Capability::kernel(
            ResourceRef::Exec {
                mode: "shell".into(),
            },
            Rights::READ | Rights::EXECUTE,
        ));
        assert!(cs.can(
            &ResourceRef::Exec {
                mode: "shell".into()
            },
            Rights::EXECUTE
        ));
        assert!(!cs.can(
            &ResourceRef::Exec {
                mode: "shell".into()
            },
            Rights::WRITE
        ));
        assert!(!cs.can(&ResourceRef::Browser, Rights::READ));
    }

    #[test]
    fn capability_delegation() {
        let issuer = Uuid::new_v4();
        let cap = Capability::delegated(
            ResourceRef::Agent { id: issuer },
            Rights::READ | Rights::DELEGATE,
            issuer,
        );
        assert!(matches!(cap.issuer, Issuer::Agent(_)));
        assert!(cap.grants(Rights::DELEGATE));
    }
}
