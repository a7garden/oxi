//! CLI argument parsing tests using clap's try_parse_from.
//!
//! Tests CliArgs and subcommand parsing directly without spawning `cargo run`,
//! making them fast (~milliseconds) and deterministic.

#[cfg(test)]
mod tests {
    use clap::Parser;
    use oxi::cli::{CliArgs, Commands, ConfigCommands, ExtCommands, PkgCommands};

    // ── Basic flag parsing ───────────────────────────────────────────

    #[test]
    fn test_version_flag() {
        let res = CliArgs::try_parse_from(["oxi", "--version"]);
        // --version causes a DisplayVersion error, not a normal parse
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("oxi"));
    }

    #[test]
    fn test_help_flag() {
        let res = CliArgs::try_parse_from(["oxi", "--help"]);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("Usage:"));
    }

    #[test]
    fn test_default_args() {
        let args = CliArgs::try_parse_from(["oxi"]).unwrap();
        assert!(args.command.is_none());
        assert!(args.provider.is_none());
        assert!(args.model.is_none());
        // default_value = "" creates a Vec with one empty string
        assert_eq!(args.prompt, vec![""]);
        assert!(!args.interactive);
        assert!(!args.print);
        assert!(!args.no_session);
    }

    #[test]
    fn test_provider_and_model_flags() {
        let args =
            CliArgs::try_parse_from(["oxi", "-p", "anthropic", "-m", "claude-sonnet-4"]).unwrap();
        assert_eq!(args.provider.as_deref(), Some("anthropic"));
        assert_eq!(args.model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn test_prompt_args() {
        let args = CliArgs::try_parse_from(["oxi", "hello", "world"]).unwrap();
        assert_eq!(args.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn test_print_mode() {
        let args = CliArgs::try_parse_from(["oxi", "--print", "test prompt"]).unwrap();
        assert!(args.print);
        assert_eq!(args.prompt, vec!["test prompt"]);
    }

    #[test]
    fn test_rpc_mode() {
        let args = CliArgs::try_parse_from(["oxi", "--mode", "rpc"]).unwrap();
        assert_eq!(args.mode.as_deref(), Some("rpc"));
    }

    #[test]
    fn test_no_session() {
        let args = CliArgs::try_parse_from(["oxi", "--no-session"]).unwrap();
        assert!(args.no_session);
    }

    #[test]
    fn test_continue_session() {
        let args = CliArgs::try_parse_from(["oxi", "-c"]).unwrap();
        assert!(args.continue_session);
    }

    #[test]
    fn test_interactive_flag() {
        let args = CliArgs::try_parse_from(["oxi", "-i"]).unwrap();
        assert!(args.interactive);
    }

    #[test]
    fn test_thinking_flag() {
        let args = CliArgs::try_parse_from(["oxi", "--thinking", "thorough"]).unwrap();
        assert_eq!(args.thinking.as_deref(), Some("thorough"));
    }

    #[test]
    fn test_timeout_flag() {
        let args = CliArgs::try_parse_from(["oxi", "--timeout", "30"]).unwrap();
        assert_eq!(args.timeout, Some(30));
    }

    // ── Subcommand parsing ──────────────────────────────────────────

    #[test]
    fn test_config_show_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "config", "show"]).unwrap();
        match args.command {
            Some(Commands::Config { action }) => match action {
                ConfigCommands::Show => {}
                other => panic!("Expected Show, got {:?}", other),
            },
            other => panic!("Expected Config command, got {:?}", other),
        }
    }

    #[test]
    fn test_sessions_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "sessions"]).unwrap();
        assert!(matches!(args.command, Some(Commands::Sessions)));
    }

    #[test]
    fn test_pkg_list_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "pkg", "list"]).unwrap();
        match args.command {
            Some(Commands::Pkg { action }) => match action {
                PkgCommands::List => {}
                other => panic!("Expected List, got {:?}", other),
            },
            other => panic!("Expected Pkg command, got {:?}", other),
        }
    }

    #[test]
    fn test_pkg_install_subcommand() {
        let args =
            CliArgs::try_parse_from(["oxi", "pkg", "install", "npm:@scope/my-package"]).unwrap();
        match args.command {
            Some(Commands::Pkg { action }) => match action {
                PkgCommands::Install { source } => {
                    assert_eq!(source, "npm:@scope/my-package");
                }
                other => panic!("Expected Install, got {:?}", other),
            },
            other => panic!("Expected Pkg command, got {:?}", other),
        }
    }

    #[test]
    fn test_ext_list_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "ext", "list"]).unwrap();
        match args.command {
            Some(Commands::Ext { action }) => match action {
                ExtCommands::List => {}
                other => panic!("Expected List, got {:?}", other),
            },
            other => panic!("Expected Ext command, got {:?}", other),
        }
    }

    #[test]
    fn test_models_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "models"]).unwrap();
        assert!(matches!(args.command, Some(Commands::Models { .. })));
    }

    #[test]
    fn test_models_with_provider_filter() {
        let args = CliArgs::try_parse_from(["oxi", "models", "--provider", "openai"]).unwrap();
        match args.command {
            Some(Commands::Models { provider }) => {
                assert_eq!(provider.as_deref(), Some("openai"));
            }
            other => panic!("Expected Models command, got {:?}", other),
        }
    }

    #[test]
    fn test_setup_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "setup"]).unwrap();
        assert!(matches!(args.command, Some(Commands::Setup { .. })));
    }

    #[test]
    fn test_setup_reset_flag() {
        let args = CliArgs::try_parse_from(["oxi", "setup", "--reset"]).unwrap();
        match args.command {
            Some(Commands::Setup { reset }) => assert!(reset),
            other => panic!("Expected Setup command, got {:?}", other),
        }
    }

    #[test]
    fn test_tree_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "tree", "session-123"]).unwrap();
        match args.command {
            Some(Commands::Tree { session_id }) => {
                assert_eq!(session_id, "session-123");
            }
            other => panic!("Expected Tree command, got {:?}", other),
        }
    }

    #[test]
    fn test_delete_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "delete", "session-abc"]).unwrap();
        match args.command {
            Some(Commands::Delete { session_id }) => {
                assert_eq!(session_id, "session-abc");
            }
            other => panic!("Expected Delete command, got {:?}", other),
        }
    }

    #[test]
    fn test_fork_subcommand() {
        let args = CliArgs::try_parse_from(["oxi", "fork", "parent-123", "entry-456"]).unwrap();
        match args.command {
            Some(Commands::Fork {
                parent_id,
                entry_id,
            }) => {
                assert_eq!(parent_id, "parent-123");
                assert_eq!(entry_id, "entry-456");
            }
            other => panic!("Expected Fork command, got {:?}", other),
        }
    }

    // ── Invalid input ───────────────────────────────────────────────

    #[test]
    fn test_unknown_subcommand_treated_as_prompt() {
        // Unknown tokens that aren't recognized subcommands get collected as prompt args
        let args = CliArgs::try_parse_from(["oxi", "nonexistent_subcommand"]);
        // clap doesn't fail here — it treats it as a positional prompt arg
        assert!(args.is_ok());
        assert_eq!(args.unwrap().prompt, vec!["nonexistent_subcommand"]);
    }

    #[test]
    fn test_pkg_subcommand_without_action_fails() {
        let res = CliArgs::try_parse_from(["oxi", "pkg"]);
        assert!(res.is_err());
    }
}
