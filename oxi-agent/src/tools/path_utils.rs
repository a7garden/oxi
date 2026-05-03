//! Path resolution utilities
//!
//! Ported from pi-mono coding-agent `path-utils.ts`.
//! Provides path normalization, home expansion, and macOS-specific path handling.

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Expand a path that may start with `~` to the full home directory path.
/// Also strips a leading `@` prefix (used by some tool interfaces).
pub fn expand_path(path: &str) -> PathBuf {
    let normalized = normalize_at_prefix(path);
    let normalized = normalize_unicode_spaces(&normalized);

    if normalized == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }

    if let Some(rest) = normalized.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }

    PathBuf::from(normalized)
}

/// Resolve a path relative to the given working directory.
/// Handles `~` expansion and absolute paths.
pub fn resolve_to_cwd(path: &str, cwd: &Path) -> PathBuf {
    let expanded = expand_path(path);
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

/// Resolve a path for reading, trying macOS-specific variants if the initial
/// path doesn't exist.
///
/// macOS may store filenames in:
/// - NFD (decomposed) Unicode form
/// - With narrow no-break spaces before AM/PM in screenshots
/// - With curly quotes (U+2019) instead of straight apostrophes
pub fn resolve_read_path(path: &str, cwd: &Path) -> PathBuf {
    let resolved = resolve_to_cwd(path, cwd);

    if resolved.exists() {
        return resolved;
    }

    // Try macOS AM/PM variant (narrow no-break space before AM/PM)
    let am_pm_variant = try_macos_screenshot_path(&resolved);
    if am_pm_variant != resolved && am_pm_variant.exists() {
        return am_pm_variant;
    }

    // Try NFD variant (macOS stores filenames in NFD form)
    let nfd_variant = try_nfd_variant(&resolved);
    if nfd_variant != resolved && nfd_variant.exists() {
        return nfd_variant;
    }

    // Try curly quote variant (macOS uses U+2019 in screenshot names)
    let curly_variant = try_curly_quote_variant(&resolved);
    if curly_variant != resolved && curly_variant.exists() {
        return curly_variant;
    }

    // Try combined NFD + curly quote
    let nfd_curly_variant = try_curly_quote_variant(&nfd_variant);
    if nfd_curly_variant != resolved && nfd_curly_variant.exists() {
        return nfd_curly_variant;
    }

    resolved
}

/// Strip leading `@` prefix from a path string.
fn normalize_at_prefix(path: &str) -> &str {
    path.strip_prefix('@').unwrap_or(path)
}

/// Normalize Unicode spaces (non-breaking spaces, etc.) to regular spaces.
fn normalize_unicode_spaces(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        result.push(if is_unicode_space(ch) { ' ' } else { ch });
    }
    result
}

/// Check if a character is a Unicode space that should be normalized.
fn is_unicode_space(ch: char) -> bool {
    matches!(
        ch,
        '\u{00A0}'      // No-Break Space (NBSP)
        | '\u{2000}'..='\u{200A}' // Various spaces (En Quad through Hair Space)
        | '\u{202F}'     // Narrow No-Break Space
        | '\u{205F}'     // Medium Mathematical Space
        | '\u{3000}'     // Ideographic Space
    )
}

/// Try replacing regular space + AM/PM with narrow no-break space + AM/PM
/// (macOS screenshot naming convention).
fn try_macos_screenshot_path(path: &PathBuf) -> PathBuf {
    let path_str = path.to_string_lossy();
    let replaced = path_str.replace(" AM.", "\u{202F}AM.").replace(" PM.", "\u{202F}PM.");
    let replaced = replaced
        .replace(" am.", "\u{202F}AM.")
        .replace(" pm.", "\u{202F}PM.");
    PathBuf::from(replaced)
}

/// Try NFD (decomposed) Unicode form of the path.
/// macOS stores filenames in NFD form, but users may provide NFC input.
fn try_nfd_variant(path: &PathBuf) -> PathBuf {
    let path_str = path.to_string_lossy();
    let nfd = path_str.nfd().collect::<String>();
    PathBuf::from(nfd)
}

/// Try replacing straight apostrophes with curly quotes (U+2019).
/// macOS uses U+2019 in screenshot names like "Capture d'écran".
fn try_curly_quote_variant(path: &PathBuf) -> PathBuf {
    let path_str = path.to_string_lossy();
    let replaced = path_str.replace('\'', "\u{2019}");
    PathBuf::from(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_path_home() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_path("~/foo.txt");
        assert_eq!(expanded, home.join("foo.txt"));
    }

    #[test]
    fn test_expand_path_home_only() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_path("~");
        assert_eq!(expanded, home);
    }

    #[test]
    fn test_expand_path_absolute() {
        let expanded = expand_path("/tmp/foo.txt");
        assert_eq!(expanded, PathBuf::from("/tmp/foo.txt"));
    }

    #[test]
    fn test_expand_path_relative() {
        let expanded = expand_path("foo.txt");
        assert_eq!(expanded, PathBuf::from("foo.txt"));
    }

    #[test]
    fn test_expand_path_at_prefix() {
        let expanded = expand_path("@/tmp/foo.txt");
        assert_eq!(expanded, PathBuf::from("/tmp/foo.txt"));
    }

    #[test]
    fn test_expand_path_unicode_spaces() {
        // Non-breaking space (U+00A0) should be normalized
        let expanded = expand_path("hello\u{00A0}world");
        assert_eq!(expanded, PathBuf::from("hello world"));
    }

    #[test]
    fn test_resolve_to_cwd_absolute() {
        let cwd = Path::new("/home/user/project");
        let resolved = resolve_to_cwd("/tmp/foo.txt", cwd);
        assert_eq!(resolved, PathBuf::from("/tmp/foo.txt"));
    }

    #[test]
    fn test_resolve_to_cwd_relative() {
        let cwd = Path::new("/home/user/project");
        let resolved = resolve_to_cwd("src/main.rs", cwd);
        assert_eq!(resolved, PathBuf::from("/home/user/project/src/main.rs"));
    }

    #[test]
    fn test_resolve_to_cwd_home() {
        let home = dirs::home_dir().unwrap();
        let cwd = Path::new("/home/user/project");
        let resolved = resolve_to_cwd("~/foo.txt", cwd);
        assert_eq!(resolved, home.join("foo.txt"));
    }

    #[test]
    fn test_resolve_read_path_existing() {
        // The current directory should always exist
        let cwd = std::env::current_dir().unwrap();
        let resolved = resolve_read_path(".", &cwd);
        assert!(resolved.exists());
    }

    #[test]
    fn test_resolve_read_path_nonexistent() {
        let cwd = Path::new("/tmp");
        let resolved = resolve_read_path("nonexistent_file_xyz.txt", cwd);
        assert!(!resolved.exists());
        assert_eq!(resolved, PathBuf::from("/tmp/nonexistent_file_xyz.txt"));
    }

    #[test]
    fn test_normalize_unicode_spaces() {
        assert_eq!(normalize_unicode_spaces("hello\u{00A0}world"), "hello world");
        assert_eq!(normalize_unicode_spaces("hello\u{202F}world"), "hello world");
        assert_eq!(normalize_unicode_spaces("hello world"), "hello world");
    }

    #[test]
    fn test_is_unicode_space() {
        assert!(is_unicode_space('\u{00A0}'));
        assert!(is_unicode_space('\u{202F}'));
        assert!(is_unicode_space('\u{3000}'));
        assert!(!is_unicode_space(' '));
        assert!(!is_unicode_space('a'));
    }
}
