//! MCP (Model Context Protocol) integration.
//!
//! Provides a built-in `mcp` tool that acts as a gateway to MCP servers,
//! plus a per-tool direct-registration path (Phase 3) and a disk-backed
//! metadata cache (Phase 1) that lets `search` / `list` / `describe`
//! work without a live connection.
//!
//! # Architecture
//!
//! ```text
//! McpTool (AgentTool) ──┐
//! McpDirectTool  (x N) ─┴─→ McpManager ─→ McpClient (per server, Transport-based)
//!                         │       ├── JSON-RPC over transport (stdio / http_sse)
//!                         │       ├── Metadata cache (disk-backed)
//!                         │       ├── Consent manager (disk-backed)
//!                         │       └── Lifecycle task (mpsc, owns idle/health timers)
//! ```
//!
//! # Concurrency
//!
//! `McpManager` is internally `Arc<McpManager>` after `spawn()`. The
//! lifecycle timer task receives a `Weak<McpManager>` so it never
//! participates in a reference cycle. The inner state is guarded by
//! `tokio::sync::Mutex` for write paths and `parking_lot::RwLock` for
//! cheap read paths (cache, consent).
//!
//! # Config format
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "my-server": {
//!       "command": "npx",
//!       "args": ["-y", "@my-org/mcp-server"],
//!       "lifecycle": "lazy",
//!       "idleTimeout": 10,
//!       "directTools": true
//!     }
//!   },
//!   "settings": {
//!     "toolPrefix": "server"
//!   }
//! }
//! ```

pub mod auth;
pub mod cache;
pub mod client;
pub mod config;
pub mod consent;
pub mod content;
pub mod direct_tool;
pub mod lifecycle;
pub mod tool;
pub mod transport;
pub mod types;

pub use auth::{Credential, McpCredentialProvider, NoopCredentialProvider};
pub use cache::MetadataCache;
pub use client::{McpClient, McpLogLevel, McpPrompt, McpPromptArgument, McpSamplingRequest};
pub use consent::ConsentManager;
pub use direct_tool::McpDirectTool;
pub use tool::McpTool;
pub use transport::{McpTransport, http::StreamableHttpTransport, stdio::StdioTransport};
pub use types::{
    ConsentState, DirectToolDef, DirectToolsConfig, LifecycleMode, McpCallResult, McpConfig,
    McpConnectionStatus, McpContent, McpDashboardData, McpServerInfo, McpSettings, McpSettingsView,
    McpToolDef, McpToolInfo, ServerEntry, ServerInfo, ServerStatus, ToolMetadata, ToolPrefix,
    effective_prefix_mode, format_schema, format_tool_name, get_server_prefix,
};

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lifecycle::{LifecycleEvent, channel as lifecycle_channel, lifecycle_event_loop};

/// Default back-off period after a server connection failure (seconds).

pub const DEFAULT_FAILURE_BACKOFF_SECS: u64 = 30;
/// Default global idle timeout (minutes).
pub const DEFAULT_IDLE_TIMEOUT_MINS: u64 = 10;

/// Inner mutable state for [`McpManager`].
pub struct McpManagerInner {
    /// Connected MCP clients (server name → client).
    clients: HashMap<String, McpClient>,
    /// Raw tool definitions (server name → list, in original naming).
    /// Prefixed names are computed at lookup time.
    raw_tool_metadata: HashMap<String, Vec<McpToolDef>>,
    /// Server connection failure timestamps (for back-off).
    failure_tracker: HashMap<String, Instant>,
    /// Servers whose connection is currently in progress.
    /// Prevents two concurrent `ensure_connected` calls from racing.
    connecting: HashSet<String>,
}

/// Central manager for all MCP server connections.
///
/// Created via [`McpManager::spawn()`] which returns an `Arc<Self>`.
/// Use [`McpManager::new_no_spawn()`] only in tests where the lifecycle
/// task is not needed.
pub struct McpManager {
    inner: tokio::sync::Mutex<McpManagerInner>,
    /// Configuration (read-mostly; `parking_lot` for cheap clones).
    config: parking_lot::RwLock<McpConfig>,
    /// On-disk + in-memory tool metadata cache.
    cache: MetadataCache,
    /// Consent decisions (per-tool Allow/Deny).
    consent: ConsentManager,
    /// Lifecycle event channel sender.
    lifecycle_tx: lifecycle::LifecycleTx,
    /// Handle to the background lifecycle task (kept alive via `Arc<Self>`).
    /// `None` when constructed with `new_no_spawn()` outside a runtime.
    _lifecycle_handle: Option<tokio::task::JoinHandle<()>>,
    /// Authentication credential provider for MCP servers.
    /// Defaults to [`NoopCredentialProvider`]; replace via
    /// [`McpManager::set_credential_provider`] to enable authenticated
    /// servers (API keys, OAuth).
    credential_provider: parking_lot::RwLock<Arc<dyn McpCredentialProvider>>,
}

impl std::fmt::Debug for McpManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpManager")
            .field("cache_path", &self.cache.path())
            .field("consent_path", &self.consent.path())
            .finish()
    }
}

