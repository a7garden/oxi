//! Agent snapshotting for suspend/resume persistence.

use crate::lifecycle::MetricsSnapshot;
use async_trait::async_trait;
use oxi_agent::{AgentConfig, AgentState, ToolRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A complete snapshot of an agent at a point in time.
///
/// Can be serialized to JSON and stored to disk for suspend/resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Unique agent identifier.
    pub agent_id: String,
    /// Configuration at time of snapshot.
    pub config: AgentConfig,
    /// Conversation state.
    pub state: AgentState,
    /// Tool manifest (names/schemas; closures cannot be serialized).
    pub tool_manifest: ToolManifest,
    /// Parent agent ID, if this agent was spawned as a child.
    pub parent_id: Option<String>,
    /// Wall-clock time when the agent was created (ms since epoch).
    pub created_at_ms: u64,
    /// Wall-clock time when the snapshot was taken (ms since epoch).
    pub snapshot_at_ms: u64,
    /// Metrics at time of snapshot.
    pub metrics: MetricsSnapshot,
    /// Arbitrary extension metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentSnapshot {
    /// Create a snapshot from a running agent.
    pub fn from_agent(
        agent_id: String,
        config: &AgentConfig,
        state: &AgentState,
        tools: &ToolRegistry,
        parent_id: Option<String>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        let now = now_ms();
        Self {
            agent_id,
            config: config.clone(),
            state: state.clone(),
            tool_manifest: ToolManifest::from_registry(tools),
            parent_id,
            created_at_ms: now,
            snapshot_at_ms: now,
            metrics: MetricsSnapshot::default(),
            metadata,
        }
    }

    /// Serialize to a byte vector.
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Deserialize from a byte slice.
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Estimate serialized size.
    pub fn estimated_size_bytes(&self) -> usize {
        serde_json::to_vec(self).map(|b| b.len()).unwrap_or(0)
    }
}

// ── ToolManifest ────────────────────────────────────────────────────

/// A serializable manifest of tools registered in an agent.
///
/// Closures cannot be serialized, so only names, descriptions, and essential
/// flags are captured. Restoring re-registers them by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    /// Tool entries.
    pub tools: Vec<ToolManifestEntry>,
}

impl ToolManifest {
    /// Build a manifest from an agent's tool registry.
    pub fn from_registry(registry: &ToolRegistry) -> Self {
        let tools = registry
            .definitions()
            .into_iter()
            .map(|d| ToolManifestEntry {
                name: d.name,
                description: d.description,
                essential: false,
            })
            .collect();
        Self { tools }
    }

    /// Determine which tools from this manifest are NOT present in `registry`.
    pub fn missing_from(&self, registry: &ToolRegistry) -> Vec<&str> {
        let names: std::collections::HashSet<_> = registry.names().into_iter().collect();
        self.tools
            .iter()
            .filter(|t| !names.contains(&t.name))
            .map(|t| t.name.as_str())
            .collect()
    }
}

/// A single tool's serializable metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifestEntry {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Whether the tool is marked essential.
    #[serde(default)]
    pub essential: bool,
}

// ── SnapshotStore trait ─────────────────────────────────────────────

/// Trait for persisting and retrieving agent snapshots.
///
/// Implementations may store to filesystem, database, or network.
/// Uses `async_trait` per project conventions.
#[async_trait]
pub trait SnapshotStore: Send + Sync {
    /// Persist a snapshot.
    async fn save(&self, snapshot: &AgentSnapshot) -> anyhow::Result<()>;

    /// Retrieve a snapshot by agent ID.
    async fn load(&self, agent_id: &str) -> anyhow::Result<Option<AgentSnapshot>>;

    /// List all agent IDs with stored snapshots.
    async fn list(&self) -> anyhow::Result<Vec<String>>;

    /// Delete a snapshot by agent ID.
    async fn delete(&self, agent_id: &str) -> anyhow::Result<()>;
}

// ── FileSnapshotStore ──────────────────────────────────────────────

/// Snapshot store backed by the local filesystem.
///
/// Stores each snapshot as `{base_dir}/{agent_id}.json`.
#[derive(Debug)]
pub struct FileSnapshotStore {
    base_dir: PathBuf,
}

