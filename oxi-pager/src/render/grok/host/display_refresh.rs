//! One-shot primary-display refresh probe (OnceLock-cached).
//! Fail-closed: never panics into callers; no TTY IO; no display mode mutation.

use std::sync::OnceLock;
use std::time::Instant;

use super::{DisplayServer, HostOs, is_wsl};

/// Sane bounds; outside → fail closed.
const MIN_HZ: u32 = 30;
const MAX_HZ: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum DisplayRefreshSource {
    None,
    MacosCoreGraphics,
    WindowsEnumDisplaySettings,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRefreshProbeResult {
    pub hz: Option<u32>,
    pub source: DisplayRefreshSource,
    /// Empty when ok; else a stable skip/error token.
    pub skip_reason: &'static str,
    pub duration_ms: u64,
}

impl DisplayRefreshProbeResult {
    /// `ok` | `skipped` | `error`
    pub fn outcome(self) -> &'static str {
        if self.hz.is_some() {
            "ok"
        } else if self.skip_reason == "error" {
            "error"
        } else {
            "skipped"
        }
    }
}

/// Once per process. Infallible; never panics.
pub fn probe_display_refresh() -> DisplayRefreshProbeResult {
    static CACHE: OnceLock<DisplayRefreshProbeResult> = OnceLock::new();
    *CACHE.get_or_init(probe_uncached)
}

fn probe_uncached() -> DisplayRefreshProbeResult {
    let start = Instant::now();
    let (hz, source, skip_reason) = probe_inner();
    DisplayRefreshProbeResult {
        hz,
        source,
        skip_reason,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

fn probe_inner() -> (Option<u32>, DisplayRefreshSource, &'static str) {
    let is_ssh = std::env::var_os("SSH_CONNECTION").is_some() // OXI-CHANGE: was xai_grok_shared::clipboard::is_remote_session
        || std::env::var_os("SSH_CLIENT").is_some()
        || std::env::var_os("SSH_TTY").is_some();
    let wsl = is_wsl();
    let os = HostOs::current();
    let display = DisplayServer::current();

    // Avoid FFI when env already forces a skip.
    let platform_hz = if precheck_skip(is_ssh, wsl).is_some() {
        None
    } else {
        match os {
            HostOs::Macos => Some(probe_macos()),
            HostOs::Windows => Some(probe_windows()),
            HostOs::Linux | HostOs::Other => None,
        }
    };

    decide(is_ssh, wsl, os, display, platform_hz)
}

/// Pure matrix used by production and tests; inject only the platform result.
fn decide(
    is_ssh: bool,
    is_wsl: bool,
    os: HostOs,
    display: DisplayServer,
    platform_hz: Option<Result<u32, &'static str>>,
) -> (Option<u32>, DisplayRefreshSource, &'static str) {
    if let Some(reason) = precheck_skip(is_ssh, is_wsl) {
        return (None, DisplayRefreshSource::None, reason);
    }
    match os {
        HostOs::Macos => {
            let source = DisplayRefreshSource::MacosCoreGraphics;
            match platform_hz.unwrap_or(Err("error")) {
                Ok(hz) => accept_hz(hz, source),
                Err(reason) => (None, source, reason),
            }
        }
        HostOs::Windows => {
            let source = DisplayRefreshSource::WindowsEnumDisplaySettings;
            match platform_hz.unwrap_or(Err("error")) {
                Ok(hz) => accept_hz(hz, source),
                Err(reason) => (None, source, reason),
            }
        }
        HostOs::Linux => {
            let reason = linux_skip_reason(display);
            (None, DisplayRefreshSource::Linux, reason)
        }
        HostOs::Other => (None, DisplayRefreshSource::None, "unsupported"),
    }
}

fn precheck_skip(is_ssh: bool, is_wsl: bool) -> Option<&'static str> {
    if is_ssh {
        return Some("ssh");
    }
    if is_wsl {
        return Some("wsl");
    }
    None
}

fn linux_skip_reason(display: DisplayServer) -> &'static str {
    match display {
        DisplayServer::Wayland => "wayland_unsupported",
        DisplayServer::X11 => "x11_unsupported",
        _ => "no_display",
    }
}

