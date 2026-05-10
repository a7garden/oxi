//! MCP (Model Context Protocol) integration.
//!
//! Provides a built-in `mcp` tool that acts as a gateway to MCP servers.
//! Supports:
//! - **stdio transport**: spawn MCP server processes, communicate via JSON-RPC
//! - **Lazy connection**: connect on first tool call, cache tool metadata
//! - **Tool discovery**: search, describe, and call MCP tools through a unified proxy
//! - **Config loading**: discover config from `~/.config/oxi/mcp.json`, `.mcp.json`, etc.
//!
//! # Architecture
//!
//! ```text
//! McpTool (AgentTool) ──→ McpManager ──→ McpClient (per server)
//!                                         ├── JSON-RPC over stdio
//!                                         ├── Tool discovery (tools/list)
//!                                         └── Tool execution (tools/call)
//! ```
//!
//! # Config format
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "my-server": {
//!       "command": "npx",
//!       "args": ["-y", "@my-org/mcp-server"],
//!       "env": { "API_KEY": "..." }
//!     }
//!   },
//!   "settings": {
//!     "toolPrefix": "server"
//!   }
//! }
//! ```

pub mod client;
pub mod config;
pub mod content;
pub mod tool;
pub mod types;

pub use client::McpClient;
pub use tool::McpTool;
pub use types::{
    effective_prefix_mode, format_schema, format_tool_name, get_server_prefix, McpCallResult,
    McpConfig, McpContent, McpSettings, McpToolDef, ServerEntry, ServerInfo, ServerStatus,
    ToolMetadata, ToolPrefix,
};

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::time::Instant;

/// Back-off period after a server connection failure (seconds).
const FAILURE_BACKOFF_SECS: u64 = 60;

/// Inner mutable state for [`McpManager`].
struct McpManagerInner {
    /// Connected MCP clients (server name → client).
    clients: HashMap<String, McpClient>,
    /// Discovered tool metadata (server name → tool list).
    tool_metadata: HashMap<String, Vec<ToolMetadata>>,
    /// Connection failure timestamps.
    failure_tracker: HashMap<String, Instant>,
}

/// Central manager for all MCP server connections.
///
/// Thread-safe via `tokio::sync::Mutex`. Holds config, connections,
/// and cached tool metadata.
pub struct McpManager {
    inner: tokio::sync::Mutex<McpManagerInner>,
    config: parking_lot::RwLock<McpConfig>,
}

impl McpManager {
    /// Create a new manager, loading config from standard locations.
    pub fn new() -> Self {
        let config = config::load_mcp_config();
        Self {
            inner: tokio::sync::Mutex::new(McpManagerInner {
                clients: HashMap::new(),
                tool_metadata: HashMap::new(),
                failure_tracker: HashMap::new(),
            }),
            config: parking_lot::RwLock::new(config),
        }
    }

    /// Create with a specific config (useful for testing).
    pub fn with_config(config: McpConfig) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(McpManagerInner {
                clients: HashMap::new(),
                tool_metadata: HashMap::new(),
                failure_tracker: HashMap::new(),
            }),
            config: parking_lot::RwLock::new(config),
        }
    }

    /// Get a snapshot of the current config.
    pub fn config(&self) -> parking_lot::RwLockReadGuard<'_, McpConfig> {
        self.config.read()
    }

    // ── Status ────────────────────────────────────────────────────

    /// Get a formatted status summary of all configured servers.
    pub async fn status(&self) -> String {
        let inner = self.inner.lock().await;
        let config = self.config.read();
        let servers = &config.mcp_servers;

        if servers.is_empty() {
            return "MCP: No servers configured. Create ~/.config/oxi/mcp.json or .mcp.json".to_string();
        }

        let mut text = String::new();
        let mut connected_count = 0;
        let mut total_tools = 0;

        for name in servers.keys() {
            let (status_str, tool_count) = if let Some(_client) = inner.clients.get(name) {
                connected_count += 1;
                let count = inner
                    .tool_metadata
                    .get(name)
                    .map(|m| m.len())
                    .unwrap_or(0);
                total_tools += count;
                ("✓ connected".to_string(), count)
            } else if let Some(failed_at) = inner.failure_tracker.get(name) {
                let ago = failed_at.elapsed().as_secs();
                if ago < FAILURE_BACKOFF_SECS {
                    (format!("✗ failed ({}s ago)", ago), 0)
                } else {
                    ("○ not connected".to_string(), 0)
                }
            } else {
                let count = inner
                    .tool_metadata
                    .get(name)
                    .map(|m| m.len())
                    .unwrap_or(0);
                total_tools += count;
                if count > 0 {
                    ("○ cached".to_string(), count)
                } else {
                    ("○ not connected".to_string(), 0)
                }
            };

            text.push_str(&format!(
                "{} {} ({} tools)\n",
                if status_str.starts_with('✓') {
                    "✓"
                } else if status_str.starts_with('✗') {
                    "✗"
                } else {
                    "○"
                },
                name,
                tool_count
            ));
        }

        format!(
            "MCP: {}/{} servers, {} tools\n\n{}",
            connected_count,
            servers.len(),
            total_tools,
            text.trim_end()
        )
    }

    // ── Connect ───────────────────────────────────────────────────

    /// Connect to a specific MCP server by name.
    pub async fn connect(&self, server_name: &str) -> Result<String> {
        let (command, args, env, cwd, debug) = {
            let config = self.config.read();
            let entry = config
                .mcp_servers
                .get(server_name)
                .ok_or_else(|| anyhow::anyhow!("Server '{}' not found", server_name))?;

            let command = entry
                .command
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Server '{}' has no command configured", server_name))?;
            let args = entry.args.clone().unwrap_or_default();
            let env = entry.env.clone().unwrap_or_default();
            let cwd = entry.cwd.clone();
            let debug = entry.debug.unwrap_or(false);
            (command, args, env, cwd, debug)
        };

        let mut client = McpClient::connect(
            &command,
            &args,
            &env,
            cwd.as_deref(),
            debug,
        )
        .await
        .with_context(|| format!("Failed to connect to MCP server '{}'", server_name))?;

        // Discover tools
        let tools = client.list_tools().await.unwrap_or_default();
        let prefix_mode = effective_prefix_mode(self.config.read().settings.as_ref());

        let metadata: Vec<ToolMetadata> = tools
            .iter()
            .map(|t| ToolMetadata {
                name: format_tool_name(&t.name, server_name, &prefix_mode),
                original_name: t.name.clone(),
                server_name: server_name.to_string(),
                description: t.description.clone().unwrap_or_default(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        let tool_names: Vec<String> = metadata.iter().map(|m| m.name.clone()).collect();

        let mut inner = self.inner.lock().await;
        inner.clients.insert(server_name.to_string(), client);
        inner
            .tool_metadata
            .insert(server_name.to_string(), metadata);
        inner.failure_tracker.remove(server_name);

        if tool_names.is_empty() {
            Ok(format!("Connected to '{}' — no tools available.", server_name))
        } else {
            Ok(format!(
                "Connected to '{}' ({} tools):\n\n{}",
                server_name,
                tool_names.len(),
                tool_names
                    .iter()
                    .map(|n| format!("- {}", n))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        }
    }

    /// Lazily connect to a server if not already connected.
    /// Returns `true` if connected (or was already connected).
    async fn lazy_connect(&self, server_name: &str) -> bool {
        // Check existing connection
        {
            let inner = self.inner.lock().await;
            if inner.clients.contains_key(server_name) {
                return true;
            }
            // Check backoff
            if let Some(failed_at) = inner.failure_tracker.get(server_name) {
                if failed_at.elapsed().as_secs() < FAILURE_BACKOFF_SECS {
                    return false;
                }
            }
        }

        // Try to connect
        match self.connect(server_name).await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("MCP: lazy connect failed for {}: {}", server_name, e);
                let inner = &mut *self.inner.lock().await;
                inner
                    .failure_tracker
                    .insert(server_name.to_string(), Instant::now());
                false
            }
        }
    }

    // ── Tool operations ───────────────────────────────────────────

    /// Call an MCP tool by name, optionally targeting a specific server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        server_override: Option<&str>,
    ) -> Result<McpCallResult> {
        // Find tool across servers
        let (server_name, tool_meta) = self.find_tool(tool_name, server_override).await?;

        // Ensure connected
        self.lazy_connect(&server_name).await;

        // Call the tool
        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(&server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;

        let result = client
            .call_tool(&tool_meta.original_name, args)
            .await
            .with_context(|| format!("Tool '{}' call failed", tool_name))?;

        let text = content::transform_mcp_content(&result.content);

        Ok(McpCallResult {
            content: vec![McpContent::Text { text }],
            is_error: result.is_error,
        })
    }

    /// Describe a tool by name.
    pub async fn describe(&self, tool_name: &str) -> Result<String> {
        let (_server_name, tool_meta) = self.find_tool(tool_name, None).await?;

        let mut text = format!("{}\n", tool_meta.name);
        text.push_str(&format!("Server: {}\n", tool_meta.server_name));
        text.push_str(&format!("\n{}\n", tool_meta.description));

        if let Some(ref schema) = tool_meta.input_schema {
            text.push_str(&format!(
                "\nParameters:\n{}",
                format_schema(schema, "  ")
            ));
        } else {
            text.push_str("\nNo parameters defined.");
        }

        Ok(text)
    }

    /// Search tools by name or description.
    pub async fn search(
        &self,
        query: &str,
        regex: bool,
        server_filter: Option<&str>,
    ) -> Result<String> {
        let pattern = if regex {
            regex::Regex::new(query)
                .with_context(|| format!("Invalid regex: {}", query))?
        } else {
            let terms: Vec<&str> = query.split_whitespace().collect();
            if terms.is_empty() {
                return Ok("Search query cannot be empty".to_string());
            }
            let escaped: Vec<String> = terms
                .iter()
                .map(|t| regex::escape(t))
                .collect();
            regex::Regex::new(&format!("(?i){}", escaped.join("|")))
                .context("Invalid search pattern")?
        };

        let inner = self.inner.lock().await;
        let mut matches = Vec::new();

        for (server_name, metadata) in &inner.tool_metadata {
            if let Some(filter) = server_filter {
                if server_name != filter {
                    continue;
                }
            }
            for tool in metadata {
                if pattern.is_match(&tool.name) || pattern.is_match(&tool.description) {
                    matches.push((server_name.clone(), tool.clone()));
                }
            }
        }

        if matches.is_empty() {
            let msg = if let Some(s) = server_filter {
                format!("No tools matching \"{}\" in \"{}\"", query, s)
            } else {
                format!("No tools matching \"{}\"", query)
            };
            return Ok(msg);
        }

        let mut text = format!(
            "Found {} tool{} matching \"{}\":\n\n",
            matches.len(),
            if matches.len() == 1 { "" } else { "s" },
            query
        );

        for (_server, tool) in &matches {
            text.push_str(&format!("{}\n", tool.name));
            if !tool.description.is_empty() {
                text.push_str(&format!("  {}\n", tool.description));
            }
            if let Some(ref schema) = tool.input_schema {
                text.push_str(&format!(
                    "  Parameters:\n{}\n",
                    format_schema(schema, "    ")
                ));
            }
            text.push('\n');
        }

        Ok(text.trim_end().to_string())
    }

    /// List tools for a specific server.
    pub async fn list_tools(&self, server_name: &str) -> Result<String> {
        // Check if server exists in config
        {
            let config = self.config.read();
            if !config.mcp_servers.contains_key(server_name) {
                return Ok(format!(
                    "Server '{}' not found. Use mcp({{}}) to see available servers.",
                    server_name
                ));
            }
        }

        // Try to connect if not already
        self.lazy_connect(server_name).await;

        let inner = self.inner.lock().await;
        let metadata = inner.tool_metadata.get(server_name);

        match metadata {
            Some(tools) if !tools.is_empty() => {
                let mut text = format!("{} ({} tools):\n\n", server_name, tools.len());
                for tool in tools {
                    text.push_str(&format!("- {}", tool.name));
                    if !tool.description.is_empty() {
                        let desc: String = tool.description.chars().take(60).collect();
                        text.push_str(&format!(" - {}", desc));
                    }
                    text.push('\n');
                }
                Ok(text.trim_end().to_string())
            }
            Some(_) => Ok(format!("Server '{}' has no tools.", server_name)),
            None => Ok(format!(
                "Server '{}' is configured but not connected. Use mcp({{ connect: \"{}\" }}) to connect.",
                server_name, server_name
            )),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────

    /// Find a tool by name across all servers.
    ///
    /// Tries exact match first, then falls back to prefix-based matching
    /// (e.g., `my_server_tool` → server `my-server`, tool `tool`).
    async fn find_tool(
        &self,
        tool_name: &str,
        server_override: Option<&str>,
    ) -> Result<(String, ToolMetadata)> {
        // If server specified, search only that server
        if let Some(server) = server_override {
            let config = self.config.read();
            if !config.mcp_servers.contains_key(server) {
                return Err(anyhow::anyhow!("Server '{}' not found", server));
            }
        }

        // Search tool metadata
        {
            let inner = self.inner.lock().await;

            // Exact match with server filter
            if let Some(server) = server_override {
                if let Some(metadata) = inner.tool_metadata.get(server) {
                    if let Some(tool) = metadata.iter().find(|t| t.name == tool_name) {
                        return Ok((server.to_string(), tool.clone()));
                    }
                    // Also check original name
                    if let Some(tool) = metadata
                        .iter()
                        .find(|t| t.original_name == tool_name)
                    {
                        return Ok((server.to_string(), tool.clone()));
                    }
                }
            }

            // Search all servers
            for (server_name, metadata) in &inner.tool_metadata {
                if let Some(server) = server_override {
                    if server_name != server {
                        continue;
                    }
                }
                // Exact name match
                if let Some(tool) = metadata.iter().find(|t| t.name == tool_name) {
                    return Ok((server_name.clone(), tool.clone()));
                }
                // Original name match
                if let Some(tool) = metadata
                    .iter()
                    .find(|t| t.original_name == tool_name)
                {
                    return Ok((server_name.clone(), tool.clone()));
                }
            }
        }

        // Try prefix-based matching (lazy connect candidates)
        let prefix_mode = effective_prefix_mode(self.config.read().settings.as_ref());

        // Collect candidates while holding the config lock
        let candidates: Vec<String> = {
            let config = self.config.read();
            config.mcp_servers.keys()
                .filter(|server_name: &&String| {
                    if let Some(server) = server_override {
                        server_name.as_str() == server
                    } else {
                        true
                    }
                })
                .filter(|server_name| {
                    let prefix = get_server_prefix(server_name, &prefix_mode);
                    !prefix.is_empty() && tool_name.starts_with(&format!("{}_", prefix))
                })
                .cloned()
                .collect()
        }; // config lock released here

        // Try connecting to candidates
        for server_name in &candidates {
            self.lazy_connect(server_name).await;

            let inner = self.inner.lock().await;
            if let Some(metadata) = inner.tool_metadata.get(server_name) {
                if let Some(tool) = metadata.iter().find(|t| t.name == tool_name) {
                    return Ok((server_name.clone(), tool.clone()));
                }
            }
        }

        // Not found — provide helpful error
        let inner = self.inner.lock().await;
        let mut hint_servers = Vec::new();
        for (server_name, metadata) in &inner.tool_metadata {
            let names: Vec<&str> = metadata.iter().map(|t| t.name.as_str()).collect();
            if !names.is_empty() {
                hint_servers.push(format!("{}: {}", server_name, names.join(", ")));
            }
        }

        let mut msg = format!("Tool '{}' not found.", tool_name);
        if !hint_servers.is_empty() {
            msg.push_str(&format!(
                "\n\nAvailable tools:\n{}",
                hint_servers
                    .iter()
                    .map(|s| format!("  {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        } else {
            msg.push_str(" Use mcp({ search: \"...\" }) to search.");
        }

        Err(anyhow::anyhow!(msg))
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
