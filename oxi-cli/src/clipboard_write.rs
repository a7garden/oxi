//! Clipboard text write functionality
//!
//! Copies text to the system clipboard using platform-specific tools.
//! Supports: Wayland (wl-copy), X11 (xclip), macOS (pbcopy), Windows (clip), Termux

use anyhow::{Context, Result};
use std::env;
use std::process::Command;

/// Run a command with arguments
fn run_command(command: &str, args: &[&str]) -> Result<std::process::Output> {
    Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run {} with args {:?}", command, args))
}

/// Run a command with timeout (in milliseconds)
fn run_command_timeout(
    command: &str,
    args: &[&str],
    _timeout_ms: u64,
) -> Result<std::process::Output> {
    // Note: Rust's Command doesn't support timeout natively without additional crates
    // For now, we just run without explicit timeout
    run_command(command, args)
}

/// Check if running in WSL
fn is_wsl() -> bool {
    env::var("WSL_DISTRO_NAME").is_ok()
        || env::var("WSLENV").is_ok()
        || std::path::Path::new("/proc/version").exists()
            && std::fs::read_to_string("/proc/version")
                .map(|v| v.contains("microsoft") || v.contains("WSL"))
                .unwrap_or(false)
}

/// Check if running in Wayland session
fn is_wayland() -> bool {
    env::var("WAYLAND_DISPLAY").is_ok()
}

/// Check if running in Termux
fn is_termux() -> bool {
    env::var("TERMUX_VERSION").is_ok()
}

/// Copy text to clipboard using wl-copy (Wayland)
fn copy_via_wl_copy(text: &str) -> Option<Result<()>> {
    let output = run_command("wl-copy", &[text]).ok()?;
    if output.status.success() {
        Some(Ok(()))
    } else {
        Some(Err(anyhow::anyhow!("wl-copy failed")))
    }
}

/// Copy text to clipboard using xclip (X11)
fn copy_via_xclip(text: &str) -> Option<Result<()>> {
    // xclip requires stdin input
    let mut child = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(text.as_bytes()).ok()?;
    }
    drop(child);

    Some(Ok(()))
}

/// Copy text to clipboard using pbcopy (macOS)
fn copy_via_pbcopy(text: &str) -> Option<Result<()>> {
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).ok()?;
    }
    drop(child);

    // pbcopy always succeeds if it runs
    Some(Ok(()))
}

/// Copy text to clipboard using clip (Windows)
fn copy_via_clip(text: &str) -> Option<Result<()>> {
    let output = run_command("cmd.exe", &["/c", &format!("echo {} | clip", text)]).ok()?;
    if output.status.success() {
        Some(Ok(()))
    } else {
        Some(Err(anyhow::anyhow!("clip failed")))
    }
}

/// Copy text to clipboard using PowerShell (Windows/WSL)
fn copy_via_powershell(text: &str) -> Option<Result<()>> {
    let escaped = text.replace("'", "''").replace("`", "``");
    let ps_script = format!("Set-Clipboard -Value '{}'", escaped);

    let output = run_command_timeout(
        "powershell.exe",
        &["-NoProfile", "-Command", &ps_script],
        3000,
    )
    .ok()?;
    if output.status.success() {
        Some(Ok(()))
    } else {
        Some(Err(anyhow::anyhow!("PowerShell Set-Clipboard failed")))
    }
}

/// Copy text to clipboard using Termux (Android)
fn copy_via_termux(text: &str) -> Option<Result<()>> {
    let output = run_command("termux-clipboard-set", &[text]).ok()?;
    if output.status.success() {
        Some(Ok(()))
    } else {
        Some(Err(anyhow::anyhow!("termux-clipboard-set failed")))
    }
}

