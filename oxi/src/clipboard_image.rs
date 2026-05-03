//! Clipboard image reading for oxi
//!
//! Reads images from the system clipboard using platform-specific tools.
//! Supports: Wayland (wl-paste), X11 (xclip), macOS (pngpaste/pbpaste), Windows (PowerShell)

use anyhow::{Context, Result};
use std::env;
use std::process::Command;

/// Magic bytes for image format detection
const MAGIC_PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const MAGIC_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];
const MAGIC_GIF: &[u8] = &[0x47, 0x49, 0x46];
const MAGIC_WEBP: &[u8] = b"RIFF";
const MAGIC_WEBP_END: &[u8] = b"WEBP";
const MAGIC_BMP: &[u8] = &[0x42, 0x4D];

/// Default timeout for clipboard operations (ms)
const DEFAULT_TIMEOUT_MS: u64 = 3000;

/// Detect MIME type from magic bytes
fn detect_mime_type(data: &[u8]) -> &'static str {
    if data.len() >= 8 {
        if data.starts_with(MAGIC_PNG) {
            return "image/png";
        }
        if data.starts_with(MAGIC_JPEG) {
            return "image/jpeg";
        }
        if data.starts_with(MAGIC_GIF) {
            return "image/gif";
        }
        if data.len() >= 12 && data.starts_with(MAGIC_WEBP) && &data[8..12] == MAGIC_WEBP_END {
            return "image/webp";
        }
        if data.starts_with(MAGIC_BMP) {
            return "image/bmp";
        }
    }
    "application/octet-stream"
}

/// Result of reading clipboard image
#[derive(Debug, Clone)]
pub struct ClipboardImage {
    /// Raw image bytes
    pub bytes: Vec<u8>,
    /// Detected MIME type
    pub mime_type: String,
}

impl ClipboardImage {
    /// Get the file extension for the detected MIME type
    pub fn extension(&self) -> &'static str {
        match self.mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            _ => "bin",
        }
    }
}

/// Run a command and capture output
fn run_command(command: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run {} with args {:?}", command, args))?;
    Ok(output)
}

/// Run a command with timeout
fn run_command_timeout(
    command: &str,
    args: &[&str],
    timeout_ms: u64,
) -> Result<std::process::Output> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run {} with args {:?}", command, args))?;
    Ok(output)
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

/// Read image from Wayland clipboard using wl-paste
fn read_image_via_wl_paste() -> Option<ClipboardImage> {
    // First, list available types
    let list_output =
        run_command_timeout("wl-paste", &["--list-types"], DEFAULT_TIMEOUT_MS).ok()?;

    if !list_output.status.success() {
        return None;
    }

    let types: Vec<&str> = list_output
        .stdout
        .split(|&b| b == b'\n')
        .filter_map(|b| std::str::from_utf8(b).ok())
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    // Try image types in preferred order
    let preferred_types = ["image/png", "image/jpeg", "image/gif", "image/webp"];

    for mime_type in preferred_types {
        if types.iter().any(|t| *t == mime_type) {
            let data = run_command_timeout(
                "wl-paste",
                &["--type", mime_type, "--no-newline"],
                DEFAULT_TIMEOUT_MS,
            )
            .ok()?;
            if data.status.success() && !data.stdout.is_empty() {
                let mime = detect_mime_type(&data.stdout);
                return Some(ClipboardImage {
                    bytes: data.stdout,
                    mime_type: mime.to_string(),
                });
            }
        }
    }

    None
}

/// Read image from X11 clipboard using xclip
fn read_image_via_xclip() -> Option<ClipboardImage> {
    // Try to list available targets
    let targets_output = run_command_timeout(
        "xclip",
        &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        DEFAULT_TIMEOUT_MS,
    )
    .ok()?;

    let candidate_types: Vec<&str> = if targets_output.status.success() {
        targets_output
            .stdout
            .split(|&b| b == b'\n')
            .filter_map(|b| std::str::from_utf8(b).ok())
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    } else {
        vec![]
    };

    // Try preferred types
    let preferred_types = ["image/png", "image/jpeg", "image/gif", "image/webp"];

    for mime_type in preferred_types {
        // Check if this type is available
        if !candidate_types.is_empty() && !candidate_types.contains(&mime_type) {
            continue;
        }

        let data = run_command_timeout(
            "xclip",
            &["-selection", "clipboard", "-t", mime_type, "-o"],
            DEFAULT_TIMEOUT_MS,
        )
        .ok()?;
        if data.status.success() && !data.stdout.is_empty() {
            let mime = detect_mime_type(&data.stdout);
            return Some(ClipboardImage {
                bytes: data.stdout,
                mime_type: mime.to_string(),
            });
        }
    }

    None
}

