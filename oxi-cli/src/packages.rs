//! Package system for oxi CLI
//!
//! Packages bundle extensions, skills, prompts, and themes for sharing.
//! Supports local directories and npm packages.
//!
//! ## Package manifest
//!
//! A package is a directory containing an `oxi-package.toml` file:
//!
//! ```toml
//! name = "@foo/oxi-tools"
//! version = "1.0.0"
//! extensions = ["ext/index.ts"]
//! skills = ["skills/code-review/SKILL.md"]
//! prompts = ["prompts/review.md"]
//! themes = ["themes/dark-pro.json"]
//! ```
//!
//! ## Resource discovery
//!
//! When a package lacks explicit resource lists, resources are discovered
//! automatically by scanning the package directory:
//! - **Extensions**: `.so`, `.dylib`, `.dll` files, or `index.ts`/`index.js` entries
//! - **Skills**: Directories containing `SKILL.md`
//! - **Prompts**: `.md` files in `prompts/` subdirectory
//! - **Themes**: `.json` files in `themes/` subdirectory

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Package manifest describing bundled resources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    /// Package name (e.g. "@foo/oxi-tools")
    pub name: String,
    /// Semantic version (e.g. "1.0.0")
    pub version: String,
    /// Extension paths relative to the package root
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Skill names/paths
    #[serde(default)]
    pub skills: Vec<String>,
    /// Prompt template paths
    #[serde(default)]
    pub prompts: Vec<String>,
    /// Theme paths
    #[serde(default)]
    pub themes: Vec<String>,
}

/// A discovered resource within a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredResource {
    /// Resource type
    pub kind: ResourceKind,
    /// Absolute path to the resource
    pub path: PathBuf,
    /// Relative path within the package
    pub relative_path: String,
}

/// Types of resources a package can contribute
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Extension,
    Skill,
    Prompt,
    Theme,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceKind::Extension => write!(f, "extension"),
            ResourceKind::Skill => write!(f, "skill"),
            ResourceKind::Prompt => write!(f, "prompt"),
            ResourceKind::Theme => write!(f, "theme"),
        }
    }
}

/// Manages installation, removal, and listing of packages
pub struct PackageManager {
    packages_dir: PathBuf,
    installed: HashMap<String, PackageManifest>,
}

impl PackageManager {
    /// Create a new PackageManager using the default packages directory
    pub fn new() -> Result<Self> {
        let base = dirs::home_dir().context("Cannot determine home directory")?;
        let packages_dir = base.join(".oxi").join("packages");
        let mut mgr = Self {
            packages_dir,
            installed: HashMap::new(),
        };
        mgr.load_installed()?;
        Ok(mgr)
    }

    /// Create a PackageManager with a custom packages directory (for testing)
    pub fn with_dir(packages_dir: PathBuf) -> Result<Self> {
        let mut mgr = Self {
            packages_dir,
            installed: HashMap::new(),
        };
        mgr.load_installed()?;
        Ok(mgr)
    }

