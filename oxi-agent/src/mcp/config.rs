//! MCP configuration loading.
//!
//! Discovers and loads MCP server configuration from standard locations:
//! - `~/.config/mcp/mcp.json` (shared global)
//! - `<config_dir>/oxi/mcp.json` (oxi-specific global)
//! - `<cwd>/.mcp.json` (shared project)
//! - `<cwd>/.oxi/mcp.json` (oxi-specific project)

use super::types::McpConfig;
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
fn read_config_file(path: &Path) -> Option<McpConfig> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_when_no_files() {
        let config = load_mcp_config_from(Path::new("/nonexistent"));
        assert!(config.mcp_servers.is_empty());
    }
}
