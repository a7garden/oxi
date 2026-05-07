//! Built-in slash command definitions.
//!
//! Built-in slash command definitions.

use crate::source_info::SourceInfo;

/// Where a slash command originates from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandSource {
/// extension variant.
    Extension,
/// prompt variant.
    Prompt,
/// skill variant.
    Skill,
}

/// Resolved information about a slash command (may be built-in or from an extension).
#[derive(Debug, Clone)]
pub struct SlashCommandInfo {
/// pub.
    pub name: String,
/// pub.
    pub description: Option<String>,
/// pub.
    pub source: SlashCommandSource,
/// pub.
    pub source_info: SourceInfo,
}

/// A built-in slash command definition.
#[derive(Debug, Clone)]
pub struct BuiltinSlashCommand {
/// pub.
    pub name: &'static str,
/// pub.
    pub description: &'static str,
}

/// All built-in slash commands available in oxi.
pub static BUILTIN_SLASH_COMMANDS: &[BuiltinSlashCommand] = &[
    BuiltinSlashCommand { name: "settings", description: "Open settings menu" },
    BuiltinSlashCommand { name: "model", description: "Select model (opens selector UI)" },
    BuiltinSlashCommand { name: "scoped-models", description: "Enable/disable models for Ctrl+P cycling" },
    BuiltinSlashCommand { name: "export", description: "Export session (HTML default, or specify path: .html/.jsonl)" },
    BuiltinSlashCommand { name: "import", description: "Import and resume a session from a JSONL file" },
    BuiltinSlashCommand { name: "share", description: "Share session as a secret GitHub gist" },
    BuiltinSlashCommand { name: "copy", description: "Copy last agent message to clipboard" },
    BuiltinSlashCommand { name: "name", description: "Set session display name" },
    BuiltinSlashCommand { name: "session", description: "Show session info and stats" },
    BuiltinSlashCommand { name: "changelog", description: "Show changelog entries" },
    BuiltinSlashCommand { name: "hotkeys", description: "Show all keyboard shortcuts" },
    BuiltinSlashCommand { name: "fork", description: "Create a new fork from a previous user message" },
    BuiltinSlashCommand { name: "clone", description: "Duplicate the current session at the current position" },
    BuiltinSlashCommand { name: "tree", description: "Navigate session tree (switch branches)" },
    BuiltinSlashCommand { name: "provider", description: "Configure API key for a provider" },
    BuiltinSlashCommand { name: "logout", description: "Remove provider authentication" },
    BuiltinSlashCommand { name: "new", description: "Start a new session" },
    BuiltinSlashCommand { name: "compact", description: "Manually compact the session context" },
    BuiltinSlashCommand { name: "resume", description: "Resume a different session" },
    BuiltinSlashCommand { name: "reload", description: "Reload keybindings, extensions, skills, prompts, and themes" },
    BuiltinSlashCommand { name: "quit", description: "Quit oxi" },
];

/// Look up a built-in slash command by name.
pub fn find_builtin_command(name: &str) -> Option<&'static BuiltinSlashCommand> {
    BUILTIN_SLASH_COMMANDS.iter().find(|c| c.name == name)
}

/// Get all built-in slash command names.
pub fn builtin_command_names() -> Vec<&'static str> {
    BUILTIN_SLASH_COMMANDS.iter().map(|c| c.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_existing() {
        assert!(find_builtin_command("settings").is_some());
        assert_eq!(find_builtin_command("settings").unwrap().name, "settings");
    }

    #[test]
    fn find_missing() {
        assert!(find_builtin_command("nonexistent").is_none());
    }

    #[test]
    fn names_match() {
        let names = builtin_command_names();
        assert!(names.contains(&"quit"));
        assert!(names.contains(&"model"));
    }
}
