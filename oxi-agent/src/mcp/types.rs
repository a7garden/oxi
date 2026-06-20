//! Config format
//! ...

#![allow(missing_docs)]
#![allow(clippy::unwrap_used)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Configuration types ──────────────────────────────────────────────

/// MCP server configuration entry.
///
/// Supports both stdio (command-based) and HTTP transports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Command to start the MCP server process (stdio transport).
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments passed to the command.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Additional environment variables for the server process.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Working directory for the server process.
    #[serde(default)]
    pub cwd: Option<String>,
    /// HTTP URL for HTTP/SSE transport.
    #[serde(default)]
    pub url: Option<String>,
    /// HTTP headers to include when connecting.
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    /// Server lifecycle mode.
    #[serde(default)]
    pub lifecycle: Option<LifecycleMode>,
    /// Idle timeout in minutes (overrides global setting).
    #[serde(default, rename = "idleTimeout", alias = "idle_timeout")]
    pub idle_timeout: Option<u64>,
    /// Show server stderr output (default: false).
    #[serde(default)]
    pub debug: Option<bool>,
    /// Direct tools registration config (Phase 3).
    #[serde(default, rename = "directTools", alias = "direct_tools")]
    pub direct_tools: Option<DirectToolsConfig>,
    /// Tools to exclude from direct/proxy registration (Phase 3).
    #[serde(default, rename = "excludeTools", alias = "exclude_tools")]
    pub exclude_tools: Option<Vec<String>>,
    /// Per-request timeout in milliseconds (`0` disables).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// OAuth2 client credentials for this server (v2.2). When present,
    /// the credential provider performs a `client_credentials` grant
    /// against `token_url` to obtain (and refresh) an `Authorization:
    /// Bearer …` header for HTTP transports.
    #[serde(default)]
    pub oauth: Option<OAuthConfig>,
}

/// OAuth2 client credentials configuration for an MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// Token endpoint (e.g. `https://auth.example.com/oauth/token`).
    #[serde(rename = "tokenUrl", alias = "token_url")]
    pub token_url: String,
    /// OAuth2 client id.
    #[serde(rename = "clientId", alias = "client_id")]
    pub client_id: String,
    /// OAuth2 client secret.
    #[serde(rename = "clientSecret", alias = "client_secret")]
    pub client_secret: String,
    /// Optional scope to request.
    #[serde(default)]
    pub scope: Option<String>,
}
/// Server lifecycle modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleMode {
    /// Keep connection alive, auto-reconnect on failure.
    KeepAlive,
    /// Connect on first use, disconnect after idle timeout.
    Lazy,
    /// Connect eagerly at startup.
    Eager,
}

/// Global MCP settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpSettings {
    /// Tool name prefix mode.
    #[serde(default, rename = "toolPrefix", alias = "tool_prefix")]
    pub tool_prefix: Option<ToolPrefix>,
    /// Global idle timeout in minutes (default: 10).
    #[serde(default, rename = "idleTimeout", alias = "idle_timeout")]
    pub idle_timeout: Option<u64>,
    /// Back-off period in seconds after a server connection failure (default: 30).
    #[serde(default, rename = "failureBackoffSecs", alias = "failure_backoff_secs")]
    pub failure_backoff_secs: Option<u64>,
    /// Global default for direct tools registration (Phase 3).
    #[serde(default, rename = "directTools", alias = "direct_tools")]
    pub direct_tools: Option<DirectToolsConfig>,
    /// If true, the `mcp` proxy tool is hidden when direct tools cover
    /// all configured servers (Phase 3).
    #[serde(default, rename = "disableProxyTool", alias = "disable_proxy_tool")]
    pub disable_proxy_tool: Option<bool>,
    /// Opt-in: also read MCP server definitions from third-party tools
    /// (`.claude/mcp.json`, `.cursor/mcp.json`, ...) under the current
    /// project. Oxi's own `.oxi/mcp.json` always wins. Default: false
    /// (preserves oxi's identity; enable to ease adoption from
    /// Claude/Cursor). See design §9.2 / G8.
    #[serde(
        default,
        rename = "discoverExternalConfigs",
        alias = "discover_external_configs"
    )]
    pub discover_external_configs: Option<bool>,
}

