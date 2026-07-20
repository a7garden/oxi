//! OSC 11 terminal background detection.
//!
//! Queries the terminal's background color via the OSC 11 escape sequence:
//!   Query:  `\x1b]11;?\x07`
//!   Reply:  `\x1b]11;rgb:RRRR/GGGG/BBBB\x07`  (or ST terminator `\x1b\\`)
//!
//! The response contains hex color values (2-digit or 4-digit per channel).
//! For 4-digit values we extract the high byte; for 2-digit we use the value
//! directly.  Relative luminance (ITU-R BT.709) classifies the background as
//! dark or light.
//!
//! This is a **startup-only** fallback — it must NOT be called once
//! crossterm's `EventStream` is active, as both compete for stdin in raw
//! mode.  The live `SystemAppearanceWatcher` uses only
//! `dark-light::detect()`.

use super::system_appearance::SystemAppearance;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::io::RawFd;

/// Luminance threshold: backgrounds with Y < 0.5 are considered dark.
const LUMINANCE_THRESHOLD: f64 = 0.5;

/// Timeout for reading the OSC 11 response from the terminal.
const OSC11_TIMEOUT: Duration = Duration::from_millis(500);

/// Detect system appearance by querying the terminal's background color.
///
/// Returns `None` if stdin is not a TTY, the terminal does not respond
/// within `OSC11_TIMEOUT`, or the response cannot be parsed.
///
/// MUST be called before crossterm's event stream is initialized.
/// Manages stdin termios locally (no `crossterm::enable_raw_mode`) and
/// routes the query write through the shared stderr lock to avoid
/// interleaving with the render writer thread.
pub fn detect_via_osc11() -> Option<SystemAppearance> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        return None;
    }

    if !crate::render::grok::terminal::probe::write_query(b"\x1b]11;?\x07") {
        return None;
    }

    let response = read_osc_response(OSC11_TIMEOUT)?;
    let (r, g, b) = parse_osc11_rgb(&response)?;

    Some(classify_luminance(r, g, b))
}

/// Classify an sRGB color as dark or light based on relative luminance.
///
/// Uses ITU-R BT.709 luminance coefficients with sRGB gamma correction.
/// Threshold at 0.5 — below is dark, at or above is light.
pub(crate) fn classify_luminance(r: u8, g: u8, b: u8) -> SystemAppearance {
    let luminance =
        0.2126 * srgb_to_linear(r) + 0.7152 * srgb_to_linear(g) + 0.0722 * srgb_to_linear(b);

    if luminance < LUMINANCE_THRESHOLD {
        SystemAppearance::Dark
    } else {
        SystemAppearance::Light
    }
}

/// Parse the RGB components from an OSC 11 response string.
///
/// Handles both 4-digit (`rgb:RRRR/GGGG/BBBB`) and 2-digit (`rgb:RR/GG/BB`)
/// hex formats.  For 4-digit values the high byte is extracted (>> 8).
pub(crate) fn parse_osc11_rgb(response: &str) -> Option<(u8, u8, u8)> {
    let rgb_start = response.find("rgb:")? + 4;
    let rgb_part = &response[rgb_start..];

    // Split on channel separator `/` and terminators (BEL, ESC).
    let parts: Vec<&str> = rgb_part.split(['/', '\x07', '\x1b']).take(3).collect();

    if parts.len() < 3 {
        return None;
    }

    Some((
        parse_channel(parts[0])?,
        parse_channel(parts[1])?,
        parse_channel(parts[2])?,
    ))
}

/// Parse a single hex color channel.
///
/// For 3–4 digit values, extracts the high byte (`>> 8`) to map to 0–255.
/// For 1–2 digit values, uses the value directly as 0–255.
fn parse_channel(s: &str) -> Option<u8> {
    let trimmed = s.trim();
    let val = u16::from_str_radix(trimmed, 16).ok()?;
    Some(if trimmed.len() > 2 {
        (val >> 8) as u8
    } else {
        val as u8
    })
}

/// Convert an sRGB channel value (0–255) to linear light.
///
/// Applies the sRGB transfer function inverse (IEC 61966-2-1).
fn srgb_to_linear(c: u8) -> f64 {
    let s = c as f64 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Restores the original termios on drop without touching crossterm's
/// process-wide `TERMINAL_MODE_PRIOR_RAW_MODE`. Calling
/// `crossterm::disable_raw_mode` here would restore the shell's
/// pre-pager cooked termios, breaking the pager's own raw mode.
#[cfg(unix)]
struct TermiosGuard {
    fd: RawFd,
    original: libc::termios,
}

#[cfg(unix)]
impl Drop for TermiosGuard {
    fn drop(&mut self) {
        // SAFETY: fd was valid at construction; original was populated
        // by a successful tcgetattr.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}

/// POSIX-portable subset of `cfmakeraw(3)`: clear the lflags that would
/// block a single-byte read (canonical mode, echo, signal interpretation,
/// extended processing).
#[cfg(unix)]
fn make_raw_termios(snapshot: &libc::termios) -> libc::termios {
    let mut raw = *snapshot;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
    raw
}

#[cfg(unix)]
fn read_osc_response(timeout: Duration) -> Option<String> {
    use std::os::unix::io::AsRawFd;
    read_osc_response_with_fd(std::io::stdin().as_raw_fd(), timeout)
}

#[cfg(not(unix))]
fn read_osc_response(_timeout: Duration) -> Option<String> {
    None
}

/// `fd`-parameterized for tests (pass `/dev/null` to exercise the
/// non-TTY path). Guard is constructed before `tcsetattr` to keep the
/// restore atomic with the switch -- POSIX guarantees `tcsetattr` is
/// atomic on failure, so a redundant restore on the early-return path
/// is harmless.
#[cfg(unix)]
fn read_osc_response_with_fd(fd: RawFd, timeout: Duration) -> Option<String> {
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: caller passes a valid fd; original is a valid owned buffer.
    if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
        return None;
    }
    let raw = make_raw_termios(&original);
    let _guard = TermiosGuard { fd, original };
    // SAFETY: raw is a valid owned buffer.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }
    read_with_timeout(timeout)
}

/// Read bytes from stdin until a terminator is found or timeout expires.
///
/// Recognizes two terminators:
/// - BEL (`\x07`)
/// - ST  (`\x1b\x5c`, i.e. ESC + backslash)
///
/// Uses `libc::poll` + `libc::read` for non-blocking reads with a timeout
/// on Unix.  Returns `None` on non-Unix platforms.
// Only invoked from `read_osc_response_with_fd`, which is Unix-only.
#[cfg(unix)]
fn read_with_timeout(timeout: Duration) -> Option<String> {
    unix_read_with_timeout(timeout)
}

/// Unix implementation: shared probe read loop with the OSC terminators
/// (BEL, or ST as `ESC \`) as the stop predicate.
#[cfg(unix)]
fn unix_read_with_timeout(timeout: Duration) -> Option<String> {
    let buf = crate::render::grok::terminal::probe::read_tty_reply(timeout, |buf, byte| {
        byte == 0x07 || (buf.len() >= 2 && buf[buf.len() - 2] == 0x1b && byte == 0x5c)
    })?;
    // Reject partial buffers: a reply truncated mid-channel would
    // mis-parse, since channel width is inferred from digit count.
    if !ends_with_osc_terminator(&buf) {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// True when the buffer ends with BEL or ST (`ESC \`).
#[cfg(any(unix, test))]
fn ends_with_osc_terminator(buf: &[u8]) -> bool {
    buf.last() == Some(&0x07) || buf.ends_with(b"\x1b\\")
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
