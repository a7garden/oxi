//! Concrete [`HashlineFs`] backed by `tokio::fs`, `PathGuard`, and the global
//! `file_mutation_queue`.
//!
//! This is the bridge between the pure-function `oxi-hashline` crate and the
//! oxi-agent runtime. The `Patcher` calls these methods; security and
//! serialization are handled transparently.

use async_trait::async_trait;
use oxi_hashline::mismatch::HashlineError;
use oxi_hashline::patcher::HashlineFs;
use std::path::{Path, PathBuf};
use tokio::fs;

use super::file_mutation_queue::global_mutation_queue;
use super::path_security::PathGuard;

/// `tokio::fs`-backed [`HashlineFs`] with workspace-bound security and
/// per-file write serialization.
pub struct TokioHashlineFs {
    root: PathBuf,
}

impl TokioHashlineFs {
    /// Create with the given root directory (workspace root).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn validate(&self, path: &str) -> Result<PathBuf, HashlineError> {
        let guard = PathGuard::new(&self.root);
        guard
            .validate_traversal(Path::new(path))
            .map_err(|e| HashlineError::Io(std::io::Error::other(e.to_string())))
    }
}

#[async_trait]
impl HashlineFs for TokioHashlineFs {
    async fn read_text(&self, path: &str) -> Result<String, HashlineError> {
        let validated = self.validate(path)?;
        match fs::read_to_string(&validated).await {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(HashlineError::NotFound {
                path: path.to_string(),
            }),
            Err(e) => Err(HashlineError::Io(e)),
        }
    }

    async fn write_text(&self, path: &str, text: &str) -> Result<String, HashlineError> {
        let validated = self.validate(path)?;
        let text_owned = text.to_string();
        let result_path = validated.clone();
        global_mutation_queue()
            .with_queue(&validated, || async {
                fs::write(&validated, &text_owned).await
            })
            .await
            .map(|_| path.to_string())
            .map_err(|e: std::io::Error| {
                let _ = &result_path; // suppress unused warning
                HashlineError::Io(e)
            })
    }

    async fn preflight_write(&self, path: &str) -> Result<(), HashlineError> {
        let validated = self.validate(path)?;
        // Check that the parent directory exists.
        if let Some(parent) = validated.parent()
            && !parent.as_os_str().is_empty()
            && fs::metadata(parent).await.is_err()
        {
            return Err(HashlineError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Parent directory does not exist: {}", parent.display()),
            )));
        }
        Ok(())
    }

    fn canonical_path(&self, path: &str) -> String {
        let guard = PathGuard::new(&self.root);
        match guard.validate_traversal(Path::new(path)) {
            Ok(p) => {
                // Strip the root prefix to get a canonical relative path.
                match p.strip_prefix(&self.root) {
                    Ok(rel) => rel.to_string_lossy().into_owned(),
                    Err(_) => p.to_string_lossy().into_owned(),
                }
            }
            Err(_) => path.to_string(),
        }
    }
}
