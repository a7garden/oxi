//! WASM-based extension system powered by Extism.
//!
//! Loads `.wasm` extension files from `~/.oxi/extensions/` and project-local
//! `.oxi/extensions/`. Each extension exports well-known functions (`init`,
//! `register_tools`, `execute_tool`) called via Extism's JSON-in/JSON-out
//! protocol. Extensions run inside a WASM sandbox with zero host access by
//! default — HTTP access is granted via the `oxi_http_request` host function.

use anyhow::{Context, Result};
use extism::{CurrentPlugin, Function, UserData, Val, ValType, PTR};
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

// ── Host Functions ────────────────────────────────────────────────────

/// Host function: `oxi_http_request(request_json) -> response_json`
///
/// Request JSON: `{"url": "...", "method": "GET", "headers": {...}, "body": "..."}`
/// Response JSON: `{"status": 200, "headers": {...}, "body": "..."}`
fn host_oxi_http_request(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    user_data: UserData<Arc<reqwest::blocking::Client>>,
) -> Result<(), extism::Error> {
    // We use anyhow internally, then convert at the boundary
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct HttpReq {
            url: String,
            #[serde(default)]
            method: String,
            #[serde(default)]
            headers: HashMap<String, String>,
            #[serde(default)]
            body: Option<String>,
        }

        let req: HttpReq = serde_json::from_str(&input_json)
            .context("oxi_http_request: invalid request JSON")?;

        let method = if req.method.is_empty() { "GET" } else { &req.method };

        // SSRF protection: block internal/private network addresses
        if let Err(e) = validate_url(&req.url) {
            anyhow::bail!("oxi_http_request: {}", e);
        }

        // UserData::get() returns Arc<Mutex<T>>
        let client_arc = user_data.get()?;
        let client = client_arc.lock().unwrap();

        let method = match method.to_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            "HEAD" => reqwest::Method::HEAD,
            other => anyhow::bail!("oxi_http_request: unsupported method '{}'", other),
        };

        let mut rb = client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(body) = &req.body {
            rb = rb.body(body.clone());
        }

        // Execute HTTP request (blocking — called from spawn_blocking in wasm_tool.rs)
        let resp = rb.send().map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;
        let status = resp.status().as_u16();
        let resp_headers: HashMap<String, String> = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (k.to_string(), v.to_str().unwrap_or("").to_string())
            })
            .collect();
        let resp_body = resp.text().map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

        let response = serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "body": resp_body,
        });

        let output = serde_json::to_string(&response)?;
        let handle = plugin.memory_new(&output)?;
        if !outputs.is_empty() {
            outputs[0] = plugin.memory_to_val(handle);
        }
        Ok(())
    })();

    result.map_err(|e| extism::Error::from(e))
}

/// Host function: `oxi_log(message)` — logs a debug message from WASM.
fn host_oxi_log(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    _outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let message: String = plugin.memory_get_val(&inputs[0])?;
    tracing::debug!("[WASM] {}", message);
    Ok(())
}

// ── SSRF Protection ─────────────────────────────────────────────────

/// Validate that a URL does not target internal/private network addresses.
fn validate_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    let host = parsed.host_str().unwrap_or("").to_lowercase();

    // Block private IPs, localhost, link-local, and internal hostnames
    let blocked = [
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        "::1",
        "[::1]",
        "169.254.169.254", // cloud metadata
        "metadata.google.internal",
    ];
    for &b in &blocked {
        if host == b || host.starts_with(b) {
            return Err(format!("Blocked internal address: {}", host));
        }
    }

    // Block private IP ranges (10.x, 172.16-31.x, 192.168.x)
    if host.starts_with("10.")
        || host.starts_with("192.168.")
        || is_172_private(&host)
    {
        return Err(format!("Blocked private address: {}", host));
    }

    Ok(())
}

/// Check if host is in 172.16.0.0/12 range.
fn is_172_private(host: &str) -> bool {
    if !host.starts_with("172.") { return false; }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 2 { return false; }
    if let Ok(second) = parts[1].parse::<u8>() {
        (16..=31).contains(&second)
    } else {
        false
    }
}

// ── Manager ──────────────────────────────────────────────────────────

/// Manages WASM extensions: discovery, loading, and tool execution.
pub struct WasmExtensionManager {
    extensions: HashMap<String, LoadedWasmExtension>,
    /// Raw Extism plugin references — needed for execute_tool calls.
    plugins: Arc<RwLock<HashMap<String, extism::Plugin>>>,
    /// Maps tool name → extension name.
    tool_to_ext: HashMap<String, String>,
    /// HTTP client shared by all extensions for oxi_http_request.
    http_client: Arc<reqwest::blocking::Client>,
}

