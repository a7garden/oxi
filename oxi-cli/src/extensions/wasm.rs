//! WASM-based extension system powered by Extism.
//!
//! Loads `.wasm` extension files from `~/.oxi/extensions/` and project-local
//! `.oxi/extensions/`. Each extension exports well-known functions (`init`,
//! `register_tools`, `execute_tool`) called via Extism's JSON-in/JSON-out
//! protocol. Extensions run inside a WASM sandbox with zero host access by
//! default — HTTP access is granted via the `oxi_http_request` host function.

use anyhow::{Context, Result};
use extism::{CurrentPlugin, Function, UserData, Val, ValType, PTR};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::cell::RefCell;
use std::io::Read as _;
use std::time::{Duration, Instant};

// ── Types ────────────────────────────────────────────────────────────

/// Metadata returned by an extension's `init()` function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Permissions requested by the extension.
    /// Supported: "fs_read", "fs_write", "exec", "env", "network"
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// A tool definition returned by `register_tools()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// A command definition returned by `register_commands()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmCommandDef {
    pub name: String,
    pub description: String,
}

/// Result of loading a WASM extension.
#[derive(Debug)]
pub struct LoadedWasmExtension {
    pub info: ExtensionInfo,
    pub tools: Vec<WasmToolDef>,
    pub commands: Vec<WasmCommandDef>,
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
        // Limit response body to 1MB to prevent memory exhaustion
        let resp_body = {
            let max_body = 1024 * 1024; // 1MB
            let body_bytes = resp.bytes().map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))?;
            if body_bytes.len() > max_body {
                tracing::warn!("HTTP response truncated: {} bytes > {} limit", body_bytes.len(), max_body);
                String::from_utf8_lossy(&body_bytes[..max_body]).to_string()
            } else {
                String::from_utf8_lossy(&body_bytes).to_string()
            }
        };

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

// ── File I/O Host Functions ─────────────────────────────────────────

/// Host function: `oxi_read_file(path_json) → result_json`
///
/// Input: `{"path": "/path/to/file", "offset": 0, "limit": 2000}`
/// Output: `{"success": true, "content": "...", "truncated": false, "bytes": 1234}` or
///         `{"success": false, "error": "..."}`
fn host_oxi_read_file(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct ReadReq {
            path: String,
            #[serde(default)]
            offset: Option<usize>,
            #[serde(default = "default_limit")]
            limit: usize,
        }
        fn default_limit() -> usize { 2000 }

        let req: ReadReq = serde_json::from_str(&input_json)
            .context("oxi_read_file: invalid request JSON")?;

        // Validate path is not outside cwd
        validate_path_allowed(&req.path)?;

        let metadata = std::fs::metadata(&req.path);
        match metadata {
            Ok(m) => {
                let max_bytes = 50 * 1024; // 50KB limit
                let file_size = m.len() as usize;

                let content = std::fs::read_to_string(&req.path)
                    .map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))?;

                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();

                let offset = req.offset.unwrap_or(0).min(total_lines);
                let end = (offset + req.limit).min(total_lines);
                let selected: Vec<&str> = lines[offset..end].to_vec();
                let mut result = selected.join("\n");

                let truncated = result.len() > max_bytes;
                if truncated {
                    result = result.chars().take(max_bytes).collect();
                }

                let response = serde_json::json!({
                    "success": true,
                    "content": result,
                    "truncated": truncated || end < total_lines,
                    "bytes": file_size,
                    "total_lines": total_lines,
                    "shown_lines": end - offset,
                });
                let output = serde_json::to_string(&response)?;
                let handle = plugin.memory_new(&output)?;
                if !outputs.is_empty() {
                    outputs[0] = plugin.memory_to_val(handle);
                }
            }
            Err(e) => {
                let response = serde_json::json!({
                    "success": false,
                    "error": format!("File not found: {}", e),
                });
                let output = serde_json::to_string(&response)?;
                let handle = plugin.memory_new(&output)?;
                if !outputs.is_empty() {
                    outputs[0] = plugin.memory_to_val(handle);
                }
            }
        }
        Ok(())
    })();
    result.map_err(extism::Error::from)
}