impl McpManager {
    /// Spawns the background lifecycle task using the oxi-default config
    /// and disk paths (`~/.config/oxi/`), and eagerly connects to any
    /// `Eager` / `KeepAlive` servers.
    ///
    /// Returns `Arc<Self>` so it can be shared freely across the agent
    /// loop, the TUI dashboard, and the lifecycle task (via `Weak`).
    pub fn spawn() -> Arc<Self> {
        Self::spawn_with_config(config::load_mcp_config())
    }

    /// Spawn with a programmatically-supplied config (used by the SDK
    /// `OxiBuilder::with_mcp_config`). Disk paths default to the oxi
    /// standard locations (`~/.config/oxi/`).
    pub fn spawn_with_config(mcp_config: McpConfig) -> Arc<Self> {
        Self::spawn_with_paths(mcp_config, None, None)
    }

    /// **Primary constructor.** Spawn with a programmatically-supplied
    /// config **and** optional custom disk paths for the metadata cache
    /// and consent store.
    ///
    /// Pass `None` for either path to use the oxi default
    /// (`~/.config/oxi/mcp-cache.json` / `mcp-consent.json`).
    ///
    /// This is the constructor SDK consumers (e.g. oxios) should use when
    /// they self-host MCP state under their own config directory. It
    /// spawns the background lifecycle task and eagerly connects to any
    /// `Eager` / `KeepAlive` servers.
    ///
    /// Returns `Arc<Self>` so it can be shared freely across the agent
    /// loop, the TUI dashboard, and the lifecycle task (via `Weak`).
    pub fn spawn_with_paths(
        mcp_config: McpConfig,
        cache_path: Option<PathBuf>,
        consent_path: Option<PathBuf>,
    ) -> Arc<Self> {
        let cache = match cache_path {
            Some(p) => MetadataCache::with_path(p),
            None => MetadataCache::new(),
        };
        // Loading is best-effort: a missing or malformed cache must not
        // prevent startup.
        let _ = cache.load();

        let consent = match consent_path {
            Some(p) => ConsentManager::with_path(p),
            None => ConsentManager::new(),
        };
        let _ = consent.load();

        // Pre-populate the in-memory cache snapshot for any servers that
        // have cached tools.
        let cached_servers = cache.cached_servers();

        // Detect whether a Tokio runtime is available. When absent (e.g.
        // unit tests calling `OxiBuilder::build()` outside a runtime), we
        // skip spawning the lifecycle task and eager connectors — those
        // require a runtime, and a manager constructed this way is only
        // useful for non-MCP work anyway. Real consumers always run inside
        // a Tokio runtime, so this guard is transparent to them.
        let has_runtime = tokio::runtime::Handle::try_current().is_ok();

        let (lifecycle_tx, lifecycle_rx) = lifecycle_channel();

        // `Arc::new_cyclic` lets us pass a `Weak<Self>` into the
        // lifecycle task during construction, avoiding any use-before-
        // initialization pattern.
        let manager = Arc::new_cyclic(|weak| {
            let _lifecycle_handle = if has_runtime {
                Some(tokio::spawn(lifecycle_event_loop(
                    lifecycle_rx,
                    weak.clone(),
                )))
            } else {
                None
            };
            Self {
                inner: tokio::sync::Mutex::new(McpManagerInner {
                    clients: HashMap::new(),
                    raw_tool_metadata: HashMap::new(),
                    failure_tracker: HashMap::new(),
                    connecting: HashSet::new(),
                }),
                config: parking_lot::RwLock::new(mcp_config),
                cache,
                consent,
                lifecycle_tx,
                _lifecycle_handle,
                credential_provider: parking_lot::RwLock::new(
                    Arc::new(NoopCredentialProvider) as Arc<dyn McpCredentialProvider>
                ),
            }
        });

        // Seed the in-memory metadata from cache, so `search` / `list` /
        // `describe` work before the first live connection.
        {
            let prefix_mode = effective_prefix_mode(manager.config.read().settings.as_ref());
            let mut inner = manager.inner.try_lock().expect("freshly constructed");
            for server in &cached_servers {
                let tools = manager.cache.get_tools(server, &prefix_mode);
                if !tools.is_empty() {
                    // Convert ToolMetadata back to raw McpToolDef for
                    // raw_tool_metadata. (The cache stores names, but we
                    // need the defs here.)
                    let raw: Vec<McpToolDef> = tools
                        .iter()
                        .map(|t| McpToolDef {
                            name: t.original_name.clone(),
                            description: Some(t.description.clone()),
                            input_schema: t.input_schema.clone(),
                        })
                        .collect();
                    inner.raw_tool_metadata.insert(server.clone(), raw);
                }
            }
        }

        // Fire-and-forget: start eager/keep-alive servers in the background.
        // Only when a runtime is available (see `has_runtime` above).
        if has_runtime {
            let mgr = manager.clone();
            tokio::spawn(async move {
                mgr.start_eager_servers().await;
            });
        }

        manager
    }

