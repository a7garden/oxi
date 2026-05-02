//! Package system for oxi CLI
//!
//! Packages bundle extensions, skills, prompts, and themes for sharing.
//! Supports local directories and npm packages.

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
                        tracing::warn!("Failed to load manifest {}: {}", manifest_path.display(), e);
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
        fs::create_dir_all(&self.packages_dir)
            .with_context(|| format!("Failed to create packages directory {}", self.packages_dir.display()))?;

        // Remove previous installation if it exists
        if dest.exists() {
            fs::remove_dir_all(&dest)
                .with_context(|| format!("Failed to remove existing package at {}", dest.display()))?;
        }

        // Copy the entire source directory
        copy_dir_recursive(source_path, &dest)
            .with_context(|| format!("Failed to copy package from {} to {}", source, dest.display()))?;

        self.installed.insert(manifest.name.clone(), manifest.clone());
        Ok(manifest)
    }

    /// Install a package from npm
    pub fn install_npm(&mut self, name: &str) -> Result<PackageManifest> {
        // npm pack the package to a temp directory
        let tmp_dir = tempfile::tempdir()
            .context("Failed to create temp directory for npm install")?;

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
        fs::create_dir_all(&self.packages_dir)
            .with_context(|| format!("Failed to create packages directory {}", self.packages_dir.display()))?;

        let safe_name = name.replace('@', "").replace('/', "-");
        let dest = self.packages_dir.join(safe_name);

        if dest.exists() {
            fs::remove_dir_all(&dest)
                .with_context(|| format!("Failed to remove existing package at {}", dest.display()))?;
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

        self.installed.insert(manifest.name.clone(), manifest.clone());
        Ok(manifest)
    }

    /// Uninstall a package by name
    pub fn uninstall(&mut self, name: &str) -> Result<()> {
        if !self.installed.contains_key(name) {
            anyhow::bail!("Package '{}' is not installed", name);
        }

        let dest = self.pkg_install_dir(name);
        if dest.exists() {
            fs::remove_dir_all(&dest)
                .with_context(|| format!("Failed to remove package directory {}", dest.display()))?;
        }

        self.installed.remove(name);
        Ok(())
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
}
