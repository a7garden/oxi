//! MCP consent management — two policies over one store.
//!
//! Persists per-tool and per-server decisions to `mcp-consent.json` so they
//! survive across sessions. Two distinct default policies share the store:
//!
//! - **Tool-call gate** ([`ConsentManager::check`]): unknown tools default
//!   to `Allow`. Used by `direct_tool.rs` which only blocks `Deny`.
//! - **Spawn gate** ([`ConsentManager::check_spawn_consent`]): unknown
//!   servers default to `Ask`, which [`crate::mcp::McpManager::connect`]
//!   treats as "not trusted — deny spawn". Closing the clone-to-RCE
//!   surface (F-2, code audit 2026-07-25). The user pre-trusts servers via
//!   `oxi mcp trust <name>`.
//!
//! `Ask` is never persisted: [`ConsentManager::decide`] silently normalizes
//! it to `Deny` so the on-disk file only ever contains `allow`/`deny`.

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

    /// Spawn-path consent check. Unknown servers return `Ask` (the caller
    /// treats `Ask` and `Deny` identically: deny the spawn and surface
    /// `McpError::ConsentDenied`). This is deliberately a *separate*
    /// default from [`check`](Self::check) so the per-tool gate at
    /// `direct_tool.rs` (which only blocks `Deny`) is unaffected.
    pub fn check_spawn_consent(&self, name: &str) -> ConsentState {
        self.store
            .read()
            .decisions
            .get(name)
            .cloned()
            .unwrap_or(ConsentState::Ask)
    }

    /// Persist a consent decision. `name` is a tool or server name.
    ///
    /// `Ask` is normalized to `Deny` — it is a transient runtime signal,
    /// never a stored state (see the module docs). Callers that resolve
    /// `Ask` interactively must pass the resolved `Allow`/`Deny` here.
    pub fn decide(&self, name: &str, state: ConsentState) -> Result<()> {
        let state = match state {
            ConsentState::Ask => {
                tracing::warn!(
                    "MCP consent: `Ask` passed to decide({name:?}) — \
                     normalizing to `Deny` (Ask is transient, never persisted)"
                );
                ConsentState::Deny
            }
            // Allow and Deny pass through unchanged.
            other => other,
        };
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

    /// Trust a server (persist `Allow`). Convenience wrapper around
    /// [`decide`](Self::decide). Used by `oxi mcp trust <server>`.
    pub fn trust(&self, name: &str) -> Result<()> {
        self.decide(name, ConsentState::Allow)
    }

    /// Revoke trust for a server (persist `Deny`). Convenience wrapper
    /// around [`decide`](Self::decide). Used by `oxi mcp untrust <server>`.
    pub fn untrust(&self, name: &str) -> Result<()> {
        self.decide(name, ConsentState::Deny)
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

    // ── F-2 spawn consent (code audit 2026-07-25) ─────────────────────

    #[test]
    fn spawn_consent_defaults_to_ask_for_unknown_servers() {
        // The spawn gate treats unknown servers as `Ask` (→ deny spawn),
        // closing the clone-to-RCE surface. Contrast with `check()` below
        // which still returns `Allow` for the per-tool gate.
        let m = ConsentManager::new();
        assert_eq!(
            m.check_spawn_consent("untrusted-server"),
            ConsentState::Ask,
            "unknown servers must be Ask at the spawn gate"
        );
        // Per-tool gate unchanged — backward compat for direct_tool.rs.
        assert_eq!(
            m.check("untrusted-server"),
            ConsentState::Allow,
            "unknown names stay Allow for the tool-call gate"
        );
    }

    #[test]
    fn trust_persists_allow_then_spawn_consent_allows() {
        let dir = TempDir::new().unwrap();
        let m = ConsentManager::with_path(dir.path().join("consent.json"));
        m.load().unwrap();

        // Before trust: spawn gate denies.
        assert_eq!(m.check_spawn_consent("github"), ConsentState::Ask);

        m.trust("github").unwrap();
        assert_eq!(m.check_spawn_consent("github"), ConsentState::Allow);
        // Tool gate also sees Allow.
        assert_eq!(m.check("github"), ConsentState::Allow);
    }

    #[test]
    fn untrust_persists_deny_then_both_gates_block() {
        let dir = TempDir::new().unwrap();
        let m = ConsentManager::with_path(dir.path().join("consent.json"));
        m.load().unwrap();

        m.untrust("sketchy").unwrap();
        assert_eq!(m.check_spawn_consent("sketchy"), ConsentState::Deny);
        assert_eq!(m.check("sketchy"), ConsentState::Deny);
    }

    #[test]
    fn decide_normalizes_ask_to_deny_never_persists_ask() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("consent.json");
        let m = ConsentManager::with_path(path.clone());
        m.load().unwrap();

        // Ask must not be stored — it's a transient runtime signal.
        m.decide("transient", ConsentState::Ask).unwrap();

        // Reload and verify only Deny was persisted.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("ask"),
            "Ask must never be written to disk, got: {raw}"
        );
        let m2 = ConsentManager::with_path(path);
        m2.load().unwrap();
        assert_eq!(m2.check("transient"), ConsentState::Deny);
    }

    #[test]
    fn trusted_server_survives_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("consent.json");

        let m1 = ConsentManager::with_path(path.clone());
        m1.load().unwrap();
        m1.trust("persistent").unwrap();

        let m2 = ConsentManager::with_path(path);
        m2.load().unwrap();
        assert_eq!(m2.check_spawn_consent("persistent"), ConsentState::Allow);
    }
}