/// Host function: `oxi_write_file(path_json) → result_json`
///
/// Input: `{"path": "/path/to/file", "content": "...", "create_dirs": true}`
/// Output: `{"success": true, "bytes_written": 1234}` or `{"success": false, "error": "..."}`
fn host_oxi_write_file(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct WriteReq {
            path: String,
            content: String,
            #[serde(default = "default_true")]
            create_dirs: bool,
        }
        fn default_true() -> bool { true }

        let req: WriteReq = serde_json::from_str(&input_json)
            .context("oxi_write_file: invalid request JSON")?;

        validate_path_allowed(&req.path)?;

        if req.create_dirs {
            if let Some(parent) = std::path::Path::new(&req.path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| anyhow::anyhow!("Failed to create directories: {}", e))?;
            }
        }

        let bytes = req.content.len();
        std::fs::write(&req.path, &req.content)
            .map_err(|e| anyhow::anyhow!("Failed to write file: {}", e))?;

        let response = serde_json::json!({
            "success": true,
            "bytes_written": bytes,
        });
        let output = serde_json::to_string(&response)?;
        let handle = plugin.memory_new(&output)?;
        if !outputs.is_empty() {
            outputs[0] = plugin.memory_to_val(handle);
        }
        Ok(())
    })();
    result.map_err(extism::Error::from)
}

/// Host function: `oxi_exec(exec_json) → result_json`
///
/// Input: `{"command": "git status", "args": [], "cwd": ".", "timeout": 30}`
/// Output: `{"success": true, "stdout": "...", "stderr": "...", "exit_code": 0}` or error.
fn host_oxi_exec(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct ExecReq {
            command: String,
            #[serde(default)]
            args: Vec<String>,
            #[serde(default)]
            cwd: Option<String>,
            #[serde(default = "default_timeout")]
            timeout: u64,
        }
        fn default_timeout() -> u64 { 30 }

        let req: ExecReq = serde_json::from_str(&input_json)
            .context("oxi_exec: invalid request JSON")?;



        let cwd = req.cwd.as_deref().unwrap_or(".");

        // Build full command string for deny-list checking (command + args)
        let full_cmd = if req.args.is_empty() {
            req.command.clone()
        } else {
            format!("{} {}", req.command, req.args.join(" "))
        };

        // Block dangerous commands — deny-list (checks combined command+args)
        let blocked_patterns = [
            "rm -rf /", "rm -rf /*", "mkfs", "dd if=", "format ",
            ":(){ :|:& };:", "chmod 777 /", "chown root",
            "> /etc/", "> /boot/", "> /dev/",
            "dd of=/dev/", "mv / /",
        ];
        for blocked in &blocked_patterns {
            if full_cmd.contains(blocked) {
                anyhow::bail!("oxi_exec: blocked dangerous command pattern");
            }
        }

        // Block obvious privilege escalation
        let cmd_lower = req.command.to_lowercase();
        if cmd_lower == "sudo" || cmd_lower == "su" || cmd_lower == "doas"
            || cmd_lower.starts_with("sudo ") || cmd_lower.starts_with("su ") || cmd_lower.starts_with("doas ")
        {
            anyhow::bail!("oxi_exec: privilege escalation commands are blocked");
        }

        // Clamp timeout to 1-30 seconds to prevent abuse
        let timeout_ms = req.timeout.max(1000).min(30000);
        let timeout_dur = Duration::from_millis(timeout_ms);

        // Spawn child process with piped stdout/stderr
        let mut child = match std::process::Command::new(&req.command)
            .args(&req.args)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let response = serde_json::json!({
                    "success": false,
                    "error": format!("Failed to execute: {}", e),
                    "exit_code": -1,
                });
                let out = serde_json::to_string(&response)?;
                let handle = plugin.memory_new(&out)?;
                if !outputs.is_empty() { outputs[0] = plugin.memory_to_val(handle); }
                return Ok(());
            }
        };

        // Poll for completion with timeout enforcement
        let start = Instant::now();
        let mut timed_out = false;
        let mut exit_status: Option<std::process::ExitStatus> = None;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_status = Some(status);
                    break;
                }
                Ok(None) => {
                    if start.elapsed() >= timeout_dur {
                        // Timeout reached — kill the process and clean up
                        tracing::warn!(
                            "oxi_exec: command '{}' timed out after {}ms",
                            req.command, timeout_ms
                        );
                        let _ = child.kill();
                        let _ = child.wait(); // reap zombie
                        timed_out = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => {
                    // try_wait failed — fall back to blocking wait
                    match child.wait() {
                        Ok(status) => { exit_status = Some(status); }
                        Err(_) => { timed_out = true; }
                    }
                    break;
                }
            }
        }

        // Collect stdout/stderr from the child pipes (read what was buffered)
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = out.read_to_end(&mut stdout_buf);
        }
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_end(&mut stderr_buf);
        }

        let stdout = String::from_utf8_lossy(&stdout_buf);
        let stderr = String::from_utf8_lossy(&stderr_buf);
        let max_output = 50 * 1024; // 50KB
        let stdout_truncated = stdout.len() > max_output;
        let stderr_truncated = stderr.len() > max_output;
        let stdout_str: String = if stdout_truncated { stdout.chars().take(max_output).collect() } else { stdout.to_string() };
        let stderr_str: String = if stderr_truncated { stderr.chars().take(max_output).collect() } else { stderr.to_string() };

        let response = serde_json::json!({
            "success": !timed_out && exit_status.map(|s| s.success()).unwrap_or(false),
            "stdout": stdout_str,
            "stderr": stderr_str,
            "exit_code": if timed_out { -2 } else { exit_status.and_then(|s| s.code()).unwrap_or(-1) },
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "timed_out": timed_out,
        });
        let out = serde_json::to_string(&response)?;
        let handle = plugin.memory_new(&out)?;
        if !outputs.is_empty() {
            outputs[0] = plugin.memory_to_val(handle);
        }
        Ok(())
    })();
    result.map_err(extism::Error::from)
}

