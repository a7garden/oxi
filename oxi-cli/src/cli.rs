//! CLI argument parsing with clap
//!
//! Provides the unified command-line argument types for the oxi CLI.
//! This is the single source of truth for all CLI parsing — main.rs
//! imports from here rather than defining its own types.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

// ── Re-exports ─────────────────────────────────────────────────────
// Use the canonical ThinkingLevel from settings (None/Minimal/Standard/Thorough).
pub use oxi_store::settings::ThinkingLevel;

// ── Main CLI arguments ─────────────────────────────────────────────

/// CLI arguments
#[derive(Debug, Clone, Parser)]
#[command(name = "oxi")]
#[command(about = "CLI coding harness for oxi")]
#[command(version)]
pub struct CliArgs {
    /// pub.
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Provider to use (e.g., anthropic, openai, google, deepseek)
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Model to use (e.g., claude-sonnet-4-20250514, gpt-4o)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Initial prompt (non-interactive mode)
    #[arg(default_value = "")]
    pub prompt: Vec<String>,

    /// Interactive mode (default when no prompt is given)
    #[arg(short, long)]
    pub interactive: bool,

    /// Thinking level (none, minimal, standard, thorough)
    #[arg(long)]
    pub thinking: Option<String>,

    /// Load an extension from a shared library (.so / .dll / .dylib).
    /// Can be specified multiple times.
    #[arg(short = 'e', long = "extension", value_name = "PATH")]
    pub extensions: Vec<PathBuf>,

    /// Output mode: text or json (newline-delimited JSON events)
    #[arg(long)]
    pub mode: Option<String>,

    /// Comma-separated list of tools to enable. Default: all builtins.
    #[arg(long)]
    pub tools: Option<String>,

    /// Append system prompt from a file
    #[arg(long)]
    pub append_system_prompt: Option<PathBuf>,

    /// Single-shot print mode (non-interactive)
    #[arg(long)]
    pub print: bool,

    /// Disable session persistence
    #[arg(long)]
    pub no_session: bool,

    /// Timeout in seconds for print mode
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Resume the most recent session for this project
    #[arg(short, long)]
    pub continue_session: bool,

    // ── Routing configuration ─────────────────────────────────────────
    /// Enable automatic model routing (falls back to cost-efficient models on errors)
    #[arg(long = "enable-routing")]
    pub enable_routing: bool,

    /// Prefer cost-efficient models when routing is enabled
    #[arg(long = "prefer-cost-efficient")]
    pub prefer_cost_efficient: bool,

    /// Fallback chain: comma-separated list of provider/model IDs (can be specified multiple times)
    #[arg(long = "fallback-chain", value_delimiter = ',')]
    pub fallback_chain: Vec<String>,

    /// Disable automatic fallback (fail fast on errors instead of trying alternatives)
    #[arg(long = "disable-fallback")]
    pub disable_fallback: bool,
}

// ── Subcommands ────────────────────────────────────────────────────

/// CLI subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// List all sessions.
    Sessions,
    /// Show session tree structure.
    Tree {
        /// Session ID to show tree for (default: current/last session)
        #[arg(default_value = "")]
        session_id: String,
    },
    /// Fork a new session from a specific entry
    Fork {
        /// Parent session ID
        parent_id: String,
        /// Entry ID to branch from
        entry_id: String,
    },
    /// Delete a session
    Delete {
        /// Session ID to delete
        session_id: String,
    },
    /// Package management
    Pkg {
        /// action.
        #[command(subcommand)]
        action: PkgCommands,
    },
    /// Configuration management
    Config {
        /// action.
        #[command(subcommand)]
        action: ConfigCommands,
    },
    /// Extension management — install, update, remove WASM extensions
    Ext {
        /// action.
        #[command(subcommand)]
        action: ExtCommands,
    },
    /// List available models
    Models {
        /// Filter by provider name (e.g., openai, anthropic, minimax)
        #[arg(long)]
        provider: Option<String>,
    },
    /// Run the interactive setup wizard
    Setup {
        /// Reset all settings to defaults
        #[arg(long)]
        reset: bool,
    },
    /// Reset all settings and data to factory defaults
    ///
    /// Use when configuration has become tangled and you want a clean start.
    /// An interactive confirmation prompt will be shown.
    Reset {
        /// Skip the confirmation prompt
        #[arg(long, short)]
        yes: bool,
        /// Also delete the project-local .oxi/ directory
        #[arg(long)]
        include_project: bool,
    },
    /// Export a session to HTML
    Export {
        /// Session ID to export (default: most recent for this project)
        session_id: Option<String>,
        /// Output file path (default: oxi-export-{id}.html in CWD)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Import a session from a JSONL file
    Import {
        /// Path to the JSONL session file
        path: PathBuf,
    },
    /// Share a session as a GitHub Gist (requires gh CLI)
    Share {
        /// Session ID to share (default: most recent for this project)
        session_id: Option<String>,
    },
}

