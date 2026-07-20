//! Shim of `xai_grok_workspace::permission`.

/// Permission ID for the "always approve" option.
pub const ENABLE_ALWAYS_APPROVE_OPTION_ID: &str = "always_approve";

/// Check whether the "enable always approve" option is available.
pub fn is_enable_always_approve_option() -> bool {
    false
}
