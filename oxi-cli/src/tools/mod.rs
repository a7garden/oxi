//! oxi-cli-specific agent tools.
//!
//! Complements the built-in tools in `oxi-agent` with product-specific
//! functionality owned by oxi-cli (composition-root pattern, see AGENTS.md
//! §"SDK is the contract, not the implementation"). Each tool here holds an
//! `Arc` to a domain store defined in `crate::store::*` and is registered
//! directly from `App::from_oxi`.

pub mod issue_tool;
pub use issue_tool::{IssueTool, format_issue_full, format_issue_line};
