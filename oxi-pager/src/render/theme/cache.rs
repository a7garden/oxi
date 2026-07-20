//! In-memory theme cache + resolution.
//!
//! The pager reads the active `ThemeKind` on every render frame, so the
//! lookup must be cheaper than re-loading from `~/.grok/config.toml`.
//! [`current_kind`] returns the in-memory value, lazily seeding from the
//! shell's layered effective config on first call.
//!
//! Disk writes are NOT performed here — they live in
//! `xai_grok_shell::util::config::set_theme()` (and friends), invoked
//! via `Effect::PersistSetting` from the dispatcher. This module is a
//! pager-side in-memory cache + resolution layer only.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::ThemeKind;
use super::system_appearance;

/// In-memory theme kind, encoded as a `u8` matching the
/// `ThemeKind` discriminants. Loaded from disk once at startup via
/// `load_from_disk()`, then kept in sync by `set()`.
static CURRENT: AtomicU8 = AtomicU8::new(ThemeKind::GrokNight as u8);
static LOADED: AtomicBool = AtomicBool::new(false);
#[cfg(any(test, feature = "test-support"))]
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Whether auto-switching mode is active. Set when the config file
/// contains `theme = "auto"`. Checked by the event loop to decide
/// whether the `SystemAppearanceWatcher` should run.
///
/// Uses `AtomicBool` for thread-safe access from the watcher task.
static AUTO_MODE: AtomicBool = AtomicBool::new(false);

/// Whether the theme is locked to `Theme::terminal_default` for the whole
/// session (minimal mode — no theming).
static TERMINAL_NATIVE_LOCK: AtomicBool = AtomicBool::new(false);

/// Decode the u8 stored in `CURRENT` back to a `ThemeKind`. Falls
/// back to `GrokNight` if the byte is somehow out of range (which
/// can't happen via `set` — the discriminant is always a valid
/// variant — but defends against a future variant addition that
/// forgot to extend this match).
fn theme_kind_from_u8(byte: u8) -> ThemeKind {
    match byte {
        x if x == ThemeKind::GrokNight as u8 => ThemeKind::GrokNight,
        x if x == ThemeKind::GrokDay as u8 => ThemeKind::GrokDay,
        x if x == ThemeKind::TokyoNight as u8 => ThemeKind::TokyoNight,
        x if x == ThemeKind::RosePineMoon as u8 => ThemeKind::RosePineMoon,
        x if x == ThemeKind::OscuraMidnight as u8 => ThemeKind::OscuraMidnight,
        x if x == ThemeKind::Auto as u8 => ThemeKind::Auto,
        _ => ThemeKind::GrokNight,
    }
}

/// Cached auto-theme configuration (which themes map to dark/light).
///
/// Uses `Mutex<Option<_>>` rather than `OnceLock` so the cache can be
/// invalidated when the user changes mappings via the settings modal
/// or the `/theme auto` slash command.
static AUTO_THEME_CONFIG: Mutex<Option<AutoThemeConfig>> = Mutex::new(None);

/// Auto-theme config: which themes map to dark/light system appearance.
///
/// `dark_theme` and `light_theme` are the user-configured overrides read
/// from `[ui].auto_dark_theme` and `[ui].auto_light_theme` in `config.toml`.
/// When `None`, `to_theme_kind()` defaults to `GrokNight` / `GrokDay`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoThemeConfig {
    pub dark_theme: Option<ThemeKind>,
    pub light_theme: Option<ThemeKind>,
}

/// Get the current theme kind.
///
/// On the first call, reads from `~/.grok/config.toml` (via the shell's
/// `load_effective_config`). After that, returns the in-memory value
/// (updated by [`set`]).
pub fn current_kind() -> ThemeKind {
    // Locked: return a constant nominal kind without seeding from disk.
    if terminal_native_locked() {
        return ThemeKind::GrokNight;
    }
    if !LOADED.load(Ordering::Acquire) {
        // Two threads racing into the seed path is harmless — the
        // disk read is idempotent and `store` is atomic. Worst case
        // both threads call `load_from_disk` once.
        if let Some(kind) = load_from_disk() {
            CURRENT.store(kind as u8, Ordering::Relaxed);
        }
        LOADED.store(true, Ordering::Release);
    }
    theme_kind_from_u8(CURRENT.load(Ordering::Relaxed))
}

