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

/// Global config paths only (`~/.config/mcp/mcp.json` and
/// `~/.config/oxi/mcp.json`). These are user-authored and trusted for the
/// one-time consent migration in [`crate::mcp::McpManager::spawn_with_paths`].
/// Project-local paths (`.mcp.json`, `.oxi/mcp.json`) are deliberately
/// excluded — a cloned repo may ship a malicious project-local config, and
/// auto-trusting it would reopen the clone-to-RCE surface F-2 closes.
pub fn global_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(config_dir.join("mcp").join("mcp.json"));
        paths.push(config_dir.join("oxi").join("mcp.json"));
    }
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
///
/// oxi's own paths are merged first (later files override earlier).
/// Then, if `settings.discoverExternalConfigs` is enabled, third-party
/// tool configs (`external_paths`) are merged with **lower priority**:
/// they only add server entries oxi did not already define and never
/// override oxi's `settings`. v2.4 / G8.
pub fn load_mcp_config_from(cwd: &Path) -> McpConfig {
    let mut merged = McpConfig::default();

    for path in config_paths(cwd) {
        if let Some(config) = read_config_file(&path) {
            for (name, entry) in config.mcp_servers {
                merged.mcp_servers.insert(name, entry);
            }
            if config.settings.is_some() {
                merged.settings = config.settings;
            }
        }
    }

    // Opt-in third-party discovery. oxi's own entries + settings win.
    let discover = merged
        .settings
        .as_ref()
        .and_then(|s| s.discover_external_configs)
        .unwrap_or(false);
    if discover {
        for path in external_paths(cwd) {
            if let Some(config) = read_config_file(&path) {
                for (name, entry) in config.mcp_servers {
                    merged.mcp_servers.entry(name).or_insert(entry);
                }
            }
        }
    }

    merged
}

/// Third-party tool config files to optionally discover (v2.4 / G8).
/// Only files using the same `{"mcpServers": {...}}` schema as oxi are
/// supported here; VS Code's `{"servers": ...}` and opencode's
/// `{"mcp": ...}` schemas need normalization and are out of v2.4 scope.
fn external_paths(cwd: &Path) -> Vec<PathBuf> {
    vec![
        cwd.join(".claude").join("mcp.json"),
        cwd.join(".cursor").join("mcp.json"),
    ]
}

/// Read and parse a single config file. Returns `None` if the file
/// doesn't exist or is invalid.
pub fn read_config_file(path: &Path) -> Option<McpConfig> {
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;

    match serde_json::from_str::<McpConfig>(&content) {
        Ok(mut config) => {
            resolve_config(&mut config);
            Some(config)
        }
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

/// Resolve `ServerEntry` string fields against the process environment
/// and (for `!cmd` values) the host shell. Called by [`read_config_file`]
/// after parsing so that values stored in `mcp.json` are ready to use at
/// connect time without further substitution.
///
/// Rules (matching the OMP behaviour):
/// - Values starting with `!` run through a shell (`sh -c` on Unix,
///   `cmd /C` on Windows) with a 10s timeout. The trimmed stdout replaces
///   the value; failure or empty stdout yields `None`.
/// - Other values have `${VAR}` and `${VAR:-default}` placeholders
///   expanded against the process environment; unresolved placeholders
///   remain literal (so a misconfigured value surfaces rather than
///   silently disappearing).
pub fn resolve_config(cfg: &mut McpConfig) {
    for (name, entry) in cfg.mcp_servers.iter_mut() {
        if let Some(s) = entry.command.as_ref() {
            entry.command = match resolve_value(s) {
                Some(v) => Some(v),
                None => {
                    tracing::warn!("MCP server '{}': failed to resolve command", name);
                    None
                }
            };
        }
        if let Some(args) = entry.args.as_mut() {
            args.retain_mut(|a| match resolve_value(a) {
                Some(r) => {
                    *a = r;
                    true
                }
                None => false,
            });
        }
        if let Some(s) = entry.cwd.as_ref() {
            entry.cwd = match resolve_value(s) {
                Some(v) => Some(v),
                None => {
                    tracing::warn!("MCP server '{}': failed to resolve cwd", name);
                    None
                }
            };
        }
        if let Some(s) = entry.url.as_ref() {
            entry.url = match resolve_value(s) {
                Some(v) => Some(v),
                None => {
                    tracing::warn!("MCP server '{}': failed to resolve url", name);
                    None
                }
            };
        }
        if let Some(env) = entry.env.as_mut() {
            env.retain(|_, v| match resolve_value(v) {
                Some(r) => {
                    *v = r;
                    true
                }
                None => false,
            });
        }
        if let Some(headers) = entry.headers.as_mut() {
            headers.retain(|_, v| match resolve_value(v) {
                Some(r) => {
                    *v = r;
                    true
                }
                None => false,
            });
        }
    }
}

fn resolve_value(value: &str) -> Option<String> {
    if let Some(cmd) = value.strip_prefix('!') {
        run_shell_capture(cmd, std::time::Duration::from_secs(10))
    } else {
        Some(expand_env_placeholders(value))
    }
}

fn run_shell_capture(cmd: &str, timeout: std::time::Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_cmd = cmd.to_string();
    std::thread::spawn(move || {
        let result = shell_command_output(&thread_cmd);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    }
}

#[cfg(unix)]
fn shell_command_output(cmd: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("sh").arg("-c").arg(cmd).output()
}

#[cfg(not(unix))]
fn shell_command_output(cmd: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("cmd")
        .arg("/C")
        .arg(cmd)
        .output()
}

fn expand_env_placeholders(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'{'
            && let Some(end_rel) = input[i + 2..].find('}')
        {
            let inner = &input[i + 2..i + 2 + end_rel];
            let (name, default) = match inner.split_once(":-") {
                Some((n, d)) => (n, Some(d)),
                None => (inner, None),
            };
            let value = std::env::var(name)
                .ok()
                .or_else(|| default.map(|d| d.to_string()));
            if let Some(v) = value {
                out.push_str(&v);
            } else {
                out.push_str(&input[i..i + 2 + end_rel + 1]);
            }
            i += 2 + end_rel + 1;
            continue;
        }
        // SAFETY: `i` is advanced only by complete UTF-8 char boundaries, so
        // `input[i..]` starts at a valid boundary while `i < bytes.len()` —
        // `chars().next()` cannot return None.
        #[allow(clippy::expect_used)]
        let ch = input[i..].chars().next().expect("valid UTF-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
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