/// Host function: `oxi_get_env(key_json) → result_json`
///
/// Input: `{"key": "HOME"}`
/// Output: `{"success": true, "value": "/home/user"}` or `{"success": false, "error": "not found"}`
fn host_oxi_get_env(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct EnvReq {
            key: String,
        }

        let req: EnvReq = serde_json::from_str(&input_json)
            .context("oxi_get_env: invalid request JSON")?;

        // Block sensitive env vars
        let blocked_keys = ["AWS_SECRET", "PRIVATE_KEY", "PASSWORD", "TOKEN", "SECRET"];
        let key_upper = req.key.to_uppercase();
        for blocked in &blocked_keys {
            if key_upper.contains(blocked) {
                anyhow::bail!("oxi_get_env: access to '{}' is blocked", req.key);
            }
        }

        let value = std::env::var(&req.key).ok();
        let response = serde_json::json!({
            "success": value.is_some(),
            "value": value.unwrap_or_default(),
        });
        let output = serde_json::to_string(&response)?;
        let handle = plugin.memory_new(&output)?;
        if !outputs.is_empty() {
            outputs[0] = plugin.memory_to_val(handle);
        }
        Ok(())
    })();
    result.map_err(extism::Error::from)
}

// ── KV Store Host Functions ─────────────────────────────────────────

/// Host function: `oxi_kv_get(key_json) → result_json`
///
/// Persistent key-value store for extension state.
/// Keys are namespaced per extension.
/// Input: `{"key": "my_state"}`
/// Output: `{"success": true, "value": "..."}` or `{"success": false}`
fn host_oxi_kv_get(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct KvReq {
            key: String,
        }

        let req: KvReq = serde_json::from_str(&input_json)
            .context("oxi_kv_get: invalid request JSON")?;

        // Namespace the key with the current extension identity
        let ext_name = current_extension_name();
        let value = kv_namespaced_get(&ext_name, &req.key);
        let response = serde_json::json!({
            "success": value.is_some(),
            "value": value.unwrap_or_default(),
        });
        let output = serde_json::to_string(&response)?;
        let handle = plugin.memory_new(&output)?;
        if !outputs.is_empty() {
            outputs[0] = plugin.memory_to_val(handle);
        }
        Ok(())
    })();
    result.map_err(extism::Error::from)
}

