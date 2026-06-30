//! MCP server — ported from omp `packages/mnemopi/src/mcp-tools.ts` +
//! `mcp-server.ts`.
//!
//! Exposes the [`Mnemopi`] engine over JSON-RPC 2.0 (stdio transport) so
//! external MCP clients (Claude Desktop, oxi's own `mcp` client, …) can
//! store and recall memories through the same engine the agent uses.
//!
//! # Tool dispatch
//!
//! The 24 omp tools split cleanly along the embedding boundary:
//!
//! - **20 facade ops** (`remember`, `recall`, `forget`, `update`, `get`,
//!   `get_stats`, `invalidate`, `sleep`, `harmonize`, …) go through
//!   [`Mnemopi`] so the dense-vector signal is preserved — both on write
//!   (auto-embed) and on read (query embed).
//! - **4 raw-conn classes** (`triples`, `graph`, `scratchpad`,
//!   `export`/`import`) touch tables that have no embedding column, so
//!   they call the module functions directly via [`crate::MnemopiDb::with_conn`].
//!
//! # Shared-surface bank
//!
//! omp exposes `mnemopi_shared_*` against a separate DB configured by
//! `MNEMOPI_SHARED_SURFACE_DB`. oxi has no multi-bank abstraction; the
//! server opens a second [`Mnemopi`] instance at `<db-dir>/shared.db`
//! with `session_id = "shared_surface"`. Same semantics, simpler shape.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Mnemopi;
use crate::embeddings::EmbeddingProvider;
use crate::episodic_graph;
use crate::error::{MnemopiError, Result};
use crate::store;
use crate::triples;
use crate::types::{MemoryScope, MnemopiConfig, RecallOptions, RememberOptions, Veracity};

// ═══════════════════════════════════════════════════════════════════════════
// Public types
// ═══════════════════════════════════════════════════════════════════════════

/// A single MCP tool definition (name + description + JSON schema).
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Server state — owns the primary + shared [`Mnemopi`] engines.
pub struct McpServer {
    primary: Mnemopi,
    shared: Mnemopi,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("primary_session", &self.primary.config().session_id)
            .field("shared_session", &self.shared.config().session_id)
            .finish()
    }
}

/// Options for constructing an [`McpServer`].
pub struct McpServerOptions {
    /// Path to the primary SQLite database.
    pub db_path: PathBuf,
    /// Logical session ID for the primary engine.
    pub session_id: String,
    /// Optional embedding provider (preserved across both engines).
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    /// Embedding model name (recorded alongside stored vectors).
    pub embedding_model: String,
}

impl McpServer {
    /// Open both engines against `options.db_path`.
    pub fn open(options: McpServerOptions) -> Result<Self> {
        let primary = open_engine(
            &options.db_path,
            &options.session_id,
            options.embedding_provider.clone(),
            &options.embedding_model,
        )?;

        // Shared-surface DB lives alongside the primary, named `shared.db`.
        let shared_path = options
            .db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("shared.db");
        let shared = open_engine(
            &shared_path,
            "shared_surface",
            options.embedding_provider,
            &options.embedding_model,
        )?;

        Ok(Self { primary, shared })
    }

    /// Open both engines in-memory (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let primary = Mnemopi::open_in_memory()?;
        let shared = Mnemopi::open_in_memory()?;
        Ok(Self { primary, shared })
    }
}

fn open_engine(
    path: &Path,
    session_id: &str,
    provider: Option<Arc<dyn EmbeddingProvider>>,
    model_name: &str,
) -> Result<Mnemopi> {
    let mut config = MnemopiConfig {
        session_id: session_id.to_string(),
        ..Default::default()
    };
    if let Some(p) = provider {
        config.embedding_provider = Some(p);
        if !model_name.is_empty() {
            config.embedding_model = Some(model_name.to_string());
        }
    }
    let engine = Mnemopi::open(path, config)?;
    Ok(engine)
}

// ═══════════════════════════════════════════════════════════════════════════
// JSON-RPC message types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// JSON-RPC error codes (per spec).
const PARSE_ERROR: i32 = -32700;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

/// MCP protocol version advertised at `initialize`.
pub const PROTOCOL_VERSION: &str = "2024-11-05";
/// Server name reported at `initialize`.
pub const SERVER_NAME: &str = "oxi-mnemopi";
/// Server version reported at `initialize`.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ═══════════════════════════════════════════════════════════════════════════
// Tool definitions
// ═══════════════════════════════════════════════════════════════════════════

/// Return the full tool list (omp parity: 24 tools).
pub fn tool_definitions() -> &'static [ToolDefinition] {
    static TOOLS: std::sync::OnceLock<Vec<ToolDefinition>> = std::sync::OnceLock::new();
    TOOLS.get_or_init(build_tools).as_slice()
}

