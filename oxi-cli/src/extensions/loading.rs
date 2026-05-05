//! Extension loading and discovery.
//!
//! Functions for loading extensions from shared libraries and discovering
//! them in standard locations.

use crate::extensions::types::Extension;
use crate::extensions::Extension as ExtensionTrait;
use crate::extensions::NoopExtension;
use anyhow::{bail, Context, Result};
use libloading::{Library, Symbol};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ═══════════════════════════════════════════════════════════════════════════
// Extension Discovery
// ═══════════════════════════════════════════════════════════════════════════

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
    SHARED_LIB_EXTENSIONS
        .iter()
        .any(|ext| name.ends_with(&format!(".{}", ext)))
}

/// Discover extension shared libraries in a directory.
///
/// Scans one level deep:
/// - Direct files: `extensions/*.so` (or `.dylib` / `.dll`) → load
/// - Subdirectory: `extensions/*/index.so` → load
///
/// No recursion beyond one level. Returns discovered paths.
pub fn discover_extensions_in_dir(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }

    let mut discovered = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if path.is_dir() {
            // Check for index.so / index.dylib / index.dll in subdirectory
            for ext in SHARED_LIB_EXTENSIONS {
                let index_path = path.join(format!("index.{}", ext));
                if index_path.exists() {
                    discovered.push(index_path);
                    break;
                }
            }
        } else if is_shared_library(file_name) {
            discovered.push(path);
        }
    }

    discovered
}

/// Discover extensions from standard locations.
///
/// Checks:
/// 1. Project-local extensions: `cwd/.oxi/extensions/`
/// 2. Global extensions: `~/.oxi/extensions/`
/// 3. Explicitly configured paths
///
/// Deduplicates resolved paths.
pub fn discover_extensions(
    cwd: &Path,
    configured_paths: &[PathBuf],
) -> Vec<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut all_paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let add_paths = |paths: &mut Vec<PathBuf>,
                     seen: &mut std::collections::HashSet<u64>,
                     new: Vec<PathBuf>| {
        for p in new {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            p.hash(&mut hasher);
            let hash = hasher.finish();
            if seen.insert(hash) {
                paths.push(p);
            }
        }
    };

    // 1. Project-local extensions
    let local_ext_dir = cwd.join(".oxi").join("extensions");
    add_paths(
        &mut all_paths,
        &mut seen,
        discover_extensions_in_dir(&local_ext_dir),
    );

    // 2. Global extensions
    if let Some(home) = dirs::home_dir() {
        let global_ext_dir = home.join(".oxi").join("extensions");
        add_paths(
            &mut all_paths,
            &mut seen,
            discover_extensions_in_dir(&global_ext_dir),
        );
    }

    // 3. Explicitly configured paths
    for p in configured_paths {
        let resolved = if p.is_absolute() {
            p.clone()
        } else {
            cwd.join(p)
        };

        if resolved.is_dir() {
            // Discover in directory
            add_paths(
                &mut all_paths,
                &mut seen,
                discover_extensions_in_dir(&resolved),
            );
        } else if resolved.exists() {
            // Direct file
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            resolved.hash(&mut hasher);
            let hash = hasher.finish();
            if seen.insert(hash) {
                all_paths.push(resolved);
            }
        }
    }

    all_paths
}

// ═══════════════════════════════════════════════════════════════════════════
// Dynamic Loading
// ═══════════════════════════════════════════════════════════════════════════

/// Expected symbol name inside a shared-library extension.
const ENTRY_SYMBOL: &[u8] = b"oxi_extension_create\0";

/// Function signature that a shared library must export.
///
/// The library must expose:
///
/// ```c,ignore
/// extern "C" fn oxi_extension_create() -> *mut dyn Extension
/// ```
type CreateFn = unsafe fn() -> *mut dyn crate::extensions::Extension;

/// Load an extension from a shared library (.so / .dll / .dylib).
///
/// The library **must** export an `oxi_extension_create` entry-point that
/// returns a heap-allocated trait object.
pub fn load_extension(path: &Path) -> Result<Arc<dyn Extension>> {
    let extension = load_extension_inner(path)?;
    Ok(extension)
}

fn load_extension_inner(path: &Path) -> Result<Arc<dyn Extension>> {
    // Validate file extension
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");

    let valid = matches!(ext, "so" | "dylib" | "dll");
    if !valid {
        bail!(
            "Unsupported extension file format: .{}. Expected .so, .dylib, or .dll",
            ext
        );
    }

    if !path.exists() {
        bail!("Extension file not found: {}", path.display());
    }

    // Safety: loading a shared library is inherently unsafe. We trust the
    // user-provided library to be well-behaved.
    let library = unsafe {
        Library::new(path).with_context(|| format!("Failed to load library: {}", path.display()))?
    };

    let create: Symbol<CreateFn> = unsafe {
        library.get(ENTRY_SYMBOL).with_context(|| {
            format!(
                "Symbol `oxi_extension_create` not found in {}",
                path.display()
            )
        })?
    };

    let raw_ptr = unsafe { create() };
    if raw_ptr.is_null() {
        bail!("oxi_extension_create returned null in {}", path.display());
    }

    // Wrap the raw pointer in an Arc directly via Box
    let boxed: Box<dyn Extension> = unsafe { Box::from_raw(raw_ptr) };
    Ok(Arc::from(boxed))
}

/// Load multiple extensions from file paths, collecting errors.
pub fn load_extensions(paths: &[&Path]) -> (Vec<Arc<dyn Extension>>, Vec<anyhow::Error>) {
    let mut loaded = Vec::with_capacity(paths.len());
    let mut errors = Vec::new();

    for &path in paths {
        match load_extension(path) {
            Ok(ext) => loaded.push(ext),
            Err(e) => {
                errors.push(e.context(format!("Failed to load extension: {}", path.display())))
            }
        }
    }

    (loaded, errors)
}