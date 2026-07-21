//! `local://` protocol handler — session-scoped artifact storage.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use oxi_sdk::SdkError;
use oxi_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};

/// Protocol handler for `local://` URLs backed by a session-scoped root.
pub struct LocalProtocolHandler {
    root: PathBuf,
}

impl LocalProtocolHandler {
    /// Create a new handler rooted at the given directory.
    /// The directory is created if it does not exist.
    pub fn new(root: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    fn resolve_path(&self, relative: &str) -> Result<PathBuf, SdkError> {
        if relative.starts_with('/') {
            return Err(SdkError::ExecutionFailed {
                reason: "Absolute paths are not allowed in local:// URLs".into(),
            });
        }
        let target = self.root.join(Path::new(relative));

        let canon_root = self
            .root
            .canonicalize()
            .map_err(|e| SdkError::ExecutionFailed {
                reason: format!("Failed to canonicalize local root: {e}"),
            })?;
        let canon_parent = target
            .parent()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
            .unwrap_or(canon_root.clone());

        if !canon_parent.starts_with(&canon_root) {
            return Err(SdkError::ExecutionFailed {
                reason: "Path traversal is not allowed in local:// URLs".into(),
            });
        }
        Ok(target)
    }
}

#[async_trait]
impl ProtocolHandler for LocalProtocolHandler {
    fn scheme(&self) -> &str {
        "local"
    }
    fn immutable(&self) -> bool {
        false
    }

    async fn resolve(
        &self,
        url: &str,
        _selector: Option<&str>,
        _ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError> {
        let relative = url.strip_prefix("local://").unwrap_or(url);

        if relative.is_empty() {
            let mut entries = Vec::new();
            let mut reader =
                tokio::fs::read_dir(&self.root)
                    .await
                    .map_err(|e| SdkError::ExecutionFailed {
                        reason: format!("Failed to read local root: {e}"),
                    })?;
            while let Some(entry) =
                reader
                    .next_entry()
                    .await
                    .map_err(|e| SdkError::ExecutionFailed {
                        reason: format!("Failed to read entry: {e}"),
                    })?
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                entries.push(if is_dir { format!("{name}/") } else { name });
            }
            entries.sort();
            return Ok(ResolvedUrl {
                url: "local://".into(),
                content: if entries.is_empty() {
                    "(empty)".into()
                } else {
                    entries.join("\n")
                },
                content_type: "text/plain".into(),
                size: None,
                source_path: Some(self.root.to_string_lossy().into_owned()),
                notes: vec![],
                immutable: false,
            });
        }

        let target = self.resolve_path(relative)?;
        let metadata =
            tokio::fs::metadata(&target)
                .await
                .map_err(|e| SdkError::ExecutionFailed {
                    reason: if e.kind() == std::io::ErrorKind::NotFound {
                        format!("local://{relative} not found")
                    } else {
                        format!("Cannot access local://{relative}: {e}")
                    },
                })?;

        if metadata.is_dir() {
            let mut entries = Vec::new();
            let mut reader =
                tokio::fs::read_dir(&target)
                    .await
                    .map_err(|e| SdkError::ExecutionFailed {
                        reason: format!("Failed to read directory: {e}"),
                    })?;
            while let Some(entry) =
                reader
                    .next_entry()
                    .await
                    .map_err(|e| SdkError::ExecutionFailed {
                        reason: format!("Failed to read entry: {e}"),
                    })?
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                entries.push(if is_dir { format!("{name}/") } else { name });
            }
            entries.sort();
            return Ok(ResolvedUrl {
                url: format!("local://{relative}"),
                content: entries.join("\n"),
                content_type: "text/plain".into(),
                size: None,
                source_path: Some(target.to_string_lossy().into_owned()),
                notes: vec![],
                immutable: false,
            });
        }

        let content =
            tokio::fs::read_to_string(&target)
                .await
                .map_err(|e| SdkError::ExecutionFailed {
                    reason: format!("Failed to read local://{relative}: {e}"),
                })?;

        let size = content.len();
        let content_type = if relative.ends_with(".md") {
            "text/markdown"
        } else if relative.ends_with(".json") {
            "application/json"
        } else {
            "text/plain"
        };

        Ok(ResolvedUrl {
            url: format!("local://{relative}"),
            content,
            content_type: content_type.into(),
            size: Some(size),
            source_path: Some(target.to_string_lossy().into_owned()),
            notes: vec![],
            immutable: false,
        })
    }
}
