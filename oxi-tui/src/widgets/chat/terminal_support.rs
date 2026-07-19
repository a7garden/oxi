//! Terminal capability detection and EPT (events-per-tick) overrides.
//!
//! Centralizes the env-variable lookup pattern for mouse scroll normalization.
//! Values are read lazily on each call so tests can set/unset env vars
//! without rebuilding the normalizer.
//!
//! ## Override precedence
//!
//! 1. `OXI_SCROLL_EPT=1..10` — explicit EPT override for all terminals.
//! 2. Built-in table by `TerminalKind`.
//!
//! If the env var is unset, invalid, or out of range, falls back to the
//! built-in table silently.

use std::time::Duration;

use crate::widgets::chat::mouse::TerminalKind;

/// Default gap between events to consider them part of the same stream.
pub const DEFAULT_FLUSH_GAP: Duration = Duration::from_millis(80);

/// EPT override range (inclusive).
const EPT_MIN: u8 = 1;
const EPT_MAX: u8 = 10;

/// Read the `OXI_SCROLL_EPT` env var and return the override if valid.
/// Returns None if unset, unparseable, or out of [EPT_MIN, EPT_MAX].
pub fn ept_override() -> Option<u8> {
    let s = std::env::var("OXI_SCROLL_EPT").ok()?;
    let n: u8 = s.parse().ok()?;
    if (EPT_MIN..=EPT_MAX).contains(&n) {
        Some(n)
    } else {
        None
    }
}

/// Compute the effective EPT for a terminal kind, honoring overrides.
///
/// Precedence:
/// 1. `OXI_SCROLL_EPT` env override (if valid)
/// 2. Built-in table for `kind`
pub fn effective_ept(kind: TerminalKind) -> u8 {
    ept_override().unwrap_or_else(|| built_in_ept(kind))
}

/// Built-in EPT table — see `TerminalKind::ept()` docs for sources.
///
/// `AppleTerminal` and `Ghostty` emit 3 events per physical wheel notch
/// in SGR mode. All other terminals (including multiplexers) emit 1.
pub fn built_in_ept(kind: TerminalKind) -> u8 {
    match kind {
        TerminalKind::AppleTerminal | TerminalKind::Ghostty => 3,
        TerminalKind::ITerm2
        | TerminalKind::VSCode
        | TerminalKind::Multiplexer
        | TerminalKind::Unknown => 1,
    }
}

/// Read the `OXI_SCROLL_FLUSH_MS` env var for stream-flush gap.
/// Returns None if unset or unparseable.
pub fn flush_gap_override() -> Option<Duration> {
    let s = std::env::var("OXI_SCROLL_FLUSH_MS").ok()?;
    let ms: u64 = s.parse().ok()?;
    if ms == 0 || ms > 5000 {
        // 0 = disabled (impossible for a duration); > 5s = nonsense.
        None
    } else {
        Some(Duration::from_millis(ms))
    }
}

/// Effective flush gap: env override or DEFAULT_FLUSH_GAP.
pub fn effective_flush_gap() -> Duration {
    flush_gap_override().unwrap_or(DEFAULT_FLUSH_GAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env access requires unsafe in Rust 2024. Tests use static mutex to serialize.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env_lock<R>(f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        f()
    }

    #[test]
    fn ept_override_unset_returns_none() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
            assert_eq!(ept_override(), None);
        });
    }

    #[test]
    fn ept_override_valid_values() {
        with_env_lock(|| {
            for v in 1..=10u8 {
                unsafe {
                    std::env::set_var("OXI_SCROLL_EPT", v.to_string());
                }
                assert_eq!(ept_override(), Some(v));
            }
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
        });
    }

    #[test]
    fn ept_override_out_of_range_returns_none() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "0");
            }
            assert_eq!(ept_override(), None);
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "11");
            }
            assert_eq!(ept_override(), None);
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "100");
            }
            assert_eq!(ept_override(), None);
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
        });
    }

    #[test]
    fn ept_override_non_numeric_returns_none() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "abc");
            }
            assert_eq!(ept_override(), None);
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
        });
    }

    #[test]
    fn effective_ept_uses_override_when_present() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "7");
            }
            // Even AppleTerminal (built-in 3) uses the override.
            assert_eq!(effective_ept(TerminalKind::AppleTerminal), 7);
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
        });
    }

    #[test]
    fn effective_ept_falls_back_to_table() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
            assert_eq!(effective_ept(TerminalKind::AppleTerminal), 3);
            assert_eq!(effective_ept(TerminalKind::Ghostty), 3);
            assert_eq!(effective_ept(TerminalKind::ITerm2), 1);
            assert_eq!(effective_ept(TerminalKind::VSCode), 1);
            assert_eq!(effective_ept(TerminalKind::Multiplexer), 1);
            assert_eq!(effective_ept(TerminalKind::Unknown), 1);
        });
    }

    #[test]
    fn built_in_ept_matches_terminal_kind_table() {
        assert_eq!(built_in_ept(TerminalKind::AppleTerminal), 3);
        assert_eq!(built_in_ept(TerminalKind::Ghostty), 3);
        assert_eq!(built_in_ept(TerminalKind::ITerm2), 1);
        assert_eq!(built_in_ept(TerminalKind::VSCode), 1);
        assert_eq!(built_in_ept(TerminalKind::Multiplexer), 1);
        assert_eq!(built_in_ept(TerminalKind::Unknown), 1);
    }

    #[test]
    fn flush_gap_override_valid() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_FLUSH_MS", "120");
            }
            assert_eq!(flush_gap_override(), Some(Duration::from_millis(120)));
            unsafe {
                std::env::remove_var("OXI_SCROLL_FLUSH_MS");
            }
        });
    }

    #[test]
    fn flush_gap_override_zero_or_huge_returns_none() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_FLUSH_MS", "0");
            }
            assert_eq!(flush_gap_override(), None);
            unsafe {
                std::env::set_var("OXI_SCROLL_FLUSH_MS", "60000"); // 60s
            }
            assert_eq!(flush_gap_override(), None);
            unsafe {
                std::env::remove_var("OXI_SCROLL_FLUSH_MS");
            }
        });
    }

    #[test]
    fn effective_flush_gap_default_is_80ms() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("OXI_SCROLL_FLUSH_MS");
            }
            assert_eq!(effective_flush_gap(), Duration::from_millis(80));
        });
    }

    #[test]
    fn effective_flush_gap_honors_override() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_FLUSH_MS", "200");
            }
            assert_eq!(effective_flush_gap(), Duration::from_millis(200));
            unsafe {
                std::env::remove_var("OXI_SCROLL_FLUSH_MS");
            }
        });
    }
}
