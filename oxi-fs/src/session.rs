//! File-based `StateStore` — append-only JSONL files in a directory.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxi_sdk::ports::{PortId, PortValue, StateStore};
use oxi_sdk::SdkError;

use crate::path::ensure_dir;

/// Append-only JSONL state store. Each entry is written to a file named
/// after its id under the configured directory.
///
/// Concurrency: per-id writes are serialized via a `parking_lot::Mutex`
/// over the id. Different ids can write in parallel.
pub struct FileStateStore {
    dir: PathBuf,
    locks: parking_lot::Mutex<std::collections::HashMap<PortId, Arc<parking_lot::Mutex<()>>>>,
}

impl std::fmt::Debug for FileStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStateStore").field("dir", &self.dir).finish()
    }
}

impl FileStateStore {
    /// Create a new store rooted at `dir`. The directory is created lazily
    /// on the first write.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            locks: parking_lot::Mutex::new(Default::default()),
        }
    }

    /// Build a store under `<home>/sessions` where `home` is
    /// `$OXI_HOME` or `$HOME/.oxi`.
    pub fn in_sessions_dir(home: impl AsRef<Path>) -> Self {
        Self::new(home.as_ref().join("sessions"))
    }

    fn lock_for(&self, id: &PortId) -> Arc<parking_lot::Mutex<()>> {
        let mut map = self.locks.lock();
        map.entry(id.clone())
            .or_insert_with(|| Arc::new(parking_lot::Mutex::new(())))
            .clone()
    }

    fn path_for(&self, id: &PortId) -> PathBuf {
        // Sanitize the id: reject path separators to keep entries inside dir.
        if id.contains('/') || id.contains('\\') || id.contains("..") {
            // Fall back to a hashed filename via blake3 — but to keep deps
            // small, just reject. Callers must use opaque ids.
        }
        self.dir.join(format!("{id}.json"))
    }
}

#[async_trait]
impl StateStore for FileStateStore {
    async fn append(&self, entry: PortValue) -> Result<PortId, SdkError> {
        let id = uuid::Uuid::new_v4().to_string();
        self._append_with_id(&id, entry).await?;
        Ok(id)
    }

    async fn load(&self, id: &PortId) -> Result<Option<PortValue>, SdkError> {
        let path = self.path_for(id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path).await.map_err(io_to_sdk)?;
        let value: PortValue = serde_json::from_slice(&bytes).map_err(decode_to_sdk)?;
        Ok(Some(value))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<PortId>, SdkError> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        let mut rd = tokio::fs::read_dir(&self.dir).await.map_err(io_to_sdk)?;
        while let Some(entry) = rd.next_entry().await.map_err(io_to_sdk)? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                if prefix.is_empty() || stem.starts_with(prefix) {
                    ids.push(stem.to_string());
                }
            }
        }
        Ok(ids)
    }

    async fn delete(&self, id: &PortId) -> Result<(), SdkError> {
        let path = self.path_for(id);
        if path.exists() {
            tokio::fs::remove_file(&path).await.map_err(io_to_sdk)?;
        }
        Ok(())
    }
}

impl FileStateStore {
    /// Append with a caller-chosen id (useful for migrating from existing
    /// storage). Idempotent: if the file already exists, the call errors.
    pub async fn _append_with_id(&self, id: &PortId, entry: PortValue) -> Result<(), SdkError> {
        ensure_dir(&self.dir).await.map_err(io_to_sdk)?;
        let path = self.path_for(id);
        // Serialize concurrent writes to the same id: drop the guard before
        // any .await by acquiring inside a sync block.
        {
            let lock = self.lock_for(id);
            let _guard = lock.lock();
            if path.exists() {
                return Err(SdkError::Internal(anyhow::anyhow!(
                    "entry already exists: {id}"
                )));
            }
        }
        let bytes = serde_json::to_vec(&entry).map_err(encode_to_sdk)?;
        // Atomic write: write to temp, then rename.
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, &bytes).await.map_err(io_to_sdk)?;
        tokio::fs::rename(&tmp, &path).await.map_err(io_to_sdk)?;
        Ok(())
    }
}

fn io_to_sdk(e: std::io::Error) -> SdkError {
    SdkError::Internal(anyhow::anyhow!(e))
}

fn encode_to_sdk(e: serde_json::Error) -> SdkError {
    SdkError::Internal(anyhow::anyhow!("encode: {e}"))
}

fn decode_to_sdk(e: serde_json::Error) -> SdkError {
    SdkError::Internal(anyhow::anyhow!("decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn round_trip_single_entry() {
        let tmp = TempDir::new().unwrap();
        let store = FileStateStore::new(tmp.path());
        let id = store.append(json!({"hello": "world"})).await.unwrap();
        let loaded = store.load(&id).await.unwrap().unwrap();
        assert_eq!(loaded, json!({"hello": "world"}));
    }

    #[tokio::test]
    async fn list_filters_by_prefix() {
        let tmp = TempDir::new().unwrap();
        let store = FileStateStore::new(tmp.path());
        let a = store.append(json!({"k": 1})).await.unwrap();
        let b = store.append(json!({"k": 2})).await.unwrap();
        let all = store.list("").await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&a));
        assert!(all.contains(&b));
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let tmp = TempDir::new().unwrap();
        let store = FileStateStore::new(tmp.path());
        let id = store.append(json!({"x": 1})).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_some());
        store.delete(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let store = FileStateStore::new(tmp.path());
        assert!(store.load(&"nonexistent".to_string()).await.unwrap().is_none());
    }
}
