//! oxicode-cli-specific agent tools.
//!
//! Complements the built-in tools in `oxicode-agent` with product-specific
//! functionality owned by oxicode-cli (composition-root pattern, see AGENTS.md
//! §"SDK is the contract, not the implementation"). Each tool here holds an
//! `Arc` to a domain store defined in `crate::store::*` and is registered
//! directly from `App::from_oxicode`.

pub mod issue_tool;
pub use issue_tool::{IssueTool, format_issue_full, format_issue_line};