fn accept_hz(
    hz: u32,
    source: DisplayRefreshSource,
) -> (Option<u32>, DisplayRefreshSource, &'static str) {
    if !(MIN_HZ..=MAX_HZ).contains(&hz) {
        return (None, source, "out_of_range");
    }
    (Some(hz), source, "")
}

#[cfg(target_os = "macos")]
fn probe_macos() -> Result<u32, &'static str> {
    // Fail-closed if FFI panics (abort builds still abort).
    match std::panic::catch_unwind(|| {
        // SAFETY: read-only CoreGraphics display query; no mode mutation.
        unsafe { macos_main_display_refresh_hz() }
    }) {
        Ok(inner) => inner,
        Err(_) => Err("error"),
    }
}

#[cfg(not(target_os = "macos"))]
fn probe_macos() -> Result<u32, &'static str> {
    Err("unsupported")
}

#[cfg(target_os = "macos")]
unsafe fn macos_main_display_refresh_hz() -> Result<u32, &'static str> {
    type CgDisplayModeRef = *mut core::ffi::c_void;
    type CgDirectDisplayId = u32;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> CgDirectDisplayId;
        fn CGDisplayCopyDisplayMode(display: CgDirectDisplayId) -> CgDisplayModeRef;
        fn CGDisplayModeGetRefreshRate(mode: CgDisplayModeRef) -> f64;
        fn CGDisplayModeRelease(mode: CgDisplayModeRef);
    }

    // SAFETY: stable public CG APIs; null mode handled; Release pairs with Copy.
    let display = unsafe { CGMainDisplayID() };
    let mode = unsafe { CGDisplayCopyDisplayMode(display) };
    if mode.is_null() {
        return Err("error");
    }
    let rate = unsafe { CGDisplayModeGetRefreshRate(mode) };
    unsafe { CGDisplayModeRelease(mode) };
    // 0.0 is documented indeterminate for some LCD/VRR panels — skip, not error.
    // Future primary-display fallback must be thread-safe; no AppKit/NSScreen here.
    if !rate.is_finite() || rate < 0.0 {
        return Err("error");
    }
    if rate == 0.0 {
        return Err("indeterminate");
    }
    Ok(rate.round() as u32)
}

#[cfg(target_os = "windows")]
fn probe_windows() -> Result<u32, &'static str> {
    match std::panic::catch_unwind(|| {
        // SAFETY: read-only EnumDisplayDevices/Settings for primary only.
        unsafe { windows_primary_display_refresh_hz() }
    }) {
        Ok(inner) => inner,
        Err(_) => Err("error"),
    }
}

#[cfg(not(target_os = "windows"))]
fn probe_windows() -> Result<u32, &'static str> {
    Err("unsupported")
}

/// Primary monitor Hz (matches macOS `CGMainDisplayID`). Null device name to
/// `EnumDisplaySettingsW` is the *current* adapter, which can differ from the
/// primary on multi-monitor machines.
#[cfg(target_os = "windows")]
unsafe fn windows_primary_display_refresh_hz() -> Result<u32, &'static str> {
    use windows_sys::Win32::Graphics::Gdi::{
        DEVMODEW, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS,
        EnumDisplayDevicesW, EnumDisplaySettingsW,
    };

    // Bound device enumeration so a broken driver cannot spin forever.
    for i in 0u32..32 {
        // SAFETY: zeroed DISPLAY_DEVICEW with cb set is the documented pattern.
        let mut device: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        device.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        // SAFETY: null parent = desktop adapters; i indexes adapters.
        let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut device, 0) };
        if ok == 0 {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE == 0 {
            continue;
        }

        // SAFETY: zeroed DEVMODEW with dmSize set; DeviceName is the primary.
        let mut devmode: DEVMODEW = unsafe { std::mem::zeroed() };
        devmode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        let ok = unsafe {
            EnumDisplaySettingsW(
                device.DeviceName.as_ptr(),
                ENUM_CURRENT_SETTINGS,
                &mut devmode,
            )
        };
        if ok == 0 {
            return Err("error");
        }
        let hz = devmode.dmDisplayFrequency;
        // 0/1 often mean "default hardware rate" — fail closed.
        if hz < 2 {
            return Err("error");
        }
        return Ok(hz);
    }
    Err("error")
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