/// Host function: `oxi_kv_set(set_json)`
///
/// Input: `{"key": "my_state", "value": "saved_data"}`
fn host_oxi_kv_set(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    _outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), extism::Error> {
    let result: anyhow::Result<()> = (|| {
        let input_json: String = plugin.memory_get_val(&inputs[0])?;

        #[derive(Deserialize)]
        struct KvSetReq {
            key: String,
            value: String,
        }

        let req: KvSetReq = serde_json::from_str(&input_json)
            .context("oxi_kv_set: invalid request JSON")?;

        // Namespace the key with the current extension identity
        let ext_name = current_extension_name();
        kv_namespaced_set(&ext_name, &req.key, &req.value);
        Ok(())
    })();
    result.map_err(extism::Error::from)
}

// ── KV Store Implementation ─────────────────────────────────────────

use std::sync::LazyLock;

static KV_STORE: LazyLock<parking_lot::RwLock<HashMap<String, String>>> =
    LazyLock::new(|| parking_lot::RwLock::new(HashMap::new()));

/// Thread-local tracking the currently executing extension name.
/// Set by `execute_tool` / `execute_command` / `load` before invoking plugin
/// calls, read by KV host functions to namespace keys.
thread_local! {
    static CURRENT_EXTENSION: RefCell<Option<String>> = RefCell::new(None);
}

/// Run a closure with the current extension name set in thread-local storage.
fn with_extension_context<F, R>(ext_name: &str, f: F) -> R
where
    F: FnOnce() -> R,
{
    CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = Some(ext_name.to_string()));
    let result = f();
    CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = None);
    result
}

/// Get the current extension name from thread-local storage.
/// Returns `"__unknown__"` if not set (e.g., during `load()` before `init()`).
fn current_extension_name() -> String {
    CURRENT_EXTENSION.with(|cell| {
        cell.borrow().clone().unwrap_or_else(|| "__unknown__".to_string())
    })
}

fn kv_store_get(key: &str) -> Option<String> {
    KV_STORE.read().get(key).cloned()
}

fn kv_store_set(key: &str, value: &str) {
    KV_STORE.write().insert(key.to_string(), value.to_string());
}

/// Namespaced KV access — prefixes key with `extension:` to prevent
/// cross-extension key collision.
fn kv_namespaced_get(extension: &str, key: &str) -> Option<String> {
    let namespaced = format!("{}:{}", extension, key);
    kv_store_get(&namespaced)
}

fn kv_namespaced_set(extension: &str, key: &str, value: &str) {
    let namespaced = format!("{}:{}", extension, key);
    kv_store_set(&namespaced, value);
}

// ── Path Validation ─────────────────────────────────────────────────

