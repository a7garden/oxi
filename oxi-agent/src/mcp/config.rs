//! MCP configuration loading.
//!
//! Discovers and loads MCP server configuration from standard locations:
//! - `~/.config/mcp/mcp.json` (shared global)
//! - `<config_dir>/oxi/mcp.json` (oxi-specific global)
//! - `<cwd>/.mcp.json` (shared project)
//! - `<cwd>/.oxi/mcp.json` (oxi-specific project)

use super::types::McpConfig;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Resolve all config file paths to try (in priority order).
pub fn config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Shared global config
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(config_dir.join("mcp").join("mcp.json"));
    }

    // 2. oxi-specific global config
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(config_dir.join("oxi").join("mcp.json"));
    }

    // 3. Shared project config
    paths.push(cwd.join(".mcp.json"));

    // 4. oxi-specific project config
    paths.push(cwd.join(".oxi").join("mcp.json"));

    paths
}

/// Load MCP configuration, merging all discovered config files.
///
/// Later files override earlier ones for server entries.
/// Returns an empty config if no files exist.
pub fn load_mcp_config() -> McpConfig {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(_) => return McpConfig::default(),
    };
    load_mcp_config_from(&cwd)
}

/// Load MCP configuration from a specific working directory.
pub fn load_mcp_config_from(cwd: &Path) -> McpConfig {
    let mut merged = McpConfig::default();

    for path in config_paths(cwd) {
        if let Some(config) = read_config_file(&path) {
            // Merge server entries (later files override)
            for (name, entry) in config.mcp_servers {
                merged.mcp_servers.insert(name, entry);
            }
            // Use the last settings found
            if config.settings.is_some() {
                merged.settings = config.settings;
            }
        }
    }

    merged
}

/// Read and parse a single config file. Returns `None` if the file
/// doesn't exist or is invalid.
pub fn read_config_file(path: &Path) -> Option<McpConfig> {
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;

    match serde_json::from_str::<McpConfig>(&content) {
        Ok(config) => Some(config),
        Err(e) => {
            tracing::warn!("Failed to parse MCP config {}: {}", path.display(), e);
            None
        }
    }
}

/// The default **write target** for global (user-wide) MCP config.
///
/// This is the oxi-owned global file (`~/.config/oxi/mcp.json` on
/// Unix, the platform equivalent elsewhere). Returns `None` only when
/// the platform has no resolvable config directory.
pub fn default_write_path_global() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("oxi").join("mcp.json"))
}

/// The default **write target** for project-local MCP config:
/// `<cwd>/.oxi/mcp.json`.
pub fn default_write_path_project(cwd: &Path) -> PathBuf {
    cwd.join(".oxi").join("mcp.json")
}

/// Load a single config file, returning an empty config if it does
/// not exist (rather than `None`) — convenient for the TUI editor
/// which wants to edit "the file" whether or not it exists yet.
pub fn load_or_default(path: &Path) -> McpConfig {
    read_config_file(path).unwrap_or_default()
}

/// Atomically write a full [`McpConfig`] to `path` as pretty-printed
/// JSON. Creates parent directories as needed. Uses the temp-file +
/// rename pattern so a crash mid-write cannot corrupt the config.
pub fn save_mcp_config(path: &Path, config: &McpConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create MCP config directory {}", parent.display())
        })?;
    }
    let json = serde_json::to_string_pretty(config).context("Failed to serialize MCP config")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .with_context(|| format!("Failed to write MCP config tmp {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to rename MCP config {} → {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_when_no_files() {
        let config = load_mcp_config_from(Path::new("/nonexistent"));
        assert!(config.mcp_servers.is_empty());
    }
}

#[cfg(test)]
mod compat_tests {
    use super::*;
    use crate::mcp::types::McpConfig;

    #[test]
    fn reads_standard_camel_case_mcp_config() {
        // The canonical MCP ecosystem format (Cursor / Claude / pi-mcp-adapter).
        let json = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem"],
                    "idleTimeout": 15,
                    "directTools": true
                }
            },
            "settings": {
                "toolPrefix": "server",
                "idleTimeout": 10
            }
        }"#;
        let cfg: McpConfig = serde_json::from_str(json).unwrap();
        let fs = cfg.mcp_servers.get("filesystem").expect("server present");
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert_eq!(fs.idle_timeout, Some(15));
        assert!(fs.direct_tools.is_some());
        assert!(cfg.settings.as_ref().unwrap().tool_prefix.is_some());
        assert_eq!(cfg.settings.as_ref().unwrap().idle_timeout, Some(10));
    }

    #[test]
    fn reads_legacy_snake_case_mcp_config() {
        let json = r#"{
            "mcpServers": {
                "legacy": {
                    "command": "node",
                    "idle_timeout": 5,
                    "exclude_tools": ["secret_tool"]
                }
            }
        }"#;
        let cfg: McpConfig = serde_json::from_str(json).unwrap();
        let legacy = cfg.mcp_servers.get("legacy").unwrap();
        assert_eq!(legacy.idle_timeout, Some(5));
        assert_eq!(
            legacy.exclude_tools.as_deref(),
            Some(&vec!["secret_tool".to_string()][..])
        );
    }

    #[test]
    fn round_trip_uses_camel_case_aliases() {
        let mut cfg = McpConfig::default();
        cfg.mcp_servers.insert(
            "s".to_string(),
            crate::mcp::types::ServerEntry {
                command: Some("npx".to_string()),
                idle_timeout: Some(7),
                ..Default::default()
            },
        );
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("mcpServers"), "serialized key must be camelCase");
        assert!(
            s.contains("idleTimeout"),
            "serialized field must be camelCase"
        );
        // And the round trip parses back.
        let back: McpConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.mcp_servers["s"].idle_timeout, Some(7));
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mcp.json");
        let mut cfg = McpConfig::default();
        cfg.mcp_servers.insert(
            "remote".to_string(),
            crate::mcp::types::ServerEntry {
                url: Some("https://example.com/mcp".to_string()),
                idle_timeout: Some(3),
                ..Default::default()
            },
        );
        save_mcp_config(&path, &cfg).unwrap();
        assert!(path.exists(), "temp file was renamed into place");
        let loaded = load_or_default(&path);
        assert_eq!(loaded.mcp_servers.len(), 1);
        assert_eq!(
            loaded.mcp_servers["remote"].url.as_deref(),
            Some("https://example.com/mcp")
        );
        assert_eq!(loaded.mcp_servers["remote"].idle_timeout, Some(3));
    }
}
