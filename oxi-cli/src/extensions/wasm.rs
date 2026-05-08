//! WASM-based extension system powered by Extism.
//!
//! Loads `.wasm` extension files from `~/.oxi/extensions/` and project-local
//! `.oxi/extensions/`. Each extension exports well-known functions (`init`,
//! `register_tools`, `execute_tool`) called via Extism's JSON-in/JSON-out
//! protocol. Extensions run inside a WASM sandbox with zero host access by
//! default.

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── Types ────────────────────────────────────────────────────────────

/// Metadata returned by an extension's `init()` function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

/// A tool definition returned by `register_tools()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Result of loading a WASM extension.
#[derive(Debug)]
pub struct LoadedWasmExtension {
    pub info: ExtensionInfo,
    pub tools: Vec<WasmToolDef>,
    pub source_path: PathBuf,
}

// ── Manager ──────────────────────────────────────────────────────────

/// Manages WASM extensions: discovery, loading, and tool execution.
pub struct WasmExtensionManager {
    extensions: HashMap<String, LoadedWasmExtension>,
    /// Raw Extism plugin references — needed for execute_tool calls.
    plugins: Arc<RwLock<HashMap<String, extism::Plugin>>>,
    /// Maps tool name → extension name.
    tool_to_ext: HashMap<String, String>,
}

