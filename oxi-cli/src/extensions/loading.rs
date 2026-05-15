//! Extension dynamic loading.
//!
//! Loads Rust extensions compiled as `cdylib` shared libraries (`.dylib`/`.so`/`.dll`).
//!
//! # Extension ABI
//!
//! Every extension must export a single entry point:
//!
//! ```ignore
//! #[no_mangle]
//! pub extern "C" fn oxi_extension_create() -> *mut oxi_cli::extensions::Extension {
//!     Box::into_raw(Box::new(MyExtension))
//! }
//! ```
//!
//! # Directory layout
//!
//! ```text
//! ~/.oxi/extensions/
//!   ├── my_ext.dylib    # macOS
//!   ├── other_ext.so    # Linux
//!   └── win_ext.dll     # Windows
//! ```
//!
//! Extensions are discovered in `~/.oxi/extensions/` and any extra paths
//! configured in settings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use sha2::Digest;

use crate::extensions::Extension;
use crate::extensions::types::ExtensionError;

/// Entry point symbol that every extension must export.
const ENTRY_SYMBOL: &[u8] = b"oxi_extension_create\0";

/// Function signature for the extension creation entry point.
type CreateFn = unsafe fn() -> *mut dyn Extension;

/// Shared library extension for the current platform.
pub const SHARED_LIB_EXTENSION: &str = if cfg!(target_os = "macos") {
    "dylib"
} else if cfg!(target_os = "windows") {
    "dll"
} else {
    "so"
};

/// Check if a file looks like a shared library for the current platform.
fn is_shared_library(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e == SHARED_LIB_EXTENSION)
        .unwrap_or(false)
}

/// Discover extension shared libraries in `~/.oxi/extensions/` and extra paths.
pub fn discover_extensions(cwd: &Path, extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // ~/.oxi/extensions/
    if let Some(home) = dirs::home_dir() {
        let ext_dir = home.join(".oxi").join("extensions");
        if ext_dir.is_dir() {
            discover_in_dir(&ext_dir, &mut paths);
        }
    }

    // .oxi/extensions/ (project-local)
    let project_ext_dir = cwd.join(".oxi").join("extensions");
    if project_ext_dir.is_dir() {
        discover_in_dir(&project_ext_dir, &mut paths);
    }

    // Extra paths from settings
    for extra in extra_paths {
        if extra.is_dir() {
            discover_in_dir(extra, &mut paths);
        } else if is_shared_library(extra) && extra.exists() {
            paths.push(extra.clone());
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

/// Discover extension shared libraries in a single directory.
pub fn discover_extensions_in_dir(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    discover_in_dir(dir, &mut paths);
    paths
}

fn discover_in_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_shared_library(&path) {
            out.push(path);
        }
    }
}

/// Load a single extension from a shared library.
///
/// # Safety
///
/// The loaded library must export `oxi_extension_create` returning a valid
/// pointer to a `dyn Extension`. The library must have been compiled with
/// a compatible Rust toolchain version.
pub fn load_extension(path: &Path) -> anyhow::Result<Arc<dyn Extension>> {
    let path_display = path.display().to_string();

    if !path.exists() {
        anyhow::bail!("Extension file not found: {}", path_display);
    }

    if !is_shared_library(path) {
        anyhow::bail!(
            "Not a shared library (expected .{}): {}",
            SHARED_LIB_EXTENSION,
            path_display
        );
    }

    // Load the library
    let library = unsafe { Library::new(path) }
        .map_err(|e| anyhow::anyhow!("Failed to load library '{}': {}", path_display, e))?;

    // Get the entry point symbol
    let create: libloading::Symbol<CreateFn> = unsafe { library.get(ENTRY_SYMBOL) }
        .map_err(|e| anyhow::anyhow!(
            "Symbol 'oxi_extension_create' not found in '{}': {}",
            path_display, e
        ))?;

    // Call the entry point
    let raw_ptr = unsafe { create() };
    if raw_ptr.is_null() {
        anyhow::bail!("oxi_extension_create returned null in '{}'", path_display);
    }

    // Wrap in Arc (take ownership from the raw pointer)
    let extension: Arc<dyn Extension> = unsafe {
        let boxed: Box<dyn Extension> = Box::from_raw(raw_ptr);
        Arc::from(boxed)
    };

    tracing::info!(
        name = %extension.name(),
        path = %path_display,
        "Extension loaded"
    );

    // IMPORTANT: We must keep the Library alive for the entire lifetime
    // of the extension. Leak it intentionally — the extension's code lives
    // in this library. Unloading it while extension objects exist would
    // cause undefined behavior.
    std::mem::forget(library);

    Ok(extension)
}

/// Load multiple extensions from the given paths.
///
/// Returns successfully loaded extensions and any errors encountered.
/// Does not abort on individual failures — loads as many as possible.
pub fn load_extensions(paths: &[&Path]) -> (Vec<Arc<dyn Extension>>, Vec<anyhow::Error>) {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();

    for path in paths {
        match load_extension(path) {
            Ok(ext) => loaded.push(ext),
            Err(e) => {
                tracing::warn!("Failed to load extension '{}': {}", path.display(), e);
                errors.push(e);
            }
        }
    }

    (loaded, errors)
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
/// Checks file existence, size bounds, and platform-appropriate extension.
pub fn validate_extension(path: &Path) -> Result<ValidatedExtension, ExtensionError> {
    if !path.exists() {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "File not found".into(),
        });
    }

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

    let data = std::fs::read(path).map_err(|e| ExtensionError::LoadFailed {
        name: path.display().to_string(),
        reason: format!("Cannot read file: {e}"),
    })?;
    let checksum = format!("{:x}", sha2::Sha256::digest(&data));

    Ok(ValidatedExtension {
        path: path.to_path_buf(),
        checksum,
    })
}