/// Tool name prefix strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolPrefix {
    /// `{server_name}_{tool_name}` (default).
    Server,
    /// No prefix.
    None,
    /// Short server name prefix.
    Short,
}

/// Root MCP configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// Map of server name → server definition.
    #[serde(rename = "mcpServers", alias = "mcp_servers")]
    pub mcp_servers: HashMap<String, ServerEntry>,
    /// Global settings override.
    pub settings: Option<McpSettings>,
}

// ── MCP protocol types ───────────────────────────────────────────────

/// Tool definition discovered from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Tool name (unique within the server).
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Option<serde_json::Value>,
}

/// Cached tool metadata with server association and naming.
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    /// Prefixed tool name (e.g. `my_server_list_files`).
    pub name: String,
    /// Original MCP tool name.
    pub original_name: String,
    /// Server that provides this tool.
    pub server_name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for parameters.
    pub input_schema: Option<serde_json::Value>,
}

/// Content types returned by MCP tool calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    /// Text content.
    #[serde(rename = "text")]
    Text { text: String },
    /// Image content (base64-encoded).
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(default)]
        mime_type: Option<String>,
    },
    /// Embedded resource content.
    #[serde(rename = "resource")]
    Resource { resource: ResourceContent },
}

/// Embedded resource returned by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    pub text: Option<String>,
    pub blob: Option<String>,
}

/// Server info returned from the MCP `initialize` handshake.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
    pub protocol_version: String,
}

/// Connection status of an MCP server.
#[derive(Debug, Clone)]
pub enum ServerStatus {
    /// Server is connected and ready.
    Connected,
    /// Connection failed with an error message.
    Failed(String),
    /// Server has not been connected yet.
    NotConnected,
}

/// Result of an MCP tool call.
#[derive(Debug, Clone)]
pub struct McpCallResult {
    /// Content blocks returned by the tool.
    pub content: Vec<McpContent>,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

// ── JSON-RPC protocol types ──────────────────────────────────────────

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 notification (no response expected).
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Raw JSON-RPC message (can be request, response, or notification).
#[derive(Debug, Clone, Deserialize)]
pub struct RawJsonRpcMessage {
    pub jsonrpc: String,
    pub id: Option<u64>,

    pub method: Option<String>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

// ── Naming helpers ───────────────────────────────────────────────────

/// Get the prefix string for a server name.
pub fn get_server_prefix(server_name: &str, mode: &ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let short = server_name
                .trim_end_matches("-mcp")
                .trim_end_matches("_mcp")
                .replace('-', "_");
            if short.is_empty() {
                "mcp".to_string()
            } else {
                short
            }
        }
        ToolPrefix::Server => server_name.replace('-', "_"),
    }
}

/// Format a tool name with server prefix.
pub fn format_tool_name(tool_name: &str, server_name: &str, mode: &ToolPrefix) -> String {
    let prefix = get_server_prefix(server_name, mode);
    if prefix.is_empty() {
        tool_name.to_string()
    } else {
        format!("{}_{}", prefix, tool_name)
    }
}

/// Get the effective prefix mode from settings (defaults to Server).
pub fn effective_prefix_mode(settings: Option<&McpSettings>) -> ToolPrefix {
    settings
        .and_then(|s| s.tool_prefix.clone())
        .unwrap_or(ToolPrefix::Server)
}

