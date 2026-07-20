//! Shim of `xai_grok_workspace::permission`.

/// Permission ID for the "always approve" option.
pub const ENABLE_ALWAYS_APPROVE_OPTION_ID: &str = "always_approve";

/// Check whether the "enable always approve" option is available.
pub fn is_enable_always_approve_option() -> bool {
    false
}

/// Stub for mcp_pretty_name_if_qualified.
pub fn mcp_pretty_name_if_qualified(name: &str) -> String {
    name.to_string()
}
/// Stub for MCP tool name delimiter.
pub const MCP_TOOL_NAME_DELIMITER: &str = "__";
/// Stub for mcp_titleize_segment.
pub fn mcp_titleize_segment(seg: &str) -> String {
    seg.to_string()
}
/// Stub for FuzzyMatchResult.
pub struct FuzzyMatchResult;
