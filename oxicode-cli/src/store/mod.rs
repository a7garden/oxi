//! oxicode-cli's self-contained domain types and adapters.
//!
//! Previously lived in a separate `oxicode-store` crate. After the
//! port-based refactor, all of oxicode-cli's storage-adjacent code lives
//! here in a single module. The `oxicode-sdk` port traits
//! (`oxicode_sdk::ports::*`) are the persistence contract; this module
//! holds concrete types and file-based adapters.
//! # Removed during legacy cleanup
//!
//! - `auth_guidance`, `settings_validation`, `session_navigation`,
//!   `model_resolver`, `model_registry` — absorbed from the old
//!   `oxicode-store` crate with zero importers.
//! - `memory_mnemopi`, `memory_sqlite`, `memory_summary`, `memory_workers`,
//!   `mnemopi`, `extracting_backend` — the pre-Foundation local durable
//!   memory stack. Under the Oxi Foundation v1 host the oxibrain daemon is
//!   the only durable-memory authority (Foundation plan §5); the legacy
//!   stores kept compiling through the migration window and were removed
//!   with the `oxicode-mnemopi` crate. `oxicode migrate brain` reads the
//!   legacy JSONL directly (foundation::migrate) and needs none of them.

pub mod access_compat;
pub mod auth_storage;
pub mod fs_util;
pub mod hook_approval;
#[allow(missing_docs, dead_code)] // surface is large; do a doc pass before stabilizing
pub mod issues;
pub mod router_config;
pub mod session;
pub mod session_cwd;
pub mod settings;
pub mod todo_state;
