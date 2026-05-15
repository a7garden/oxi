//! Miscellaneous utilities — git, HTTP, system info
pub(crate) mod git_utils;
pub(crate) mod http_client;
pub(crate) mod pi_user_agent;
pub(crate) mod provider_display_names;
pub(crate) mod slash_commands;

// Re-exports for convenience
pub use slash_commands::BUILTIN_SLASH_COMMANDS;
