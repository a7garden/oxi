//! Extension package manager — install, update, remove WASM extensions from GitHub releases.
//!
//! `oxi ext install user/repo` — download .wasm from GitHub releases
//! `oxi ext list`               — show installed extensions
//! `oxi ext update`             — update all or specific extension
//! `oxi ext remove user/repo`   — uninstall extension
//!
//! Metadata stored in `~/.oxi/extensions/registry.json`.

use crate::util::http_client::shared_http_client;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Registry ──────────────────────────────────────────────────────

/// Per-extension metadata stored in registry.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionEntry {
    /// GitHub source (e.g. "a7garden/oxi-web-search").
    pub source: String,
    /// Installed version.
    pub version: String,
    /// Installation timestamp (ISO 8601).
    pub installed_at: String,
    /// WASM filename in extensions dir.
    pub wasm_file: String,
}

/// The on-disk registry of installed extensions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtensionRegistry {
    pub extensions: std::collections::HashMap<String, ExtensionEntry>,
}

impl ExtensionRegistry {
    /// Load registry from `~/.oxi/extensions/registry.json`.
    pub fn load() -> Result<Self> {
        let path = Self::registry_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("Failed to parse {}", path.display()))
    }

    /// Save registry to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::registry_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Path to `~/.oxi/extensions/registry.json`.
    pub fn registry_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(home.join(".oxi").join("extensions").join("registry.json"))
    }

    /// Path to `~/.oxi/extensions/`.
    pub fn extensions_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(home.join(".oxi").join("extensions"))
    }
}

// ── GitHub Release API ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
    prerelease: bool,
    draft: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Fetch latest release from a GitHub repo.
async fn fetch_latest_release(source: &str, include_prerelease: bool) -> Result<GitHubRelease> {
    let url = format!("https://api.github.com/repos/{}/releases", source);
    let client = shared_http_client();
    let mut request = client.get(&url).header("User-Agent", "oxi-ext");

    // Use GITHUB_TOKEN if available for higher rate limits
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let releases: Vec<GitHubRelease> = request
        .send()
        .await
        .with_context(|| format!("Failed to fetch releases for {}", source))?
        .json()
        .await
        .with_context(|| format!("Failed to parse releases for {}", source))?;

    releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter(|r| include_prerelease || !r.prerelease)
        .find(|r| r.assets.iter().any(|a| a.name.ends_with(".wasm")))
        .context(format!("No release with .wasm asset found for {}", source))
}

/// Download a file from URL to a local path.
async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("Failed to download {}", url))?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed with status: {}", response.status());
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read download response")?;

    std::fs::write(dest, &bytes).with_context(|| format!("Failed to write {}", dest.display()))?;

    Ok(())
}

// ── Public API ────────────────────────────────────────────────────

/// Result of an install operation.
#[derive(Debug)]
pub struct InstallResult {
    pub name: String,
    pub version: String,
    pub source: String,
    pub wasm_file: String,
}

