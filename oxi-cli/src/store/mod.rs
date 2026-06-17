//! oxi-cli's self-contained domain types and adapters.
//!
//! Previously lived in a separate `oxi-store` crate. After the
//! port-based refactor, all of oxi-cli's storage-adjacent code lives
//! here in a single module. The `oxi-sdk` port traits
//! (`oxi_sdk::ports::*`) are the persistence contract; this module
//! holds concrete types and file-based adapters.
//!
//! # Removed during legacy cleanup
//!
//! The following modules were absorbed from the old `oxi-store` crate
//! but had **zero importers** in oxi-cli, so they were deleted:
//!
//! - `auth_guidance` (124 lines)        — UX strings, never wired
//! - `settings_validation` (346 lines)  — validation messages, never wired
//! - `session_navigation` (1,451 lines) — tree traversal, never wired
//! - `model_resolver` (1,430 lines)     — name → metadata, never wired
//! - `model_registry` (1,792 lines)     — replaced by `oxi_sdk::ModelRegistry`

pub mod auth_storage;
#[allow(missing_docs, dead_code)] // surface is large; do a doc pass before stabilizing
pub mod issues;
pub mod fs_util;
pub mod router_config;
pub mod session;
pub mod session_cwd;
pub mod settings;
