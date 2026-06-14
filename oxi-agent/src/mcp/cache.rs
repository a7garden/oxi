//! MCP tool metadata disk cache.
//!
//! Persists the tool list discovered from each server so that proxy `search`,
//! `list`, and `describe` operations can work without a live connection
//! (and without paying the cost of spawning every server at startup).
//!
//! **Important:** the cache stores **original (unprefixed) tool names only**.
//! The prefixed name is computed at runtime from the current `ToolPrefix` setting.
//! This way, changing `tool_prefix` in `mcp.json` does not invalidate the cache.
//!
//! # File format
//!
//! ```json
//! {
//!   "version": 1,
//!   "servers": {
//!     "chrome-devtools": {
//!       "updated_at": "2026-06-13T10:30:00Z",
//!       "tools": [
//!         { "name": "take_screenshot", "description": "...", "input_schema": { ... } }
//!       ]
//!     }
//!   }
//! }
//! ```

use crate::mcp::types::{McpToolDef, ToolMetadata, ToolPrefix, format_tool_name};
use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Current cache schema version.
const CACHE_VERSION: u32 = 1;

/// On-disk representation of the metadata cache.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheStore {
    /// Schema version (currently 1).
    #[serde(default = "default_version")]
    version: u32,
    /// Server name → cached tool list.
    servers: HashMap<String, ServerCacheEntry>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerCacheEntry {
    /// ISO-8601 timestamp of when this server's tools were last discovered.
    updated_at: String,
    /// Original (unprefixed) tool definitions.
    tools: Vec<CachedToolDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedToolDef {
    name: String,
    description: String,
    #[serde(default)]
    input_schema: Option<serde_json::Value>,
}

/// In-memory + on-disk metadata cache.
pub struct MetadataCache {
    /// Path to the cache file (e.g. `~/Library/Application Support/oxi/mcp-cache.json`).
    cache_path: PathBuf,
    /// In-memory cache, guarded by a sync lock so it can be read from
    /// both async and sync contexts (e.g. `render()`).
    cache: RwLock<CacheStore>,
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataCache {
    /// Create a new cache, resolving the default path under
    /// `dirs::config_dir()/oxi/mcp-cache.json`.
    pub fn new() -> Self {
        let cache_path = default_cache_path();
        Self {
            cache_path,
            cache: RwLock::new(CacheStore::default()),
        }
    }

    /// Create a cache rooted at a specific file path (used by tests).
    pub fn with_path(cache_path: PathBuf) -> Self {
        Self {
            cache_path,
            cache: RwLock::new(CacheStore::default()),
        }
    }

    /// Returns the path the cache reads from / writes to.
    pub fn path(&self) -> &Path {
        &self.cache_path
    }

    /// Load the cache from disk. Missing file is not an error.
    pub fn load(&self) -> Result<()> {
        match std::fs::read_to_string(&self.cache_path) {
            Ok(contents) => match serde_json::from_str::<CacheStore>(&contents) {
                Ok(store) => {
                    *self.cache.write() = store;
                }
                Err(e) => {
                    tracing::warn!(
                        "MCP cache: failed to parse {}: {} (starting fresh)",
                        self.cache_path.display(),
                        e
                    );
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No cache yet — that's fine.
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to read MCP cache {}: {}",
                    self.cache_path.display(),
                    e
                ));
            }
        }
        Ok(())
    }

    /// Return the cached tools for a server, converted to `ToolMetadata`
    /// using the supplied `prefix_mode` for the display name.
    ///
    /// Returns an empty `Vec` if the server has no cached tools.
    pub fn get_tools(&self, server_name: &str, prefix_mode: &ToolPrefix) -> Vec<ToolMetadata> {
        let cache = self.cache.read();
        cache
            .servers
            .get(server_name)
            .map(|entry| {
                entry
                    .tools
                    .iter()
                    .map(|t| ToolMetadata {
                        name: format_tool_name(&t.name, server_name, prefix_mode),
                        original_name: t.name.clone(),
                        server_name: server_name.to_string(),
                        description: t.description.clone(),
                        input_schema: t.input_schema.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Update the cached tools for a server and atomically persist to disk.
    pub fn update(&self, server_name: &str, tools: &[McpToolDef]) -> Result<()> {
        let entry = ServerCacheEntry {
            updated_at: chrono_now_iso8601(),
            tools: tools
                .iter()
                .map(|t| CachedToolDef {
                    name: t.name.clone(),
                    description: t.description.clone().unwrap_or_default(),
                    input_schema: t.input_schema.clone(),
                })
                .collect(),
        };

        {
            let mut cache = self.cache.write();
            cache.version = CACHE_VERSION;
            cache.servers.insert(server_name.to_string(), entry);
            // Write a clone to disk under the write lock to keep the
            // in-memory state and disk state in sync from the caller's
            // perspective.
            let snapshot = CacheStore {
                version: cache.version,
                servers: cache.servers.clone(),
            };
            drop(cache);
            self.write_to_disk(&snapshot)?;
        }

        Ok(())
    }

    /// Remove a server's cached tools and persist.
    pub fn invalidate(&self, server_name: &str) -> Result<()> {
        let snapshot;
        {
            let mut cache = self.cache.write();
            cache.servers.remove(server_name);
            snapshot = CacheStore {
                version: cache.version,
                servers: cache.servers.clone(),
            };
        }
        self.write_to_disk(&snapshot)
    }

    /// All server names that have cached tools.
    pub fn cached_servers(&self) -> Vec<String> {
        self.cache.read().servers.keys().cloned().collect()
    }

    /// Atomic write: serialize to a temp file, fsync, rename over the target.
    fn write_to_disk(&self, store: &CacheStore) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create MCP cache directory {}", parent.display())
            })?;
        }

        let json = serde_json::to_string_pretty(store).context("Failed to serialize MCP cache")?;

        let tmp = self.cache_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("Failed to write MCP cache tmp {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.cache_path).with_context(|| {
            format!(
                "Failed to rename MCP cache {} → {}",
                tmp.display(),
                self.cache_path.display()
            )
        })?;
        Ok(())
    }
}

/// Default cache path: `dirs::config_dir()/oxi/mcp-cache.json`.
fn default_cache_path() -> PathBuf {
    if let Some(config_dir) = dirs::config_dir() {
        config_dir.join("oxi").join("mcp-cache.json")
    } else {
        PathBuf::from(".oxi/mcp-cache.json")
    }
}

/// Minimal ISO-8601 timestamp (no chrono dependency).
fn chrono_now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_tools() -> Vec<McpToolDef> {
        vec![
            McpToolDef {
                name: "take_screenshot".to_string(),
                description: Some("Take a screenshot".to_string()),
                input_schema: Some(serde_json::json!({"type": "object"})),
            },
            McpToolDef {
                name: "navigate".to_string(),
                description: Some("Navigate to URL".to_string()),
                input_schema: None,
            },
        ]
    }

    #[test]
    fn empty_cache_loads_cleanly_from_missing_file() {
        let dir = TempDir::new().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("mcp-cache.json"));
        assert!(cache.load().is_ok());
        assert!(cache.get_tools("any", &ToolPrefix::Server).is_empty());
    }

    #[test]
    fn update_then_reload_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-cache.json");

        let cache = MetadataCache::with_path(path.clone());
        cache.load().unwrap();
        cache.update("chrome", &sample_tools()).unwrap();

        // New instance from same path
        let cache2 = MetadataCache::with_path(path);
        cache2.load().unwrap();
        let tools = cache2.get_tools("chrome", &ToolPrefix::Server);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].original_name, "take_screenshot");
        assert_eq!(tools[0].server_name, "chrome");
        // Prefixed name with Server mode = "chrome_take_screenshot"
        assert_eq!(tools[0].name, "chrome_take_screenshot");
    }

    #[test]
    fn prefix_mode_changes_display_name_but_not_cache() {
        let dir = TempDir::new().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("mcp-cache.json"));
        cache.load().unwrap();
        cache.update("chrome", &sample_tools()).unwrap();

        // Same cache, two prefix modes
        let server_mode = cache.get_tools("chrome", &ToolPrefix::Server);
        let none_mode = cache.get_tools("chrome", &ToolPrefix::None);

        assert_eq!(server_mode[0].name, "chrome_take_screenshot");
        assert_eq!(none_mode[0].name, "take_screenshot");
        // Original name is identical
        assert_eq!(server_mode[0].original_name, none_mode[0].original_name);
    }

    #[test]
    fn invalidate_removes_server() {
        let dir = TempDir::new().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("mcp-cache.json"));
        cache.load().unwrap();
        cache.update("chrome", &sample_tools()).unwrap();
        assert_eq!(cache.cached_servers(), vec!["chrome".to_string()]);

        cache.invalidate("chrome").unwrap();
        assert!(cache.cached_servers().is_empty());
    }
}
