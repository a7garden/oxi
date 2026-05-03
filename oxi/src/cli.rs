//! CLI argument parsing with clap
//!
//! Provides command-line argument parsing for the oxi CLI.

use clap::{Arg, ArgAction, Command, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::str::FromStr;

/// Thinking level options
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl std::fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThinkingLevel::Off => write!(f, "off"),
            ThinkingLevel::Minimal => write!(f, "minimal"),
            ThinkingLevel::Low => write!(f, "low"),
            ThinkingLevel::Medium => write!(f, "medium"),
            ThinkingLevel::High => write!(f, "high"),
            ThinkingLevel::XHigh => write!(f, "xhigh"),
        }
    }
}

impl FromStr for ThinkingLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(ThinkingLevel::Off),
            "minimal" => Ok(ThinkingLevel::Minimal),
            "low" => Ok(ThinkingLevel::Low),
            "medium" => Ok(ThinkingLevel::Medium),
            "high" => Ok(ThinkingLevel::High),
            "xhigh" | "x-high" => Ok(ThinkingLevel::XHigh),
            _ => Err(format!("Invalid thinking level: {}", s)),
        }
    }
}

/// Output mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputMode {
    Text,
    Json,
    Rpc,
}

impl std::fmt::Display for OutputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputMode::Text => write!(f, "text"),
            OutputMode::Json => write!(f, "json"),
            OutputMode::Rpc => write!(f, "rpc"),
        }
    }
}

/// CLI arguments for the main chat command
/// CLI arguments for the install command
#[derive(Debug, Clone, Parser)]
pub struct InstallArgs {
    /// Source to install (URL, git repo, or npm package)
    pub source: String,

    /// Local only (don't add to settings)
    #[arg(short = 'l', long)]
    pub local: bool,

    /// Install globally
    #[arg(short = 'g', long)]
    pub global: bool,
}

/// CLI arguments for the remove command
#[derive(Debug, Clone, Parser)]
pub struct RemoveArgs {
    /// Source to remove
    pub source: String,

    /// Local only
    #[arg(short = 'l', long)]
    pub local: bool,
}

/// CLI arguments for the update command
#[derive(Debug, Clone, Parser)]
pub struct UpdateArgs {
    /// Source to update (or 'self' for oxi, 'pi' for package)
    pub source: Option<String>,

    /// Update all
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Force update
    #[arg(short = 'f', long)]
    pub force: bool,
}

/// CLI arguments for the list command
#[derive(Debug, Clone, Parser)]
pub struct ListArgs {
    /// Show installed extensions
    #[arg(long)]
    pub extensions: bool,

    /// Show installed skills
    #[arg(long)]
    pub skills: bool,

    /// Show installed prompts
    #[arg(long)]
    pub prompts: bool,

    /// Show installed themes
    #[arg(long)]
    pub themes: bool,

    /// Include disabled
    #[arg(long, short = 'a')]
    pub include_disabled: bool,
}

/// CLI subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Install extension source
    Install(InstallArgs),
    /// Remove extension source
    Remove(RemoveArgs),
    /// Uninstall extension source (alias for remove)
    Uninstall(RemoveArgs),
    /// Update oxi and extensions
    Update(UpdateArgs),
    /// List installed resources
    List(ListArgs),
    /// Open config selector TUI
    Config,
}

/// Main CLI arguments
#[derive(Debug, Clone, Parser)]
#[command(name = "oxi")]
#[command(about = "AI coding assistant with read, bash, edit, write tools")]
pub struct CliArgs {
    /// Provider name
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Model pattern
    #[arg(short, long)]
    pub model: Option<String>,

    /// API key
    #[arg(long)]
    pub api_key: Option<String>,

    /// System prompt
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Append system prompt
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Vec<String>,

    /// Thinking level
    #[arg(long)]
    pub thinking: Option<ThinkingLevel>,

    /// Continue session
    #[arg(short = 'c', long)]
    pub continue_session: bool,

    /// Resume session
    #[arg(short = 'r', long)]
    pub resume: bool,

    /// Session
    #[arg(long)]
    pub session: Option<String>,

    /// Fork session
    #[arg(long)]
    pub fork: Option<String>,

    /// Session directory
    #[arg(long)]
    pub session_dir: Option<PathBuf>,

    /// No session
    #[arg(long)]
    pub no_session: bool,

    /// Models for cycling
    #[arg(long)]
    pub models: Option<String>,

    /// No tools
    #[arg(long = "no-tools", short = 't')]
    pub no_tools: bool,

    /// No built-in tools
    #[arg(long = "no-builtin-tools")]
    pub no_builtin_tools: bool,

    /// Tools allowlist
    #[arg(short = 'o', long)]
    pub tools: Option<String>,

    /// Print mode
    #[arg(long)]
    pub print: bool,

    /// Export
    #[arg(long)]
    pub export: Option<PathBuf>,

    /// Extensions
    #[arg(short = 'e', long)]
    pub extension: Vec<PathBuf>,

    /// No extensions
    #[arg(long)]
    pub no_extensions: bool,

    /// Skills
    #[arg(long)]
    pub skill: Vec<PathBuf>,

    /// No skills
    #[arg(long = "no-skills")]
    pub no_skills: bool,