/// Set the in-memory theme kind without writing to disk.
///
/// Used by the dispatcher (after `Action::SetTheme` is processed) and
/// by the live-preview path during the picker. Disk-write happens via
/// `Effect::PersistSetting`, NOT here.
pub fn set(kind: ThemeKind) {
    CURRENT.store(kind as u8, Ordering::Relaxed);
    LOADED.store(true, Ordering::Release);
}

// -- Terminal-native lock (minimal mode) --------------------------------------

/// Whether the theme is locked to the terminal-native palette.
#[must_use]
pub fn terminal_native_locked() -> bool {
    TERMINAL_NATIVE_LOCK.load(Ordering::Relaxed)
}

/// Engage or clear the terminal-native theme lock.
pub fn set_terminal_native_lock(locked: bool) {
    TERMINAL_NATIVE_LOCK.store(locked, Ordering::Relaxed);
    // Cap quantization at ANSI-16 and switch syntax tokens to the dual-
    // polarity accent map (default-fg grays + base ANSI hues). Without the
    // polarity-safe remap, night-theme pastels collapse to White and vanish
    // on light terminal profiles in minimal mode.
    oxi_vendor_grok_markdown::set_color_level_cap(if locked {
        oxi_vendor_grok_markdown::ColorLevel::Basic
    } else {
        oxi_vendor_grok_markdown::ColorLevel::TrueColor
    });
    oxi_vendor_grok_markdown::set_polarity_safe_syntax(locked);
}

// -- Auto-mode ---------------------------------------------------------------

/// Whether auto-switching mode is active.
#[must_use]
pub fn is_auto_mode() -> bool {
    AUTO_MODE.load(Ordering::Relaxed)
}

/// Set or clear auto-switching mode.
pub fn set_auto_mode(enabled: bool) {
    AUTO_MODE.store(enabled, Ordering::Relaxed);
}

/// Get the cached auto-theme configuration, loading from config on first access.
///
/// The cache can be invalidated via [`invalidate_auto_theme_config`] so
/// subsequent lookups re-read from disk.
#[must_use]
pub fn auto_theme_config() -> AutoThemeConfig {
    let mut guard = AUTO_THEME_CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    *guard.get_or_insert_with(load_auto_theme_config)
}

