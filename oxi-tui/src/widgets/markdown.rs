//! Lightweight markdown utilities for terminal rendering.
//!
//! Provides helper functions for markdown content that supplements tui-markdown.

// This module previously contained an inline markdown parser (parse_inline),
// LineType detection, and style helpers that were never used in production.
// They have been removed. All markdown rendering goes through `tui_markdown::from_str()`
// in chat.rs. If custom inline parsing is needed in the future, re-implement here.
