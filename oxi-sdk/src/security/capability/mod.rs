//! Capability module — seL4-inspired capability-based access control.

pub mod resolve;
pub mod template;
pub mod types;

pub use resolve::resolve_cspace;
pub use template::CapabilityTemplate;
pub use types::{CSpace, Capability, CapabilityId, Issuer, ResourceRef, Rights};
