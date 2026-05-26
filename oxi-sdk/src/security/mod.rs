//! Capability-based security module.
//!
//! Provides fine-grained, deny-by-default permissions for agents:
//! - **Capability**: individual permission (FileRead, Bash, etc.)
//! - **CapabilitySet**: named preset bundles (coding, read_only, etc.)
//! - **Authorizer**: grant/check/revoke with role hierarchy
//! - **SecurityMiddleware**: tool execution guard via Middleware trait

mod authorizer;
mod capability;
pub mod middleware;

pub use authorizer::{Authorizer, DefaultPolicy};
pub use capability::{Capability, CapabilitySet, CapabilitySubject, StringPattern};
pub use middleware::SecurityMiddleware;
