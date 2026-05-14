//! Miscellaneous utilities — file paths, git, system info, messages
pub(crate) mod git_utils;
pub(crate) mod paths;
pub(crate) mod source_info;
pub(crate) mod pi_user_agent;
pub(crate) mod provider_display_names;
pub(crate) mod messages;
pub(crate) mod tmux_detect;
pub(crate) mod telemetry;
pub(crate) mod defaults;
pub(crate) mod slash_commands;
pub(crate) mod sleep;

// Re-exports for convenience
pub use slash_commands::BUILTIN_SLASH_COMMANDS;
