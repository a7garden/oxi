//! Grammar tokens and keywords. Mirrors omp's `grammar.lark` but as Rust
//! constants/enums — no parser generator needed (the parser is hand-written).
//!
//! All line ops supported in the default build (block-ops require tree-sitter
//! and are behind the `block-ops` feature gate — future).

// Keywords already re-exported from format.rs, but this module provides
// the semantic grouping and doc comments for quick reference.

/// All hunk-header keywords that introduce a line operation.
pub const LINE_OP_KEYWORDS: &[&str] =
    &["SWAP", "DEL", "INS.PRE", "INS.POST", "INS.HEAD", "INS.TAIL"];

/// Check whether a keyword string is a known line operation.
pub fn is_line_op(keyword: &str) -> bool {
    LINE_OP_KEYWORDS.contains(&keyword)
}

/// Check whether a keyword is an insert variant.
pub fn is_insert_op(keyword: &str) -> bool {
    keyword.starts_with("INS.")
}