/// Format a JSON Schema into a human-readable string.
pub fn format_schema(schema: &serde_json::Value, indent: &str) -> String {
    let s = match schema.as_object() {
        Some(obj) => obj,
        None => return format!("{indent}(no schema)"),
    };

    let schema_type = s.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let properties = s.get("properties").and_then(|p| p.as_object());
    let required = s
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if schema_type == "object"
        && let Some(props) = properties
    {
        if props.is_empty() {
            return format!("{indent}(no parameters)");
        }
        let mut lines = Vec::new();
        for (name, prop_schema) in props {
            let is_required = required.iter().any(|r| r == name);
            let type_str = prop_schema
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("any");
            let desc = prop_schema
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let req_mark = if is_required { " *required*" } else { "" };
            let desc_part = if desc.is_empty() {
                String::new()
            } else {
                format!(" - {desc}")
            };
            lines.push(format!("{indent}{name} ({type_str}){req_mark}{desc_part}"));
        }
        return lines.join("\n");
    }

    format!("{indent}({schema_type})")
}

// ── Phase 3: Direct tools / consent ───────────────────────────────

/// Configuration for direct tool registration (Phase 3).
///
/// Can be a boolean (all tools) or a list of specific tool names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DirectToolsConfig {
    /// `true` = register all tools as direct, `false` = proxy only.
    All(bool),
    /// Register only these specific tools as direct (by original name).
    Specific(Vec<String>),
}

impl Default for DirectToolsConfig {
    fn default() -> Self {
        DirectToolsConfig::All(false)
    }
}

/// MCP tool execution consent state (Phase 3, extended in Phase 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConsentState {
    /// Always allow execution.
    #[default]
    Allow,
    /// Always deny execution.
    Deny,
}

/// Definition for a tool to be registered as a direct `AgentTool` (Phase 3).
#[derive(Debug, Clone)]
pub struct DirectToolDef {
    /// Prefixed tool name (already computed at registration time so that
    /// `AgentTool::name() -> &str` can return a reference into `self`).
    pub prefixed_name: String,
    /// Original (unprefixed) MCP tool name.
    pub original_name: String,
    /// Server that provides this tool.
    pub server_name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for parameters.
    pub input_schema: Option<serde_json::Value>,
}

// ── Phase 2: TUI dashboard data ────────────────────────────────────

/// Connection status of an MCP server (Phase 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpConnectionStatus {
    /// Server is connected and ready.
    Connected,
    /// Server is configured but not connected (lazy).
    Disconnected,
    /// Connection attempt is in progress.
    Connecting,
    /// Connection failed with an error message.
    Error(String),
}

/// One server's information for the TUI dashboard (Phase 2).
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub status: McpConnectionStatus,
    /// Human-readable lifecycle string ("lazy", "eager", "keep-alive", "none").
    pub lifecycle: String,
    /// Number of tools (cached or live).
    pub tool_count: usize,
    /// Per-tool information (empty if server is not connected and has no cache).
    pub tools: Vec<McpToolInfo>,
}

/// One tool's information for the TUI dashboard (Phase 2).
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    /// Prefixed tool name.
    pub name: String,
    /// Original (unprefixed) tool name.
    pub original_name: String,
    pub description: String,
    /// Whether this tool is registered as a direct `AgentTool` (Phase 3).
    pub is_direct: bool,
    /// Current consent state (Phase 3).
    pub consent: ConsentState,
}

/// Settings summary for the dashboard header.
#[derive(Debug, Clone)]
pub struct McpSettingsView {
    /// Current tool prefix mode as a string ("server", "short", "none").
    pub tool_prefix: String,
    /// Global idle timeout in minutes.
    pub idle_timeout: Option<u64>,
    /// Total number of configured servers.
    pub total_servers: usize,
    /// Number of currently connected servers.
    pub connected_servers: usize,
    /// Total number of known tools (across all servers).
    pub total_tools: usize,
}

/// The full structured snapshot consumed by the TUI dashboard (Phase 2).
#[derive(Debug, Clone)]
pub struct McpDashboardData {
    pub servers: Vec<McpServerInfo>,
    pub settings: McpSettingsView,
}
