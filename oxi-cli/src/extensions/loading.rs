//! Extension loading, discovery, and validation.

#![allow(unused)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::extensions::types::ExtensionError;

/// Supported shared library file extensions for the current platform.
const SHARED_LIB_EXTENSIONS: &[&str] = if cfg!(target_os = "macos") {
    &["dylib"]
} else if cfg!(target_os = "windows") {
    &["dll"]
} else {
    &["so"]
};

/// Check if a file name looks like a shared library.
fn is_shared_library(name: &str) -> bool {
    SHARED_LIB_EXTENSIONS.iter().any(|ext| name.ends_with(&format!(".{}", ext)))
}

/// Discover extension shared libraries in the default and extra paths.
pub fn discover_extensions(_cwd: &Path, _extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    vec![]
}

/// Discover extension shared libraries in a single directory.
pub fn discover_extensions_in_dir(_dir: &Path) -> Vec<PathBuf> {
    vec![]
}

/// Load a single extension from a shared library path.
pub fn load_extension(_path: &Path) -> anyhow::Result<Arc<dyn crate::extensions::Extension>> {
    anyhow::bail!("Extension loading requires full implementation")
}

/// Load multiple extensions from the given paths.
pub fn load_extensions(_paths: &[&Path]) -> (Vec<Arc<dyn crate::extensions::Extension>>, Vec<anyhow::Error>) {
    (vec![], vec![])
}

/// Extension binary validation result.
pub struct ValidatedExtension {
    /// Path to the validated extension binary.
    pub path: PathBuf,
    /// SHA-256 hex digest of the file contents.
    pub checksum: String,
}

/// Perform pre-load validation on an extension binary.
///
/// Checks file existence, size bounds, platform-appropriate extension,
/// and computes a SHA-256 checksum of the file.
pub fn validate_extension(path: &Path) -> Result<ValidatedExtension, ExtensionError> {
    // 1. File existence
    if !path.exists() {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "File not found".into(),
        });
    }

    // 2. File size bounds
    let metadata = std::fs::metadata(path).map_err(|e| ExtensionError::LoadFailed {
        name: path.display().to_string(),
        reason: format!("Cannot read file metadata: {e}"),
    })?;

    if metadata.len() == 0 {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "Empty file".into(),
        });
    }
    if metadata.len() > 100 * 1024 * 1024 {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "File too large (>100MB)".into(),
        });
    }

    // 3. Platform-appropriate file extension
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let valid_ext = match std::env::consts::OS {
        "linux" => ext == "so",
        "macos" => ext == "dylib",
        "windows" => ext == "dll",
        _ => true,
    };
    if !valid_ext {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: format!("Invalid extension: .{ext}"),
        });
    }

    // 4. SHA-256 checksum
    let data = std::fs::read(path).map_err(|e| ExtensionError::LoadFailed {
        name: path.display().to_string(),
        reason: format!("Cannot read file: {e}"),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let checksum = format!("{:x}", hasher.finalize());

    Ok(ValidatedExtension {
        path: path.to_path_buf(),
        checksum,
    })
}

/// Built-in no-op extension for testing.
pub struct NoopExtension;
impl crate::extensions::Extension for NoopExtension {
    fn name(&self) -> &str { "noop" }
    fn description(&self) -> &str { "Built-in noop extension" }
}
