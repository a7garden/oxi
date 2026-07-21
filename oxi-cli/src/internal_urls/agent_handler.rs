//! `agent://` protocol handler — resolves subagent output artifacts.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use oxi_sdk::SdkError;
use oxi_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};
use parking_lot::RwLock;

/// Stores per-session agent output artifacts for `agent://` URL resolution.
#[derive(Clone, Default)]
pub struct AgentArtifactStore {
    dirs: Arc<RwLock<Vec<PathBuf>>>,
}

impl AgentArtifactStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a session's artifacts directory.
    pub fn register_dir(&self, dir: PathBuf) {
        self.dirs.write().push(dir);
    }

    fn find_output(&self, candidate_ids: &[String]) -> Option<(PathBuf, String)> {
        let dirs = self.dirs.read();
        for dir in dirs.iter() {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();
                let Some(id) = name.strip_suffix(".md") else {
                    continue;
                };
                for candidate in candidate_ids {
                    if id == *candidate
                        && let Ok(content) = std::fs::read_to_string(entry.path())
                    {
                        return Some((entry.path(), content));
                    }
                }
            }
        }
        None
    }
}

/// Protocol handler for `agent://` URLs.
pub struct AgentProtocolHandler {
    store: AgentArtifactStore,
}

impl AgentProtocolHandler {
    /// Create a new handler backed by the given artifact store.
    pub fn new(store: AgentArtifactStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ProtocolHandler for AgentProtocolHandler {
    fn scheme(&self) -> &str {
        "agent"
    }
    fn immutable(&self) -> bool {
        true
    }

    async fn resolve(
        &self,
        url: &str,
        _selector: Option<&str>,
        _ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError> {
        let suffix = url.strip_prefix("agent://").unwrap_or(url);
        if suffix.is_empty() {
            return Err(SdkError::ExecutionFailed {
                reason: "agent:// URL requires an agent output ID".into(),
            });
        }

        let parts: Vec<&str> = suffix.split('/').collect();
        let dotted = parts.join(".");
        let candidates = vec![suffix.to_string(), dotted];

        match self.store.find_output(&candidates) {
            Some((path, content)) => Ok(ResolvedUrl {
                url: format!("agent://{suffix}"),
                content,
                content_type: "text/markdown".into(),
                size: None,
                source_path: Some(path.to_string_lossy().into_owned()),
                notes: vec![],
                immutable: true,
            }),
            None => Err(SdkError::ExecutionFailed {
                reason: format!(
                    "Agent output '{suffix}' not found in any registered artifacts directory"
                ),
            }),
        }
    }
}
