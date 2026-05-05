# Progress

## 2026-05-05 — Fix: oxi-cli unify CLI parsers (main.rs + cli.rs)

- **cli.rs**: Replaced disconnected `CliArgs`/`Commands`/`ThinkingLevel`/`OutputMode`/`InstallArgs`/`RemoveArgs`/`UpdateArgs`/`ListArgs` with unified types that match main.rs's actual usage
- **cli.rs**: Removed duplicate `ThinkingLevel` enum (6 variants: Off/Minimal/Low/Medium/High/XHigh) — now re-exports the canonical `settings::ThinkingLevel` (4 variants: None/Minimal/Standard/Thorough)
- **main.rs**: Removed all local type definitions (`Args`, `Commands`, `PkgCommands`, `ConfigCommands`, `parse_thinking_level`) — now imports from `cli` module
- **main.rs**: Uses `CliArgs` instead of `Args`; references `cli::Commands`, `cli::PkgCommands`, `cli::ConfigCommands`
- All subcommands preserved: sessions, tree, fork, delete, pkg (install/list/uninstall/update), config (show/list/enable/disable/set/get)
- `cargo check -p oxi-cli` passes clean (0 errors)

## 2026-05-05 — Fix oxi-cli ignored tests

- Fixed `test_parse_version_invalid`: Changed `parse_version()` to reject non-exactly-3-component version strings (`parts.len() == 3` instead of `>= 3`)
- Fixed `test_calculate_versions_behind`: Rewrote to use per-component difference with weighting (major × 2, minor × 1, patch × 1) instead of raw `compare_versions` result
- Removed both `#[ignore]` attributes
- All 22 version_check tests pass (0 ignored)