    /// Load all installed package manifests from disk
    fn load_installed(&mut self) -> Result<()> {
        if !self.packages_dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.packages_dir)? {
            let entry = entry?;
            let manifest_path = entry.path().join("oxi-package.toml");
            if manifest_path.exists() {
                match Self::read_manifest(&manifest_path) {
                    Ok(manifest) => {
                        self.installed.insert(manifest.name.clone(), manifest);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to load manifest {}: {}",
                            manifest_path.display(),
                            e
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Read and parse a package manifest from disk
    fn read_manifest(path: &Path) -> Result<PackageManifest> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest {}", path.display()))?;
        let manifest: PackageManifest = toml::from_str(&content)
            .with_context(|| format!("Failed to parse manifest {}", path.display()))?;
        Ok(manifest)
    }

    /// Get the installation directory for a package
    fn pkg_install_dir(&self, name: &str) -> PathBuf {
        // Sanitise name: replace @ and / with -
        let safe_name = name.replace('@', "").replace('/', "-");
        self.packages_dir.join(safe_name)
    }

    /// Install a package from a local directory path
    pub fn install(&mut self, source: &str) -> Result<PackageManifest> {
        let source_path = Path::new(source);
        let manifest_path = source_path.join("oxi-package.toml");

        let manifest = Self::read_manifest(&manifest_path)
            .with_context(|| format!("No valid oxi-package.toml found in {}", source))?;

        let dest = self.pkg_install_dir(&manifest.name);

        // Ensure packages directory exists
        fs::create_dir_all(&self.packages_dir).with_context(|| {
            format!(
                "Failed to create packages directory {}",
                self.packages_dir.display()
            )
        })?;

        // Remove previous installation if it exists
        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| {
                format!("Failed to remove existing package at {}", dest.display())
            })?;
        }

        // Copy the entire source directory
        copy_dir_recursive(source_path, &dest).with_context(|| {
            format!(
                "Failed to copy package from {} to {}",
                source,
                dest.display()
            )
        })?;

        self.installed
            .insert(manifest.name.clone(), manifest.clone());
        Ok(manifest)
    }

    /// Install a package from npm
    pub fn install_npm(&mut self, name: &str) -> Result<PackageManifest> {
        // npm pack the package to a temp directory
        let tmp_dir =
            tempfile::tempdir().context("Failed to create temp directory for npm install")?;

        let status = std::process::Command::new("npm")
            .args(["pack", name, "--pack-destination"])
            .arg(tmp_dir.path())
            .current_dir(tmp_dir.path())
            .output()
            .context("Failed to run npm pack")?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            anyhow::bail!("npm pack failed for '{}': {}", name, stderr);
        }

        // Find the tarball
        let tarball = fs::read_dir(tmp_dir.path())?
            .filter_map(|e| e.ok())
            .find(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "tgz")
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .context("No .tgz file found after npm pack")?;

        // Extract tarball into a subdirectory
        let extract_dir = tmp_dir.path().join("extracted");
        fs::create_dir_all(&extract_dir)?;

        let tar_status = std::process::Command::new("tar")
            .args(["-xzf", &tarball.to_string_lossy(), "-C"])
            .arg(&extract_dir)
            .current_dir(tmp_dir.path())
            .output()
            .context("Failed to run tar")?;

        if !tar_status.status.success() {
            let stderr = String::from_utf8_lossy(&tar_status.stderr);
            anyhow::bail!("tar extraction failed: {}", stderr);
        }

        // npm pack extracts into a "package" subdirectory
        let pkg_source = extract_dir.join("package");

        // Ensure packages directory exists
        fs::create_dir_all(&self.packages_dir).with_context(|| {
            format!(
                "Failed to create packages directory {}",
                self.packages_dir.display()
            )
        })?;

        let safe_name = name.replace('@', "").replace('/', "-");
        let dest = self.packages_dir.join(safe_name);

        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| {
                format!("Failed to remove existing package at {}", dest.display())
            })?;
        }

        copy_dir_recursive(&pkg_source, &dest)
            .with_context(|| format!("Failed to copy npm package for '{}'", name))?;

        // Read the manifest from the installed location
        let manifest_path = dest.join("oxi-package.toml");
        let manifest = if manifest_path.exists() {
            Self::read_manifest(&manifest_path)?
        } else {
            // Synthesise a minimal manifest if the package doesn't have one
            PackageManifest {
                name: name.to_string(),
                version: "0.0.0".to_string(),
                extensions: Vec::new(),
                skills: Vec::new(),
                prompts: Vec::new(),
                themes: Vec::new(),
            }
        };

        self.installed
            .insert(manifest.name.clone(), manifest.clone());
        Ok(manifest)
    }

    /// Uninstall a package by name
    pub fn uninstall(&mut self, name: &str) -> Result<()> {
        if !self.installed.contains_key(name) {
            anyhow::bail!("Package '{}' is not installed", name);
        }

        let dest = self.pkg_install_dir(name);
        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| {
                format!("Failed to remove package directory {}", dest.display())
            })?;
        }

        self.installed.remove(name);
        Ok(())
    }

    /// Update a package (re-install from the same source).
    /// For npm packages, re-runs `npm pack` to get the latest version.
    /// For local packages, re-copies from the source path (if available).
    pub fn update(&mut self, name: &str) -> Result<PackageManifest> {
        if !self.installed.contains_key(name) {
            anyhow::bail!("Package '{}' is not installed", name);
        }

        // For npm packages, re-run install_npm
        // The package name IS the npm spec for npm packages
        let _manifest = self.installed.get(name).cloned().unwrap();

        // Re-install: try npm first (most common), fall back to no-op
        let result = self.install_npm(name)?;
        Ok(result)
    }

    /// List all installed packages
    pub fn list(&self) -> Vec<&PackageManifest> {
        self.installed.values().collect()
    }

    /// Check whether a package is installed
    pub fn is_installed(&self, name: &str) -> bool {
        self.installed.contains_key(name)
    }

    /// Get the packages directory path
    pub fn packages_dir(&self) -> &Path {
        &self.packages_dir
    }

    /// Get the install directory for a package (if it exists on disk)
    pub fn get_install_dir(&self, name: &str) -> Option<PathBuf> {
        let dir = self.pkg_install_dir(name);
        if dir.exists() {
            Some(dir)
        } else {
            None
        }
    }

    /// Discover all resources from an installed package.
    ///
    /// If the manifest lists explicit paths, those are resolved against the
    /// install directory. Otherwise, auto-discovery is performed.
    pub fn discover_resources(&self, name: &str) -> Result<Vec<DiscoveredResource>> {
        let manifest = self
            .installed
            .get(name)
            .with_context(|| format!("Package '{}' not found", name))?;

        let install_dir = self.pkg_install_dir(name);
        if !install_dir.exists() {
            anyhow::bail!("Install directory for '{}' does not exist", name);
        }

        let mut resources = Vec::new();

        // If manifest has explicit entries, use them
        let has_explicit = !manifest.extensions.is_empty()
            || !manifest.skills.is_empty()
            || !manifest.prompts.is_empty()
            || !manifest.themes.is_empty();

        if has_explicit {
            for ext in &manifest.extensions {
                let path = install_dir.join(ext);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Extension,
                        path,
                        relative_path: ext.clone(),
                    });
                }
            }
            for skill in &manifest.skills {
                let path = install_dir.join(skill);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Skill,
                        path,
                        relative_path: skill.clone(),
                    });
                }
            }
            for prompt in &manifest.prompts {
                let path = install_dir.join(prompt);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Prompt,
                        path,
                        relative_path: prompt.clone(),
                    });
                }
            }
            for theme in &manifest.themes {
                let path = install_dir.join(theme);
                if path.exists() {
                    resources.push(DiscoveredResource {
                        kind: ResourceKind::Theme,
                        path,
                        relative_path: theme.clone(),
                    });
                }
            }
        } else {
            // Auto-discover resources
            resources.extend(discover_extensions(&install_dir));
            resources.extend(discover_skills(&install_dir));
            resources.extend(discover_prompts(&install_dir));
            resources.extend(discover_themes(&install_dir));
        }

        Ok(resources)
    }

    /// Get resource counts for a package
    pub fn resource_counts(&self, name: &str) -> Result<ResourceCounts> {
        let resources = self.discover_resources(name)?;
        let mut counts = ResourceCounts::default();
        for r in &resources {
            match r.kind {
                ResourceKind::Extension => counts.extensions += 1,
                ResourceKind::Skill => counts.skills += 1,
                ResourceKind::Prompt => counts.prompts += 1,
                ResourceKind::Theme => counts.themes += 1,
            }
        }
        Ok(counts)
    }
}