    /// Construct a manager without spawning the lifecycle task.
    /// Intended for tests that don't need timer/disconnect behaviour.
    pub fn new_no_spawn() -> Self {
        let cache = MetadataCache::new();
        let _ = cache.load();
        let consent = ConsentManager::new();
        let _ = consent.load();
        let (lifecycle_tx, _lifecycle_rx) = lifecycle_channel();
        let handle = tokio::runtime::Handle::try_current()
            .ok()
            .map(|h| h.spawn(async {}));
        Self {
            inner: tokio::sync::Mutex::new(McpManagerInner {
                clients: HashMap::new(),
                raw_tool_metadata: HashMap::new(),
                failure_tracker: HashMap::new(),
                connecting: HashSet::new(),
            }),
            config: parking_lot::RwLock::new(config::load_mcp_config()),
            cache,
            consent,
            lifecycle_tx,
            _lifecycle_handle: handle,
            credential_provider: parking_lot::RwLock::new(
                Arc::new(NoopCredentialProvider) as Arc<dyn McpCredentialProvider>
            ),
        }
    }

    /// Get a snapshot of the current config.
    pub fn config(&self) -> parking_lot::RwLockReadGuard<'_, McpConfig> {
        self.config.read()
    }

    /// Hot-replace the in-memory MCP configuration.
    ///
    /// Used by the `/mcp` management overlay after it writes a new
    /// config to disk: the updated server map becomes visible to
    /// `connect()`, `status()`, and the proxy tool **without** a full
    /// process restart. Existing live connections are left untouched;
    /// newly added servers connect lazily on first use (or eagerly if
    /// their [`LifecycleMode`] is `eager`/`keep-alive`).
    ///
    /// Direct-tool registration happens once at boot from the cache, so
    /// newly-added `directTools` servers still require a restart to
    /// surface as first-class agent tools — but the `mcp` proxy tool
    /// can reach them immediately.
    pub fn replace_config(&self, new_config: McpConfig) {
        *self.config.write() = new_config;
    }

    /// Inject a credential provider. Replaces the noop default.
    /// Called by oxi-cli bootstrap (or any host product) to enable
    /// authenticated MCP servers.
    ///
    /// Existing transports retain the provider they were constructed
    /// with; new transports created after this call pick up the new
    /// one at their next construction.
    pub fn set_credential_provider(&self, provider: Arc<dyn McpCredentialProvider>) {
        *self.credential_provider.write() = provider;
    }

    /// Get the consent manager.
    pub fn consent(&self) -> &ConsentManager {
        &self.consent
    }

    /// Get the metadata cache.
    pub fn cache(&self) -> &MetadataCache {
        &self.cache
    }

    fn failure_backoff_secs(&self) -> u64 {
        self.config
            .read()
            .settings
            .as_ref()
            .and_then(|s| s.failure_backoff_secs)
            .unwrap_or(DEFAULT_FAILURE_BACKOFF_SECS)
    }

    fn global_idle_timeout(&self) -> Duration {
        let mins = self
            .config
            .read()
            .settings
            .as_ref()
            .and_then(|s| s.idle_timeout)
            .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINS);
        Duration::from_secs(mins.saturating_mul(60))
    }

    // ── Eager / Keep-Alive startup ─────────────────────────────────

    /// Connect to all servers whose lifecycle is `Eager` or `KeepAlive`.
    async fn start_eager_servers(self: &Arc<Self>) {
        let eager_servers: Vec<(String, LifecycleMode, Option<u64>)> = {
            let config = self.config.read();
            config
                .mcp_servers
                .iter()
                .filter_map(|(name, entry)| {
                    let mode = entry.lifecycle.clone().unwrap_or(LifecycleMode::Lazy);
                    match mode {
                        LifecycleMode::Eager | LifecycleMode::KeepAlive => {
                            Some((name.clone(), mode, entry.idle_timeout))
                        }
                        LifecycleMode::Lazy => None,
                    }
                })
                .collect()
        };

        for (name, mode, idle_override) in eager_servers {
            if let Err(e) = self.connect(&name).await {
                tracing::warn!("MCP: eager connect to '{}' failed: {}", name, e);
                continue;
            }
            match mode {
                LifecycleMode::KeepAlive => {
                    let _ = self.lifecycle_tx.send(LifecycleEvent::StartHealthCheck {
                        server: name.clone(),
                    });
                }
                LifecycleMode::Eager => {
                    if let Some(mins) = idle_override {
                        let _ = self.lifecycle_tx.send(LifecycleEvent::StartIdleTimer {
                            server: name.clone(),
                            timeout: Duration::from_secs(mins.saturating_mul(60)),
                        });
                    }
                }
                LifecycleMode::Lazy => unreachable!(),
            }
        }
    }

    // ── Status ─────────────────────────────────────────────────────

    /// Get a formatted status summary (legacy `mcp({})` interface).
    pub async fn status(self: &Arc<Self>) -> String {
        let inner = self.inner.lock().await;
        let config = self.config.read();
        let servers = &config.mcp_servers;

        if servers.is_empty() {
            return "MCP: No servers configured. Create ~/.config/oxi/mcp.json or .mcp.json"
                .to_string();
        }

        let mut text = String::new();
        let mut connected_count = 0;
        let mut total_tools = 0;

        for name in servers.keys() {
            let (status_marker, tool_count) = if inner.clients.contains_key(name) {
                connected_count += 1;
                let count = inner
                    .raw_tool_metadata
                    .get(name)
                    .map(|m| m.len())
                    .unwrap_or(0);
                total_tools += count;
                ("✓", count)
            } else if let Some(failed_at) = inner.failure_tracker.get(name) {
                let ago = failed_at.elapsed().as_secs();
                if ago < self.failure_backoff_secs() {
                    ("✗", 0)
                } else {
                    ("○", 0)
                }
            } else {
                let count = inner
                    .raw_tool_metadata
                    .get(name)
                    .map(|m| m.len())
                    .unwrap_or(0);
                total_tools += count;
                ("○", count)
            };

            text.push_str(&format!(
                "{} {} ({} tools)\n",
                status_marker, name, tool_count
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

    // ── Dashboard (Phase 2) ────────────────────────────────────────

    /// Snapshot of dashboard data (Phase 2). Synchronous — only reads
    /// `parking_lot`-guarded state and in-memory copies of cached tool
    /// lists, so it is safe to call from `render()`.
    pub fn dashboard_data(self: &Arc<Self>) -> McpDashboardData {
        use McpConnectionStatus as CS;
        let config = self.config.read();
        let prefix_mode = effective_prefix_mode(config.settings.as_ref());

        let inner = self.inner.try_lock();
        let (clients_connected, raw_metadata) = match &inner {
            Ok(g) => (
                g.clients.keys().cloned().collect::<HashSet<_>>(),
                g.raw_tool_metadata.clone(),
            ),
            Err(_) => (HashSet::new(), HashMap::new()),
        };

        let mut servers = Vec::new();
        let mut total_tools = 0usize;
        let mut connected_servers = 0usize;

        for (name, entry) in &config.mcp_servers {
            let lifecycle = entry
                .lifecycle
                .as_ref()
                .map(|l| match l {
                    LifecycleMode::Lazy => "lazy".to_string(),
                    LifecycleMode::Eager => "eager".to_string(),
                    LifecycleMode::KeepAlive => "keep-alive".to_string(),
                })
                .unwrap_or_else(|| "lazy".to_string());

            let raw_tools = raw_metadata.get(name);
            let tool_count = raw_tools.map(|t| t.len()).unwrap_or(0);
            total_tools += tool_count;

            let status = if clients_connected.contains(name) {
                connected_servers += 1;
                CS::Connected
            } else {
                CS::Disconnected
            };

            let direct_set = collect_direct_tool_names(entry, config.settings.as_ref());
            let exclude: HashSet<String> = entry
                .exclude_tools
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();

            let tools: Vec<McpToolInfo> = raw_tools
                .map(|defs| {
                    defs.iter()
                        .filter(|d| !exclude.contains(&d.name))
                        .map(|d| McpToolInfo {
                            name: format_tool_name(&d.name, name, &prefix_mode),
                            original_name: d.name.clone(),
                            description: d.description.clone().unwrap_or_default(),
                            is_direct: direct_set.contains(&d.name),
                            consent: self.consent.check(&d.name),
                        })
                        .collect()
                })
                .unwrap_or_default();

            servers.push(McpServerInfo {
                name: name.clone(),
                status,
                lifecycle,
                tool_count,
                tools,
            });
        }

        let settings = McpSettingsView {
            tool_prefix: match prefix_mode {
                ToolPrefix::Server => "server".to_string(),
                ToolPrefix::Short => "short".to_string(),
                ToolPrefix::None => "none".to_string(),
            },
            idle_timeout: config.settings.as_ref().and_then(|s| s.idle_timeout),
            total_servers: config.mcp_servers.len(),
            connected_servers,
            total_tools,
        };

        McpDashboardData { servers, settings }
    }

    // ── Connect / disconnect ──────────────────────────────────────

    /// Connect to a specific MCP server by name. Selects the transport
    /// from the entry: `command` → stdio (spawn), `url` → Streamable HTTP.
    /// Stores the connected client and updates the metadata cache.
    pub async fn connect(self: &Arc<Self>, server_name: &str) -> Result<String> {
        let (command, args, env, cwd, debug, url, timeout_ms) = {
            let config = self.config.read();
            let entry = config
                .mcp_servers
                .get(server_name)
                .ok_or_else(|| anyhow::anyhow!("Server '{}' not found", server_name))?;
            (
                entry.command.clone(),
                entry.args.clone().unwrap_or_default(),
                entry.env.clone().unwrap_or_default(),
                entry.cwd.clone(),
                entry.debug.unwrap_or(false),
                entry.url.clone(),
                entry
                    .timeout
                    .unwrap_or(crate::mcp::transport::http::DEFAULT_TIMEOUT_MS),
            )
        };

        let provider = self.credential_provider.read().clone();
        let transport: Box<dyn McpTransport> = match (command, url) {
            (Some(cmd), _) => Box::new(
                StdioTransport::spawn(&cmd, &args, &env, cwd.as_deref(), debug)
                    .with_context(|| format!("Failed to spawn MCP server '{}'", server_name))?,
            ),
            (None, Some(endpoint)) => Box::new(
                StreamableHttpTransport::new(server_name, &endpoint, Some(provider), timeout_ms)
                    .with_context(|| {
                        format!("Failed to build HTTP transport for '{}'", server_name)
                    })?,
            ),
            (None, None) => anyhow::bail!(
                "Server '{}' has neither 'command' nor 'url' configured",
                server_name
            ),
        };

        let mut client = McpClient::connect_with_transport(transport)
            .await
            .with_context(|| format!("Failed to connect to MCP server '{}'", server_name))?;

        let tools = client.list_tools().await.unwrap_or_default();

        if let Err(e) = self.cache.update(server_name, &tools) {
            tracing::warn!("MCP: failed to update cache for '{}': {}", server_name, e);
        }

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

        let mut inner = self.inner.lock().await;
        inner.clients.insert(server_name.to_string(), client);
        inner
            .raw_tool_metadata
            .insert(server_name.to_string(), tools);
        inner.failure_tracker.remove(server_name);
        inner.connecting.remove(server_name);

        if tool_names.is_empty() {
            Ok(format!(
                "Connected to '{}' — no tools available.",
                server_name
            ))
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

    /// Lazily connect (or return true if already connected).
    pub async fn ensure_connected(self: &Arc<Self>, server_name: &str) -> bool {
        let should_connect = {
            let mut inner = self.inner.lock().await;
            if inner.clients.contains_key(server_name) {
                return true;
            }
            if inner.connecting.contains(server_name) {
                return false;
            }
            if let Some(failed_at) = inner.failure_tracker.get(server_name)
                && failed_at.elapsed().as_secs() < self.failure_backoff_secs()
            {
                return false;
            }
            inner.connecting.insert(server_name.to_string());
            true
        };

        if !should_connect {
            return false;
        }

        let result = self.connect(server_name).await;
        self.inner.lock().await.connecting.remove(server_name);
        match result {
            Ok(_) => {
                // Reset/clear any pending idle timer — the server is
                // now connected and we want to start a fresh timer
                // when the next call happens.
                let _ = self.lifecycle_tx.send(LifecycleEvent::CancelIdleTimer {
                    server: server_name.to_string(),
                });
                true
            }
            Err(e) => {
                tracing::warn!("MCP: lazy connect failed for {}: {}", server_name, e);
                let mut inner = self.inner.lock().await;
                inner
                    .failure_tracker
                    .insert(server_name.to_string(), Instant::now());
                false
            }
        }
    }

    /// Disconnect a single server (used by the lifecycle idle timer).
    async fn disconnect_server(self: &Arc<Self>, server_name: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(mut client) = inner.clients.remove(server_name) {
            let _ = client.close().await;
        }
        inner.raw_tool_metadata.remove(server_name);
        inner.connecting.remove(server_name);
        drop(inner);

        let _ = self.lifecycle_tx.send(LifecycleEvent::ServerStopped {
            server: server_name.to_string(),
        });
        tracing::info!("MCP: disconnected '{}' (idle timeout)", server_name);
        Ok(())
    }

    /// Health check + reconnect for a keep-alive server (v2.3 G6).
    ///
    /// Retries reconnect with exponential backoff (500ms, 1s, 2s) so a
    /// single transient blip no longer permanently kills the health
    /// check (v2.0 only attempted reconnect once before giving up).
    /// A per-server circuit breaker (5 failures / 30s → pause) is the
    /// planned 2nd defence; see design §8.
    async fn health_check_and_reconnect(self: &Arc<Self>, server_name: &str) -> Result<()> {
        const BACKOFFS: [Duration; 3] = [
            Duration::from_millis(500),
            Duration::from_secs(1),
            Duration::from_secs(2),
        ];
        // Fast path: a healthy client answers a ping.
        {
            let mut inner = self.inner.lock().await;
            if let Some(client) = inner.clients.get_mut(server_name)
                && client.ping().await.is_ok()
            {
                return Ok(());
            }
        }
        // Reconnect with backoff. On success → return; on final failure → Err.
        for (i, delay) in BACKOFFS.iter().enumerate() {
            tracing::warn!(
                "MCP: health check '{}' failed; reconnect attempt {}/{} after {:?}",
                server_name,
                i + 1,
                BACKOFFS.len(),
                delay
            );
            tokio::time::sleep(*delay).await;
            match self.connect(server_name).await {
                Ok(_) => return Ok(()),
                Err(e) => tracing::warn!(
                    "MCP: reconnect attempt {} for '{}' failed: {}",
                    i + 1,
                    server_name,
                    e
                ),
            }
        }
        Err(anyhow::anyhow!(
            "Health check for '{}' exhausted {} reconnect attempts",
            server_name,
            BACKOFFS.len()
        ))
    }

    /// Force-refresh the OAuth credential for `server_name` (v2.2).
    /// Used by `/mcp reauth <server>` to rotate credentials without
    /// restarting the process. Returns an error if no credential
    /// provider is configured, the server has no `url`, or the
    /// refresh fails.
    pub async fn reauth_server(self: &Arc<Self>, server_name: &str) -> Result<()> {
        let url = {
            let config = self.config.read();
            config
                .mcp_servers
                .get(server_name)
                .and_then(|e| e.url.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Server '{}' not found or has no URL (auth requires Streamable HTTP)",
                        server_name
                    )
                })?
        };
        let cred = self
            .credential_provider
            .read()
            .refresh(server_name, &url)
            .await;
        match cred {
            Some(_) => Ok(()),
            None => Err(anyhow::anyhow!(
                "Credential refresh for '{}' failed (see logs for details)",
                server_name
            )),
        }
    }

    // ── Tool operations ───────────────────────────────────────────

    /// Call an MCP tool by name, optionally targeting a specific server.
    pub async fn call_tool(
        self: &Arc<Self>,
        tool_name: &str,
        args: serde_json::Value,
        server_override: Option<&str>,
    ) -> Result<McpCallResult> {
        let (server_name, original_name) = self.find_tool(tool_name, server_override).await?;

        // Consent gate (Phase 3) — proxy path also honors consent.
        if self.consent.check(&original_name) == ConsentState::Deny {
            return Err(anyhow::anyhow!(
                "Tool '{}' is denied by consent policy",
                original_name
            ));
        }

        self.ensure_connected(&server_name).await;

        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(&server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;

        let result = client
            .call_tool(&original_name, args)
            .await
            .with_context(|| format!("Tool '{}' call failed", tool_name))?;
        drop(inner);

        // Reset idle timer after a successful call.
        self.reset_idle_timer(&server_name);

        let text = content::transform_mcp_content(&result.content);
        Ok(McpCallResult {
            content: vec![McpContent::Text { text }],
            is_error: result.is_error,
        })
    }

    /// List resources from a connected MCP server (v2.3 G5).
    pub async fn list_resources(
        self: &Arc<Self>,
        server_name: &str,
    ) -> Result<Vec<serde_json::Value>> {
        self.ensure_connected(server_name).await;
        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;
        client
            .list_resources()
            .await
            .with_context(|| format!("list_resources on '{}' failed", server_name))
    }

    /// Read a resource URI from a connected MCP server (v2.3 G5).
    pub async fn read_resource(
        self: &Arc<Self>,
        server_name: &str,
        uri: &str,
    ) -> Result<Vec<McpContent>> {
        self.ensure_connected(server_name).await;
        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;
        client
            .read_resource(uri)
            .await
            .with_context(|| format!("read_resource '{}' on '{}' failed", uri, server_name))
    }

    /// List prompt templates from a connected MCP server (v2.3 G5).
    pub async fn list_prompts(self: &Arc<Self>, server_name: &str) -> Result<Vec<McpPrompt>> {
        self.ensure_connected(server_name).await;
        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;
        client
            .list_prompts()
            .await
            .with_context(|| format!("list_prompts on '{}' failed", server_name))
    }

    /// Get a prompt template with arguments (v2.3 G5).
    pub async fn get_prompt(
        self: &Arc<Self>,
        server_name: &str,
        name: &str,
        arguments: std::collections::HashMap<String, String>,
    ) -> Result<Vec<serde_json::Value>> {
        self.ensure_connected(server_name).await;
        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;
        client
            .get_prompt(name, arguments)
            .await
            .with_context(|| format!("get_prompt '{}' on '{}' failed", name, server_name))
    }

    /// Reset (or start) the idle-disconnect timer for a server.
    /// Called after every successful tool use.
    pub fn reset_idle_timer(self: &Arc<Self>, server_name: &str) {
        let timeout = {
            let config = self.config.read();
            let per_server = config
                .mcp_servers
                .get(server_name)
                .and_then(|e| e.idle_timeout)
                .map(|m| Duration::from_secs(m.saturating_mul(60)));
            per_server.unwrap_or_else(|| self.global_idle_timeout())
        };
        let _ = self.lifecycle_tx.send(LifecycleEvent::StartIdleTimer {
            server: server_name.to_string(),
            timeout,
        });
    }

    /// Describe a tool by name.
    pub async fn describe(self: &Arc<Self>, tool_name: &str) -> Result<String> {
        let (server_name, original_name) = self.find_tool(tool_name, None).await?;

        // Look up the cached/live def to get description + schema.
        let prefix_mode = effective_prefix_mode(self.config.read().settings.as_ref());
        let prefixed = format_tool_name(&original_name, &server_name, &prefix_mode);

        let (description, input_schema) = {
            let inner = self.inner.lock().await;
            inner
                .raw_tool_metadata
                .get(&server_name)
                .and_then(|defs| defs.iter().find(|d| d.name == original_name).cloned())
                .map(|d| (d.description.unwrap_or_default(), d.input_schema))
                .unwrap_or_default()
        };

        let mut text = format!("{}\n", prefixed);
        text.push_str(&format!("Server: {}\n", server_name));
        text.push_str(&format!("\n{}\n", description));

        if let Some(ref schema) = input_schema {
            text.push_str(&format!("\nParameters:\n{}", format_schema(schema, "  ")));
        } else {
            text.push_str("\nNo parameters defined.");
        }

        Ok(text)
    }

    /// Search tools by name or description.
    pub async fn search(
        self: &Arc<Self>,
        query: &str,
        regex: bool,
        server_filter: Option<&str>,
    ) -> Result<String> {
        let pattern = if regex {
            regex::Regex::new(query).with_context(|| format!("Invalid regex: {}", query))?
        } else {
            let terms: Vec<&str> = query.split_whitespace().collect();
            if terms.is_empty() {
                return Ok("Search query cannot be empty".to_string());
            }
            let escaped: Vec<String> = terms.iter().map(|t| regex::escape(t)).collect();
            regex::Regex::new(&format!("(?i){}", escaped.join("|")))
                .context("Invalid search pattern")?
        };

        let inner = self.inner.lock().await;
        let mut matches = Vec::new();

        for (server_name, raw_tools) in &inner.raw_tool_metadata {
            if let Some(filter) = server_filter
                && server_name != filter
            {
                continue;
            }
            for tool in raw_tools {
                let prefixed = format_tool_name(
                    &tool.name,
                    server_name,
                    &effective_prefix_mode(self.config.read().settings.as_ref()),
                );
                let description = tool.description.clone().unwrap_or_default();
                if pattern.is_match(&prefixed) || pattern.is_match(&description) {
                    matches.push((
                        server_name.clone(),
                        tool.name.clone(),
                        description,
                        tool.input_schema.clone(),
                    ));
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

        for (server, original, description, schema) in &matches {
            let prefixed = format_tool_name(
                original,
                server,
                &effective_prefix_mode(self.config.read().settings.as_ref()),
            );
            text.push_str(&format!("{}\n", prefixed));
            if !description.is_empty() {
                text.push_str(&format!("  {}\n", description));
            }
            if let Some(s) = schema {
                text.push_str(&format!("  Parameters:\n{}\n", format_schema(s, "    ")));
            }
            text.push('\n');
        }

        Ok(text.trim_end().to_string())
    }

    /// List tools for a specific server.
    pub async fn list_tools(self: &Arc<Self>, server_name: &str) -> Result<String> {
        {
            let config = self.config.read();
            if !config.mcp_servers.contains_key(server_name) {
                return Ok(format!(
                    "Server '{}' not found. Use mcp({{}}) to see available servers.",
                    server_name
                ));
            }
        }

        self.ensure_connected(server_name).await;

        let inner = self.inner.lock().await;
        let metadata = inner.raw_tool_metadata.get(server_name);
        let prefix_mode = effective_prefix_mode(self.config.read().settings.as_ref());

        match metadata {
            Some(tools) if !tools.is_empty() => {
                let mut text = format!("{} ({} tools):\n\n", server_name, tools.len());
                for tool in tools {
                    let prefixed = format_tool_name(&tool.name, server_name, &prefix_mode);
                    text.push_str(&format!("- {}", prefixed));
                    if let Some(desc) = &tool.description {
                        let short: String = desc.chars().take(60).collect();
                        text.push_str(&format!(" - {}", short));
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

    /// Direct tool definitions for `ToolRegistry` registration (Phase 3).
    /// Reads from the metadata cache, applies `direct_tools` /
    /// `exclude_tools` filters, and returns the precomputed prefixed names.
    pub fn direct_tools_from_cache(self: &Arc<Self>) -> Vec<DirectToolDef> {
        let config = self.config.read();
        let prefix_mode = effective_prefix_mode(config.settings.as_ref());
        let global_direct = config
            .settings
            .as_ref()
            .and_then(|s| s.direct_tools.clone());

        let mut out = Vec::new();

        for (server_name, entry) in &config.mcp_servers {
            // Determine whether to honor this server at all
            let effective = entry.direct_tools.clone().or_else(|| global_direct.clone());
            let is_direct_enabled = match &effective {
                None => false,
                Some(DirectToolsConfig::All(b)) => *b,
                Some(DirectToolsConfig::Specific(_)) => true,
            };
            if !is_direct_enabled {
                continue;
            }
            let exclude: HashSet<String> = entry
                .exclude_tools
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();

            // Iterate over cached tools for this server.
            let tools = self.cache.get_tools(server_name, &prefix_mode);
            for t in tools {
                if exclude.contains(&t.original_name) {
                    continue;
                }
                let in_set = match &effective {
                    Some(DirectToolsConfig::All(_)) => true,
                    Some(DirectToolsConfig::Specific(list)) => list.contains(&t.original_name),
                    None => false,
                };
                if !in_set {
                    continue;
                }
                out.push(DirectToolDef {
                    prefixed_name: format_tool_name(&t.original_name, server_name, &prefix_mode),
                    original_name: t.original_name.clone(),
                    server_name: server_name.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                });
            }
        }

        out
    }

    /// Whether the `mcp` proxy tool should be hidden in the tool registry
    /// (Phase 3 — `settings.disable_proxy_tool: true`).
    pub fn should_disable_proxy(self: &Arc<Self>) -> bool {
        self.config
            .read()
            .settings
            .as_ref()
            .and_then(|s| s.disable_proxy_tool)
            .unwrap_or(false)
    }

    // ── Internal helpers ──────────────────────────────────────────

    /// Find a tool by name across all known (cached + live) servers.
    async fn find_tool(
        self: &Arc<Self>,
        tool_name: &str,
        server_override: Option<&str>,
    ) -> Result<(String, String)> {
        // If a specific server was requested, verify it exists.
        if let Some(server) = server_override {
            let config = self.config.read();
            if !config.mcp_servers.contains_key(server) {
                return Err(anyhow::anyhow!("Server '{}' not found", server));
            }
        }

        // 1. Try exact match against the in-memory metadata.
        {
            let inner = self.inner.lock().await;
            let server_keys: Vec<String> = if let Some(s) = server_override {
                vec![s.to_string()]
            } else {
                inner.raw_tool_metadata.keys().cloned().collect()
            };
            for server_name in &server_keys {
                if let Some(raw) = inner.raw_tool_metadata.get(server_name)
                    && let Some(d) = raw.iter().find(|t| t.name == tool_name)
                {
                    return Ok((server_name.clone(), d.name.clone()));
                }
            }
        }

        // 2. Try prefix-based matching on configured server names
        //    (e.g. `chrome_take_screenshot` → server `chrome`,
        //    tool `take_screenshot`).
        let prefix_mode = effective_prefix_mode(self.config.read().settings.as_ref());
        let candidates: Vec<String> = {
            let config = self.config.read();
            config
                .mcp_servers
                .keys()
                .filter(|server_name| {
                    if let Some(s) = server_override {
                        server_name.as_str() == s
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
        };

        for server_name in &candidates {
            self.ensure_connected(server_name).await;
            let inner = self.inner.lock().await;
            if let Some(raw) = inner.raw_tool_metadata.get(server_name) {
                // Look for prefixed match
                for d in raw {
                    if format_tool_name(&d.name, server_name, &prefix_mode) == tool_name {
                        return Ok((server_name.clone(), d.name.clone()));
                    }
                }
            }
        }

        // 3. Not found — helpful error.
        let inner = self.inner.lock().await;
        let mut hint_servers = Vec::new();
        let prefix_mode = effective_prefix_mode(self.config.read().settings.as_ref());
        for (server_name, raw) in &inner.raw_tool_metadata {
            let names: Vec<String> = raw
                .iter()
                .map(|d| format_tool_name(&d.name, server_name, &prefix_mode))
                .collect();
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

/// Compute the set of tool original-names that should be exposed as direct
/// for the given server, taking the per-server `direct_tools` override
/// first, then the global default.
fn collect_direct_tool_names(
    entry: &ServerEntry,
    settings: Option<&McpSettings>,
) -> HashSet<String> {
    let cfg = entry
        .direct_tools
        .clone()
        .or_else(|| settings.and_then(|s| s.direct_tools.clone()));
    match cfg {
        Some(DirectToolsConfig::All(true)) => HashSet::new(), // "all" sentinel: handled elsewhere
        Some(DirectToolsConfig::All(false)) => HashSet::new(),
        Some(DirectToolsConfig::Specific(list)) => list.into_iter().collect(),
        None => HashSet::new(),
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new_no_spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn new_no_spawn_succeeds() {
        let m = McpManager::new_no_spawn();
        assert_eq!(m.config().mcp_servers.len(), 0);
    }

    #[tokio::test]
    async fn spawn_with_paths_uses_supplied_paths() {
        let dir = TempDir::new().unwrap();
        let cache_p = dir.path().join("c.json");
        let consent_p = dir.path().join("consent.json");
        let mgr = McpManager::spawn_with_paths(
            McpConfig::default(),
            Some(cache_p.clone()),
            Some(consent_p.clone()),
        );
        assert_eq!(mgr.cache().path(), cache_p);
        assert_eq!(mgr.consent().path(), consent_p);
    }

    #[tokio::test]
    async fn spawn_with_paths_none_uses_default_paths() {
        // None, None → 기본 경로 사용 (에러 없이 spawn).
        let mgr = McpManager::spawn_with_paths(McpConfig::default(), None, None);
        assert!(!mgr.cache().path().as_os_str().is_empty());
        assert!(!mgr.consent().path().as_os_str().is_empty());
    }

    #[test]
    fn dashboard_data_empty_config() {
        let mgr = Arc::new(McpManager::new_no_spawn());
        let data = mgr.dashboard_data();
        assert!(data.servers.is_empty());
        assert_eq!(data.settings.total_servers, 0);
    }

    #[tokio::test]
    async fn direct_tools_from_cache_respects_specific_list() {
        let dir = TempDir::new().unwrap();
        let cache = MetadataCache::with_path(dir.path().join("mcp-cache.json"));
        let consent = ConsentManager::with_path(dir.path().join("consent.json"));

        // Manually populate the cache with a single server + 2 tools.
        let defs = vec![
            McpToolDef {
                name: "take_screenshot".into(),
                description: Some("screenshot".into()),
                input_schema: None,
            },
            McpToolDef {
                name: "navigate".into(),
                description: Some("go to url".into()),
                input_schema: None,
            },
        ];
        cache.update("chrome", &defs).unwrap();

        // Build a config that asks for only `take_screenshot` as direct.
        let mut cfg = McpConfig::default();
        cfg.mcp_servers.insert(
            "chrome".into(),
            ServerEntry {
                command: Some("echo".into()),
                direct_tools: Some(DirectToolsConfig::Specific(vec!["take_screenshot".into()])),
                ..Default::default()
            },
        );

        // Manually construct a manager so we can install our cache/consent.
        let (lifecycle_tx, _rx) = lifecycle_channel();
        let mgr = Arc::new(McpManager {
            inner: tokio::sync::Mutex::new(McpManagerInner {
                clients: HashMap::new(),
                raw_tool_metadata: HashMap::new(),
                failure_tracker: HashMap::new(),
                connecting: HashSet::new(),
            }),
            config: parking_lot::RwLock::new(cfg),
            cache,
            consent,
            lifecycle_tx,
            credential_provider: parking_lot::RwLock::new(Arc::new(NoopCredentialProvider)),
            _lifecycle_handle: Some(tokio::spawn(async {})),
        });

        let direct = mgr.direct_tools_from_cache();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].original_name, "take_screenshot");
        assert_eq!(direct[0].prefixed_name, "chrome_take_screenshot");
    }
}
