//! Host platform and display server classification.

use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::OnceLock;

mod display_refresh;

pub use display_refresh::{DisplayRefreshProbeResult, DisplayRefreshSource, probe_display_refresh};

/// Process env as UTF-8. Skips non-Unicode entries (`vars()` panics on those).
pub fn collect_unicode_env() -> HashMap<String, String> {
    unicode_env_from_os(std::env::vars_os())
}

/// Pure helper: drop OsString pairs that are not valid Unicode.
pub fn unicode_env_from_os(
    iter: impl IntoIterator<Item = (OsString, OsString)>,
) -> HashMap<String, String> {
    iter.into_iter()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum HostOs {
    Macos,
    Linux,
    Windows,
    #[default]
    Other,
}

impl HostOs {
    /// Call on demand since cfg is compile-time constant.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// OXI-CHANGE: upstream re-exported `xai_tty_utils::is_wsl`. We inline a
/// minimal WSL detector (avoids vendoring the full xai-tty-utils crate).
pub fn is_wsl() -> bool {
    // WSL sets WSL_DISTRO_NAME and/or WSL_INTEROP; /proc/sys/kernel/osrelease
    // contains "microsoft" on WSL1 and "microsoft-standard" on WSL2.
    if std::env::var_os("WSL_DISTRO_NAME").is_some() || std::env::var_os("WSL_INTEROP").is_some() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
            let l = s.to_lowercase();
            return l.contains("microsoft");
        }
    }
    false
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum DisplayServer {
    Quartz,
    Wayland,
    X11,
    Win32,
    #[default]
    Unknown,
}

impl DisplayServer {
    /// Detect the display server. Cached for process lifetime on Linux
    /// (env vars don't change); compile-time constant on macOS/Windows.
    pub fn current() -> Self {
        static CACHE: OnceLock<DisplayServer> = OnceLock::new();
        *CACHE.get_or_init(|| {
            let env = collect_unicode_env();
            Self::detect_from_env(&env)
        })
    }

    /// Pure helper so tests can drive env directly.
    fn detect_from_env(env: &HashMap<String, String>) -> Self {
        match HostOs::current() {
            HostOs::Macos => Self::Quartz,
            HostOs::Windows => Self::Win32,
            HostOs::Linux => {
                if env.get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
                    Self::Wayland
                } else if env.get("DISPLAY").is_some_and(|v| !v.is_empty()) {
                    Self::X11
                } else {
                    Self::Unknown
                }
            }
            HostOs::Other => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod unicode_env_tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn unicode_env_from_os_skips_non_unicode_key_or_value() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let bad = OsString::from_vec(vec![0xff, 0xfe]);
            let map = unicode_env_from_os([
                (bad.clone(), OsString::from("ok")),
                (OsString::from("OK_KEY"), bad),
                (OsString::from("GOOD"), OsString::from("yes")),
            ]);
            assert_eq!(map, HashMap::from([("GOOD".into(), "yes".into())]));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let bad = OsString::from_wide(&[0xD800]); // lone surrogate
            let map = unicode_env_from_os([
                (bad.clone(), OsString::from("ok")),
                (OsString::from("OK_KEY"), bad),
                (OsString::from("GOOD"), OsString::from("yes")),
            ]);
            assert_eq!(map, HashMap::from([("GOOD".into(), "yes".into())]));
        }
    }
}

// All remaining tests here are Linux-only DisplayServer tests; WSL detection
// tests live with the implementation in `xai-tty-utils`.
// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