// Helper builders for compact schema construction.
fn props_obj(props: &[(&str, Value)]) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in props {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

fn s(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn s_enum(desc: &str, variants: &[&str]) -> Value {
    json!({ "type": "string", "description": desc, "enum": variants })
}

fn s_default(desc: &str, default: &str) -> Value {
    json!({ "type": "string", "description": desc, "default": default })
}

fn n(desc: &str) -> Value {
    json!({ "type": "number", "description": desc })
}

fn n_default(desc: &str, default: f64) -> Value {
    json!({ "type": "number", "description": desc, "default": default })
}

fn i_default(desc: &str, default: i64) -> Value {
    json!({ "type": "integer", "description": desc, "default": default })
}

fn b_default(desc: &str, default: bool) -> Value {
    json!({ "type": "boolean", "description": desc, "default": default })
}

fn obj(desc: &str) -> Value {
    json!({ "type": "object", "description": desc, "default": {} })
}

fn schema(props: &[(&str, Value)], required: &[&str]) -> Value {
    let required_arr: Vec<Value> = required
        .iter()
        .map(|s| Value::String((*s).into()))
        .collect();
    json!({
        "type": "object",
        "properties": props_obj(props),
        "required": required_arr,
    })
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn build_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "mnemopi_remember",
            description: "Store a durable memory in Mnemopi.",
            input_schema: schema(
                &[
                    ("content", s("The memory content to store.")),
                    (
                        "importance",
                        n_default("Importance score from 0.0 to 1.0.", 0.5),
                    ),
                    ("source", s_default("Source tag for this memory.", "mcp")),
                    ("veracity", s_default("Confidence label.", "unknown")),
                    ("memory_type", s("Optional memory type label.")),
                    ("scope", s_default("Memory scope.", "session")),
                    (
                        "extract",
                        b_default("Run heuristic fact extraction.", false),
                    ),
                    (
                        "extract_entities",
                        b_default("Extract named entities.", false),
                    ),
                    ("metadata", obj("Optional key-value metadata.")),
                ],
                &["content"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_recall",
            description: "Search memories with hybrid FTS5 + vector scoring.",
            input_schema: schema(
                &[
                    ("query", s("Natural-language search query.")),
                    ("limit", i_default("Maximum results to return.", 5)),
                    ("top_k", i_default("Alias for limit.", 5)),
                    ("vec_weight", n("Vector similarity weight override.")),
                    ("fts_weight", n("Full-text search weight override.")),
                    ("importance_weight", n("Importance weight override.")),
                ],
                &["query"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_shared_remember",
            description: "Store compact cross-agent surface memory.",
            input_schema: schema(
                &[
                    ("content", s("Surface memory content to store.")),
                    (
                        "kind",
                        s_enum(
                            "Surface category.",
                            &["meta", "preference", "correction", "identity"],
                        ),
                    ),
                    ("importance", n_default("Importance score.", 0.8)),
                    ("veracity", s_default("Confidence label.", "unknown")),
                    ("metadata", obj("Optional metadata.")),
                ],
                &["content"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_shared_recall",
            description: "Search only the shared Mnemopi surface DB.",
            input_schema: schema(
                &[
                    ("query", s("Surface memory query.")),
                    ("limit", i_default("Max results.", 5)),
                ],
                &["query"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_shared_forget",
            description: "Delete one shared-surface memory by ID.",
            input_schema: schema(&[("memory_id", s("Memory ID to delete."))], &["memory_id"]),
        },
        ToolDefinition {
            name: "mnemopi_shared_stats",
            description: "Return shared surface DB statistics.",
            input_schema: empty_schema(),
        },
        ToolDefinition {
            name: "mnemopi_sleep",
            description: "Run the consolidation sleep cycle (working → episodic).",
            input_schema: schema(
                &[
                    ("dry_run", b_default("Preview without writes.", false)),
                    ("ttl_hours", i_default("Working-memory TTL in hours.", 24)),
                ],
                &[],
            ),
        },
        ToolDefinition {
            name: "mnemopi_stats",
            description: "Return Mnemopi memory statistics.",
            input_schema: empty_schema(),
        },
        ToolDefinition {
            name: "mnemopi_get_stats",
            description: "Alias for mnemopi_stats.",
            input_schema: empty_schema(),
        },
        ToolDefinition {
            name: "mnemopi_invalidate",
            description: "Mark a memory as expired or superseded.",
            input_schema: schema(
                &[
                    ("memory_id", s("ID of memory to invalidate.")),
                    ("replacement_id", s("Optional replacement memory ID.")),
                ],
                &["memory_id"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_validate",
            description: "Attest, update, invalidate, or delete a memory.",
            input_schema: schema(
                &[
                    ("memory_id", s("ID of memory to validate.")),
                    (
                        "action",
                        s_enum(
                            "Validation action.",
                            &["attest", "update", "invalidate", "delete"],
                        ),
                    ),
                    ("validator", s("Agent identifier performing validation.")),
                    ("new_content", s("New content for action=update.")),
                ],
                &["memory_id", "action"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_get",
            description: "Retrieve one memory by ID.",
            input_schema: schema(
                &[("memory_id", s("The memory ID to retrieve."))],
                &["memory_id"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_triple_add",
            description: "Add a temporal fact triple (subject-predicate-object).",
            input_schema: schema(
                &[
                    ("subject", s("Triple subject.")),
                    ("predicate", s("Triple predicate.")),
                    ("object", s("Triple object.")),
                    ("valid_from", s("ISO start date.")),
                    ("source", s_default("Source tag.", "conversation")),
                    ("confidence", n_default("Confidence score.", 1.0)),
                ],
                &["subject", "predicate", "object"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_triple_query",
            description: "Query temporal fact triples.",
            input_schema: schema(
                &[
                    ("subject", s("Filter by subject.")),
                    ("predicate", s("Filter by predicate.")),
                    ("object", s("Filter by object.")),
                    ("as_of", s("ISO date for temporal filter.")),
                ],
                &[],
            ),
        },
        ToolDefinition {
            name: "mnemopi_scratchpad_write",
            description: "Write a temporary scratchpad note.",
            input_schema: schema(&[("content", s("Content to write."))], &["content"]),
        },
        ToolDefinition {
            name: "mnemopi_scratchpad_read",
            description: "Read scratchpad entries.",
            input_schema: empty_schema(),
        },
        ToolDefinition {
            name: "mnemopi_scratchpad_clear",
            description: "Clear all scratchpad entries.",
            input_schema: empty_schema(),
        },
        ToolDefinition {
            name: "mnemopi_export",
            description: "Export Mnemopi memories to a JSON file.",
            input_schema: schema(
                &[("output_path", s("File path to write the export JSON."))],
                &["output_path"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_update",
            description: "Update the content or importance of an existing memory.",
            input_schema: schema(
                &[
                    ("memory_id", s("ID of the memory to update.")),
                    ("content", s("New content for the memory.")),
                    ("importance", n("New importance score.")),
                ],
                &["memory_id"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_forget",
            description: "Permanently delete a memory by ID.",
            input_schema: schema(
                &[("memory_id", s("ID of the memory to delete."))],
                &["memory_id"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_import",
            description: "Import Mnemopi memories from a JSON file.",
            input_schema: schema(
                &[
                    ("input_path", s("File path to read the export JSON from.")),
                    (
                        "force",
                        b_default("Overwrite existing instead of skipping.", false),
                    ),
                ],
                &["input_path"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_diagnose",
            description: "Run PII-safe diagnostics on the active Mnemopi database.",
            input_schema: empty_schema(),
        },
        ToolDefinition {
            name: "mnemopi_graph_query",
            description: "Traverse the memory graph from a seed memory.",
            input_schema: schema(
                &[
                    ("seed_memory_id", s("Seed memory ID.")),
                    ("max_hops", i_default("Max BFS depth.", 2)),
                    ("edge_type", s("Filter by edge type.")),
                    ("min_weight", n_default("Minimum edge weight.", 0.0)),
                ],
                &["seed_memory_id"],
            ),
        },
        ToolDefinition {
            name: "mnemopi_graph_link",
            description: "Declare a semantic edge between two memories.",
            input_schema: schema(
                &[
                    ("source_id", s("Source memory ID.")),
                    ("target_id", s("Target memory ID.")),
                    ("relationship", s("Edge type label.")),
                    ("weight", n_default("Edge weight.", 0.5)),
                ],
                &["source_id", "target_id", "relationship"],
            ),
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════════
// Argument extraction helpers
// ═══════════════════════════════════════════════════════════════════════════

fn arg_str(args: &Value, key: &str, fallback: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn arg_str_opt(args: &Value, key: &str) -> Option<String> {
    let s = arg_str(args, key, "");
    if s.is_empty() { None } else { Some(s) }
}

fn arg_num(args: &Value, key: &str, fallback: f64) -> f64 {
    args.get(key)
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .filter(|n| n.is_finite())
        .unwrap_or(fallback)
}

fn arg_num_opt(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .filter(|n| n.is_finite())
    })
}

fn arg_int(args: &Value, key: &str, fallback: i64) -> i64 {
    arg_num(args, key, fallback as f64) as i64
}

fn arg_bool(args: &Value, key: &str, fallback: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(fallback)
}

fn arg_metadata(args: &Value) -> Option<crate::types::Metadata> {
    args.get("metadata")
        .and_then(|v| v.as_object())
        .filter(|m| !m.is_empty())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Require a non-empty string argument or return an error result.
fn require_str(args: &Value, key: &str) -> Result<String, Value> {
    let v = arg_str(args, key, "");
    if v.is_empty() {
        Err(json!({ "error": format!("{key} is required") }))
    } else {
        Ok(v)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tool dispatch
// ═══════════════════════════════════════════════════════════════════════════

/// Dispatch a `tools/call` request. Returns the JSON payload to embed in the
/// MCP `content[0].text` field.
pub async fn handle_tool_call(server: &McpServer, name: &str, args: &Value) -> Value {
    match name {
        // ── Facade ops (embedding-preserving) ──────────────────────────
        "mnemopi_remember" => handle_remember(&server.primary, args).await,
        "mnemopi_recall" => handle_recall(&server.primary, args).await,
        "mnemopi_shared_remember" => handle_shared_remember(&server.shared, args).await,
        "mnemopi_shared_recall" => handle_recall(&server.shared, args).await,
        "mnemopi_shared_forget" => handle_forget(&server.shared, args).await,
        "mnemopi_shared_stats" => handle_stats(&server.shared).await,
        "mnemopi_sleep" => handle_sleep(&server.primary, args).await,
        "mnemopi_stats" | "mnemopi_get_stats" => handle_stats(&server.primary).await,
        "mnemopi_invalidate" => handle_invalidate(&server.primary, args).await,
        "mnemopi_validate" => handle_validate(&server.primary, args).await,
        "mnemopi_get" => handle_get(&server.primary, args).await,
        "mnemopi_update" => handle_update(&server.primary, args).await,
        "mnemopi_forget" => handle_forget(&server.primary, args).await,
        "mnemopi_diagnose" => handle_diagnose(&server.primary).await,

        // ── Raw-conn ops (no embeddings involved) ──────────────────────
        "mnemopi_triple_add" => handle_triple_add(&server.primary, args).await,
        "mnemopi_triple_query" => handle_triple_query(&server.primary, args).await,
        "mnemopi_scratchpad_write" => handle_scratchpad_write(&server.primary, args).await,
        "mnemopi_scratchpad_read" => handle_scratchpad_read(&server.primary, args).await,
        "mnemopi_scratchpad_clear" => handle_scratchpad_clear(&server.primary, args).await,
        "mnemopi_export" => handle_export(&server.primary, args).await,
        "mnemopi_import" => handle_import(&server.primary, args).await,
        "mnemopi_graph_query" => handle_graph_query(&server.primary, args).await,
        "mnemopi_graph_link" => handle_graph_link(&server.primary, args).await,

        other => json!({ "error": format!("unknown tool: {other}") }),
    }
}

// ── Facade handlers ────────────────────────────────────────────────────────

async fn handle_remember(engine: &Mnemopi, args: &Value) -> Value {
    let content = match require_str(args, "content") {
        Ok(c) => c,
        Err(e) => return e,
    };
    let options = build_remember_options(args);
    match engine.remember(&content, options).await {
        Ok(id) => json!({
            "status": "stored",
            "memory_id": id,
            "content_preview": preview(&content, 100),
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_shared_remember(engine: &Mnemopi, args: &Value) -> Value {
    let content = match require_str(args, "content") {
        Ok(c) => c,
        Err(e) => return e,
    };
    let kind = arg_str(args, "kind", "meta");
    if !["meta", "preference", "correction", "identity"].contains(&kind.as_str()) {
        return json!({ "error": "kind must be one of: meta, preference, correction, identity" });
    }
    let labelled = format!("Surface {kind}: {content}");
    let mut options = build_remember_options(args);
    options.importance = Some(arg_num(args, "importance", 0.8).clamp(0.0, 1.0));
    options.scope = Some(MemoryScope::Global);
    match engine.remember(&labelled, options).await {
        Ok(id) => json!({
            "status": "stored_shared",
            "memory_id": id,
            "kind": kind,
            "content_preview": preview(&labelled, 120),
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

fn build_remember_options(args: &Value) -> RememberOptions {
    RememberOptions {
        source: arg_str_opt(args, "source"),
        importance: Some(arg_num(args, "importance", 0.5).clamp(0.0, 1.0)),
        metadata: arg_metadata(args),
        veracity: arg_str_opt(args, "veracity").map(|s| Veracity::from_str_lossy(&s)),
        memory_type: arg_str_opt(args, "memory_type"),
        scope: arg_str_opt(args, "scope").map(|s| match s.as_str() {
            "global" => MemoryScope::Global,
            "session" => MemoryScope::Session,
            "channel" => MemoryScope::Channel,
            other => MemoryScope::Other(other.to_string()),
        }),
        timestamp: None,
        extract: arg_bool(args, "extract", false),
        extract_entities: arg_bool(args, "extract_entities", false),
        embedding: None,
    }
}

async fn handle_recall(engine: &Mnemopi, args: &Value) -> Value {
    let query = match require_str(args, "query") {
        Ok(q) => q,
        Err(e) => return e,
    };
    let limit = arg_int(args, "top_k", arg_int(args, "limit", 5)).max(1) as usize;
    let options = RecallOptions {
        limit: Some(limit),
        vec_weight: arg_num_opt(args, "vec_weight").map(|v| v as f32),
        fts_weight: arg_num_opt(args, "fts_weight").map(|v| v as f32),
        importance_weight: arg_num_opt(args, "importance_weight").map(|v| v as f32),
        ..Default::default()
    };
    match engine.recall(&query, options).await {
        Ok(results) => json!({
            "status": "ok",
            "query": query,
            "count": results.len(),
            "results": results,
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_sleep(engine: &Mnemopi, args: &Value) -> Value {
    let dry_run = arg_bool(args, "dry_run", false);
    let ttl_hours = arg_int(args, "ttl_hours", 24);
    match engine.sleep(ttl_hours, dry_run).await {
        Ok(result) => json!({
            "status": "ok",
            "dry_run": dry_run,
            "result": result,
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_stats(engine: &Mnemopi) -> Value {
    match engine.get_stats().await {
        Ok(stats) => json!({
            "status": "ok",
            "provider": "mnemopi",
            "stats": stats,
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_invalidate(engine: &Mnemopi, args: &Value) -> Value {
    let id = match require_str(args, "memory_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let replacement = arg_str_opt(args, "replacement_id");
    match engine.invalidate(&id, replacement.as_deref()).await {
        Ok(true) => json!({ "status": "invalidated", "memory_id": id }),
        Ok(false) => json!({ "status": "not_found", "memory_id": id }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_validate(engine: &Mnemopi, args: &Value) -> Value {
    let id = match require_str(args, "memory_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let action = arg_str(args, "action", "");
    if !["attest", "update", "invalidate", "delete"].contains(&action.as_str()) {
        return json!({ "error": format!("unknown action: {action}") });
    }

    let existing = match engine.get(&id).await {
        Ok(Some(row)) => row,
        Ok(None) => return json!({ "error": "memory_not_found", "memory_id": id }),
        Err(e) => return json!({ "error": e.to_string() }),
    };

    let status = match action.as_str() {
        "delete" => match engine.forget(&id).await {
            Ok(true) => "validation_delete",
            Ok(false) => "not_found",
            Err(e) => return json!({ "error": e.to_string() }),
        },
        "update" => {
            let new_content = match arg_str_opt(args, "new_content") {
                Some(c) => c,
                None => return json!({ "error": "new_content is required for action=update" }),
            };
            match engine.update(&id, Some(&new_content), None).await {
                Ok(true) => "validation_update",
                Ok(false) => "not_found",
                Err(e) => return json!({ "error": e.to_string() }),
            }
        }
        "invalidate" => match engine.invalidate(&id, None).await {
            Ok(true) => "validation_invalidate",
            Ok(false) => "not_found",
            Err(e) => return json!({ "error": e.to_string() }),
        },
        _ => "validation_attest",
    };

    json!({
        "status": status,
        "memory_id": id,
        "validator": arg_str(args, "validator", "unknown"),
        "previous_content": existing.content.chars().take(200).collect::<String>(),
    })
}

async fn handle_get(engine: &Mnemopi, args: &Value) -> Value {
    let id = match require_str(args, "memory_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    match engine.get(&id).await {
        Ok(Some(memory)) => json!({ "status": "ok", "memory": memory }),
        Ok(None) => json!({ "status": "not_found", "memory_id": id }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_update(engine: &Mnemopi, args: &Value) -> Value {
    let id = match require_str(args, "memory_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let content = arg_str_opt(args, "content");
    let importance = arg_num_opt(args, "importance");
    if content.is_none() && importance.is_none() {
        return json!({ "error": "content or importance is required" });
    }
    if let Some(ref c) = content
        && c.trim().is_empty()
    {
        return json!({ "error": "content must not be empty" });
    }
    match engine.update(&id, content.as_deref(), importance).await {
        Ok(true) => json!({ "status": "updated", "memory_id": id }),
        Ok(false) => json!({ "status": "not_found", "memory_id": id }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_forget(engine: &Mnemopi, args: &Value) -> Value {
    let id = match require_str(args, "memory_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    match engine.forget(&id).await {
        Ok(true) => json!({ "status": "deleted", "memory_id": id }),
        Ok(false) => json!({ "status": "not_found", "memory_id": id }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_diagnose(engine: &Mnemopi) -> Value {
    let stats = engine.blocking_get_stats();
    json!({
        "status": "ok",
        "db_path": engine.db_path().map(|p| p.to_string_lossy().into_owned()),
        "stats": stats,
    })
}

// ── Raw-conn handlers (triples, scratchpad, export/import, graph) ──────────

async fn handle_triple_add(engine: &Mnemopi, args: &Value) -> Value {
    let subject = match require_str(args, "subject") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let predicate = match require_str(args, "predicate") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let object = match require_str(args, "object") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let opts = triples::TripleWriteOptions {
        valid_from: arg_str_opt(args, "valid_from"),
        valid_until: None,
        source: arg_str_opt(args, "source"),
        confidence: Some(arg_num(args, "confidence", 1.0)),
    };
    let result = engine
        .spawn_blocking(move |conn| triples::add_triple(conn, &subject, &predicate, &object, &opts))
        .await;
    match result {
        Ok(id) => json!({ "status": "stored", "triple_id": id, "store": "triples" }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_triple_query(engine: &Mnemopi, args: &Value) -> Value {
    let query = triples::TripleQuery {
        subject: arg_str_opt(args, "subject"),
        predicate: arg_str_opt(args, "predicate"),
        object: arg_str_opt(args, "object"),
        valid_only: arg_str_opt(args, "as_of").is_some(),
        limit: Some(100),
    };
    let result = engine
        .spawn_blocking(move |conn| triples::query_triples(conn, &query))
        .await;
    match result {
        Ok(rows) => json!({
            "count": rows.len(),
            "results": rows,
            "store": "triples",
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_scratchpad_write(engine: &Mnemopi, args: &Value) -> Value {
    let content = match require_str(args, "content") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session_id = engine.config().session_id.clone();
    let result = engine
        .spawn_blocking(move |conn| scratchpad_write(conn, &session_id, &content))
        .await;
    match result {
        Ok(id) => json!({ "status": "written", "id": id, "entry_id": id }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_scratchpad_read(engine: &Mnemopi, args: &Value) -> Value {
    let _ = args; // currently ignores bank; reads the engine's session
    let session_id = engine.config().session_id.clone();
    let result = engine
        .spawn_blocking(move |conn| scratchpad_read(conn, &session_id))
        .await;
    match result {
        Ok(entries) => json!({
            "status": "ok",
            "entries_count": entries.len(),
            "count": entries.len(),
            "entries": entries,
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_scratchpad_clear(engine: &Mnemopi, _args: &Value) -> Value {
    let session_id = engine.config().session_id.clone();
    let result = engine
        .spawn_blocking(move |conn| scratchpad_clear(conn, &session_id))
        .await;
    match result {
        Ok(deleted) => json!({ "status": "cleared", "deleted": deleted }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_export(engine: &Mnemopi, args: &Value) -> Value {
    let output_path = match require_str(args, "output_path") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session_id = engine.config().session_id.clone();
    let result = engine
        .spawn_blocking(move |conn| export_to_dict(conn, &session_id))
        .await;
    match result {
        Ok(data) => {
            if let Err(e) = std::fs::write(
                &output_path,
                serde_json::to_vec_pretty(&data).unwrap_or_default(),
            ) {
                return json!({ "status": "error", "message": format!("write {output_path}: {e}") });
            }
            json!({ "status": "exported", "output_path": output_path })
        }
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_import(engine: &Mnemopi, args: &Value) -> Value {
    let input_path = match require_str(args, "input_path") {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !Path::new(&input_path).exists() {
        return json!({ "error": format!("input_path does not exist: {input_path}") });
    }
    let data = match std::fs::read_to_string(&input_path) {
        Ok(s) => s,
        Err(e) => return json!({ "error": format!("read {input_path}: {e}") }),
    };
    let parsed: Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => return json!({ "error": format!("parse {input_path}: {e}") }),
    };
    let session_id = engine.config().session_id.clone();
    let result = engine
        .spawn_blocking(move |conn| import_from_dict(conn, &session_id, &parsed))
        .await;
    match result {
        Ok(stats) => json!({ "status": "imported", "stats": stats }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_graph_query(engine: &Mnemopi, args: &Value) -> Value {
    let seed_id = match require_str(args, "seed_memory_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let max_hops = arg_int(args, "max_hops", 2).max(0) as usize;
    let edge_type_filter = arg_str_opt(args, "edge_type");
    let min_weight = arg_num(args, "min_weight", 0.0);
    let seed_id_for_closure = seed_id.clone();
    let result = engine
        .spawn_blocking(move |conn| {
            let mut related =
                episodic_graph::find_related_memories(conn, &seed_id_for_closure, max_hops)?;
            related.retain(|r| {
                (edge_type_filter.as_ref().is_none()
                    || edge_type_filter.as_deref() == Some(&r.edge_type))
                    && r.weight >= min_weight
            });
            Ok::<_, MnemopiError>(related)
        })
        .await;
    match result {
        Ok(related) => json!({
            "status": "ok",
            "seed_memory_id": seed_id,
            "count": related.len(),
            "results": related,
            "related_memories": related,
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

async fn handle_graph_link(engine: &Mnemopi, args: &Value) -> Value {
    let source_id = match require_str(args, "source_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let target_id = match require_str(args, "target_id") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let relationship = match require_str(args, "relationship") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let weight = arg_num(args, "weight", 0.5);
    let (src, tgt, rel) = (source_id.clone(), target_id.clone(), relationship.clone());
    let result = engine
        .spawn_blocking(move |conn| graph_add_edge(conn, &src, &tgt, &rel, weight))
        .await;
    match result {
        Ok(_) => json!({
            "status": "linked",
            "source_id": source_id,
            "target_id": target_id,
            "relationship": relationship,
            "edge_type": relationship,
            "weight": weight,
        }),
        Err(e) => json!({ "status": "error", "message": e.to_string() }),
    }
}

// ── Inline helpers (no facade method exists for these) ─────────────────────

fn scratchpad_write(
    conn: &rusqlite::Connection,
    session_id: &str,
    content: &str,
) -> Result<String> {
    let id = format!("sp-{}", uuid::Uuid::new_v4());
    conn.execute(
        "INSERT INTO scratchpad (id, content, session_id) VALUES (?1, ?2, ?3)",
        params![id, content, session_id],
    )?;
    Ok(id)
}

#[derive(Debug, Clone, Serialize)]
struct ScratchpadEntry {
    id: String,
    content: String,
    session_id: String,
    created_at: String,
    updated_at: String,
}

fn scratchpad_read(conn: &rusqlite::Connection, session_id: &str) -> Result<Vec<ScratchpadEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, session_id, created_at, updated_at
         FROM scratchpad WHERE session_id = ?1
         ORDER BY created_at DESC LIMIT 200",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        Ok(ScratchpadEntry {
            id: row.get(0)?,
            content: row.get(1)?,
            session_id: row.get(2)?,
            created_at: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            updated_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn scratchpad_clear(conn: &rusqlite::Connection, session_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM scratchpad WHERE session_id = ?1",
        params![session_id],
    )?)
}

/// Dump all non-superseded working-memory rows for `session_id`.
///
/// Mirrors omp's `exportToDict` — selects every row scoped to the
/// session, excluding superseded entries. Uses `store::row_to_memory_row`
/// to hydrate each row into a [`MemoryRow`].
fn export_working_memory(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<crate::types::MemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, session_id, importance,
                metadata_json, veracity, memory_type, recall_count,
                last_recalled, valid_until, superseded_by, scope,
                author_id, author_type, channel_id, created_at
         FROM working_memory
         WHERE COALESCE(session_id, 'default') = ?1 AND superseded_by IS NULL
         ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map(params![session_id], store::row_to_memory_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn export_to_dict(conn: &rusqlite::Connection, session_id: &str) -> Result<Value> {
    let working = export_working_memory(conn, session_id)?;
    let triples_rows = triples::query_triples(conn, &triples::TripleQuery::default())?;
    let stats = store::get_stats(conn)?;
    Ok(json!({
        "session_id": session_id,
        "working_memory": working,
        "triples": triples_rows,
        "stats": stats,
    }))
}

fn import_from_dict(conn: &rusqlite::Connection, session_id: &str, data: &Value) -> Result<Value> {
    let mut inserted = 0usize;
    let mut skipped = 0usize;
    if let Some(arr) = data.get("working_memory").and_then(|v| v.as_array()) {
        for row in arr {
            let content = row.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.is_empty() {
                skipped += 1;
                continue;
            }
            let options = RememberOptions {
                source: Some(session_id.to_string()),
                importance: row.get("importance").and_then(|v| v.as_f64()),
                veracity: row
                    .get("veracity")
                    .and_then(|v| v.as_str())
                    .map(Veracity::from_str_lossy),
                ..Default::default()
            };
            match store::remember(conn, content, session_id, &options, None) {
                Ok(_) => inserted += 1,
                Err(_) => skipped += 1,
            }
        }
    }
    Ok(json!({ "inserted": inserted, "skipped": skipped }))
}

fn graph_add_edge(
    conn: &rusqlite::Connection,
    source: &str,
    target: &str,
    edge_type: &str,
    weight: f64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO episodic_edges (source, target, edge_type, weight)
         VALUES (?1, ?2, ?3, ?4)",
        params![source, target, edge_type, weight],
    )?;
    Ok(())
}

fn preview(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// JSON-RPC dispatcher
// ═══════════════════════════════════════════════════════════════════════════

impl McpServer {
    /// Handle a single JSON-RPC request, returning the response (or `None`
    /// for notifications, which receive no reply per spec).
    pub async fn handle_request(&self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        // Notifications (no `id`) are silently dropped.
        req.id.as_ref()?;
        let id = req.id.clone();
        let result: Result<Value, JsonRpcResponse> = match req.method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                "capabilities": { "tools": {} },
            })),
            "tools/list" => Ok(json!({
                "tools": tool_definitions(),
            })),
            "tools/call" => {
                let name = req
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if name.is_empty() {
                    Err(JsonRpcResponse::err(
                        id.clone(),
                        INVALID_PARAMS,
                        "tools/call requires params.name",
                    ))
                } else {
                    let args = req
                        .params
                        .get("arguments")
                        .cloned()
                        .filter(|v| v.is_object())
                        .unwrap_or(json!({}));
                    let payload = handle_tool_call(self, name, &args).await;
                    Ok(json!({
                        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default() }],
                        "isError": payload.get("error").is_some(),
                    }))
                }
            }
            "ping" => Ok(json!({})),
            other => Err(JsonRpcResponse::err(
                id.clone(),
                METHOD_NOT_FOUND,
                format!("Unknown method: {other}"),
            )),
        };
        match result {
            Ok(value) => Some(JsonRpcResponse::ok(id, value)),
            Err(resp) => Some(resp),
        }
    }

    /// Read line-delimited JSON-RPC from `reader`, write responses to
    /// `writer`. Runs until EOF on the reader. Each request line must be
    /// a complete JSON object terminated by `\n`.
    pub async fn run_stdio<R, W>(self, reader: R, writer: W) -> std::io::Result<()>
    where
        R: tokio::io::AsyncBufRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let mut reader = reader;
        let mut writer = writer;
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break; // EOF
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(_) => {
                    let resp = JsonRpcResponse::err(None, PARSE_ERROR, "Parse error");
                    let bytes = serde_json::to_vec(&resp).unwrap_or_default();
                    writer.write_all(&bytes).await?;
                    writer.write_all(b"\n").await?;
                    continue;
                }
            };

            if let Some(resp) = self.handle_request(&req).await {
                let bytes = serde_json::to_vec(&resp).unwrap_or_default();
                writer.write_all(&bytes).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
        }

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: Some("2.0".into()),
            id: Some(json!(1)),
            method: method.into(),
            params,
        }
    }

    /// Extract the JSON payload from a `tools/call` response's
    /// `content[0].text` field.
    fn extract_payload(resp: JsonRpcResponse) -> Value {
        let result = resp.result.expect("no result");
        let text = result["content"][0]["text"]
            .as_str()
            .expect("missing text field");
        serde_json::from_str(text).expect("invalid JSON in text")
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version() {
        let server = McpServer::open_in_memory().unwrap();
        let resp = server
            .handle_request(&req("initialize", json!({})))
            .await
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
    }

    #[tokio::test]
    async fn tools_list_returns_all_24_tools() {
        let server = McpServer::open_in_memory().unwrap();
        let resp = server
            .handle_request(&req("tools/list", json!({})))
            .await
            .unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 24, "expected 24 tools, got {}", tools.len());
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"mnemopi_remember"));
        assert!(names.contains(&"mnemopi_recall"));
        assert!(names.contains(&"mnemopi_graph_link"));
        assert!(names.contains(&"mnemopi_shared_stats"));
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let server = McpServer::open_in_memory().unwrap();
        let resp = server
            .handle_request(&req("bogus/method", json!({})))
            .await
            .unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn notification_returns_none() {
        let server = McpServer::open_in_memory().unwrap();
        let mut notification = req("initialized", json!({}));
        notification.id = None;
        assert!(server.handle_request(&notification).await.is_none());
    }

    #[tokio::test]
    async fn remember_then_recall_round_trip() {
        let server = McpServer::open_in_memory().unwrap();
        let put_resp = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_remember",
                    "arguments": { "content": "Rust ownership prevents data races at compile time." }
                }),
            ))
            .await
            .unwrap();
        let put_payload = extract_payload(put_resp);
        assert_eq!(put_payload["status"], "stored");
        assert!(put_payload["memory_id"].as_str().is_some());

        let recall_resp = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_recall",
                    "arguments": { "query": "rust ownership", "limit": 5 }
                }),
            ))
            .await
            .unwrap();
        let recall_payload = extract_payload(recall_resp);
        assert_eq!(recall_payload["status"], "ok");
        assert!(recall_payload["count"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn remember_requires_content() {
        let server = McpServer::open_in_memory().unwrap();
        let resp = server
            .handle_request(&req(
                "tools/call",
                json!({ "name": "mnemopi_remember", "arguments": json!({}) }),
            ))
            .await
            .unwrap();
        let payload = extract_payload(resp);
        assert!(
            payload["error"]
                .as_str()
                .unwrap()
                .contains("content is required")
        );
    }

    #[tokio::test]
    async fn triple_add_and_query_round_trip() {
        let server = McpServer::open_in_memory().unwrap();
        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_triple_add",
                    "arguments": {
                        "subject": "alice",
                        "predicate": "works_on",
                        "object": "oxi",
                    }
                }),
            ))
            .await
            .unwrap();

        let resp = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_triple_query",
                    "arguments": { "subject": "alice" }
                }),
            ))
            .await
            .unwrap();
        let payload = extract_payload(resp);
        assert_eq!(payload["count"].as_u64().unwrap(), 1);
        assert_eq!(payload["results"][0]["object"], "oxi");
    }

    #[tokio::test]
    async fn scratchpad_write_read_clear() {
        let server = McpServer::open_in_memory().unwrap();
        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_scratchpad_write",
                    "arguments": { "content": "todo: ship" }
                }),
            ))
            .await
            .unwrap();

        let read = server
            .handle_request(&req(
                "tools/call",
                json!({ "name": "mnemopi_scratchpad_read", "arguments": json!({}) }),
            ))
            .await
            .unwrap();
        let payload = extract_payload(read);
        assert_eq!(payload["count"].as_u64().unwrap(), 1);
        assert_eq!(payload["entries"][0]["content"], "todo: ship");

        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({ "name": "mnemopi_scratchpad_clear", "arguments": json!({}) }),
            ))
            .await
            .unwrap();
        let read2 = server
            .handle_request(&req(
                "tools/call",
                json!({ "name": "mnemopi_scratchpad_read", "arguments": json!({}) }),
            ))
            .await
            .unwrap();
        let payload2 = extract_payload(read2);
        assert_eq!(payload2["count"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn graph_link_then_query() {
        let server = McpServer::open_in_memory().unwrap();
        // Need two memories to link.
        let a = server
            .handle_request(&req(
                "tools/call",
                json!({ "name": "mnemopi_remember", "arguments": { "content": "alpha" } }),
            ))
            .await
            .unwrap();
        let b = server
            .handle_request(&req(
                "tools/call",
                json!({ "name": "mnemopi_remember", "arguments": { "content": "beta" } }),
            ))
            .await
            .unwrap();
        let a_id = extract_payload(a)["memory_id"]
            .as_str()
            .unwrap()
            .to_string();
        let b_id = extract_payload(b)["memory_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_graph_link",
                    "arguments": {
                        "source_id": a_id,
                        "target_id": b_id,
                        "relationship": "related_to",
                    }
                }),
            ))
            .await
            .unwrap();

        let resp = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_graph_query",
                    "arguments": { "seed_memory_id": a_id, "max_hops": 1 }
                }),
            ))
            .await
            .unwrap();
        let payload = extract_payload(resp);
        assert_eq!(payload["status"], "ok");
        assert!(payload["count"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn export_then_import_round_trip() {
        let server = McpServer::open_in_memory().unwrap();
        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_remember",
                    "arguments": { "content": "exportable fact" }
                }),
            ))
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let export_path = tmp.path().join("export.json");
        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_export",
                    "arguments": { "output_path": export_path.to_string_lossy() }
                }),
            ))
            .await
            .unwrap();
        assert!(export_path.exists());

        let import_resp = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_import",
                    "arguments": { "input_path": export_path.to_string_lossy() }
                }),
            ))
            .await
            .unwrap();
        let payload = extract_payload(import_resp);
        assert_eq!(payload["status"], "imported");
        assert!(payload["stats"]["inserted"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn shared_bank_is_separate_from_primary() {
        let server = McpServer::open_in_memory().unwrap();
        let _ = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_shared_remember",
                    "arguments": { "content": "shared fact", "kind": "preference" }
                }),
            ))
            .await
            .unwrap();

        // Primary recall should NOT see the shared memory.
        let primary = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_recall",
                    "arguments": { "query": "shared fact" }
                }),
            ))
            .await
            .unwrap();
        let primary_payload = extract_payload(primary);
        assert_eq!(primary_payload["count"].as_u64().unwrap(), 0);

        // Shared recall should find it.
        let shared = server
            .handle_request(&req(
                "tools/call",
                json!({
                    "name": "mnemopi_shared_recall",
                    "arguments": { "query": "shared fact" }
                }),
            ))
            .await
            .unwrap();
        let shared_payload = extract_payload(shared);
        assert!(shared_payload["count"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn preview_truncates_long_strings() {
        let s = "x".repeat(200);
        let out = preview(&s, 50);
        assert!(out.chars().count() <= 51);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn arg_helpers_default_correctly() {
        let args = json!({ "present": "hi", "num": 3.5, "flag": true });
        assert_eq!(arg_str(&args, "present", "fallback"), "hi");
        assert_eq!(arg_str(&args, "missing", "fallback"), "fallback");
        assert_eq!(arg_num(&args, "num", 0.0), 3.5);
        assert_eq!(arg_num(&args, "missing", 9.9), 9.9);
        assert!(arg_bool(&args, "flag", false));
        assert!(!arg_bool(&args, "missing", false));
        assert_eq!(arg_str_opt(&args, "missing"), None);
        assert_eq!(arg_str_opt(&args, "present"), Some("hi".into()));
    }
}
