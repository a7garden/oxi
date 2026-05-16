//! oxi-store — Shared persistent state for oxi
//!
//! Provides session management, settings, auth storage, and model registry
//! for use across oxi-cli, oxi-app, and future consumers.

#![allow(unexpected_cfgs)]

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

/// Persistent credential storage for API keys and tokens.
pub use auth_storage::AuthStorage;

/// CLI-specific model registry with auth storage integration.
pub use model_registry::CliModelRegistry;

/// Backward-compatible alias for [`CliModelRegistry`].
pub use model_registry::ModelRegistry;

/// Session persistence, navigation, and message types.
pub use session::{SessionEntry, SessionManager, SessionTreeNode, AgentMessage, ContentValue, ContentBlock, AssistantContentBlock};

/// User-configurable settings (model, theme, keybindings, etc.).
pub use settings::Settings;

/// Settings validation report and diagnostics.
pub use settings_validation::ValidationReport;