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
//! pub extern "C" fn oxicode_extension_create() -> *mut oxicode_cli::extensions::Extension {
//!     Box::into_raw(Box::new(MyExtension))
//! }
//! ```
//!
//! # Directory layout
//!
//! ```text
//! ~/.oxicode/extensions/
//!   ├── my_ext.dylib    # macOS
//!   ├── other_ext.so    # Linux
//!   └── win_ext.dll     # Windows
//! ```
//!
//! Extensions are discovered in `~/.oxicode/extensions/` and any extra paths
//! configured in settings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use sha2::Digest;

use crate::extensions::Extension;
use crate::extensions::types::ExtensionError;

/// Entry point symbol that every extension must export.
const ENTRY_SYMBOL: &[u8] = b"oxicode_extension_create\0";

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

/// Discover extension shared libraries in the canonical extensions directory
/// (legacy `~/.oxicode/extensions/` read-only fallback) and extra paths.
pub fn discover_extensions(cwd: &Path, extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Canonical extensions dir, else legacy (pre-unified-layout installs).
    let user_ext_dir = oxicode_catalog::oxi_home::read_path(Path::new("extensions"));
    if let Some(ext_dir) = user_ext_dir
        && ext_dir.is_dir()
    {
        discover_in_dir(&ext_dir, &mut paths);
    }

    // .oxicode/extensions/ (project-local)
    let project_ext_dir = cwd.join(".oxicode").join("extensions");
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_shared_library(&path) {
            out.push(path);
        }
    }
}