impl WasmExtensionManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self {
            extensions: HashMap::new(),
            plugins: Arc::new(RwLock::new(HashMap::new())),
            tool_to_ext: HashMap::new(),
            http_client: Arc::new(
                reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .no_proxy() // Prevent proxy-based SSRF
                    .build()
                    .expect("Failed to build HTTP client")
            ),
        }
    }

    /// Create with a custom HTTP client (useful for testing with a mock client).
    pub fn with_http_client(client: reqwest::blocking::Client) -> Self {
        Self {
            extensions: HashMap::new(),
            plugins: Arc::new(RwLock::new(HashMap::new())),
            tool_to_ext: HashMap::new(),
            http_client: Arc::new(client),
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

    /// Build the host functions available to all WASM extensions.
    fn host_functions(http_client: &Arc<reqwest::blocking::Client>) -> Vec<Function> {
        let http_fn = Function::new(
            "oxi_http_request",
            [PTR],
            [PTR],
            UserData::new(http_client.clone()),
            host_oxi_http_request,
        );

        let log_fn = Function::new(
            "oxi_log",
            [PTR],
            [],
            UserData::new(()),
            host_oxi_log,
        );

        vec![http_fn, log_fn]
    }

    /// Load a single `.wasm` extension.
    pub fn load(&mut self, path: &Path) -> Result<ExtensionInfo> {
        let path_display = path.display().to_string();
        tracing::info!("Loading WASM extension: {}", path_display);

        let wasm_bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read extension: {}", path_display))?;

        let manifest = extism::Manifest::new([extism::Wasm::data(wasm_bytes)]);
        let mut plugin = extism::PluginBuilder::new(manifest)
            .with_wasi(true)
            .with_functions(Self::host_functions(&self.http_client))
            .build()
            .with_context(|| format!("Failed to create Extism plugin from {}", path_display))?;

        // Call init()
        let info: ExtensionInfo = match plugin.call::<&str, &str>("init", "{}") {
            Ok(output) => {
                serde_json::from_str(output)
                    .with_context(|| format!("init() returned invalid JSON: {}", output))?
            }
            Err(_) => {
                // No init function — derive name from filename
                let name = path
                    .file_stem()
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
        let tools: Vec<WasmToolDef> = match plugin.call::<&str, &str>("register_tools", "{}") {
            Ok(output) => {
                let resp: Value = serde_json::from_str(output)
                    .with_context(|| format!("register_tools() invalid JSON: {}", output))?;
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

        let ext_name = info.name.clone();

        // Warn on name collision
        if self.extensions.contains_key(&ext_name) {
            tracing::warn!(
                "Extension '{}' already loaded, replacing with '{}'",
                ext_name, path_display
            );
            // Remove old tool mappings
            self.tool_to_ext.retain(|_, v| v != &ext_name);
        }

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
    pub fn execute_tool(&self, tool_name: &str, params: Value) -> Result<Value> {
        let ext_name = self.tool_to_ext.get(tool_name)
            .with_context(|| format!("No extension registered for tool: {}", tool_name))?;

        let mut plugins = self.plugins.write();
        let plugin = plugins.get_mut(ext_name)
            .with_context(|| format!("Extension '{}' not loaded", ext_name))?;

        let input = serde_json::json!({
            "tool": tool_name,
            "params": params,
        });
        let input_str = serde_json::to_string(&input)?;

        let output: &str = plugin.call("execute_tool", &input_str)
            .with_context(|| format!("execute_tool('{}') failed in '{}'", tool_name, ext_name))?;

        let result: Value = serde_json::from_str(output)
            .with_context(|| format!("execute_tool() returned invalid JSON: {}", output))?;

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

    #[test]
    fn test_ssrf_blocks_localhost() {
        assert!(validate_url("http://localhost/admin").is_err());
        assert!(validate_url("http://127.0.0.1/secret").is_err());
        assert!(validate_url("http://10.0.0.1/internal").is_err());
        assert!(validate_url("http://192.168.1.1/router").is_err());
        assert!(validate_url("http://172.16.0.1/corp").is_err());
        assert!(validate_url("http://169.254.169.254/metadata").is_err());
        assert!(validate_url("http://[::1]/ipv6").is_err());
        // Also test without brackets (parsed hostname)
        assert!(validate_url("http://0.0.0.0/admin").is_err());
    }

    #[test]
    fn test_ssrf_allows_public() {
        assert!(validate_url("https://api.github.com/repos/test").is_ok());
        assert!(validate_url("https://example.com/api").is_ok());
        assert!(validate_url("https://search.brave.com/api/search?q=test").is_ok());
    }

    #[test]
    fn test_ssrf_172_range() {
        assert!(validate_url("http://172.16.0.1/test").is_err());
        assert!(validate_url("http://172.31.255.255/test").is_err());
        assert!(validate_url("http://172.15.0.1/test").is_ok());
        assert!(validate_url("http://172.32.0.1/test").is_ok());
    }
}