    /// Prompt templates
    #[arg(long = "prompt-template")]
    pub prompt_template: Vec<PathBuf>,

    /// No prompt templates
    #[arg(long = "no-prompt-templates")]
    pub no_prompt_templates: bool,

    /// Themes
    #[arg(long)]
    pub theme: Vec<PathBuf>,

    /// No themes
    #[arg(long)]
    pub no_themes: bool,

    /// No context files
    #[arg(long = "no-context-files")]
    pub no_context_files: bool,

    /// List models
    #[arg(long)]
    pub list_models: Option<Option<String>>,

    /// Verbose
    #[arg(long)]
    pub verbose: bool,

    /// Offline
    #[arg(long)]
    pub offline: bool,

    /// Command to run
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Messages
    pub messages: Vec<String>,

    /// File arguments
    #[arg(short = 'f')]
    pub file_args: Vec<PathBuf>,
}

/// Parse CLI arguments from the command line
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

/// Check if stdin is piped (for print mode detection)
pub fn is_stdin_piped() -> bool {
    // Simple check using is_terminal()
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        return !std::io::stdin().is_terminal();
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Detect if we're running in print mode (non-interactive)
pub fn detect_print_mode() -> bool {
    // Check for print flag
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-p" || a == "--print") {
        return true;
    }

    // Check if stdin is piped
    is_stdin_piped()
}

/// Get the version string
pub fn get_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!("{}", version)
}

/// Generate shell completion script
pub fn generate_completion(shell: &str) -> String {
    // Requires clap_complete crate
    format!("# Shell completion for {} is not yet implemented.\n# Install clap_complete to enable this feature.", shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_args() {
        let args = parse_args_from(["oxi", "Hello", "world"]).unwrap();
        assert_eq!(args.messages, vec!["Hello", "world"]);
    }

    #[test]
    fn test_parse_with_provider_and_model() {
        let args = parse_args_from([
            "oxi",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
            "Hello",
        ])
        .unwrap();
        assert_eq!(args.provider, Some("anthropic".to_string()));
        assert_eq!(args.model, Some("claude-sonnet-4-5".to_string()));
    }

    #[test]
    fn test_parse_with_thinking_level() {
        let args = parse_args_from(["oxi", "--thinking", "high", "Hello"]).unwrap();
        assert_eq!(args.thinking, Some(ThinkingLevel::High));
    }

    #[test]
    fn test_parse_with_tools() {
        let args = parse_args_from(["oxi", "-o", "read,bash,edit", "Hello"]).unwrap();
        assert_eq!(args.tools, Some("read,bash,edit".to_string()));
    }

    #[test]
    fn test_parse_with_multiple_files() {
        let args = parse_args_from([
            "oxi",
            "@file1.txt",
            "@file2.txt",
            "Hello",
        ])
        .unwrap();
        assert_eq!(args.file_args.len(), 2);
    }

    #[test]
    fn test_parse_print_mode() {
        let args = parse_args_from(["oxi", "--print", "Hello"]).unwrap();
        assert!(args.print);
    }

    #[test]
    fn test_parse_resume_flag() {
        let args = parse_args_from(["oxi", "-r"]).unwrap();
        assert!(args.resume);
    }

    #[test]
    fn test_parse_continue_flag() {
        let args = parse_args_from(["oxi", "-c"]).unwrap();
        assert!(args.continue_session);
    }

    #[test]
    fn test_parse_subcommand() {
        let args = parse_args_from(["oxi", "config"]).unwrap();
        assert!(matches!(args.command, Some(Commands::Config)));
    }

    #[test]
    fn test_parse_install_command() {
        let args = parse_args_from(["oxi", "install", "git:https://github.com/example/ext"])
            .unwrap();
        match args.command {
            Some(Commands::Install(install_args)) => {
                assert_eq!(install_args.source, "git:https://github.com/example/ext");
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_parse_remove_command() {
        let args = parse_args_from(["oxi", "remove", "example-ext"]).unwrap();
        match args.command {
            Some(Commands::Remove(remove_args)) => {
                assert_eq!(remove_args.source, "example-ext");
            }
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_parse_update_command() {
        let args = parse_args_from(["oxi", "update", "self"]).unwrap();
        match args.command {
            Some(Commands::Update(update_args)) => {
                assert_eq!(update_args.source, Some("self".to_string()));
            }
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_parse_list_command() {
        let args = parse_args_from(["oxi", "list", "--extensions"]).unwrap();
        match args.command {
            Some(Commands::List(list_args)) => {
                assert!(list_args.extensions);
            }
            _ => panic!("Expected List command"),
        }
    }

    #[test]
    fn test_thinking_level_from_str() {
        assert_eq!("high".parse::<ThinkingLevel>().unwrap(), ThinkingLevel::High);
        assert_eq!("off".parse::<ThinkingLevel>().unwrap(), ThinkingLevel::Off);
        assert_eq!("xhigh".parse::<ThinkingLevel>().unwrap(), ThinkingLevel::XHigh);
        assert!("invalid".parse::<ThinkingLevel>().is_err());
    }

    #[test]
    fn test_thinking_level_display() {
        assert_eq!(ThinkingLevel::High.to_string(), "high");
        assert_eq!(ThinkingLevel::XHigh.to_string(), "xhigh");
    }
}