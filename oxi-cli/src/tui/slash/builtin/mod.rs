//! Built-in slash command implementations.
//!
//! Each command lives in its own submodule and implements
//! [`super::registry::SlashCommand`]. [`register_all`] wires them into a
//! [`SlashRegistry`].
//!
//! During migration (design step 2), ported commands register here while their
//! legacy `match` arms are deleted from `super::mod.rs`.

mod clipboard;
mod export_grp;
mod issue;
mod model;
mod overlay_commands;
mod provider;
mod quit;
mod router;
mod session_grp;
mod settings;
mod skill;
mod tools_commands;

use super::registry::SlashRegistry;

/// Register every built-in slash command.
///
/// Called by [`SlashRegistry::builtins`](super::registry::SlashRegistry::builtins).
pub(crate) fn register_all(registry: &mut SlashRegistry) {
    registry.register(Box::new(quit::QuitCommand));
    registry.register(Box::new(overlay_commands::HelpCommand));
    registry.register(Box::new(overlay_commands::HotkeysCommand));
    registry.register(Box::new(overlay_commands::ExtensionsCommand));
    registry.register(Box::new(overlay_commands::ChangelogCommand));
    registry.register(Box::new(tools_commands::CompactCommand));
    registry.register(Box::new(tools_commands::SessionCommand));
    registry.register(Box::new(tools_commands::SettingsCommand));
    registry.register(Box::new(tools_commands::McpCommand));
    registry.register(Box::new(tools_commands::ToolsCommand));
    registry.register(Box::new(clipboard::CopyCommand));
    registry.register(Box::new(session_grp::NameCommand));
    registry.register(Box::new(session_grp::NewCommand));
    registry.register(Box::new(session_grp::CloneCommand));
    registry.register(Box::new(session_grp::ResumeCommand));
    registry.register(Box::new(session_grp::TreeCommand));
    registry.register(Box::new(session_grp::ForkCommand));
    registry.register(Box::new(settings::ReloadCommand));
    registry.register(Box::new(provider::LogoutCommand));
    registry.register(Box::new(provider::ProviderCommand));
    registry.register(Box::new(router::RouterCommand));
    registry.register(Box::new(issue::IssueCommand));
    registry.register(Box::new(export_grp::ExportCommand));
    registry.register(Box::new(export_grp::ImportCommand));
    registry.register(Box::new(export_grp::ShareCommand));
    registry.register(Box::new(skill::SkillCommand));
    registry.register(Box::new(model::ModelCommand));
    registry.register(Box::new(model::ScopedModelsCommand));
}