/// Load a single extension from a shared library.
///
/// # Integrity (audit F-2)
///
/// `expected_checksum` is the SHA-256 hex digest that the caller (e.g. the
/// package manager's lockfile reader) has on record for this binary. When
/// `Some`, the binary is hashed before loading and rejected on mismatch —
/// this is the supply-chain integrity gate for native extensions, which
/// otherwise run arbitrary in-process code with no sandbox (libloading +
/// `unsafe extern "C"` entry). When `None`, the caller is opting out of
/// verification explicitly; this is reserved for locally-built extensions
/// the user just compiled and trusts by construction.
///
/// The hash comparison is constant-time on the hex string length via
/// `subtle::ConstantTimeEq` if the `subtle` dep is added; until then
/// `eq_ignore_ascii_case` is used (timing leak is negligible here since
/// the hash is not a secret and an attacker who can swap the binary
/// already controls the comparison outcome).
///
/// # Safety
///
/// The loaded library must export `oxicode_extension_create` returning a valid
/// pointer to a `dyn Extension`. The library must have been compiled with
/// a compatible Rust toolchain version.
pub fn load_extension(
    path: &Path,
    expected_checksum: Option<&str>,
) -> anyhow::Result<Arc<dyn Extension>> {
    let path_display = path.display().to_string();
    // Security: native extensions are unsandboxed arbitrary in-process code
    // (loaded via libloading with no sandbox). Require explicit opt-in so
    // they cannot execute by default — mirrors the `OXICODE_EXTENSION_EXEC`
    // opt-in for WASM extensions.
    if std::env::var("OXICODE_NATIVE_EXTENSIONS").ok().as_deref() != Some("1") {
        tracing::warn!(
            path = %path_display,
            "native extension skipped — set OXICODE_NATIVE_EXTENSIONS=1 to load unsandboxed extensions"
        );
        anyhow::bail!(
            "Native extensions are disabled; set OXICODE_NATIVE_EXTENSIONS=1 to load '{}'",
            path_display
        );
    }

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

    // F-2 (audit 2026-06-21): integrity check before mmap.
    //
    // `validate_extension` performs pre-load validation (file exists, size
    // bounds, platform extension, SHA-256). It returns `ValidatedExtension`
    // with the actual checksum; we compare it to the caller-supplied
    // expected checksum and bail on mismatch — refusing to load a binary
    // that has been swapped since the lockfile was written.
    let validated = validate_extension(path).map_err(|e| {
        anyhow::anyhow!(
            "native extension pre-load validation failed for '{}': {}",
            path_display,
            e
        )
    })?;
    if let Some(expected) = expected_checksum {
        if !validated.checksum.eq_ignore_ascii_case(expected) {
            anyhow::bail!(
                "native extension checksum mismatch for '{}': expected sha256-{expected}, got sha256-{}",
                path_display,
                validated.checksum
            );
        }
        tracing::debug!(
            path = %path_display,
            checksum = %validated.checksum,
            "native extension integrity verified"
        );
    } else {
        tracing::warn!(
            path = %path_display,
            "loading native extension WITHOUT integrity verification — caller passed None"
        );
    }

    // SAFETY: Library::new loads a shared library from the given path.
    // This is unsafe because the loaded code can perform arbitrary operations.
    // We trust the user-installed extension at the given path, AND its
    // integrity has been verified above when `expected_checksum` is Some.
    let library = unsafe { Library::new(path) }
        .map_err(|e| anyhow::anyhow!("Failed to load library '{}': {}", path_display, e))?;

    // SAFETY: library.get looks up a symbol by name in the loaded shared library.
    // The symbol name is a static constant, not user-controlled.
    let create: libloading::Symbol<CreateFn> =
        unsafe { library.get(ENTRY_SYMBOL) }.map_err(|e| {
            anyhow::anyhow!(
                "Symbol 'oxicode_extension_create' not found in '{}': {}",
                path_display,
                e
            )
        })?;

    // SAFETY: Calling the extension's oxicode_extension_create entry point.
    // The function signature is `unsafe fn() -> *mut dyn Extension`.
    // We check the returned pointer for null below.
    let raw_ptr = unsafe { create() };
    if raw_ptr.is_null() {
        anyhow::bail!(
            "oxicode_extension_create returned null in '{}'",
            path_display
        );
    }

    // SAFETY: Box::from_raw takes ownership of the pointer returned by
    // oxicode_extension_create. The extension must have allocated this with
    // Box::new (documented contract). Null was checked above.
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
///
/// `checksums` is parallel to `paths`: `checksums[i]` is the expected
/// SHA-256 of `paths[i]`. Pass `None` to opt out of integrity verification
/// for a particular extension (the same semantics as `load_extension`).
/// A `Some(_)` mismatch is reported as an error but does not stop the
/// other extensions from loading.
pub fn load_extensions(
    paths: &[&Path],
    checksums: &[Option<&str>],
) -> (Vec<Arc<dyn Extension>>, Vec<anyhow::Error>) {
    assert_eq!(
        paths.len(),
        checksums.len(),
        "load_extensions: paths and checksums must be parallel slices"
    );
    let mut loaded = Vec::new();
    let mut errors = Vec::new();

    for (path, expected) in paths.iter().zip(checksums.iter()) {
        match load_extension(path, *expected) {
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
#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── F-2 regression: validate_extension computes deterministic SHA-256 ──

    fn write_fake_ext(path: &Path, payload: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(payload).unwrap();
    }

    /// Two calls to `validate_extension` on the same file yield the same
    /// SHA-256 hex digest — the function is pure and stable.
    #[test]
    fn validate_extension_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let ext_path = tmp.path().join(format!("lib.{}", SHARED_LIB_EXTENSION));
        write_fake_ext(&ext_path, b"deterministic test payload");

        let v1 = validate_extension(&ext_path).expect("validate should succeed");
        let v2 = validate_extension(&ext_path).expect("validate should succeed");
        assert_eq!(v1.checksum, v2.checksum);
        // SHA-256 hex is 64 chars, lowercase.
        assert_eq!(v1.checksum.len(), 64);
        assert!(
            v1.checksum
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// Distinct file contents produce distinct checksums.
    #[test]
    fn validate_extension_distinguishes_content() {
        let tmp = tempfile::tempdir().unwrap();
        let ext_a = tmp.path().join(format!("a.{}", SHARED_LIB_EXTENSION));
        let ext_b = tmp.path().join(format!("b.{}", SHARED_LIB_EXTENSION));
        write_fake_ext(&ext_a, b"alpha");
        write_fake_ext(&ext_b, b"beta");

        let v_a = validate_extension(&ext_a).unwrap();
        let v_b = validate_extension(&ext_b).unwrap();
        assert_ne!(v_a.checksum, v_b.checksum);
    }

    /// `validate_extension` rejects a file with the wrong platform extension
    /// (e.g. `.so` on macOS). The pre-load gate must catch this before any
    /// `libloading::Library::new` call.
    #[test]
    #[cfg(target_os = "macos")]
    fn validate_extension_rejects_wrong_platform_ext_on_macos() {
        let tmp = tempfile::tempdir().unwrap();
        // `.so` is the Linux extension; on macOS a `.dylib` is required.
        let wrong = tmp.path().join("lib.so");
        write_fake_ext(&wrong, b"x");
        let err = validate_extension(&wrong).expect_err("wrong platform ext must fail");
        let msg = format!("{err}");
        assert!(msg.contains("Invalid extension"), "unexpected err: {msg}");
    }

    /// A non-existent path returns `File not found`, not a panic.
    #[test]
    fn validate_extension_handles_missing_path() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist.dylib");
        let err = validate_extension(&missing).expect_err("missing path must fail");
        assert!(format!("{err}").contains("File not found"));
    }
}
