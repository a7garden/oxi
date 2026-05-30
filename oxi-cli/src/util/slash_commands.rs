//! Built-in slash command definitions.
//!
//! Built-in slash command definitions.

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
    BuiltinSlashCommand {
        name: "help",
        description: "Show help and available commands",
    },
    BuiltinSlashCommand {
        name: "quit",
        description: "Quit oxi",
    },
    BuiltinSlashCommand {
        name: "settings",
        description: "Show current settings",
    },
    BuiltinSlashCommand {
        name: "model",
        description: "Select model (opens selector UI)",
    },
    BuiltinSlashCommand {
        name: "scoped-models",
        description: "Enable/disable models for Ctrl+P cycling",
    },
    BuiltinSlashCommand {
        name: "export",
        description: "Export session (HTML default, or specify path: .html/.jsonl)",
    },
    BuiltinSlashCommand {
        name: "import",
        description: "Import and resume a session from a JSONL file",
    },
    BuiltinSlashCommand {
        name: "copy",
        description: "Copy last agent message to clipboard",
    },
    BuiltinSlashCommand {
        name: "name",
        description: "Set session display name",
    },
    BuiltinSlashCommand {
        name: "session",
        description: "Show session info and stats",
    },
    BuiltinSlashCommand {
        name: "share",
        description: "Share session as a GitHub Gist (requires gh CLI)",
    },
    BuiltinSlashCommand {
        name: "changelog",
        description: "Show changelog entries",
    },
    BuiltinSlashCommand {
        name: "hotkeys",
        description: "Show all keyboard shortcuts",
    },
    BuiltinSlashCommand {
        name: "fork",
        description: "Create a new fork from a previous user message",
    },
    BuiltinSlashCommand {
        name: "clone",
        description: "Duplicate the current session at the current position",
    },
    BuiltinSlashCommand {
        name: "tree",
        description: "Show session tree structure",
    },
    BuiltinSlashCommand {
        name: "provider",
        description: "Configure API key for a provider",
    },
    BuiltinSlashCommand {
        name: "logout",
        description: "Remove provider authentication",
    },
    BuiltinSlashCommand {
        name: "new",
        description: "Start a new session",
    },
    BuiltinSlashCommand {
        name: "compact",
        description: "Manually compact the session context",
    },
    BuiltinSlashCommand {
        name: "resume",
        description: "Resume a different session",
    },
    BuiltinSlashCommand {
        name: "reload",
        description: "Reload settings, theme, and extensions",
    },
    BuiltinSlashCommand {
        name: "tools",
        description: "List active tools or toggle tool on/off",
    },
    BuiltinSlashCommand {
        name: "extensions",
        description: "List extensions & WASM tools",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match() {
        let names: Vec<_> = BUILTIN_SLASH_COMMANDS.iter().map(|c| c.name).collect();
        assert!(names.contains(&"quit"));
        assert!(names.contains(&"model"));
    }
}
