#![allow(unused_doc_comments)]
/// Agent tools system
/// This module provides the tool abstraction layer and built-in tools.
use crate::types::ToolDefinition;
use async_trait::async_trait;
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Context passed to tools at execution time.
///
/// This allows tools to operate on a specific workspace without being
/// rebuilt. When `root_dir` is `Some`, tools use it as their base directory.
/// When `None`, tools should fall back to `workspace_dir`.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Primary workspace directory (used when root_dir is None).
    pub workspace_dir: PathBuf,
    /// Optional explicit root directory for file tools.
    /// Takes priority over workspace_dir if present.
    pub root_dir: Option<PathBuf>,
    /// Session identifier for logging/tracing.
    pub session_id: Option<String>,
}

impl ToolContext {
    /// Create a new context with the given workspace.
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            root_dir: None,
            session_id: None,
        }
    }

    /// Get the effective root directory.
    /// Returns root_dir if set, otherwise workspace_dir.
    pub fn root(&self) -> &Path {
        self.root_dir.as_deref().unwrap_or(&self.workspace_dir)
    }

    /// Set a session ID.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set an explicit root directory.
    pub fn with_root(mut self, root_dir: impl Into<PathBuf>) -> Self {
        self.root_dir = Some(root_dir.into());
        self
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            workspace_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            root_dir: None,
            session_id: None,
        }
    }
}

/// Result type for tool execution
pub type ToolError = String;

/// Result of tool execution
#[derive(Debug)]
pub struct AgentToolResult {
    /// pub.
    pub success: bool,
    /// pub.
    pub output: String,
    /// pub.
    pub metadata: Option<serde_json::Value>,
    /// Optional content blocks (e.g., image blocks) to include in the tool result message.
    /// When present, these are used as the content of the ToolResultMessage instead of
    /// wrapping `output` in a Text block.
    pub content_blocks: Option<Vec<oxi_ai::ContentBlock>>,
    /// When `true`, signals that the agent loop should terminate after this batch
    /// of tool calls completes.  Defaults to `false` so that the loop continues
    /// unless a tool explicitly opts-in to termination.
    pub terminate: bool,
}

impl AgentToolResult {
    /// Creates a successful tool result with the given output text.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            metadata: None,
            content_blocks: None,
            terminate: false,
        }
    }

    /// Creates an error tool result with the given error message.
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            success: false,
            output: output.into(),
            metadata: None,
            content_blocks: None,
            terminate: false,
        }
    }

    /// Attaches structured metadata (JSON) to this result.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Attaches rich content blocks (images, code, etc.) to this result.
    pub fn with_content_blocks(mut self, blocks: Vec<oxi_ai::ContentBlock>) -> Self {
        self.content_blocks = Some(blocks);
        self
    }

    /// Mark this result as requesting agent-loop termination.
    pub fn with_terminate(mut self) -> Self {
        self.terminate = true;
        self
    }
}

impl fmt::Display for AgentToolResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.output)
    }
}

/// Callback type for progress updates
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Structured progress event for tool execution streaming.
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// Status message (progress in progress)
    Status {
        /// The status text.
        message: String,
    },
    /// Partial output (e.g., bash stdout streaming)
    PartialOutput {
        /// The partial output text.
        output: String,
        /// Whether this came from stderr.
        is_error: bool,
    },
    /// Progress percentage (0.0 - 1.0)
    Percentage {
        /// Current progress value.
        current: f64,
        /// Optional total value.
        total: Option<f64>,
        /// Optional human-readable message.
        message: Option<String>,
    },
    /// File operation progress
    FileOperation {
        /// Type of file operation.
        operation: FileOp,
        /// File path being operated on.
        path: std::path::PathBuf,
        /// Bytes processed so far.
        bytes_processed: Option<u64>,
        /// Total bytes to process.
        total_bytes: Option<u64>,
    },
}

/// File operation types for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    /// Reading a file.
    Reading,
    /// Writing a file.
    Writing,
    /// Searching file contents.
    Searching,
    /// Editing a file.
    Editing,
}