// ── Package subcommands ────────────────────────────────────────────

/// Package management subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum PkgCommands {
    /// Install a package from a local path or npm:@scope/name
    Install {
        /// Package source: a local directory path or npm:@scope/name
        source: String,
    },
    /// List installed packages
    List,
    /// Uninstall a package by name
    Uninstall {
        /// Package name to uninstall
        name: String,
    },
    /// Update a package to the latest version
    Update {
        /// Package name to update (updates all if omitted)
        name: Option<String>,
    },
}

// ── Extension subcommands ──────────────────────────────────────────────

/// Extension management subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum ExtCommands {
    /// Install a WASM extension from a GitHub repo (owner/repo or owner/repo@version)
    Install {
        /// Extension source: owner/repo or owner/repo@version
        source: String,
        /// Include pre-release versions
        #[arg(long)]
        prerelease: bool,
    },
    /// List installed extensions
    List,
    /// Remove an installed extension
    Remove {
        /// Extension name to remove
        name: String,
    },
    /// Update extension(s) to latest version
    Update {
        /// Extension name to update (updates all if omitted)
        name: Option<String>,
    },
    /// Show info about a remote extension (without installing)
    Info {
        /// Extension source: owner/repo
        source: String,
    },
}

// ── Config subcommands ─────────────────────────────────────────────

/// Configuration management subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration
    Show,
    /// List all enabled resources
    List {
        /// Resource type filter (extensions, skills, prompts, themes)
        resource_type: Option<String>,
    },
    /// Enable a resource (extension, skill, prompt, or theme)
    Enable {
        /// Resource type: extension, skill, prompt, or theme
        resource_type: String,
        /// Resource path or name
        name: String,
    },
    /// Disable a resource
    Disable {
        /// Resource type: extension, skill, prompt, or theme
        resource_type: String,
        /// Resource path or name
        name: String,
    },
    /// Set a configuration value
    Set {
        /// Setting key (e.g. theme, model, thinking_level)
        key: String,
        /// Setting value
        value: String,
    },
    /// Get a configuration value
    Get {
        /// Setting key
        key: String,
    },
    /// Add a custom OpenAI-compatible provider
    AddProvider {
        /// Provider name (e.g. minimax)
        name: String,
        /// Base URL (e.g. <https://api.minimax.chat/v1>)
        base_url: String,
        /// Environment variable name for API key (e.g. MINIMAX_API_KEY)
        api_key_env: String,
        /// API type: openai-completions or openai-responses (default: openai-completions)
        #[arg(default_value = "openai-completions")]
        api: String,
    },
    /// Remove a custom provider
    RemoveProvider {
        /// Provider name to remove
        name: String,
    },
    /// Reset credentials (auth.json) and optionally settings
    Reset {
        /// Also reset settings (settings.toml / settings.json)
        #[arg(long, short)]
        all: bool,
    },
}

// ── Parsing helpers ────────────────────────────────────────────────

/// Parse CLI arguments from the command line
///
/// # Examples
///
/// ```ignore
/// use oxi_cli::CliArgs;
///
/// fn main() {
///     let args = CliArgs::parse();
///     match args.command {
///         Some(Commands::Sessions) => { /* list sessions */ }
///         Some(Commands::Tree { session_id }) => { /* show tree */ }
///         _ => { /* interactive mode */ }
///     }
/// }
/// ```
pub fn parse_args() -> CliArgs {
    CliArgs::parse()
}

