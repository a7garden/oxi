//! MCP tool execution consent management (Phase 3).
//!
//! Stores per-tool (or per-server) decisions in memory and on disk so that
//! consent survives across sessions. The default is [`ConsentState::Allow`],
//! which preserves the previous (no-consent) behaviour.
//!
//! Phase 4+ may extend this with a runtime `Ask` mode; for now the only
//! two states are `Allow` and `Deny`.

use super::types::ConsentState;
use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// On-disk representation of the consent store.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ConsentStore {
    /// Schema version.
    #[serde(default = "default_version")]
    version: u32,
    /// Map of "tool_or_server_name" → consent state.
    decisions: HashMap<String, ConsentState>,
}

fn default_version() -> u32 {
    1
}

/// In-memory + on-disk consent manager.
pub struct ConsentManager {
    /// Persisted state, plus in-memory copy under `RwLock`.
    store: RwLock<ConsentStore>,
    /// Path to the consent file.
    persist_path: PathBuf,
}

impl std::fmt::Debug for ConsentManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsentManager")
            .field("decisions", &self.store.read().decisions)
            .field("path", &self.persist_path)
            .finish()
    }
}

impl ConsentManager {
    /// Create a new consent manager, resolving the default path under
    /// `dirs::config_dir()/oxi/mcp-consent.json`.
    pub fn new() -> Self {
        Self::with_path(default_consent_path())
    }

    /// Create a manager with a custom file path (used by tests).
    pub fn with_path(persist_path: PathBuf) -> Self {
        Self {
            store: RwLock::new(ConsentStore::default()),
            persist_path,
        }
    }

    /// Returns the path the manager reads from / writes to.
    pub fn path(&self) -> &Path {
        &self.persist_path
    }

    /// Load decisions from disk. Missing file is not an error.
    pub fn load(&self) -> Result<()> {
        match std::fs::read_to_string(&self.persist_path) {
            Ok(contents) => match serde_json::from_str::<ConsentStore>(&contents) {
                Ok(store) => {
                    *self.store.write() = store;
                }
                Err(e) => {
                    tracing::warn!(
                        "MCP consent: failed to parse {}: {} (starting fresh)",
                        self.persist_path.display(),
                        e
                    );
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to read MCP consent file {}: {}",
                    self.persist_path.display(),
                    e
                ));
            }
        }
        Ok(())
    }

    /// Look up the consent for a given name. Defaults to `Allow`.
    pub fn check(&self, name: &str) -> ConsentState {
        self.store
            .read()
            .decisions
            .get(name)
            .cloned()
            .unwrap_or(ConsentState::Allow)
    }

    /// Persist a consent decision. `name` is typically a tool name
    /// (or a server name as a broader rule).
    pub fn decide(&self, name: &str, state: ConsentState) -> Result<()> {
        let snapshot;
        {
            let mut store = self.store.write();
            store.version = 1;
            store.decisions.insert(name.to_string(), state);
            snapshot = ConsentStore {
                version: store.version,
                decisions: store.decisions.clone(),
            };
        }
        self.write_to_disk(&snapshot)
    }

    /// All current decisions (for the TUI dashboard).
    pub fn all_decisions(&self) -> HashMap<String, ConsentState> {
        self.store.read().decisions.clone()
    }

    /// Atomic write: serialize, write to `.tmp`, rename.
    fn write_to_disk(&self, store: &ConsentStore) -> Result<()> {
        if let Some(parent) = self.persist_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create MCP consent directory {}",
                    parent.display()
                )
            })?;
        }
        let json =
            serde_json::to_string_pretty(store).context("Failed to serialize MCP consent store")?;
        let tmp = self.persist_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("Failed to write MCP consent tmp {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.persist_path).with_context(|| {
            format!(
                "Failed to rename MCP consent {} → {}",
                tmp.display(),
                self.persist_path.display()
            )
        })?;
        Ok(())
    }
}

impl Default for ConsentManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Default consent path: `dirs::config_dir()/oxi/mcp-consent.json`.
fn default_consent_path() -> PathBuf {
    if let Some(config_dir) = dirs::config_dir() {
        config_dir.join("oxi").join("mcp-consent.json")
    } else {
        PathBuf::from(".oxi/mcp-consent.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_is_allow() {
        let m = ConsentManager::new();
        assert_eq!(m.check("anything"), ConsentState::Allow);
    }

    #[test]
    fn decide_then_check() {
        let dir = TempDir::new().unwrap();
        let m = ConsentManager::with_path(dir.path().join("consent.json"));
        m.load().unwrap();
        m.decide("dangerous_tool", ConsentState::Deny).unwrap();
        assert_eq!(m.check("dangerous_tool"), ConsentState::Deny);
    }

    #[test]
    fn reload_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("consent.json");

        let m1 = ConsentManager::with_path(path.clone());
        m1.load().unwrap();
        m1.decide("tool_a", ConsentState::Deny).unwrap();
        m1.decide("tool_b", ConsentState::Allow).unwrap();

        let m2 = ConsentManager::with_path(path);
        m2.load().unwrap();
        assert_eq!(m2.check("tool_a"), ConsentState::Deny);
        assert_eq!(m2.check("tool_b"), ConsentState::Allow);
        assert_eq!(m2.check("unknown"), ConsentState::Allow);
    }
}
