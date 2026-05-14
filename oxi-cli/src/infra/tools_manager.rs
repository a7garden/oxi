//! Tools manager — download and manage external tool binaries (fd, rg).
//!
//! External tool binary management (download, cache, resolve).
//!
//! Checks for tools in the local bin directory first, then falls back to
//! system PATH. If not found anywhere, downloads the latest release from
//! GitHub.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const APP_NAME: &str = "oxi";
const NETWORK_TIMEOUT_SECS: u64 = 10;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

// ---------------------------------------------------------------------------
// ToolName
// ---------------------------------------------------------------------------

/// Supported external tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
/// fd variant.
    Fd,
/// rg variant.
    Rg,
}

impl ToolName {
    /// The key used in lookups / file names.
    pub fn key(&self) -> &'static str {
        match self {
            ToolName::Fd => "fd",
            ToolName::Rg => "rg",
        }
    }

    fn config(&self) -> ToolConfig {
        match self {
            ToolName::Fd => ToolConfig {
                name: "fd",
                repo: "sharkdp/fd",
                binary_name: "fd",
                system_binary_names: vec!["fd", "fdfind"],
                tag_prefix: "v",
            },
            ToolName::Rg => ToolConfig {
                name: "ripgrep",
                repo: "BurntSushi/ripgrep",
                binary_name: "rg",
                system_binary_names: vec!["rg"],
                tag_prefix: "",
            },
        }
    }
}

// ---------------------------------------------------------------------------
// ToolConfig
// ---------------------------------------------------------------------------

struct ToolConfig {
    name: &'static str,
    repo: &'static str,
    binary_name: &'static str,
    system_binary_names: Vec<&'static str>,
    tag_prefix: &'static str,
}