/// Counts of each resource type in a package
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceCounts {
    pub extensions: usize,
    pub skills: usize,
    pub prompts: usize,
    pub themes: usize,
}

impl std::fmt::Display for ResourceCounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.extensions > 0 {
            parts.push(format!("{} ext", self.extensions));
        }
        if self.skills > 0 {
            parts.push(format!("{} skill", self.skills));
        }
        if self.prompts > 0 {
            parts.push(format!("{} prompt", self.prompts));
        }
        if self.themes > 0 {
            parts.push(format!("{} theme", self.themes));
        }
        if parts.is_empty() {
            write!(f, "-")?;
        } else {
            write!(f, "{}", parts.join(", "))?;
        }
        Ok(())
    }
}

// ── Auto-discovery helpers ────────────────────────────────────────────

/// Discover extension files in a directory.
///
/// Looks for:
/// - `.so`, `.dylib`, `.dll` files (compiled extensions)
/// - `index.ts` or `index.js` files (TypeScript/JavaScript extensions)
fn discover_extensions(dir: &Path) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_extensions_recursive(dir, dir, &mut results);
    results
}

fn discover_extensions_recursive(
    base: &Path,
    current: &Path,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files and node_modules
        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            // Check for index.ts / index.js in subdirectory
            for index in &["index.ts", "index.js"] {
                let index_path = path.join(index);
                if index_path.exists() {
                    let rel = path.strip_prefix(base).unwrap_or(&path);
                    results.push(DiscoveredResource {
                        kind: ResourceKind::Extension,
                        path: index_path,
                        relative_path: rel.join(index).to_string_lossy().to_string(),
                    });
                }
            }
            // Don't recurse into extension directories — just check top-level
        } else {
            // Check for compiled extension files
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "so" | "dylib" | "dll") {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Extension,
                    path: path.clone(),
                    relative_path: rel.to_string_lossy().to_string(),
                });
            }
            // Check for top-level .ts/.js files
            if matches!(ext, "ts" | "js") && !name_str.starts_with('.') {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Extension,
                    path: path.clone(),
                    relative_path: rel.to_string_lossy().to_string(),
                });
            }
        }
    }
}