/// Read image from macOS clipboard using pngpaste or pbpaste
fn read_image_via_macos() -> Option<ClipboardImage> {
    // Try pngpaste first (if available, gives PNG directly)
    let output = run_command_timeout("pngpaste", &[], DEFAULT_TIMEOUT_MS).ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        let mime = detect_mime_type(&output.stdout);
        return Some(ClipboardImage {
            bytes: output.stdout,
            mime_type: mime.to_string(),
        });
    }

    // Fallback to pbpaste (converts to PNG format)
    let output = run_command_timeout("pbpaste", &[], DEFAULT_TIMEOUT_MS).ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        let mime = detect_mime_type(&output.stdout);
        return Some(ClipboardImage {
            bytes: output.stdout,
            mime_type: mime.to_string(),
        });
    }

    None
}

/// Read image from Windows clipboard using PowerShell
fn read_image_via_powershell() -> Option<ClipboardImage> {
    use std::fs;
    use std::path::PathBuf;

    // Create temp file for the image
    let temp_dir = env::var("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    let temp_file = temp_dir.join(format!("oxi-clipboard-{}.png", uuid::Uuid::new_v4()));
    let temp_path = temp_file.to_string_lossy().to_string();

    let ps_script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         Add-Type -AssemblyName System.Drawing; \
         $img = [System.Windows.Forms.Clipboard]::GetImage(); \
         if ($img) {{ $img.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'ok' }} else {{ Write-Output 'empty' }}",
        temp_path.replace("'", "''")
    );

    let output = run_command_timeout(
        "powershell.exe",
        &["-NoProfile", "-Command", &ps_script],
        5000,
    )
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().contains("ok") {
        return None;
    }

    // Read the temp file
    let bytes = fs::read(&temp_file).ok()?;

    // Clean up temp file
    let _ = fs::remove_file(&temp_file);

    if bytes.is_empty() {
        return None;
    }

    Some(ClipboardImage {
        bytes,
        mime_type: "image/png".to_string(),
    })
}

/// Read image from clipboard, trying platform-specific methods in order.
///
/// Tries in order:
/// - Linux/Wayland: wl-paste, then xclip
/// - Linux/X11: xclip, then wl-paste
/// - macOS: pngpaste, then pbpaste
/// - Windows: PowerShell
/// - WSL: PowerShell (for Windows clipboard)
pub fn read_image_from_clipboard() -> Result<ClipboardImage> {
    if is_termux() {
        anyhow::bail!("Clipboard image reading not available in Termux");
    }

    let image = if cfg!(target_os = "linux") {
        if is_wayland() {
            read_image_via_wl_paste().or_else(read_image_via_xclip)
        } else {
            read_image_via_xclip().or_else(read_image_via_wl_paste)
        }
    } else if cfg!(target_os = "macos") {
        read_image_via_macos()
    } else if cfg!(target_os = "windows") {
        read_image_via_powershell()
    } else if is_wsl() {
        // Try Linux clipboard first, then Windows
        read_image_via_wl_paste()
            .or_else(read_image_via_xclip)
            .or_else(read_image_via_powershell)
    } else {
        None
    };

    image.ok_or_else(|| anyhow::anyhow!("No image found in clipboard"))
}

/// Detect MIME type from magic bytes (public API)
pub fn detect_image_mime_type(data: &[u8]) -> &'static str {
    detect_mime_type(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_mime_type_png() {
        let png_header = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_mime_type(&png_header), "image/png");
    }

    #[test]
    fn test_detect_mime_type_jpeg() {
        let jpeg_header = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_mime_type(&jpeg_header), "image/jpeg");
    }

    #[test]
    fn test_detect_mime_type_gif() {
        let gif_header = vec![0x47, 0x49, 0x46, 0x38, 0x39, 0x61];
        assert_eq!(detect_mime_type(&gif_header), "image/gif");
    }

    #[test]
    fn test_detect_mime_type_webp() {
        // RIFF....WEBP
        let webp_header = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        assert_eq!(detect_mime_type(&webp_header), "image/webp");
    }

    #[test]
    fn test_detect_mime_type_bmp() {
        let bmp_header = vec![0x42, 0x4D, 0x00, 0x00];
        assert_eq!(detect_mime_type(&bmp_header), "image/bmp");
    }

    #[test]
    fn test_detect_mime_type_unknown() {
        let random_data = vec![0x00, 0x01, 0x02, 0x03];
        assert_eq!(detect_mime_type(&random_data), "application/octet-stream");
    }

    #[test]
    fn test_detect_mime_type_short() {
        let short_data = vec![0x00, 0x01];
        assert_eq!(detect_mime_type(&short_data), "application/octet-stream");
    }

    #[test]
    fn test_clipboard_image_extension() {
        let image = ClipboardImage {
            bytes: vec![],
            mime_type: "image/png".to_string(),
        };
        assert_eq!(image.extension(), "png");

        let image = ClipboardImage {
            bytes: vec![],
            mime_type: "image/jpeg".to_string(),
        };
        assert_eq!(image.extension(), "jpg");
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
}