/// Tool execution mode for parallel safety.
#[derive(Debug, Clone)]
pub enum ToolExecutionMode {
    /// Safe to run in parallel with any other tool
    ParallelSafe,
    /// Must run sequentially — no parallel execution
    SequentialOnly,
    /// Mutates a specific file — file_mutation_queue serializes same-file access
    MutatesFile(std::path::PathBuf),
    /// Read-only — always parallel safe
    ReadOnly,
}

/// Render output for TUI visualization.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    /// Rendered text content (markdown or plain)
    pub content: String,
    /// Whether to show collapsed by default
    pub collapsed: bool,
    /// Optional summary text for TUI footer
    pub summary: Option<String>,
}

/// Structured progress callback (alongside existing String callback)
pub type StructuredProgressCallback = Arc<dyn Fn(ToolProgress) + Send + Sync>;

/// Core trait for all agent tools
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Tool name (used in function calls)
    fn name(&self) -> &str;

    /// Human-readable label
    fn label(&self) -> &str;

    /// Description for the model
    fn description(&self) -> &str;

    /// JSON Schema for parameters
    fn parameters_schema(&self) -> Value;

    /// Whether this tool is essential (cannot be disabled).
    /// Essential tools: read, write, edit, bash, grep, find, ls
    /// Optional tools: web_search, github, subagent, etc.
    fn essential(&self) -> bool {
        false
    }

    /// Execute the tool with the given tool call ID and parameters.
    ///
    /// The `ctx` parameter provides workspace information. File tools should
    /// use `ctx.root()` to get the effective directory. Custom tools can use
    /// `ctx.workspace_dir` for workspace-relative operations.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use oxi_agent::{AgentTool, AgentToolResult, ToolContext};
    /// use serde_json::json;
    /// use async_trait::async_trait;
    ///
    /// struct MyTool;
    ///
    /// #[async_trait]
    /// impl AgentTool for MyTool {
    ///     fn name(&self) -> &str { "my_tool" }
    ///     fn label(&self) -> &str { "My Tool" }
    ///     fn description(&self) -> &str { "A custom tool" }
    ///     fn parameters_schema(&self) -> Value { json!({
    ///         "type": "object",
    ///         "properties": {}
    ///     }) }
    ///
    ///     async fn execute(&self, tool_call_id: &str, params: Value, _signal: Option<oneshot::Receiver<()>>, ctx: &ToolContext) -> Result<AgentToolResult, String> {
    ///         println!("Tool '{}' called with params: {:?}, workspace: {:?}", tool_call_id, params, ctx.workspace_dir);
    ///         Ok(AgentToolResult::success("Done!"))
    ///     }
    /// }
    /// ```
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError>;

    /// Called with progress updates during execution.
    /// Tools can override this to emit streaming updates.
    fn on_progress(&self, _callback: ProgressCallback) {
        // Default no-op
    }

    /// Structured progress callback for streaming tool execution updates.
    /// Default implementation is no-op. Override in tools that support
    /// structured progress (e.g., BashTool for partial output streaming).
    fn on_structured_progress(&self, _callback: StructuredProgressCallback) {}

    /// Custom rendering for tool call (TUI visualization).
    /// Return None to use the default tool_renderer.rs formatter.
    fn render_call(&self, _params: &serde_json::Value) -> Option<RenderOutput> {
        None
    }

    /// Custom rendering for tool result (TUI visualization).
    /// Return None to use the default tool_renderer.rs formatter.
    fn render_result(&self, _result: &AgentToolResult) -> Option<RenderOutput> {
        None
    }

    /// Execution mode for parallel safety.
    /// Defaults to ParallelSafe. Override for file-mutating or sequential tools.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::ParallelSafe
    }

    /// Convert to ToolDefinition
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: serde_json::from_value(self.parameters_schema()).unwrap_or_default(),
        }
    }
}

