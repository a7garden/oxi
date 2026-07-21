//! CLI-side LSP adapter.
//!
//! Implements [`oxi_agent::tools::LspProvider`] by routing actions
//! to one or more [`oxi_lsp::LspClient`] instances. The adapter owns
//! the multi-server lifecycle (per-language servers) and exposes a
//! single `LspProvider` surface to the agent's `lsp` tool.
//!
//! See `docs/designs/2026-07-18-stub-completion.md` §4.3.
//!
//! # Scope
//!
//! MVP: a static server table mapping file extensions to spawn
//! commands. The first request for a given extension lazily starts
//! that server. Crash recovery / lifetime restart budget / folder
//! trust gating are documented follow-ups (see design doc).
//!
//! # Server configuration
//!
//! Servers are configured via two sources (in priority order):
//!
//! 1. Programmatic: [`CliLspProvider::with_servers`].
//! 2. Default table: `default_servers()` returns `rust-analyzer`
//!    for `.rs` files when the binary is on `PATH`.
//!
//! Settings-file discovery (`.oxi/lsp.toml`) is intentionally
//! deferred — the programmatic path is enough for SDK consumers and
//! the `oxi` CLI's bootstrap.

pub mod manager;
pub mod provider;

pub use manager::{LspManager, LspServerConfig};
pub use provider::CliLspProvider;