/// Install an extension from a GitHub repo.
///
/// `source` should be in "owner/repo" format.
/// Optionally specify version as "owner/repo@version".
pub async fn install_extension(source: &str, include_prerelease: bool) -> Result<InstallResult> {
    let (repo, wanted_version) = if let Some((r, v)) = source.split_once('@') {
        (r, Some(v.to_string()))
    } else {
        (source, None)
    };

    // Validate source format
    if !repo.contains('/') || repo.split('/').count() != 2 {
        anyhow::bail!(
            "Invalid source format: '{}'. Use 'owner/repo' (e.g. 'a7garden/oxi-web-search')",
            repo
        );
    }

    let release = if let Some(tag) = &wanted_version {
        // Fetch specific release by tag
        let url = format!(
            "https://api.github.com/repos/{}/releases/tags/{}",
            repo, tag
        );
        let client = shared_http_client();
        let mut request = client.get(&url).header("User-Agent", "oxi-ext");
        if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
            request = request.header("Authorization", format!("Bearer {}", token));
        }
        request
            .send()
            .await
            .with_context(|| format!("Failed to fetch release {} for {}", tag, repo))?
            .json()
            .await
            .with_context(|| format!("Release {} not found for {}", tag, repo))?
    } else {
        fetch_latest_release(repo, include_prerelease).await?
    };

    // Find .wasm asset
    let wasm_asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".wasm"))
        .context(format!(
            "No .wasm file found in release {} of {}",
            release.tag_name, repo
        ))?;

    // Determine extension name from wasm filename
    let ext_name = wasm_asset
        .name
        .strip_suffix(".wasm")
        .unwrap_or(&wasm_asset.name)
        .to_string();

    // Download to extensions dir
    let extensions_dir = ExtensionRegistry::extensions_dir()?;
    std::fs::create_dir_all(&extensions_dir)?;
    let dest = extensions_dir.join(&wasm_asset.name);

    println!(
        "Downloading {} v{} ({:.1} KB)...",
        ext_name,
        release.tag_name,
        wasm_asset.size as f64 / 1024.0
    );

    download_file(&wasm_asset.browser_download_url, &dest).await?;

    // Update registry
    let mut registry = ExtensionRegistry::load()?;
    let entry = ExtensionEntry {
        source: repo.to_string(),
        version: release.tag_name.clone(),
        installed_at: chrono::Utc::now().to_rfc3339(),
        wasm_file: wasm_asset.name.clone(),
    };
    registry.extensions.insert(repo.to_string(), entry);
    registry.save()?;

    Ok(InstallResult {
        name: repo.to_string(),
        version: release.tag_name,
        source: repo.to_string(),
        wasm_file: wasm_asset.name.clone(),
    })
}

/// Remove an installed extension by source (owner/repo).
pub fn remove_extension(source: &str) -> Result<()> {
    let mut registry = ExtensionRegistry::load()?;

    let entry = registry
        .extensions
        .remove(source)
        .context(format!("Extension '{}' not found in registry", source))?;

    // Delete the .wasm file
    let extensions_dir = ExtensionRegistry::extensions_dir()?;
    let wasm_path = extensions_dir.join(&entry.wasm_file);
    if wasm_path.exists() {
        std::fs::remove_file(&wasm_path)
            .with_context(|| format!("Failed to delete {}", wasm_path.display()))?;
    }

    registry.save()?;
    Ok(())
}

/// List installed extensions.
pub fn list_extensions() -> Result<Vec<(String, ExtensionEntry)>> {
    let registry = ExtensionRegistry::load()?;
    let mut entries: Vec<_> = registry.extensions.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

/// Update an extension by source (owner/repo), or all if None.
pub async fn update_extension(source: Option<&str>) -> Result<Vec<InstallResult>> {
    let registry = ExtensionRegistry::load()?;
    let mut results = Vec::new();

    let targets: Vec<(String, ExtensionEntry)> = if let Some(src) = source {
        let entry = registry
            .extensions
            .get(src)
            .cloned()
            .context(format!("Extension '{}' not found", src))?;
        vec![(src.to_string(), entry)]
    } else {
        registry.extensions.into_iter().collect()
    };

    for (_ext_source, entry) in targets {
        let label = entry.wasm_file.strip_suffix(".wasm").unwrap_or(&entry.wasm_file);
        match install_extension(&entry.source, false).await {
            Ok(result) => {
                println!("Updated {} to {}", label, result.version);
                results.push(result);
            }
            Err(e) => {
                eprintln!("{}", crate::print_mode::format_error(&format!("Failed to update {}: {}", label, e)));
            }
        }
    }

    Ok(results)
}

/// Show info about a remote extension (without installing).
pub async fn info_extension(source: &str) -> Result<()> {
    let release = fetch_latest_release(source, true).await?;
    println!("Extension: {}", source);
    println!("Latest version: {}", release.tag_name);
    println!("Pre-release: {}", release.prerelease);
    println!("Assets:");
    for asset in &release.assets {
        println!("  {} ({:.1} KB)", asset.name, asset.size as f64 / 1024.0);
    }
    Ok(())
}
