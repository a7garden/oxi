//! Capability templates — preset CSpace configurations for common agent roles.
//!
//! ```text
//! worker()       → Exec + Browser
//!   standard()   → worker + Memory(READ)
//!   operator()   → standard + Space + Agent + A2a + Program + MCP + Memory(WRITE)
//!   supervisor() → operator + Security + Budget + Resource + Cron
//! ```

use super::types::{CSpace, Capability, CapabilityId, Issuer, ResourceRef, Rights};
use uuid::Uuid;

/// Builder for constructing preset capability spaces.
#[derive(Debug, Clone)]
pub struct CapabilityTemplate {
    caps: Vec<(ResourceRef, Rights)>,
}

impl CapabilityTemplate {
    /// **Worker** — minimal execution capability.
    pub fn worker() -> Self {
        let mut t = Self { caps: Vec::new() };
        t.caps.push((
            ResourceRef::Exec {
                mode: "shell".into(),
            },
            Rights::EXECUTE | Rights::READ,
        ));
        t.caps
            .push((ResourceRef::Browser, Rights::READ | Rights::EXECUTE));
        t
    }

    /// **Standard** — worker + memory read.
    pub fn standard() -> Self {
        let mut t = Self::worker();
        t.caps.push((
            ResourceRef::KernelDomain {
                domain: "memory".into(),
            },
            Rights::READ,
        ));
        t
    }

    /// **Operator** — standard + space, agent, A2A, persona, program, MCP, memory write.
    pub fn operator() -> Self {
        let mut t = Self::standard();
        let extra = vec![
            (
                ResourceRef::Space { id: Uuid::nil() },
                Rights::READ | Rights::WRITE,
            ),
            (
                ResourceRef::Agent { id: Uuid::nil() },
                Rights::READ | Rights::WRITE,
            ),
            (ResourceRef::A2a, Rights::READ | Rights::WRITE | Rights::EXECUTE),
            (
                ResourceRef::KernelDomain {
                    domain: "persona".into(),
                },
                Rights::READ | Rights::WRITE,
            ),
            (
                ResourceRef::KernelDomain {
                    domain: "program".into(),
                },
                Rights::READ | Rights::WRITE | Rights::EXECUTE,
            ),
            (
                ResourceRef::Mcp { server: "*".into() },
                Rights::READ | Rights::EXECUTE,
            ),
            (
                ResourceRef::KernelDomain {
                    domain: "memory".into(),
                },
                Rights::READ | Rights::WRITE,
            ),
        ];
        t.caps.extend(extra);
        t
    }

    /// **Supervisor** — operator + security, budget, resource, cron.
    pub fn supervisor() -> Self {
        let mut t = Self::operator();
        let admin = vec![
            (
                ResourceRef::KernelDomain {
                    domain: "security".into(),
                },
                Rights::ALL,
            ),
            (
                ResourceRef::KernelDomain {
                    domain: "budget".into(),
                },
                Rights::READ | Rights::WRITE,
            ),
            (
                ResourceRef::KernelDomain {
                    domain: "resource".into(),
                },
                Rights::READ | Rights::WRITE,
            ),
            (
                ResourceRef::KernelDomain {
                    domain: "cron".into(),
                },
                Rights::READ | Rights::WRITE | Rights::EXECUTE,
            ),
        ];
        t.caps.extend(admin);
        t
    }

    /// **With skills** — worker + specific named skills.
    pub fn with_skills(names: &[&str]) -> Self {
        let mut t = Self::worker();
        for name in names {
            t.caps.push((
                ResourceRef::Skill {
                    name: (*name).into(),
                },
                Rights::EXECUTE | Rights::READ,
            ));
        }
        t
    }

    /// Add an additional capability.
    pub fn with(mut self, resource: ResourceRef, rights: Rights) -> Self {
        self.caps.push((resource, rights));
        self
    }

    /// Build a CSpace for a fresh agent ID.
    pub fn build(&self) -> CSpace {
        self.build_for(Uuid::new_v4())
    }

    /// Build a CSpace for a specific agent.
    pub fn build_for(&self, agent_id: Uuid) -> CSpace {
        let mut cspace = CSpace::new(agent_id);
        for (resource, rights) in &self.caps {
            let cap = Capability {
                id: CapabilityId::new(),
                resource: resource.clone(),
                rights: *rights,
                issuer: Issuer::Kernel,
            };
            cspace.insert(cap);
        }
        cspace
    }

    /// Number of capabilities in this template.
    pub fn len(&self) -> usize {
        self.caps.len()
    }

    /// Whether the template is empty.
    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }
}

impl Default for CapabilityTemplate {
    fn default() -> Self {
        Self::worker()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_has_exec_and_browser() {
        let cs = CapabilityTemplate::worker().build();
        assert!(cs.can(
            &ResourceRef::Exec {
                mode: "shell".into()
            },
            Rights::EXECUTE
        ));
        assert!(cs.can(&ResourceRef::Browser, Rights::READ));
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn standard_adds_memory_read() {
        let cs = CapabilityTemplate::standard().build();
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "memory".into()
            },
            Rights::READ
        ));
        assert!(!cs.can(
            &ResourceRef::KernelDomain {
                domain: "memory".into()
            },
            Rights::WRITE
        ));
    }

    #[test]
    fn operator_has_a2a_and_mcp() {
        let cs = CapabilityTemplate::operator().build();
        assert!(cs.can(&ResourceRef::A2a, Rights::EXECUTE));
        assert!(cs.can(
            &ResourceRef::Mcp { server: "*".into() },
            Rights::EXECUTE
        ));
    }

    #[test]
    fn supervisor_has_security_all() {
        let cs = CapabilityTemplate::supervisor().build();
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "security".into()
            },
            Rights::ALL
        ));
    }

    #[test]
    fn with_skills_scoped() {
        let cs = CapabilityTemplate::with_skills(&["git", "gh"]).build();
        assert!(cs.can(
            &ResourceRef::Skill { name: "git".into() },
            Rights::EXECUTE
        ));
        assert!(!cs.can(
            &ResourceRef::Skill {
                name: "curl".into()
            },
            Rights::EXECUTE
        ));
    }

    #[test]
    fn builder_chaining() {
        let cs = CapabilityTemplate::worker()
            .with(
                ResourceRef::KernelDomain {
                    domain: "custom".into(),
                },
                Rights::READ,
            )
            .build();
        assert!(cs.can(
            &ResourceRef::KernelDomain {
                domain: "custom".into()
            },
            Rights::READ
        ));
    }
}