/// Invalidate the cached auto-theme configuration.
///
/// Call after updating `auto_dark_theme` or `auto_light_theme` in config
/// so subsequent lookups see the new values. Used by the settings modal
/// and the `/theme auto` slash command.
pub fn invalidate_auto_theme_config() {
    *AUTO_THEME_CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// -- Theme resolution --------------------------------------------------------

/// Resolve the effective theme, respecting the full precedence chain.
///
/// Called once at startup. Returns the concrete `ThemeKind` (never `Auto`).
///
/// Precedence:
/// 1. Environment variable (`GROK_THEME`)
/// 2. Config file (`[ui].theme`)
/// 3. Default: `GrokNight`
#[must_use]
pub fn resolve_initial_theme() -> ThemeKind {
    // 1. Environment variable (for desktop app integration)

    // 2. Config file + 3. Default
    resolve_from_config(load_from_disk(), true)
}

/// Inner resolution logic, factored out for testability.
fn resolve_from_config(config_theme: Option<ThemeKind>, osc11_fallback: bool) -> ThemeKind {
    if let Some(kind) = config_theme {
        if kind.is_auto() {
            set_auto_mode(true);
            let appearance = if osc11_fallback {
                system_appearance::detect_with_osc11_fallback()
            } else {
                system_appearance::detect()
            };
            return resolve_from_appearance(appearance);
        }
        return kind;
    }

    // Default: GrokNight
    ThemeKind::GrokNight
}

/// Map an optional appearance detection result to a concrete `ThemeKind`.
fn resolve_from_appearance(appearance: Option<system_appearance::SystemAppearance>) -> ThemeKind {
    let config = auto_theme_config();
    appearance
        .map(|a| system_appearance::to_theme_kind(a, config.dark_theme, config.light_theme))
        .unwrap_or(ThemeKind::GrokNight)
}

/// Resolve "auto" by detecting system appearance and mapping via config.
///
/// Returns the concrete `ThemeKind` based on the current system appearance
/// and the user's dark/light theme mapping. Falls back to `GrokNight`
/// when detection fails.
///
/// Uses desktop APIs only (no OSC 11) — safe to call at runtime while
/// crossterm's `EventStream` is active. Called from the settings modal
/// and the `/theme auto` slash command.
#[must_use]
pub fn resolve_auto() -> ThemeKind {
    resolve_from_appearance(system_appearance::detect())
}

/// Variant of [`resolve_initial_theme`] without the OSC 11 startup
/// fallback, for resolution after the terminal is initialized.
#[must_use]
pub fn resolve_initial_theme_no_osc11() -> ThemeKind {
    resolve_from_config(load_from_disk(), false)
}

// -- Disk reads --------------------------------------------------------------
//
// All writes go through `xai_grok_shell::util::config::set_theme()` (and
// friends) via `Effect::PersistSetting`. This module only READS from the
// shell's layered effective config.

/// Read the theme from the effective config (managed_config.toml merged
/// under config.toml — user wins).
///
/// Checks `[ui].theme` first (the canonical location), then falls back
/// to a top-level `theme` key for backwards compatibility.
fn load_from_disk() -> Option<ThemeKind> {
    // OXI-CHANGE: was `xai_grok_config::load_effective_config_disk_only()`.
    // oxi owns its own settings file (~/.oxi/settings.toml); the oxi_tui::Theme
    // → grok theme bridge pushes the effective kind via `Theme::apply_kind()`.
    // Disk read here would be redundant — return None and let the caller fall
    // back to the in-memory current kind.
    None
}

/// Load auto-theme configuration from the effective config.
///
/// Reads `[ui].auto_dark_theme` and `[ui].auto_light_theme`, parsing them
/// as theme names. Filters out `Auto` to prevent circular reference.
fn load_auto_theme_config() -> AutoThemeConfig {
    // OXI-CHANGE: was `xai_grok_config::load_effective_config_disk_only()`.
    // See `load_from_disk` above — oxi doesn't read xai config files.
    AutoThemeConfig::default()
}

// -- Test support ------------------------------------------------------------

#[cfg(any(test, feature = "test-support"))]
pub fn reset_for_test() {
    // Tests are serialized via TEST_LOCK so the AtomicU8/AtomicBool
    // pair is safe to reset without any cross-thread coordination.
    CURRENT.store(ThemeKind::GrokNight as u8, Ordering::Relaxed);
    LOADED.store(false, Ordering::Release);
    AUTO_MODE.store(false, Ordering::Relaxed);
    set_terminal_native_lock(false);
    *AUTO_THEME_CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Seed `AUTO_THEME_CONFIG` with explicit defaults so `auto_theme_config()`
/// never falls through to `load_auto_theme_config()` (which reads the
/// user's real `config.toml`). Call from test setup after `reset_for_test()`.
#[cfg(any(test, feature = "test-support"))]
pub fn seed_auto_theme_defaults_for_test() {
    *AUTO_THEME_CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = Some(AutoThemeConfig::default());
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_lock() -> &'static Mutex<()> {
    &TEST_LOCK
}

/// Pin a deterministic theme + color level for a test's duration so exact
/// height / screen-position assertions are hermetic. Rendered heights are
/// computed under the process-global `Theme::current()` (which concurrent
/// `set_theme` tests mutate) and `Theme::current()` reads the global color
/// level; holding the shared test lock blocks a mid-test theme change. Hold the
/// returned guard for the whole test.
#[cfg(any(test, feature = "test-support"))]
pub fn pin_theme() -> std::sync::MutexGuard<'static, ()> {
    let guard = test_lock().lock().unwrap_or_else(|e| e.into_inner());
    set(ThemeKind::GrokNight);
    // Color level is a write-once `OnceLock`; tests run without a TTY so it
    // resolves to `TrueColor` anyway. Pin it explicitly (best-effort: ignore the
    // already-initialized `Err`) so the measure path that reads it stays fixed.
    let _ = super::color_support::set(super::color_support::ColorLevel::TrueColor);
    guard
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
