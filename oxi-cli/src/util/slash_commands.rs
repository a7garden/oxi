//! Built-in slash command definitions.
//!
//! Single source of truth for all slash command names and descriptions.
//! Used by both the TUI completion system and help overlays.

/// A built-in slash command definition.
#[derive(Debug, Clone)]
pub struct BuiltinSlashCommand {
    /// Command name (without the leading `/`).
    pub name: &'static str,
    /// Short description shown in completions and help.
    pub description: &'static str,
}

/// All built-in slash commands available in oxi.
///
/// Keep in sync with `handle_slash_command()` in `tui/slash.rs`.
pub static BUILTIN_SLASH_COMMANDS: &[BuiltinSlashCommand] = &[
    // ── Session ────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "help",
        description: "Show help and available commands",
    },
    BuiltinSlashCommand {
        name: "quit",
        description: "Quit oxi (aliases: /exit, /q)",
    },
    BuiltinSlashCommand {
        name: "new",
        description: "Start a new session",
    },
    BuiltinSlashCommand {
        name: "clone",
        description: "Duplicate the current session at the current position",
    },
    BuiltinSlashCommand {
        name: "resume",
        description: "Resume a different session",
    },
    BuiltinSlashCommand {
        name: "fork",
        description: "Create a new fork from a previous user message",
    },
    BuiltinSlashCommand {
        name: "tree",
        description: "Show session tree structure",
    },
    BuiltinSlashCommand {
        name: "session",
        description: "Show session info and stats",
    },
    BuiltinSlashCommand {
        name: "name",
        description: "Set session display name",
    },
    // ── Model ──────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "model",
        description: "Select or switch model (opens selector UI)",
    },
    BuiltinSlashCommand {
        name: "scoped-models",
        description: "Set/get models for Ctrl+P cycling (alias: /models)",
    },
    BuiltinSlashCommand {
        name: "router",
        description: "Configure or inspect model router",
    },
    // ── Skills ─────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "skill",
        description: "List, activate, or deactivate skills",
    },
    // ── Context ────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "compact",
        description: "Manually compact the session context",
    },
    // ── Tools ──────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "tools",
        description: "List active tools or toggle tool on/off",
    },
    BuiltinSlashCommand {
        name: "mcp",
        description: "Manage MCP servers (add/edit/remove/view) or open status dashboard",
    },
    BuiltinSlashCommand {
        name: "extensions",
        description: "List extensions & WASM tools (alias: /ext)",
    },
    // ── Export ─────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "export",
        description: "Export session to HTML",
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
        name: "share",
        description: "Share session as a GitHub Gist (requires gh CLI)",
    },
    // ── Auth ───────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "provider",
        description: "Configure API key for a provider",
    },
    BuiltinSlashCommand {
        name: "logout",
        description: "Remove provider authentication",
    },
    // ── Info ───────────────────────────────────────────────────
    BuiltinSlashCommand {
        name: "settings",
        description: "Show current settings",
    },
    BuiltinSlashCommand {
        name: "hotkeys",
        description: "Show all keyboard shortcuts (alias: /keys)",
    },
    BuiltinSlashCommand {
        name: "changelog",
        description: "Show changelog entries",
    },
    BuiltinSlashCommand {
        name: "reload",
        description: "Reload settings, theme, and extensions",
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
