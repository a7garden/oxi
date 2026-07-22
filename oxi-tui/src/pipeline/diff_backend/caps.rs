//! Terminal capability detection — re-export shim.
//!
//! As of Task 13 (PR-3 2/3), `TerminalCaps` lives in
//! [`crate::theme::capability`], where detection and theme color-level
//! adaptation share the same module. This shim preserves the
//! `pipeline::diff_backend::caps::TerminalCaps` import path so existing
//! callers in `DiffBackend` continue to work without churn.
//!
//! See `theme::capability` for the full capability table, the
//! `TerminalKind` classifier, the `ColorLevel` enum, and the
//! `adapt_color` / `adapt_theme` helpers.

pub use crate::theme::capability::{ImageProtocol, TerminalCaps};