// Built-in tools
/// Bash shell execution tool.
pub mod bash;
/// Browser tools (engine abstraction always compiled).
pub mod browse;
/// Context7 documentation tools.
pub mod context7;
/// In-place file edit tool.
pub mod edit;
/// Diff-based edit helpers.
pub mod edit_diff;
/// Serialised file-mutation queue.
pub mod file_mutation_queue;
/// File-fsystem find tool.
pub mod find;
/// Image generation tool (OpenRouter API).
pub mod generate_image;
/// GitHub integration tool (gh CLI-based).
pub mod github;
/// GitHub repository search tool (legacy REST API).
pub mod github_search;
/// Content search (grep) tool.
pub mod grep;
/// Shared HTTP client singleton.
pub mod http_client;
/// Directory listing tool.
pub mod ls;
/// Path security (traversal protection).
pub mod path_security;
/// Path manipulation utilities.
pub mod path_utils;
/// Questionnaire tool — interactive multi-question TUI overlay.
pub mod questionnaire;
/// File reading tool.
pub mod read;
/// Rendering utilities for tool output.
pub mod render_utils;
/// Search result cache and get_search_results tool.
pub mod search_cache;
/// Sub-agent delegation tool.
pub mod subagent;
/// Tool definition wrapper helpers.
pub mod tool_definition_wrapper;
/// Output truncation helpers.
pub mod truncate;
/// Multi-engine web search tool (a3s-search library + DuckDuckGo fallback).
pub mod web_search;
/// File writing tool.
pub mod write;

// Re-export for convenience
pub use bash::BashTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read::ReadTool;
// pub use search_cache;

pub use crate::mcp::McpTool;
pub use context7::{Context7QueryDocsTool, Context7ResolveLibraryIdTool};
pub use questionnaire::{QuestionnaireBridge, QuestionnaireTool};
pub use subagent::SubagentTool;
pub use write::WriteTool;