/// Validate that a path is within the allowed directories.
/// Extensions can only read/write outside protected system paths.
fn validate_path_allowed(path: &str) -> Result<()> {
    let p = std::path::Path::new(path);

    // Resolve to absolute, then canonicalize to eliminate ../ and symlinks
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };

    // Try canonicalize for existing paths; fall back to resolved absolute
    let resolved = if abs.exists() {
        abs.canonicalize().unwrap_or(abs)
    } else {
        // For new files, resolve parent if it exists
        if let Some(parent) = abs.parent() {
            if parent.exists() {
                let canon_parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
                canon_parent.join(abs.file_name().unwrap_or_default())
            } else {
                abs
            }
        } else {
            abs
        }
    };

    let abs_str = resolved.to_string_lossy();

    // Block sensitive system paths
    let blocked_prefixes = [
        "/etc", "/sys", "/proc", "/dev", "/boot", "/root",
        "/System", "/Library/System",
        "/usr/bin", "/usr/sbin", "/bin", "/sbin",
    ];
    for prefix in &blocked_prefixes {
        if abs_str.starts_with(prefix) {
            anyhow::bail!("Path '{}' is in a protected system directory", path);
        }
    }

    // Block hidden sensitive paths in home directory
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if abs_str.starts_with(&*home_str) {
            let blocked_home_suffixes = [
                "/.ssh/", "/.gnupg/", "/.aws/", "/.config/gcloud/",
                "/.kube/", "/.docker/", "/.npmrc", "/.netrc",
            ];
            for suffix in &blocked_home_suffixes {
                if abs_str.contains(suffix) {
                    anyhow::bail!("Path '{}' is in a protected directory", path);
                }
            }
        }
    }

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
///
/// # Thread Safety
/// `plugins` is wrapped in `Arc<Mutex<>>` for exclusive thread-safe access.
/// All host functions run inside `spawn_blocking`, so WASM execution
/// never blocks the async runtime.
pub struct WasmExtensionManager {
    extensions: HashMap<String, LoadedWasmExtension>,
    /// Raw Extism plugin references — needed for execute_tool calls.
    /// Wrapped in Mutex for exclusive thread-safe access (required because
    /// extism::Plugin's Send+Sync bounds are not publicly documented).
    /// All access goes through `plugins.lock()` which ensures mutual exclusion.
    pub(crate) plugins: Arc<parking_lot::Mutex<HashMap<String, extism::Plugin>>>,
    /// Maps tool name → extension name.
    tool_to_ext: HashMap<String, String>,
    /// HTTP client shared by all extensions for oxi_http_request.
    http_client: Arc<reqwest::blocking::Client>,
    /// Permissions granted to each extension (ext_name → set of permission strings).
    permissions: HashMap<String, std::collections::HashSet<String>>,
}