/// Discover skill directories containing SKILL.md
fn discover_skills(dir: &Path) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                let rel = path.strip_prefix(dir).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Skill,
                    path: skill_file,
                    relative_path: rel.join("SKILL.md").to_string_lossy().to_string(),
                });
            }

            // Also check skills/ subdirectory
            let skills_subdir = dir.join("skills");
            if skills_subdir.exists() && skills_subdir.is_dir() {
                let sub_entries = match fs::read_dir(&skills_subdir) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for sub_entry in sub_entries.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.is_dir() {
                        let sf = sub_path.join("SKILL.md");
                        if sf.exists() {
                            let rel = sub_path.strip_prefix(dir).unwrap_or(&sub_path);
                            results.push(DiscoveredResource {
                                kind: ResourceKind::Skill,
                                path: sf,
                                relative_path: rel.join("SKILL.md").to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    results
}

/// Discover prompt template files (.md in prompts/ subdirectory)
fn discover_prompts(dir: &Path) -> Vec<DiscoveredResource> {
    let prompts_dir = dir.join("prompts");
    discover_files_by_ext(
        if prompts_dir.exists() {
            &prompts_dir
        } else {
            dir
        },
        "md",
        ResourceKind::Prompt,
    )
}

/// Discover theme files (.json in themes/ subdirectory)
fn discover_themes(dir: &Path) -> Vec<DiscoveredResource> {
    let themes_dir = dir.join("themes");
    discover_files_by_ext(
        if themes_dir.exists() {
            &themes_dir
        } else {
            dir
        },
        "json",
        ResourceKind::Theme,
    )
}

/// Recursively find files with a given extension
fn discover_files_by_ext(dir: &Path, ext: &str, kind: ResourceKind) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_files_recursive(dir, dir, ext, kind, &mut results);
    results
}

fn discover_files_recursive(
    base: &Path,
    current: &Path,
    ext: &str,
    kind: ResourceKind,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            discover_files_recursive(base, &path, ext, kind, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            results.push(DiscoveredResource {
                kind,
                path: path.clone(),
                relative_path: rel.to_string_lossy().to_string(),
            });
        }
    }
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_temp_packages_dir() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let packages_dir = tmp.path().join("packages");
        fs::create_dir_all(&packages_dir).unwrap();
        (tmp, packages_dir)
    }

    fn create_test_package(base: &Path, name: &str, version: &str) -> PathBuf {
        let pkg_dir = base.join("source-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();

        let manifest = PackageManifest {
            name: name.to_string(),
            version: version.to_string(),
            extensions: vec!["ext1.so".to_string()],
            skills: vec!["skill-a".to_string()],
            prompts: vec![],
            themes: vec![],
        };

        let toml_content = toml::to_string_pretty(&manifest).unwrap();
        fs::write(pkg_dir.join("oxi-package.toml"), toml_content).unwrap();
        fs::write(pkg_dir.join("ext1.so"), "fake extension").unwrap();
        fs::create_dir_all(pkg_dir.join("skill-a")).unwrap();
        fs::write(pkg_dir.join("skill-a").join("SKILL.md"), "# Skill A").unwrap();

        pkg_dir
    }

    fn create_test_package_with_auto_discovery(base: &Path, name: &str, version: &str) -> PathBuf {
        let pkg_dir = base.join("source-pkg-auto");
        fs::create_dir_all(&pkg_dir).unwrap();

        // Minimal manifest with no explicit resource lists
        let manifest = PackageManifest {
            name: name.to_string(),
            version: version.to_string(),
            extensions: vec![],
            skills: vec![],
            prompts: vec![],
            themes: vec![],
        };
        let toml_content = toml::to_string_pretty(&manifest).unwrap();
        fs::write(pkg_dir.join("oxi-package.toml"), toml_content).unwrap();

        // Create auto-discoverable resources
        fs::write(pkg_dir.join("myext.so"), "extension").unwrap();
        fs::create_dir_all(pkg_dir.join("my-skill")).unwrap();
        fs::write(pkg_dir.join("my-skill").join("SKILL.md"), "# My Skill").unwrap();
        fs::create_dir_all(pkg_dir.join("prompts")).unwrap();
        fs::write(pkg_dir.join("prompts").join("review.md"), "# Review").unwrap();
        fs::create_dir_all(pkg_dir.join("themes")).unwrap();
        fs::write(pkg_dir.join("themes").join("dark.json"), "{}").unwrap();

        pkg_dir
    }

    #[test]
    fn test_install_and_list() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        let manifest = mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        assert_eq!(manifest.name, "test-pkg");
        assert_eq!(manifest.version, "1.0.0");

        let installed = mgr.list();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].name, "test-pkg");
    }

    #[test]
    fn test_uninstall() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        assert!(mgr.is_installed("test-pkg"));

        mgr.uninstall("test-pkg").unwrap();
        assert!(!mgr.is_installed("test-pkg"));
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_uninstall_not_installed() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        let result = mgr.uninstall("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_install_scoped_package() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "@foo/oxi-tools", "2.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();

        let manifest = mgr.install(pkg_dir.to_str().unwrap()).unwrap();
        assert_eq!(manifest.name, "@foo/oxi-tools");

        // Install dir should use sanitised name
        let expected_dir = packages_dir.join("foo-oxi-tools");
        assert!(expected_dir.exists());
    }

    #[test]
    fn test_reinstall_overwrites() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        // Install again (same name, different version)
        let pkg_dir_v2 = tmp.path().join("source-pkg-v2");
        fs::create_dir_all(&pkg_dir_v2).unwrap();
        let manifest_v2 = PackageManifest {
            name: "test-pkg".to_string(),
            version: "2.0.0".to_string(),
            extensions: vec![],
            skills: vec![],
            prompts: vec![],
            themes: vec![],
        };
        fs::write(
            pkg_dir_v2.join("oxi-package.toml"),
            toml::to_string_pretty(&manifest_v2).unwrap(),
        )
        .unwrap();

        mgr.install(pkg_dir_v2.to_str().unwrap()).unwrap();

        let installed = mgr.list();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version, "2.0.0");
    }

    #[test]
    fn test_empty_packages_dir() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mgr = PackageManager::with_dir(packages_dir).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_packages_dir_not_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("does-not-exist");
        let mgr = PackageManager::with_dir(nonexistent).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_discover_resources_explicit() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let resources = mgr.discover_resources("test-pkg").unwrap();
        assert_eq!(resources.len(), 2); // ext1.so + skill-a/SKILL.md

        let extensions: Vec<_> = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Extension)
            .collect();
        let skills: Vec<_> = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Skill)
            .collect();
        assert_eq!(extensions.len(), 1);
        assert_eq!(skills.len(), 1);
    }

    #[test]
    fn test_discover_resources_auto() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package_with_auto_discovery(tmp.path(), "auto-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let resources = mgr.discover_resources("auto-pkg").unwrap();

        // Should discover: myext.so, my-skill/SKILL.md, prompts/review.md, themes/dark.json
        let ext_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Extension)
            .count();
        let skill_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Skill)
            .count();
        let prompt_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Prompt)
            .count();
        let theme_count = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Theme)
            .count();

        assert!(
            ext_count >= 1,
            "Expected at least 1 extension, got {}",
            ext_count
        );
        assert!(
            skill_count >= 1,
            "Expected at least 1 skill, got {}",
            skill_count
        );
        assert!(
            prompt_count >= 1,
            "Expected at least 1 prompt, got {}",
            prompt_count
        );
        assert!(
            theme_count >= 1,
            "Expected at least 1 theme, got {}",
            theme_count
        );
    }

    #[test]
    fn test_resource_counts() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let counts = mgr.resource_counts("test-pkg").unwrap();
        assert_eq!(counts.extensions, 1);
        assert_eq!(counts.skills, 1);
        assert_eq!(counts.prompts, 0);
        assert_eq!(counts.themes, 0);
    }

    #[test]
    fn test_resource_counts_display() {
        let counts = ResourceCounts {
            extensions: 2,
            skills: 1,
            prompts: 0,
            themes: 3,
        };
        assert_eq!(counts.to_string(), "2 ext, 1 skill, 3 theme");

        let empty = ResourceCounts::default();
        assert_eq!(empty.to_string(), "-");
    }

    #[test]
    fn test_resource_kind_display() {
        assert_eq!(ResourceKind::Extension.to_string(), "extension");
        assert_eq!(ResourceKind::Skill.to_string(), "skill");
        assert_eq!(ResourceKind::Prompt.to_string(), "prompt");
        assert_eq!(ResourceKind::Theme.to_string(), "theme");
    }

    #[test]
    fn test_get_install_dir() {
        let (tmp, packages_dir) = setup_temp_packages_dir();

        let pkg_dir = create_test_package(tmp.path(), "test-pkg", "1.0.0");
        let mut mgr = PackageManager::with_dir(packages_dir.clone()).unwrap();
        mgr.install(pkg_dir.to_str().unwrap()).unwrap();

        let dir = mgr.get_install_dir("test-pkg").unwrap();
        assert!(dir.exists());
        assert!(dir.join("oxi-package.toml").exists());

        assert!(mgr.get_install_dir("nonexistent").is_none());
    }

    #[test]
    fn test_discover_resources_not_installed() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mgr = PackageManager::with_dir(packages_dir).unwrap();

        let result = mgr.discover_resources("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_not_installed() {
        let (_tmp, packages_dir) = setup_temp_packages_dir();
        let mut mgr = PackageManager::with_dir(packages_dir).unwrap();

        let result = mgr.update("nonexistent");
        assert!(result.is_err());
    }
}