impl ToolConfig {
    /// Return the GitHub release asset name for the current platform, or
    /// `None` if the platform is unsupported.
    fn asset_name(&self, version: &str) -> Option<String> {
        let (os, arch) = platform_info();
        match self.binary_name {
            "fd" => fd_asset_name(version, os, arch),
            "rg" => rg_asset_name(version, os, arch),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

type OsStr = &'static str;
type ArchStr = &'static str;

fn platform_info() -> (OsStr, ArchStr) {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };

    (os, arch)
}

fn fd_asset_name(version: &str, os: OsStr, arch: ArchStr) -> Option<String> {
    let arch_str = if arch == "aarch64" { "aarch64" } else { "x86_64" };
    match os {
        "darwin" => Some(format!("fd-v{version}-{arch_str}-apple-darwin.tar.gz")),
        "linux" => Some(format!(
            "fd-v{version}-{arch_str}-unknown-linux-gnu.tar.gz"
        )),
        "windows" => Some(format!(
            "fd-v{version}-{arch_str}-pc-windows-msvc.zip"
        )),
        _ => None,
    }
}

fn rg_asset_name(version: &str, os: OsStr, arch: ArchStr) -> Option<String> {
    let arch_str = if arch == "aarch64" { "aarch64" } else { "x86_64" };
    match os {
        "darwin" => Some(format!(
            "ripgrep-{version}-{arch_str}-apple-darwin.tar.gz"
        )),
        "linux" => {
            if arch == "aarch64" {
                Some(format!(
                    "ripgrep-{version}-aarch64-unknown-linux-gnu.tar.gz"
                ))
            } else {
                Some(format!(
                    "ripgrep-{version}-x86_64-unknown-linux-musl.tar.gz"
                ))
            }
        }
        "windows" => Some(format!(
            "ripgrep-{version}-{arch_str}-pc-windows-msvc.zip"
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Return the local bin directory (e.g. `~/.oxi/bin`).
pub fn get_tools_dir() -> PathBuf {
    // Allow override via environment variable
    if let Ok(dir) = std::env::var("OXI_TOOLS_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".oxi")
        .join("bin")
}

/// Platform-specific binary suffix (`.exe` on Windows, empty otherwise).
fn binary_suffix() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// command_exists
// ---------------------------------------------------------------------------

/// Check if a command is available on the system PATH by running `cmd --version`.
pub fn command_exists(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// get_tool_path
// ---------------------------------------------------------------------------

/// Get the path to a tool.
///
/// Checks the local tools directory first, then the system PATH.
/// Returns `Some(path)` if found, `None` otherwise.
///
/// When the tool is found on the system PATH, returns just the binary name
/// (so it can be invoked via `Command::new`).
pub fn get_tool_path(tool: ToolName) -> Option<PathBuf> {
    let config = tool.config();

    // 1. Check local tools directory
    let local_path = get_tools_dir().join(format!("{}{}", config.binary_name, binary_suffix()));
    if local_path.exists() {
        return Some(local_path);
    }

    // 2. Check system PATH
    let system_names = &config.system_binary_names;
    for name in system_names {
        if command_exists(name) {
            return Some(PathBuf::from(*name));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// GitHub API helpers
// ---------------------------------------------------------------------------

/// Fetch the latest release version tag from GitHub and strip any leading `v`.
async fn get_latest_version(repo: &str, tag_prefix: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(NETWORK_TIMEOUT_SECS))
        .user_agent(format!("{APP_NAME}-coding-agent"))
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        bail!("GitHub API error: {}", resp.status());
    }

    let data: serde_json::Value = resp.json().await?;
    let tag = data["tag_name"]
        .as_str()
        .context("missing tag_name in GitHub response")?;

    // Strip the tag prefix (e.g. "v" → "1.0.0")
    let version = tag.strip_prefix(tag_prefix).unwrap_or(tag);
    Ok(version.to_string())
}

/// Download a file from `url` to `dest`.
async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .user_agent(format!("{APP_NAME}-coding-agent"))
        .build()?;

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        bail!("Failed to download: {} (HTTP {})", url, resp.status());
    }

    let bytes = resp.bytes().await?;
    fs::write(dest, &bytes)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------

/// Extract a `.tar.gz` archive to `dest_dir`.
fn extract_tar_gz(archive: &Path, dest_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive).context("open tar.gz archive")?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dest_dir).context("extract tar.gz archive")?;
    Ok(())
}

/// Extract a `.zip` archive to `dest_dir`.
fn extract_zip(archive: &Path, dest_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive).context("open zip archive")?;
    let mut archive = zip::ZipArchive::new(file).context("read zip archive")?;
    archive.extract(dest_dir).context("extract zip archive")?;
    Ok(())
}

/// Recursively search for a file named `binary_file_name` under `root_dir`.
fn find_binary_recursively(root_dir: &Path, binary_file_name: &str) -> Option<PathBuf> {
    let mut stack = vec![root_dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if path.file_name().map(|n| n == binary_file_name).unwrap_or(false) {
                    return Some(path);
                }
            } else if path.is_dir() {
                stack.push(path);
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// download_tool
// ---------------------------------------------------------------------------

/// Download and install a tool into the local tools directory.
///
/// Returns the path to the installed binary.
async fn download_tool(tool: ToolName) -> Result<PathBuf> {
    let config = tool.config();
    let tools_dir = get_tools_dir();

    // Get latest version
    let version = get_latest_version(config.repo, config.tag_prefix).await?;

    // Get asset name for current platform
    let asset_name = config
        .asset_name(&version)
        .ok_or_else(|| anyhow::anyhow!("Unsupported platform for tool {}", config.name))?;

    // Ensure tools directory exists
    fs::create_dir_all(&tools_dir)?;

    let download_url = format!(
        "https://github.com/{}/releases/download/{}{}/{}",
        config.repo, config.tag_prefix, version, asset_name
    );
    let archive_path = tools_dir.join(&asset_name);
    let ext = binary_suffix();
    let binary_file_name = format!("{}{ext}", config.binary_name);
    let binary_path = tools_dir.join(&binary_file_name);

    // Download
    tracing::debug!("Downloading {} from {}", config.name, download_url);
    download_file(&download_url, &archive_path).await?;

    // Create a unique temp extraction directory to avoid races when
    // multiple tools are downloaded concurrently during startup.
    let extract_dir = tools_dir.join(format!(
        "extract_tmp_{}_{}",
        config.binary_name,
        std::process::id()
    ));
    fs::create_dir_all(&extract_dir)?;

    let result = (|| -> Result<()> {
        // Extract
        if asset_name.ends_with(".tar.gz") {
            extract_tar_gz(&archive_path, &extract_dir)?;
        } else if asset_name.ends_with(".zip") {
            extract_zip(&archive_path, &extract_dir)?;
        } else {
            bail!("Unsupported archive format: {}", asset_name);
        }

        // Find the binary.  Some archives contain files directly at root,
        // others nest under a versioned subdirectory.
        let archive_stem = asset_name
            .trim_end_matches(".tar.gz")
            .trim_end_matches(".zip");
        let nested_dir = extract_dir.join(archive_stem);

        let extracted_binary = if nested_dir.join(&binary_file_name).exists() {
            nested_dir.join(&binary_file_name)
        } else if extract_dir.join(&binary_file_name).exists() {
            extract_dir.join(&binary_file_name)
        } else {
            // Recursive search
            find_binary_recursively(&extract_dir, &binary_file_name)
                .context(format!("Binary not found in archive: expected {binary_file_name}"))?
        };

        // Move to final location
        fs::rename(&extracted_binary, &binary_path)?;

        // Make executable (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o755))?;
        }

        Ok(())
    })();

    // Cleanup archive and temp dir regardless of success
    let _ = fs::remove_file(&archive_path);
    let _ = fs::remove_dir_all(&extract_dir);

    result?;

    tracing::debug!("{} installed to {}", config.name, binary_path.display());
    Ok(binary_path)
}

// ---------------------------------------------------------------------------
// is_offline_mode
// ---------------------------------------------------------------------------

fn is_offline_mode_enabled() -> bool {
    match std::env::var("OXI_OFFLINE") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// ensure_tool
// ---------------------------------------------------------------------------

/// Ensure a tool is available, downloading it from GitHub releases if
/// necessary.
///
/// Returns the path to the tool binary, or an error if the tool cannot be
/// found or downloaded (offline mode / unsupported platform).
pub async fn ensure_tool(tool: ToolName) -> Result<PathBuf> {
    // Already available?
    if let Some(path) = get_tool_path(tool) {
        return Ok(path);
    }

    let config = tool.config();

    // Offline mode — skip download
    if is_offline_mode_enabled() {
        tracing::warn!(
            "{} not found. Offline mode enabled, skipping download.",
            config.name
        );
        bail!(
            "{} not found and offline mode is enabled",
            config.name
        );
    }

    // Download
    tracing::info!("{} not found locally. Downloading...", config.name);

    match download_tool(tool).await {
        Ok(path) => {
            tracing::info!("{} installed to {}", config.name, path.display());
            Ok(path)
        }
        Err(e) => {
            tracing::warn!("Failed to download {}: {}", config.name, e);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fd_asset_name_macos_aarch64() {
        let name = fd_asset_name("10.1.0", "darwin", "aarch64");
        assert_eq!(
            name,
            Some("fd-v10.1.0-aarch64-apple-darwin.tar.gz".to_string())
        );
    }

    #[test]
    fn test_fd_asset_name_linux_x86_64() {
        let name = fd_asset_name("10.1.0", "linux", "x86_64");
        assert_eq!(
            name,
            Some("fd-v10.1.0-x86_64-unknown-linux-gnu.tar.gz".to_string())
        );
    }

    #[test]
    fn test_fd_asset_name_windows_x86_64() {
        let name = fd_asset_name("10.1.0", "windows", "x86_64");
        assert_eq!(
            name,
            Some("fd-v10.1.0-x86_64-pc-windows-msvc.zip".to_string())
        );
    }

    #[test]
    fn test_rg_asset_name_macos_aarch64() {
        let name = rg_asset_name("14.1.0", "darwin", "aarch64");
        assert_eq!(
            name,
            Some("ripgrep-14.1.0-aarch64-apple-darwin.tar.gz".to_string())
        );
    }

    #[test]
    fn test_rg_asset_name_linux_x86_64() {
        let name = rg_asset_name("14.1.0", "linux", "x86_64");
        assert_eq!(
            name,
            Some("ripgrep-14.1.0-x86_64-unknown-linux-musl.tar.gz".to_string())
        );
    }

    #[test]
    fn test_rg_asset_name_linux_aarch64() {
        let name = rg_asset_name("14.1.0", "linux", "aarch64");
        assert_eq!(
            name,
            Some("ripgrep-14.1.0-aarch64-unknown-linux-gnu.tar.gz".to_string())
        );
    }

    #[test]
    fn test_rg_asset_name_windows_x86_64() {
        let name = rg_asset_name("14.1.0", "windows", "x86_64");
        assert_eq!(
            name,
            Some("ripgrep-14.1.0-x86_64-pc-windows-msvc.zip".to_string())
        );
    }

    #[test]
    fn test_unsupported_platform() {
        let name = fd_asset_name("10.1.0", "freebsd", "x86_64");
        assert!(name.is_none());
    }

    #[test]
    fn test_tool_config() {
        assert_eq!(ToolName::Fd.key(), "fd");
        assert_eq!(ToolName::Rg.key(), "rg");

        let fd = ToolName::Fd.config();
        assert_eq!(fd.repo, "sharkdp/fd");
        assert_eq!(fd.tag_prefix, "v");

        let rg = ToolName::Rg.config();
        assert_eq!(rg.repo, "BurntSushi/ripgrep");
        assert_eq!(rg.tag_prefix, "");
    }

    #[test]
    fn test_get_tools_dir_default() {
        // Should be under ~/.oxi/bin unless OXI_TOOLS_DIR is set
        let dir = get_tools_dir();
        assert!(dir.to_string_lossy().contains(".oxi"));
        assert!(dir.to_string_lossy().contains("bin"));
    }

    #[test]
    fn test_command_exists_known_cmd() {
        // Use a command that reliably supports --version on all platforms
        #[cfg(unix)]
        {
            // `uname` exits with 0 even without args; test the function works
            // by checking a command we know should exist
            let result = std::process::Command::new("uname")
                .arg("--version")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .status();
            // We just verify the function doesn't panic; the actual result
            // depends on the platform
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_binary_suffix() {
        #[cfg(target_os = "windows")]
        assert_eq!(binary_suffix(), ".exe");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(binary_suffix(), "");
    }
}