impl WasmExtensionManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self {
            extensions: HashMap::new(),
            plugins: Arc::new(Mutex::new(HashMap::new())),
            tool_to_ext: HashMap::new(),
            http_client: Arc::new(
                reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .no_proxy() // Prevent proxy-based SSRF
                    .build()
                    .expect("Failed to build HTTP client")
            ),
            permissions: HashMap::new(),
        }
    }

    /// Create with a custom HTTP client (useful for testing with a mock client).
    pub fn with_http_client(client: reqwest::blocking::Client) -> Self {
        Self {
            extensions: HashMap::new(),
            plugins: Arc::new(Mutex::new(HashMap::new())),
            tool_to_ext: HashMap::new(),
            http_client: Arc::new(client),
            permissions: HashMap::new(),
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

        let read_fn = Function::new(
            "oxi_read_file",
            [PTR],
            [PTR],
            UserData::new(()),
            host_oxi_read_file,
        );

        let write_fn = Function::new(
            "oxi_write_file",
            [PTR],
            [PTR],
            UserData::new(()),
            host_oxi_write_file,
        );

        let exec_fn = Function::new(
            "oxi_exec",
            [PTR],
            [PTR],
            UserData::new(()),
            host_oxi_exec,
        );

        let get_env_fn = Function::new(
            "oxi_get_env",
            [PTR],
            [PTR],
            UserData::new(()),
            host_oxi_get_env,
        );

        let kv_get_fn = Function::new(
            "oxi_kv_get",
            [PTR],
            [PTR],
            UserData::new(()),
            host_oxi_kv_get,
        );

        let kv_set_fn = Function::new(
            "oxi_kv_set",
            [PTR],
            [],
            UserData::new(()),
            host_oxi_kv_set,
        );

        vec![http_fn, log_fn, read_fn, write_fn, exec_fn, get_env_fn, kv_get_fn, kv_set_fn]
    }

    /// Load a single `.wasm` extension.
    pub fn load(&mut self, path: &Path) -> Result<ExtensionInfo> {
        let path_display = path.display().to_string();
        tracing::info!("Loading WASM extension: {}", path_display);

        let wasm_bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read extension: {}", path_display))?;

        let wasm = extism::Wasm::data(wasm_bytes);
        // Limit WASM memory to 64 pages (4MB) to prevent unbounded allocation
        let manifest = extism::Manifest::new([wasm]).with_memory_max(64);
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
                    permissions: vec![],
                }
            }
        };

        // Set extension context so KV operations during register_tools/register_commands
        // are properly namespaced
        let ext_name_for_ctx = info.name.clone();
        CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = Some(ext_name_for_ctx));
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

        // Call register_commands() — optional
        let commands: Vec<WasmCommandDef> = match plugin.call::<&str, &str>("register_commands", "{}") {
            Ok(output) => {
                let resp: Value = serde_json::from_str(output)
                    .with_context(|| format!("register_commands() invalid JSON: {}", output))?;
                resp.get("commands")
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
            Err(_) => vec![], // No commands
        };

        // Clear extension context set during register_tools/register_commands
        CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = None);

        let ext_name = info.name.clone();

        // Warn on name collision — clean up old extension fully
        if self.extensions.contains_key(&ext_name) {
            tracing::warn!(
                "Extension '{}' already loaded, replacing with '{}'",
                ext_name, path_display
            );
            // Remove old tool mappings
            self.tool_to_ext.retain(|_, v| v != &ext_name);
            // Remove old plugin instance
            self.plugins.lock().remove(&ext_name);
        }

        for tool in &tools {
            self.tool_to_ext.insert(tool.name.clone(), ext_name.clone());
        }

        let loaded = LoadedWasmExtension {
            info: info.clone(),
            tools,
            commands,
            source_path: path.to_path_buf(),
        };

        self.extensions.insert(ext_name.clone(), loaded);
        self.plugins.lock().insert(ext_name, plugin);

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
        let ext_name = self
            .tool_to_ext
            .get(tool_name)
            .with_context(|| format!("No extension registered for tool: {}", tool_name))?
            .clone();

        let mut plugins = self.plugins.lock();
        let plugin = plugins
            .get_mut(&ext_name)
            .with_context(|| format!("Extension '{}' not loaded", ext_name))?;

        let input = serde_json::json!({
            "tool": tool_name,
            "params": params,
        });
        let input_str = serde_json::to_string(&input)?;

        // Set thread-local extension context so KV host functions can namespace keys
        CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = Some(ext_name.clone()));
        let call_result = plugin.call("execute_tool", &input_str);
        CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = None);

        let output: &str = call_result
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

    // ── Commands ──────────────────────────────────────────────────

    /// Get all command definitions from all loaded extensions.
    pub fn all_command_defs(&self) -> Vec<(&str, &WasmCommandDef)> {
        let mut cmds = Vec::new();
        for ext in self.extensions.values() {
            for cmd in &ext.commands {
                cmds.push((ext.info.name.as_str(), cmd));
            }
        }
        cmds
    }

    /// Execute a command via the WASM extension.
    /// Returns the output text to display to the user.
    pub fn execute_command(&self, command_name: &str, args: &str) -> Result<String> {
        // Find which extension owns this command
        let ext_name = self.extensions.iter()
            .find(|(_, ext)| ext.commands.iter().any(|c| c.name == command_name))
            .map(|(name, _)| name.clone())
            .with_context(|| format!("No extension registered for command: /{}", command_name))?;

        let mut plugins = self.plugins.lock();
        let plugin = plugins.get_mut(&ext_name)
            .with_context(|| format!("Extension '{}' not loaded", ext_name))?;

        let input = serde_json::json!({
            "command": command_name,
            "args": args,
        });
        let input_str = serde_json::to_string(&input)?;

        let output: &str = {
            CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = Some(ext_name.clone()));
            let result = plugin.call("execute_command", &input_str);
            CURRENT_EXTENSION.with(|cell| *cell.borrow_mut() = None);
            result
        }
        .with_context(|| format!("execute_command('/{}') failed in '{}'", command_name, ext_name))?;

        // Parse response — extension returns {"output": "..."} or plain string
        let result: Value = serde_json::from_str(output)
            .unwrap_or_else(|_| serde_json::json!({"output": output}));

        Ok(result.get("output")
            .and_then(|v| v.as_str())
            .unwrap_or(output)
            .to_string())
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
