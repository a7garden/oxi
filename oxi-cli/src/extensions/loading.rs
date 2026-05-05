//! Extension loading and discovery (stub).

#![allow(unused)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// TODO: document.
pub fn discover_extensions(_cwd: &Path, _extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    vec![]
}

/// TODO: document.
pub fn discover_extensions_in_dir(_dir: &Path) -> Vec<PathBuf> {
    vec![]
}

/// TODO: document.
pub fn load_extension(_path: &Path) -> anyhow::Result<Arc<dyn crate::extensions::Extension>> {
    anyhow::bail!("Extension loading requires full implementation")
}

/// TODO: document.
pub fn load_extensions(_paths: &[&Path]) -> (Vec<Arc<dyn crate::extensions::Extension>>, Vec<anyhow::Error>) {
    (vec![], vec![])
}

// Built-in noop extension
/// pub.
pub struct NoopExtension;
impl crate::extensions::Extension for NoopExtension {
    fn name(&self) -> &str { "noop" }
    fn description(&self) -> &str { "Built-in noop extension" }
}