impl FileSnapshotStore {
    /// Create a new store rooted at `base_dir`.
    ///
    /// The directory is created if it does not exist.
    pub fn new(base_dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let base_dir = base_dir.into();
        std::fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    fn snapshot_path(&self, agent_id: &str) -> PathBuf {
        self.base_dir.join(format!("{agent_id}.json"))
    }
}

#[async_trait]
impl SnapshotStore for FileSnapshotStore {
    async fn save(&self, snapshot: &AgentSnapshot) -> anyhow::Result<()> {
        let path = self.snapshot_path(&snapshot.agent_id);
        let bytes = serde_json::to_vec_pretty(snapshot)?;
        tokio::fs::write(&path, bytes).await?;
        Ok(())
    }

    async fn load(&self, agent_id: &str) -> anyhow::Result<Option<AgentSnapshot>> {
        let path = self.snapshot_path(agent_id);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path).await?;
        let snapshot: AgentSnapshot = serde_json::from_slice(&bytes)?;
        Ok(Some(snapshot))
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.base_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            if entry.path().extension().is_some_and(|e| e == "json") {
                if let Some(name) = entry.path().file_stem() {
                    entries.push(name.to_string_lossy().to_string());
                }
            }
        }
        Ok(entries)
    }

    async fn delete(&self, agent_id: &str) -> anyhow::Result<()> {
        let path = self.snapshot_path(agent_id);
        if path.is_file() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_snapshot() -> AgentSnapshot {
        AgentSnapshot {
            agent_id: "test-agent".into(),
            config: AgentConfig::default(),
            state: AgentState::default(),
            tool_manifest: ToolManifest { tools: vec![] },
            parent_id: None,
            created_at_ms: 1_000_000_000_000,
            snapshot_at_ms: 1_000_000_000_100,
            metrics: MetricsSnapshot {
                total_runs: 5,
                successful_runs: 4,
                failed_runs: 1,
                total_tokens: 50_000,
                tool_calls: 20,
                total_duration_ms: 30_000,
            },
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn snapshot_roundtrip_json() {
        let snapshot = test_snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: AgentSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent_id, "test-agent");
        assert_eq!(back.metrics.total_runs, 5);
    }

    #[test]
    fn snapshot_roundtrip_bytes() {
        let snapshot = test_snapshot();
        let bytes = snapshot.to_bytes().unwrap();
        let back = AgentSnapshot::from_bytes(&bytes).unwrap();
        assert_eq!(back.agent_id, "test-agent");
    }

    #[test]
    fn snapshot_estimated_size() {
        let snapshot = test_snapshot();
        assert!(snapshot.estimated_size_bytes() > 0);
    }

    #[test]
    fn tool_manifest_from_empty_registry() {
        let registry = Arc::new(ToolRegistry::new());
        let manifest = ToolManifest::from_registry(&registry);
        assert!(manifest.tools.is_empty());
        assert!(manifest.missing_from(&registry).is_empty());
    }

    #[tokio::test]
    async fn file_snapshot_store_save_load() {
        let tmp = TempDir::new().unwrap();
        let store = FileSnapshotStore::new(tmp.path()).unwrap();

        let snapshot = test_snapshot();
        store.save(&snapshot).await.unwrap();

        let loaded = store.load("test-agent").await.unwrap().unwrap();
        assert_eq!(loaded.agent_id, "test-agent");
        assert_eq!(loaded.metrics.total_runs, 5);
    }

    #[tokio::test]
    async fn file_snapshot_store_load_missing() {
        let tmp = TempDir::new().unwrap();
        let store = FileSnapshotStore::new(tmp.path()).unwrap();
        let result = store.load("does-not-exist").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn file_snapshot_store_delete() {
        let tmp = TempDir::new().unwrap();
        let store = FileSnapshotStore::new(tmp.path()).unwrap();

        let snapshot = test_snapshot();
        store.save(&snapshot).await.unwrap();
        store.delete("test-agent").await.unwrap();

        let result = store.load("test-agent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn file_snapshot_store_list() {
        let tmp = TempDir::new().unwrap();
        let store = FileSnapshotStore::new(tmp.path()).unwrap();

        let mut s1 = test_snapshot();
        s1.agent_id = "alpha".into();
        store.save(&s1).await.unwrap();

        let mut s2 = test_snapshot();
        s2.agent_id = "beta".into();
        store.save(&s2).await.unwrap();

        let ids = store.list().await.unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"alpha".into()));
        assert!(ids.contains(&"beta".into()));
    }
}
