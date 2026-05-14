//! oxi-store — Shared persistent state for oxi
//!
//! Provides session management, settings, auth storage, and model registry
//! for use across oxi-cli, oxi-app, and future consumers.

pub mod session;
pub mod session_navigation;
pub mod session_cwd;
pub mod settings;
pub mod settings_validation;
pub mod model_registry;
pub mod model_resolver;
pub mod auth_storage;
pub mod auth_guidance;

// Public re-exports
pub use auth_storage::AuthStorage;
pub use model_registry::ModelRegistry;
pub use session::{SessionEntry, SessionManager, SessionTreeNode, AgentMessage, ContentValue, ContentBlock, AssistantContentBlock};
pub use settings::Settings;
pub use settings_validation::ValidationReport;