/// Parse CLI arguments from a specific iterator
pub fn parse_args_from<I, T>(iter: I) -> Result<CliArgs, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    CliArgs::try_parse_from(iter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_prompt() {
        let args = parse_args_from(["oxi", "Hello", "world"]).unwrap();
        assert_eq!(args.prompt, vec!["Hello", "world"]);
    }

    #[test]
    fn test_parse_with_provider_and_model() {
        let args = parse_args_from([
            "oxi",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-20250514",
            "Hello",
        ])
        .unwrap();
        assert_eq!(args.provider, Some("anthropic".to_string()));
        assert_eq!(args.model, Some("claude-sonnet-4-20250514".to_string()));
    }

    #[test]
    fn test_parse_interactive_flag() {
        let args = parse_args_from(["oxi", "-i"]).unwrap();
        assert!(args.interactive);
    }

    #[test]
    fn test_parse_extension_paths() {
        let args =
            parse_args_from(["oxi", "-e", "/path/to/ext.so", "-e", "/other/ext.so"]).unwrap();
        assert_eq!(args.extensions.len(), 2);
    }

    #[test]
    fn test_parse_sessions_command() {
        let args = parse_args_from(["oxi", "sessions"]).unwrap();
        assert!(matches!(args.command, Some(Commands::Sessions)));
    }

    #[test]
    fn test_parse_tree_command() {
        let args = parse_args_from(["oxi", "tree", "abc-123"]).unwrap();
        match args.command {
            Some(Commands::Tree { session_id }) => {
                assert_eq!(session_id, "abc-123");
            }
            _ => panic!("Expected Tree command"),
        }
    }

    #[test]
    fn test_parse_tree_command_default() {
        let args = parse_args_from(["oxi", "tree"]).unwrap();
        match args.command {
            Some(Commands::Tree { session_id }) => {
                assert_eq!(session_id, "");
            }
            _ => panic!("Expected Tree command"),
        }
    }

    #[test]
    fn test_parse_fork_command() {
        let args = parse_args_from(["oxi", "fork", "parent-id", "entry-id"]).unwrap();
        match args.command {
            Some(Commands::Fork {
                parent_id,
                entry_id,
            }) => {
                assert_eq!(parent_id, "parent-id");
                assert_eq!(entry_id, "entry-id");
            }
            _ => panic!("Expected Fork command"),
        }
    }

    #[test]
    fn test_parse_delete_command() {
        let args = parse_args_from(["oxi", "delete", "session-123"]).unwrap();
        match args.command {
            Some(Commands::Delete { session_id }) => {
                assert_eq!(session_id, "session-123");
            }
            _ => panic!("Expected Delete command"),
        }
    }

    #[test]
    fn test_parse_pkg_install() {
        let args = parse_args_from(["oxi", "pkg", "install", "npm:@scope/name"]).unwrap();
        match args.command {
            Some(Commands::Pkg { action }) => match action {
                PkgCommands::Install { source } => {
                    assert_eq!(source, "npm:@scope/name");
                }
                _ => panic!("Expected Install subcommand"),
            },
            _ => panic!("Expected Pkg command"),
        }
    }

    #[test]
    fn test_parse_pkg_list() {
        let args = parse_args_from(["oxi", "pkg", "list"]).unwrap();
        match args.command {
            Some(Commands::Pkg { action }) => {
                assert!(matches!(action, PkgCommands::List));
            }
            _ => panic!("Expected Pkg command"),
        }
    }

    #[test]
    fn test_parse_pkg_update_all() {
        let args = parse_args_from(["oxi", "pkg", "update"]).unwrap();
        match args.command {
            Some(Commands::Pkg { action }) => match action {
                PkgCommands::Update { name } => assert!(name.is_none()),
                _ => panic!("Expected Update subcommand"),
            },
            _ => panic!("Expected Pkg command"),
        }
    }

    #[test]
    fn test_parse_pkg_update_named() {
        let args = parse_args_from(["oxi", "pkg", "update", "my-pkg"]).unwrap();
        match args.command {
            Some(Commands::Pkg { action }) => match action {
                PkgCommands::Update { name } => assert_eq!(name, Some("my-pkg".to_string())),
                _ => panic!("Expected Update subcommand"),
            },
            _ => panic!("Expected Pkg command"),
        }
    }

    #[test]
    fn test_parse_config_show() {
        let args = parse_args_from(["oxi", "config", "show"]).unwrap();
        assert!(matches!(
            args.command,
            Some(Commands::Config {
                action: ConfigCommands::Show
            })
        ));
    }

    #[test]
    fn test_parse_config_set() {
        let args = parse_args_from(["oxi", "config", "set", "theme", "dracula"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::Set { key, value } => {
                    assert_eq!(key, "theme");
                    assert_eq!(value, "dracula");
                }
                _ => panic!("Expected Set subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_get() {
        let args = parse_args_from(["oxi", "config", "get", "theme"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::Get { key } => {
                    assert_eq!(key, "theme");
                }
                _ => panic!("Expected Get subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_enable() {
        let args = parse_args_from(["oxi", "config", "enable", "extension", "my-ext"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::Enable {
                    resource_type,
                    name,
                } => {
                    assert_eq!(resource_type, "extension");
                    assert_eq!(name, "my-ext");
                }
                _ => panic!("Expected Enable subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_disable() {
        let args = parse_args_from(["oxi", "config", "disable", "skill", "my-skill"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::Disable {
                    resource_type,
                    name,
                } => {
                    assert_eq!(resource_type, "skill");
                    assert_eq!(name, "my-skill");
                }
                _ => panic!("Expected Disable subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_list() {
        let args = parse_args_from(["oxi", "config", "list"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::List { resource_type } => {
                    assert!(resource_type.is_none());
                }
                _ => panic!("Expected List subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_list_filtered() {
        let args = parse_args_from(["oxi", "config", "list", "extensions"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::List { resource_type } => {
                    assert_eq!(resource_type, Some("extensions".to_string()));
                }
                _ => panic!("Expected List subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_thinking_level_reexport() {
        // Verify the re-export from settings works
        assert_eq!(format!("{:?}", ThinkingLevel::Medium), "Medium");
    }

    #[test]
    fn test_parse_config_add_provider() {
        let args = parse_args_from([
            "oxi",
            "config",
            "add-provider",
            "minimax",
            "https://api.minimax.chat/v1",
            "MINIMAX_API_KEY",
            "openai-completions",
        ])
        .unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::AddProvider {
                    name,
                    base_url,
                    api_key_env,
                    api,
                } => {
                    assert_eq!(name, "minimax");
                    assert_eq!(base_url, "https://api.minimax.chat/v1");
                    assert_eq!(api_key_env, "MINIMAX_API_KEY");
                    assert_eq!(api, "openai-completions");
                }
                _ => panic!("Expected AddProvider subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_add_provider_default_api() {
        let args = parse_args_from([
            "oxi",
            "config",
            "add-provider",
            "zai",
            "https://api.z.ai/v1",
            "ZAI_API_KEY",
        ])
        .unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::AddProvider {
                    name,
                    base_url,
                    api_key_env,
                    api,
                } => {
                    assert_eq!(name, "zai");
                    assert_eq!(base_url, "https://api.z.ai/v1");
                    assert_eq!(api_key_env, "ZAI_API_KEY");
                    assert_eq!(api, "openai-completions"); // default
                }
                _ => panic!("Expected AddProvider subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_config_remove_provider() {
        let args = parse_args_from(["oxi", "config", "remove-provider", "minimax"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::RemoveProvider { name } => {
                    assert_eq!(name, "minimax");
                }
                _ => panic!("Expected RemoveProvider subcommand"),
            },
            _ => panic!("Expected Config command"),
        }
    }

    #[test]
    fn test_parse_models_command() {
        let args = parse_args_from(["oxi", "models"]).unwrap();
        match args.command {
            Some(Commands::Models { provider }) => {
                assert!(provider.is_none());
            }
            _ => panic!("Expected Models command"),
        }
    }

    #[test]
    fn test_parse_models_with_provider() {
        let args = parse_args_from(["oxi", "models", "--provider", "minimax"]).unwrap();
        match args.command {
            Some(Commands::Models { provider }) => {
                assert_eq!(provider, Some("minimax".to_string()));
            }
            _ => panic!("Expected Models command"),
        }
    }

    #[test]
    fn test_parse_setup_command() {
        let args = parse_args_from(["oxi", "setup"]).unwrap();
        match args.command {
            Some(Commands::Setup { reset }) => {
                assert!(!reset);
            }
            _ => panic!("Expected Setup command"),
        }
    }

    #[test]
    fn test_parse_setup_reset() {
        let args = parse_args_from(["oxi", "setup", "--reset"]).unwrap();
        match args.command {
            Some(Commands::Setup { reset }) => {
                assert!(reset);
            }
            _ => panic!("Expected Setup command with reset"),
        }
    }

    // ── Routing flags ──────────────────────────────────────────────

    #[test]
    fn test_parse_enable_routing_flag() {
        let args = parse_args_from(["oxi", "--enable-routing", "Hello"]).unwrap();
        assert!(args.enable_routing);
        assert!(!args.prefer_cost_efficient);
        assert!(args.fallback_chain.is_empty());
        assert!(!args.disable_fallback);
    }

    #[test]
    fn test_parse_prefer_cost_efficient_flag() {
        let args = parse_args_from(["oxi", "--prefer-cost-efficient", "Hello"]).unwrap();
        // prefer_cost_efficient alone should NOT set enable_routing
        assert!(!args.enable_routing); // enable_routing is a separate flag
        assert!(args.prefer_cost_efficient);
        assert!(args.fallback_chain.is_empty());
        assert!(!args.disable_fallback);
    }

    #[test]
    fn test_parse_fallback_chain_single() {
        let args = parse_args_from(["oxi", "--fallback-chain", "openai/gpt-4o", "Hello"]).unwrap();
        assert_eq!(args.fallback_chain, vec!["openai/gpt-4o"]);
    }

    #[test]
    fn test_parse_fallback_chain_comma_separated() {
        let args = parse_args_from([
            "oxi",
            "--fallback-chain",
            "openai/gpt-4o,anthropic/claude-3",
            "Hello",
        ])
        .unwrap();
        assert_eq!(
            args.fallback_chain,
            vec!["openai/gpt-4o", "anthropic/claude-3"]
        );
    }

    #[test]
    fn test_parse_fallback_chain_multiple_args() {
        let args = parse_args_from([
            "oxi",
            "--fallback-chain",
            "openai/gpt-4o",
            "--fallback-chain",
            "anthropic/claude-3",
            "Hello",
        ])
        .unwrap();
        assert_eq!(
            args.fallback_chain,
            vec!["openai/gpt-4o", "anthropic/claude-3"]
        );
    }

    #[test]
    fn test_parse_fallback_chain_empty() {
        let args = parse_args_from(["oxi", "Hello"]).unwrap();
        assert!(args.fallback_chain.is_empty());
    }

    #[test]
    fn test_parse_disable_fallback_flag() {
        let args = parse_args_from(["oxi", "--disable-fallback", "Hello"]).unwrap();
        assert!(args.disable_fallback);
    }

    #[test]
    fn test_parse_routing_all_flags() {
        let args = parse_args_from([
            "oxi",
            "--enable-routing",
            "--prefer-cost-efficient",
            "--fallback-chain",
            "openai/gpt-4o,anthropic/claude-3",
            "--disable-fallback",
            "Hello",
        ])
        .unwrap();
        assert!(args.enable_routing);
        assert!(args.prefer_cost_efficient);
        assert_eq!(
            args.fallback_chain,
            vec!["openai/gpt-4o", "anthropic/claude-3"]
        );
        assert!(args.disable_fallback);
    }

    // ── Reset command ────────────────────────────────────────────

    #[test]
    fn test_parse_reset_command() {
        let args = parse_args_from(["oxi", "reset"]).unwrap();
        match args.command {
            Some(Commands::Reset {
                yes,
                include_project,
            }) => {
                assert!(!yes);
                assert!(!include_project);
            }
            _ => panic!("Expected Reset command"),
        }
    }

    #[test]
    fn test_parse_reset_yes_flag() {
        let args = parse_args_from(["oxi", "reset", "--yes"]).unwrap();
        match args.command {
            Some(Commands::Reset {
                yes,
                include_project,
            }) => {
                assert!(yes);
                assert!(!include_project);
            }
            _ => panic!("Expected Reset command with --yes"),
        }
    }

    #[test]
    fn test_parse_reset_include_project() {
        let args = parse_args_from(["oxi", "reset", "--yes", "--include-project"]).unwrap();
        match args.command {
            Some(Commands::Reset {
                yes,
                include_project,
            }) => {
                assert!(yes);
                assert!(include_project);
            }
            _ => panic!("Expected Reset command with all flags"),
        }
    }
}