impl WasmExtensionManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self {
            extensions: HashMap::new(),
            plugins: Arc::new(RwLock::new(HashMap::new())),
            tool_to_ext: HashMap::new(),
        }
    }

    // ── Discovery ──────────────────────────────────────────────────

    /// Discover `.wasm` files in standard extension directories.
    pub fn discover(cwd: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // ~/.oxi/extensions/
        if let Some(home) = dirs::home_dir() {
            let dir = home.join(".oxi").join("extensions");
            if dir.is_dir() {
                Self::discover_in_dir(&dir, &mut paths);
            }
        }

        // .oxi/extensions/ (project-local)
        let local_dir = cwd.join(".oxi").join("extensions");
        if local_dir.is_dir() {
            Self::discover_in_dir(&local_dir, &mut paths);
        }

        paths.sort();
        paths.dedup();
        paths
    }

    fn discover_in_dir(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                out.push(path);
            }
        }
    }

    // ── Loading ────────────────────────────────────────────────────

    /// Load a single `.wasm` extension.
    pub fn load(&mut self, path: &Path) -> Result<ExtensionInfo> {
        let path_display = path.display().to_string();
        tracing::info!("Loading WASM extension: {}", path_display);

        let wasm_bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read extension: {}", path_display))?;

        let manifest = extism::Manifest::new([extism::Wasm::data(wasm_bytes.clone())]);
        let mut plugin = extism::Plugin::new(&manifest, [], true)
            .with_context(|| format!("Failed to create Extism plugin from {}", path_display))?;

        // Call init()
        let info: ExtensionInfo = match plugin.call("init", "{}") {
            Ok(output) => {
                let json_str = std::str::from_utf8(output)
                    .with_context(|| "init() returned non-UTF-8")?;
                serde_json::from_str(json_str)
                    .with_context(|| format!("init() returned invalid JSON: {}", json_str))?
            }
            Err(_) => {
                // No init function — derive name from filename
                let name = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                ExtensionInfo {
                    name,
                    version: "0.0.0".to_string(),
                    description: String::new(),
                }
            }
        };

        // Call register_tools()
        let tools: Vec<WasmToolDef> = match plugin.call("register_tools", "{}") {
            Ok(output) => {
                let json_str = std::str::from_utf8(output)
                    .with_context(|| "register_tools() returned non-UTF-8")?;
                let resp: Value = serde_json::from_str(json_str)
                    .with_context(|| format!("register_tools() invalid JSON: {}", json_str))?;
                resp.get("tools")
                    .cloned()
                    .unwrap_or(Value::Array(vec![]))
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Err(_) => vec![], // No tools — event-only extension
        };

        // Register tool → extension mapping
        let ext_name = info.name.clone();
        for tool in &tools {
            self.tool_to_ext.insert(tool.name.clone(), ext_name.clone());
        }

        let loaded = LoadedWasmExtension {
            info: info.clone(),
            tools,
            source_path: path.to_path_buf(),
        };

        self.extensions.insert(ext_name.clone(), loaded);
        self.plugins.write().insert(ext_name, plugin);

        tracing::info!(
            name = %info.name,
            version = %info.version,
            tools = self.tool_to_ext.len(),
            "WASM extension loaded"
        );

        Ok(info)
    }

    /// Load all discovered extensions, collecting errors.
    pub fn load_all(&mut self, paths: &[PathBuf]) -> (Vec<ExtensionInfo>, Vec<anyhow::Error>) {
        let mut loaded = Vec::new();
        let mut errors = Vec::new();

        for path in paths {
            match self.load(path) {
                Ok(info) => loaded.push(info),
                Err(e) => {
                    tracing::warn!("Failed to load extension '{}': {}", path.display(), e);
                    errors.push(e);
                }
            }
        }

        (loaded, errors)
    }

    // ── Execution ──────────────────────────────────────────────────

    /// Execute a tool via the WASM extension.
    pub fn execute_tool(&mut self, tool_name: &str, params: Value) -> Result<Value> {
        let ext_name = self.tool_to_ext.get(tool_name)
            .with_context(|| format!("No extension registered for tool: {}", tool_name))?;

        let plugins = self.plugins.read();
        let plugin = plugins.get(ext_name)
            .with_context(|| format!("Extension '{}' not loaded", ext_name))?;

        let input = serde_json::json!({
            "tool": tool_name,
            "params": params,
        });
        let input_bytes = serde_json::to_vec(&input)?;

        let output = plugin.call("execute_tool", &input_bytes)
            .with_context(|| format!("execute_tool('{}') failed in '{}'", tool_name, ext_name))?;

        let json_str = std::str::from_utf8(output)
            .with_context(|| "execute_tool() returned non-UTF-8")?;
        let result: Value = serde_json::from_str(json_str)
            .with_context(|| format!("execute_tool() returned invalid JSON: {}", json_str))?;

        Ok(result)
    }

    // ── Accessors ──────────────────────────────────────────────────

    /// Get all tool definitions from all loaded extensions.
    pub fn all_tool_defs(&self) -> Vec<&WasmToolDef> {
        self.extensions.values()
            .flat_map(|e| e.tools.iter())
            .collect()
    }

    /// Check if a tool name belongs to a WASM extension.
    pub fn is_wasm_tool(&self, tool_name: &str) -> bool {
        self.tool_to_ext.contains_key(tool_name)
    }

    /// List loaded extension names.
    pub fn extension_names(&self) -> impl Iterator<Item = &str> {
        self.extensions.keys().map(|s| s.as_str())
    }

    /// Get extension info by name.
    pub fn get_info(&self, name: &str) -> Option<&ExtensionInfo> {
        self.extensions.get(name).map(|e| &e.info)
    }

    /// Number of loaded extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Whether any extensions are loaded.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = WasmExtensionManager::discover(dir.path());
        assert!(paths.is_empty());
    }

    #[test]
    fn test_discover_finds_wasm_files() {
        let dir = tempfile::tempdir().unwrap();
        let wasm_path = dir.path().join("test_ext.wasm");
        std::fs::write(&wasm_path, b"\x00asm").unwrap();
        // Create a non-wasm file that should be ignored
        std::fs::write(dir.path().join("readme.txt"), b"hello").unwrap();

        let mut paths = Vec::new();
        WasmExtensionManager::discover_in_dir(dir.path(), &mut paths);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("test_ext.wasm"));
    }

    #[test]
    fn test_extension_info_parse() {
        let json = r#"{"name":"my_ext","version":"1.0.0","description":"Test"}"#;
        let info: ExtensionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "my_ext");
        assert_eq!(info.version, "1.0.0");
    }

    #[test]
    fn test_tool_def_parse() {
        let json = r#"{"name":"search","description":"Search","schema":{"type":"object"}}"#;
        let tool: WasmToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "search");
    }

    #[test]
    fn test_manager_new_is_empty() {
        let mgr = WasmExtensionManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn test_is_wasm_tool_false() {
        let mgr = WasmExtensionManager::new();
        assert!(!mgr.is_wasm_tool("anything"));
    }

    #[test]
    fn test_extension_info_default_description() {
        let json = r#"{"name":"test","version":"0.1"}"#;
        let info: ExtensionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.description, "");
    }
}