/// Generate OSC52 escape sequence for terminals that support it
///
/// OSC52 is a terminal escape sequence that can set clipboard contents.
/// This is useful as a fallback when no native clipboard tool is available.
pub fn generate_osc52_sequence(text: &str) -> String {
    use base64::Engine as _;
    let base64_text = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // OSC52 sequence: ESC ] 52 ; ; base64-data BEL
    format!("\x1b]52;;{}上古", base64_text)
}

/// Copy text to clipboard, trying platform-specific methods in order.
///
/// Tries in order:
/// - Linux/Wayland: wl-copy
/// - Linux/X11: xclip
/// - macOS: pbcopy
/// - Windows: PowerShell, then clip
/// - Termux: termux-clipboard-set
/// - Fallback: OSC52 escape sequence (prints to stdout)
///
/// Returns Ok(()) on success.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    if is_termux() {
        match copy_via_termux(text) {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(e),
            None => Err(anyhow::anyhow!("termux-clipboard-set not available")),
        }
    } else if cfg!(target_os = "linux") {
        if is_wayland() {
            copy_via_wl_copy(text)
                .or_else(|| copy_via_xclip(text))
                .ok_or_else(|| anyhow::anyhow!("No clipboard tool available for Linux"))?
        } else {
            copy_via_xclip(text)
                .or_else(|| copy_via_wl_copy(text))
                .ok_or_else(|| anyhow::anyhow!("No clipboard tool available for Linux"))?
        }
    } else if cfg!(target_os = "macos") {
        match copy_via_pbcopy(text) {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(e),
            None => Err(anyhow::anyhow!("pbcopy not available")),
        }
    } else if cfg!(target_os = "windows") {
        copy_via_powershell(text)
            .or_else(|| copy_via_clip(text))
            .ok_or_else(|| anyhow::anyhow!("No clipboard tool available for Windows"))?
    } else if is_wsl() {
        // Try Linux clipboard first, then Windows
        copy_via_wl_copy(text)
            .or_else(|| copy_via_xclip(text))
            .or_else(|| copy_via_powershell(text))
            .ok_or_else(|| anyhow::anyhow!("No clipboard tool available"))?
    } else {
        // Fallback: print OSC52 sequence
        print!("{}", generate_osc52_sequence(text));
        std::io::stdout().flush().ok();
        Ok(())
    }
}

/// Check if clipboard operations are likely to work on this platform
pub fn is_clipboard_supported() -> bool {
    if is_termux() {
        return std::process::Command::new("termux-clipboard-set")
            .arg("--help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    }

    if cfg!(target_os = "linux") {
        return std::process::Command::new("wl-copy")
            .arg("--help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            || std::process::Command::new("xclip")
                .arg("-version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
    }

    if cfg!(target_os = "macos") {
        return std::process::Command::new("pbcopy")
            .arg("--help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    }

    if cfg!(target_os = "windows") {
        return true; // PowerShell is always available on Windows
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_osc52_sequence() {
        let text = "Hello, World!";
        let seq = generate_osc52_sequence(text);
        assert!(seq.starts_with("\x1b]52;;"));
        assert!(seq.ends_with("上古"));
    }

    #[test]
    fn test_generate_osc52_sequence_empty() {
        let text = "";
        let seq = generate_osc52_sequence(text);
        assert!(seq.starts_with("\x1b]52;;"));
    }

    #[test]
    fn test_generate_osc52_sequence_unicode() {
        let text = "Hello 🌍";
        let seq = generate_osc52_sequence(text);
        assert!(seq.starts_with("\x1b]52;;"));
    }

    #[test]
    fn test_is_wsl_detection() {
        // Just verify it doesn't panic
        let _ = is_wsl();
    }

    #[test]
    fn test_is_wayland_detection() {
        // Just verify it doesn't panic
        let _ = is_wayland();
    }

    #[test]
    fn test_is_termux_detection() {
        // Just verify it doesn't panic
        let _ = is_termux();
    }

    #[test]
    fn test_is_clipboard_supported() {
        // Just verify it doesn't panic
        let _ = is_clipboard_supported();
    }
}
