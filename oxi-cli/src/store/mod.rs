//! oxi-cli's self-contained domain types and adapters.
//!
//! Previously lived in a separate `oxi-store` crate. After the
//! port-based refactor, all of oxi-cli's storage-adjacent code lives
//! here in a single module. The `oxi-sdk` port traits
//! (`oxi_sdk::ports::*`) are the persistence contract; this module
//! holds concrete types and file-based adapters (moved from oxi-fs
//! and now co-located with oxi-cli).

pub mod auth_guidance;
pub mod auth_storage;
pub mod model_registry;
pub mod model_resolver;
pub mod router_config;
pub mod session;
pub mod session_cwd;
pub mod session_navigation;
pub mod settings;
pub mod settings_validation;