/// Tool registry for managing available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<parking_lot::RwLock<std::collections::HashMap<String, Arc<dyn AgentTool>>>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Creates an empty tool registry.
    pub fn new() -> Self {
        Self {
            tools: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Register a tool
    pub fn register(&self, tool: impl AgentTool + 'static) {
        let name = tool.name().to_string();
        self.tools.write().insert(name, Arc::new(tool));
    }

    /// Register a tool that is already wrapped in an `Arc`.
    /// This is the primary path for extensions that produce `Arc<dyn AgentTool>`.
    pub fn register_arc(&self, tool: Arc<dyn AgentTool>) {
        let name = tool.name().to_string();
        self.tools.write().insert(name, tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.read().get(name).cloned()
    }

    /// Unregister a tool by name.
    /// Returns `true` if the tool was present and removed.
    pub fn unregister(&self, name: &str) -> bool {
        self.tools.write().remove(name).is_some()
    }

    /// List all registered tool names
    pub fn names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

    /// Get all tool definitions
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .values()
            .map(|t| t.to_definition())
            .collect()
    }

    /// Get all tools as a slice
    pub fn get_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.read().values().cloned().collect()
    }

    /// Check whether all tools in `required` are registered.
    ///
    /// Useful for validating program/module dependencies before execution.
    ///
    /// # Example
    ///
    /// ```
    /// use oxi_agent::ToolRegistry;
    /// let registry = ToolRegistry::new();
    /// assert!(!registry.has_all(&["read", "write"]));
    /// ```
    pub fn has_all(&self, required: &[&str]) -> bool {
        let tools = self.tools.read();
        required.iter().all(|name| tools.contains_key(*name))
    }

    /// Return the subset of `required` tool names that are **not** registered.
    ///
    /// # Example
    ///
    /// ```
    /// use oxi_agent::ToolRegistry;
    /// let registry = ToolRegistry::new();
    /// let missing = registry.missing(&["read", "exec", "nonexistent"]);
    /// assert_eq!(missing, vec!["read", "exec", "nonexistent"]);
    /// ```
    pub fn missing<'a>(&self, required: &[&'a str]) -> Vec<&'a str> {
        let tools = self.tools.read();
        required
            .iter()
            .filter(|name| !tools.contains_key(**name))
            .copied()
            .collect()
    }

    /// Create a registry with all built-in tools
    ///
    /// # Examples
    ///
    /// ```
    /// use oxi_agent::ToolRegistry;
    /// let registry = ToolRegistry::with_builtins();
    /// let tools = registry.names();
    /// assert!(tools.contains(&"read".to_string()));
    /// assert!(tools.contains(&"write".to_string()));
    /// assert!(tools.contains(&"bash".to_string()));
    /// ```
    pub fn with_builtins() -> Self {
        Self::with_builtins_cwd(PathBuf::from("."), &[])
    }

    /// Create a registry with all built-in tools, using the given cwd.
    ///
    /// Pass `disabled_tools` to selectively disable built-in tools
    /// (e.g. `["web_search", "github_search"]` for a minimal setup).
    pub fn with_builtins_cwd(cwd: PathBuf, disabled_tools: &[String]) -> Self {
        let registry = Self::new();
        let disabled: std::collections::HashSet<&str> =
            disabled_tools.iter().map(|s| s.as_str()).collect();

        // Helper to create shared cache on demand
        let cache_once: std::cell::OnceCell<Arc<search_cache::SearchCache>> =
            std::cell::OnceCell::new();

        // MCP: use OnceCell to avoid re-creating McpManager on repeated calls
        let mcp_once: std::cell::OnceCell<Arc<crate::mcp::McpManager>> = std::cell::OnceCell::new();
        let mcp_manager = mcp_once
            .get_or_init(|| Arc::new(crate::mcp::McpManager::new()))
            .clone();

        // Register all builtin tools — essential ones ignore disabled list
        let mut all_tools: Vec<Box<dyn AgentTool>> = vec![
            Box::new(ReadTool::with_cwd(cwd.clone())),
            Box::new(WriteTool::with_cwd(cwd.clone())),
            Box::new(EditTool::with_cwd(cwd.clone())),
            Box::new(BashTool::with_cwd(cwd.clone())),
            Box::new(GrepTool::with_cwd(cwd.clone())),
            Box::new(FindTool::with_cwd(cwd.clone())),
            Box::new(LsTool::with_cwd(cwd.clone())),
            Box::new(web_search::WebSearchTool::new(
                cache_once
                    .get_or_init(|| Arc::new(search_cache::SearchCache::new()))
                    .clone(),
            )),
            Box::new(search_cache::GetSearchResultsTool::new(
                cache_once
                    .get_or_init(|| Arc::new(search_cache::SearchCache::new()))
                    .clone(),
            )),
            Box::new(github::GitHubTool::new(
                cache_once
                    .get_or_init(|| Arc::new(search_cache::SearchCache::new()))
                    .clone(),
            )),
            Box::new(SubagentTool::with_cwd(cwd)),
        ];

        all_tools.push(Box::new(crate::mcp::McpTool::new(mcp_manager)));
        all_tools.push(Box::new(context7::Context7ResolveLibraryIdTool::new()));
        all_tools.push(Box::new(context7::Context7QueryDocsTool::new()));
        all_tools.push(Box::new(generate_image::GenerateImageTool::new()));

        for tool in all_tools {
            if tool.essential() || !disabled.contains(tool.name()) {
                // web_search ↔ get_search_results coupling
                if tool.name() == "get_search_results" && disabled.contains("web_search") {
                    continue;
                }
                registry.register_arc(Arc::from(tool));
            }
        }

        registry
    }

    /// Extend this registry with all tools from another registry.
    ///
    /// Useful for composing tool sets from multiple sources
    /// (e.g., coding tools + kernel tools + browser tools).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let base = ToolRegistry::new();
    /// base.extend_from(&other_registry);
    /// ```
    pub fn extend_from(&self, other: &ToolRegistry) {
        for name in other.names() {
            if let Some(tool) = other.get(&name) {
                self.register_arc(tool);
            }
        }
    }

    /// Create registry with selected builtins only.
    pub fn with_selected_tools(cwd: PathBuf, names: &[&str]) -> Self {
        let full = Self::with_builtins_cwd(cwd, &[]);
        let registry = Self::new();
        let set: std::collections::HashSet<&str> = names.iter().copied().collect();
        for name in full.names() {
            if set.contains(name.as_str()) {
                if let Some(tool) = full.get(&name) {
                    registry.register_arc(tool);
                }
            }
        }
        registry
    }
}
