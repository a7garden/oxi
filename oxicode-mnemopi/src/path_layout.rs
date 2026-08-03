//! Workspace path layout utilities — ported from grok-build
//! `xai-grok-memory/src/storage.rs:55-74` (Apache-2.0).
//!
//! Helpers for deriving stable, filesystem-safe identifiers from a
//! workspace path so per-project memory banks can be auto-named without
//! user configuration.
//!
//! ## `{slug}-{hash8}` convention
//!
//! The grok-build pattern is `{directory_basename}-{blake3(cwd)[..8]}` —
//! e.g. cwd `/Users/me/code/oxicode` → bank directory `oxicode-a3f7b2c9`. The
//! hash disambiguates same-name projects in different parent paths.
//!
//! ## Ephemeral CWD detection
//!
//! When the CWD is a temp directory (`/tmp`, OS temp, `cargo-target`,
//! `*scratch*`), writing persistent memory is usually wrong — the
//! directory vanishes on reboot. [`is_ephemeral_cwd`] returns true for
//! those paths so callers can skip or reroute memory writes.

use std::path::Path;

use sha2::{Digest, Sha256};

/// Compute the grok-style `{slug}-{hash8}` workspace identifier.
///
/// `slug` is the basename of `cwd` (e.g. `oxicode` for `/Users/me/code/oxicode`).
/// The hash is the first 8 hex chars of SHA-256 over the canonicalized
/// absolute path. Returns `"default"` for empty paths.
///
/// The result is filesystem-safe: alphanumeric + `-`, no path separators.
pub fn workspace_slug_hash(cwd: &Path) -> String {
    let slug = cwd
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("default");

    // Use the absolute path if available; fall back to the raw input.
    // `dunce::canonicalize` would be ideal for Windows UNC simplification
    // but we keep the dep surface small and accept symlinks as-is.
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let hash_input = canonical.to_string_lossy();
    let hash = Sha256::digest(hash_input.as_bytes());
    let hash8: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();

    // Sanitize slug: keep alphanumerics + dashes, drop everything else
    // (replaces spaces, dots in names like `my.project`).
    let clean_slug: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                // Replace dots, underscores, spaces, and any other
                // non-alphanumeric char with a dash.
                '-'
            }
        })
        .collect();
    // Collapse runs of '-' and trim.
    let mut collapsed = String::with_capacity(clean_slug.len());
    let mut prev_dash = false;
    for c in clean_slug.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push('-');
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    let final_slug = if trimmed.is_empty() {
        "default"
    } else {
        trimmed
    };

    format!("{final_slug}-{hash8}")
}

/// Returns `true` when `cwd` is a temporary or scratch directory where
/// persistent memory storage is usually unwanted.
///
/// Recognized patterns:
/// - System temp dirs (`std::env::temp_dir()` and `/tmp` on Unix).
/// - Common scratch / cache patterns: paths containing `scratch`,
///   `.cache`, `.tmp`, `tempfile`, `cargo-target`.
/// - macOS `/private/var/folders/*` (system-managed temp).
///
/// Returns `false` for any non-UTF8 path or unrecognized location.
pub fn is_ephemeral_cwd(cwd: &Path) -> bool {
    let Some(s) = cwd.to_str() else {
        return false;
    };

    // System temp dir.
    let system_temp = std::env::temp_dir();
    if cwd.starts_with(&system_temp) {
        return true;
    }

    let lower = s.to_lowercase();

    // macOS /private/var/folders (the resolved form of $TMPDIR).
    if lower.starts_with("/private/var/folders/") {
        return true;
    }

    // /tmp on Unix.
    if lower.starts_with("/tmp/") || lower == "/tmp" {
        return true;
    }

    // Filename / extension patterns.
    let patterns = [
        "scratch",
        ".cache",
        ".tmp",
        "tempfile",
        "cargo-target",
        "/.npm/",
        "build-tmp",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_slug_hash_basic() {
        let id = workspace_slug_hash(Path::new("/Users/me/code/oxicode"));
        assert!(id.starts_with("oxicode-"), "got {id}");
        // Hash suffix is 8 hex chars.
        let hash = id.strip_prefix("oxicode-").unwrap();
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn workspace_slug_hash_deterministic() {
        // Same input → same output (sha256 is deterministic).
        let a = workspace_slug_hash(Path::new("/Users/me/code/oxicode"));
        let b = workspace_slug_hash(Path::new("/Users/me/code/oxicode"));
        assert_eq!(a, b);
    }

    #[test]
    fn workspace_slug_hash_different_paths_differ() {
        let a = workspace_slug_hash(Path::new("/Users/me/code/oxicode"));
        let b = workspace_slug_hash(Path::new("/Users/you/code/oxicode"));
        // Slug is the same ("oxicode") but hash differs.
        assert_ne!(a, b);
    }

    #[test]
    fn workspace_slug_hash_handles_empty_path() {
        let id = workspace_slug_hash(Path::new(""));
        assert!(id.starts_with("default-"), "got {id}");
    }

    #[test]
    fn workspace_slug_hash_sanitizes_special_chars() {
        let id = workspace_slug_hash(Path::new("/path/with spaces/oxicode"));
        assert!(id.starts_with("oxicode-"), "got {id}");
    }

    #[test]
    fn workspace_slug_hash_collapses_dashes() {
        // A basename like "my.project" becomes slug "my-project" not
        // "my--project" — single dash from the dot, no doubling.
        let id = workspace_slug_hash(Path::new("/tmp/my.project"));
        assert!(
            id.starts_with("my-project-") || id.starts_with("my-project"),
            "got {id}"
        );
        assert!(!id.contains("--"), "no double dashes in {id}");
    }

    #[test]
    fn is_ephemeral_cwd_tmp() {
        assert!(is_ephemeral_cwd(Path::new("/tmp/foo")));
        assert!(is_ephemeral_cwd(Path::new("/tmp")));
    }

    #[test]
    fn is_ephemeral_cwd_system_temp() {
        let tmp = std::env::temp_dir().join("oxicode-test");
        assert!(is_ephemeral_cwd(&tmp));
    }

    #[test]
    fn is_ephemeral_cwd_macos_private_var() {
        assert!(is_ephemeral_cwd(Path::new(
            "/private/var/folders/aa/xxxxxxxx/T/abc"
        )));
    }

    #[test]
    fn is_ephemeral_cwd_scratch_pattern() {
        assert!(is_ephemeral_cwd(Path::new("/home/me/scratch-pad")));
        assert!(is_ephemeral_cwd(Path::new("/home/me/project/.tmp")));
        assert!(is_ephemeral_cwd(Path::new(
            "/home/me/project/cargo-target/debug"
        )));
    }

    #[test]
    fn is_ephemeral_cwd_rejects_normal_paths() {
        assert!(!is_ephemeral_cwd(Path::new("/Users/me/code/oxicode")));
        assert!(!is_ephemeral_cwd(Path::new("/home/me/project")));
    }

    #[test]
    fn is_ephemeral_cwd_handles_non_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let non_utf8 = std::ffi::OsStr::from_bytes(b"/tmp/\xff/invalid");
        assert!(!is_ephemeral_cwd(Path::new(non_utf8)));
    }
}
