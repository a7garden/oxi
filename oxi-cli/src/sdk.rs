//! SDK — high-level library entry point for oxi.
//!
//! Ported from `pi-mono/packages/coding-agent/src/core/sdk.ts`.
//!
//! This module provides the main SDK interface that wraps [`Agent`] and
//! [`AgentSession`] into an easy-to-use library API. It handles:
//!
//! - SDK configuration (model, thinking level, tools, working directory)
//! - Provider creation and API key resolution
//! - Session creation with persistence and restoration
//! - Tool registration (built-in and custom)
//! - Settings management
//!
//! # Example
//!
//! ```no_run
//! use oxi::sdk::{Sdk, SdkConfig};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Minimal — uses defaults
//!     let sdk = Sdk::new(SdkConfig::default()).await?;
//!
//!     // Run a prompt
//!     let response = sdk.run_prompt("Explain Rust traits").await?;
//!     println!("{}", response);
//!
//!     Ok(())
//! }
//! ```

use crate::agent_session::{AgentSession, CompactionReason};
use crate::auth_storage::AuthStorage;
use crate::defaults;
use crate::messages::convert_to_llm;
use crate::model_registry::ModelRegistry;
use crate::model_resolver;
use crate::resource_loader::{ResourceLoader, DefaultResourceLoader};
use crate::session::SessionManager;
use crate::settings::{Settings, ThinkingLevel};
use crate::telemetry;
use crate::timings;
use anyhow::{Context, Result};
use oxi_agent::{Agent, AgentConfig, AgentEvent, ToolRegistry};
use oxi_ai::{get_model, get_provider, stream_simple, Message, Model, Provider};
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

// =============================================================================
// Re-exports
// =============================================================================

// Re-export agent session types
pub use crate::agent_session::SessionEvent;

// Re-export tool types
pub use oxi_agent::{
    AgentTool, AgentToolResult, BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool,
    WriteTool,
};

// Re-export settings
pub use crate::settings::Settings as SdkSettings;

// =============================================================================
// SDK Configuration
// =============================================================================

/// Configuration for creating an SDK instance.
///
/// All fields are optional — sensible defaults are applied when omitted.
///
/// # Defaults
///
/// | Field               | Default                                         |
/// |---------------------|-------------------------------------------------|
/// | `cwd`               | Current working directory                       |
/// | `agent_dir`         | `~/.oxi`                                        |
/// | `model`             | From settings, then first available             |
/// | `thinking_level`    | From settings, then `Standard`                  |
/// | `tools`             | `["read", "bash", "edit", "write"]`             |
/// | `no_tools`          | `None` (all default tools enabled)              |
#[derive(Debug, Clone)]
pub struct SdkConfig {
    // ── Paths ──────────────────────────────────────────────────────
    /// Working directory for project-local discovery.
    /// Default: current working directory.
    pub cwd: Option<PathBuf>,

    /// Global config directory.
    /// Default: `~/.oxi`.
    pub agent_dir: Option<PathBuf>,

    // ── Auth & Models ──────────────────────────────────────────────
    /// Auth storage for credentials.
    /// Default: created from `agent_dir/auth.json`.
    pub auth_storage: Option<AuthStorage>,

    /// Model registry.
    /// Default: created from auth storage and `agent_dir/models.json`.
    pub model_registry: Option<ModelRegistry>,

    /// Model to use, in `provider/model` format.
    /// Default: from settings, else first available.
    pub model: Option<String>,

    /// Thinking level for the agent.
    /// Default: from settings, else [`ThinkingLevel::Standard`].
    pub thinking_level: Option<ThinkingLevel>,

    /// Models available for cycling (Ctrl+P in interactive mode).
    pub scoped_models: Vec<ScopedModel>,

    // ── Tools ──────────────────────────────────────────────────────
    /// Tool suppression mode.
    ///
    /// - `None`: default tools enabled
    /// - `Some(NoTools::All)`: start with no tools
    /// - `Some(NoTools::Builtin)`: disable built-in tools, keep custom
    pub no_tools: Option<NoTools>,

    /// Optional allowlist of tool names.
    ///
    /// When provided, only the listed tool names are enabled.
    pub tools: Option<Vec<String>>,

    // ── Session ────────────────────────────────────────────────────
    /// Session manager.
    /// Default: created from `cwd`.
    pub session_manager: Option<SessionManager>,

    /// Settings manager.
    /// Default: loaded from `cwd` and `agent_dir`.
    pub settings: Option<Settings>,
}

/// A model scoped for availability in the SDK (for model cycling).
#[derive(Debug, Clone)]
pub struct ScopedModel {
    /// Model ID in `provider/model` format.
    pub model: String,
    /// Optional thinking level override for this scoped model.
    pub thinking_level: Option<ThinkingLevel>,
}

/// Tool suppression mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTools {
    /// Start with no tools enabled at all.
    All,
    /// Disable built-in tools (read, bash, edit, write) but keep custom tools.
    Builtin,
}

impl Default for SdkConfig {
    fn default() -> Self {
        Self {
            cwd: None,
            agent_dir: None,
            auth_storage: None,
            model_registry: None,
            model: None,
            thinking_level: None,
            scoped_models: Vec::new(),
            no_tools: None,
            tools: None,
            session_manager: None,
            settings: None,
        }
    }
}

// =============================================================================
// SDK Creation Result
// =============================================================================

/// Result of creating an SDK session.
#[derive(Debug)]
pub struct SdkCreateResult {
    /// The created SDK instance.
    pub sdk: Sdk,
    /// Warning if the session was restored with a different model than saved.
    pub model_fallback_message: Option<String>,
}

// =============================================================================
// SDK Entry Point
// =============================================================================

/// Main SDK entry point wrapping an [`Agent`] and [`AgentSession`].
///
/// Provides a high-level interface for:
/// - Running prompts (single-shot and streaming)
/// - Managing sessions (persistence, restoration, branching)
/// - Registering tools (built-in and custom)
/// - Switching models mid-session
/// - Accessing agent state
pub struct Sdk {
    /// The underlying agent.
    agent: Arc<Agent>,
    /// Session manager for persistence.
    session_manager: SessionManager,
    /// Settings reference.
    settings: Settings,
    /// Working directory.
    cwd: PathBuf,
    /// Active tool names.
    active_tool_names: Vec<String>,
    /// Allowed tool names (if restricted).
    allowed_tool_names: Option<Vec<String>>,
    /// Model registry.
    model_registry: ModelRegistry,
    /// Resource loader.
    resource_loader: DefaultResourceLoader,
    /// Scoped models for cycling.
    scoped_models: Vec<ScopedModel>,
    /// Session ID.
    session_id: uuid::Uuid,
}

// =============================================================================
// SDK Implementation
// =============================================================================

impl Sdk {
    /// Create a new SDK instance with the given configuration.
    ///
    /// This is the primary entry point. It:
    /// 1. Resolves defaults for all config fields
    /// 2. Loads or creates auth storage and model registry
    /// 3. Resolves the model (from config, settings, or first available)
    /// 4. Creates an [`Agent`] with the resolved model
    /// 5. Creates a [`SessionManager`] for persistence
    /// 6. Returns the SDK handle and any fallback warnings
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Settings cannot be loaded
    /// - No model is available (no API keys configured)
    /// - The session manager cannot be initialized
    pub async fn new(config: SdkConfig) -> Result<SdkCreateResult> {
        // Resolve paths
        let cwd = config
            .cwd
            .or_else(|| config.session_manager.as_ref().map(|sm| sm.cwd().to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let agent_dir = config
            .agent_dir
            .unwrap_or_else(default_agent_dir);

        // Load settings
        let settings = config.settings.unwrap_or_else(|| {
            Settings::load_from(&cwd).unwrap_or_default()
        });

        // Create auth storage and model registry
        let auth_path = agent_dir.join("auth.json");
        let auth_storage = config
            .auth_storage
            .unwrap_or_else(|| AuthStorage::from_path(&auth_path));

        let models_path = agent_dir.join("models.json");
        let model_registry = config
            .model_registry
            .unwrap_or_else(|| ModelRegistry::new_with_auth(auth_storage, &models_path));

        // Create resource loader
        let resource_loader = DefaultResourceLoader::new(&cwd, &agent_dir, &settings);
        resource_loader.reload().await;
        timings::time("resource_loader.reload");

        // Create session manager
        let session_manager = config.session_manager.unwrap_or_else(|| {
            let session_dir = settings
                .effective_session_dir()
                .unwrap_or_else(|_| cwd.join(".oxi").join("sessions"));
            SessionManager::new(&cwd, &session_dir)
        });

        let session_id = session_manager.session_id();

        // Check if session has existing data to restore
        let existing_session = session_manager.build_session_context();
        let has_existing_session = !existing_session.messages.is_empty();

        // Resolve model
        let (model_id, model_fallback_message) = resolve_model(
            &config,
            &settings,
            &model_registry,
            has_existing_session,
            &existing_session,
        );

        // If no model was resolved, return an error with guidance
        let model_id = match model_id {
            Some(id) => id,
            None => {
                return Err(anyhow::anyhow!(
                    "No models available. Please configure an API key.\n\
                     Run `oxi auth login <provider>` or set the appropriate \
                     environment variable (e.g., ANTHROPIC_API_KEY, OPENAI_API_KEY)."
                ));
            }
        };

        // Resolve thinking level
        let thinking_level = resolve_thinking_level(
            config.thinking_level,
            &settings,
            has_existing_session,
        );

        // Resolve tool configuration
        let default_active_tool_names = vec![
            "read".to_string(),
            "bash".to_string(),
            "edit".to_string(),
            "write".to_string(),
        ];

        let (active_tool_names, allowed_tool_names) = resolve_tool_config(
            &config,
            &default_active_tool_names,
        );

        // Create the provider
        let provider = create_provider_for_model(&model_id, &model_registry)?;

        // Build agent config
        let system_prompt = build_sdk_system_prompt(&settings, &thinking_level);
        let compaction_strategy = if settings.auto_compaction {
            oxi_ai::CompactionStrategy::Threshold(defaults::DEFAULT_COMPACTION_THRESHOLD)
        } else {
            oxi_ai::CompactionStrategy::Disabled
        };

        let agent_config = AgentConfig {
            name: "oxi-sdk".to_string(),
            description: Some("oxi SDK agent".to_string()),
            model_id: model_id.clone(),
            system_prompt: Some(system_prompt),
            max_iterations: defaults::DEFAULT_MAX_ITERATIONS as usize,
            timeout_seconds: settings.tool_timeout_seconds,
            temperature: settings.effective_temperature(),
            max_tokens: settings.effective_max_tokens(),
            compaction_strategy,
            compaction_instruction: None,
            context_window: defaults::DEFAULT_CONTEXT_WINDOW,
        };

        // Create the agent
        let agent = Arc::new(Agent::new(provider, agent_config));

        // Restore messages if session has existing data
        if has_existing_session {
            // In the TS version, agent.state.messages = existing_session.messages
            // For now we note this — actual message restoration would need
            // Agent::set_messages or similar
            tracing::debug!(
                "Restoring session with {} existing messages",
                existing_session.messages.len()
            );
        }

        let sdk = Sdk {
            agent,
            session_manager,
            settings,
            cwd,
            active_tool_names,
            allowed_tool_names,
            model_registry,
            resource_loader,
            scoped_models: config.scoped_models,
            session_id,
        };

        Ok(SdkCreateResult {
            sdk,
            model_fallback_message,
        })
    }

    // ── Prompt Execution ────────────────────────────────────────────

    /// Run a single prompt and return the response text.
    ///
    /// This is the simplest way to use the SDK — send a prompt, get a response.
    pub async fn run_prompt(&self, prompt: impl Into<String>) -> Result<String> {
        let (response, _events) = self.agent.run(prompt.into()).await?;
        Ok(response.content)
    }

    /// Run a prompt with a streaming event callback.
    ///
    /// The callback receives [`AgentEvent`]s as they arrive.
    /// Returns the final response text when complete.
    pub async fn run_prompt_streaming<F>(&self, prompt: impl Into<String, mut on_event: F) -> Result<String>
    where
        F: FnMut(AgentEvent) + Send + 'static,
    {
        self.agent.run_streaming(prompt.into(), on_event).await?;
        // Get last assistant message
        let state = self.agent.state();
        for msg in state.messages.iter().rev() {
            if let Message::Assistant(a) = msg {
                return Ok(a.text_content());
            }
        }
        Ok(String::new())
    }

    /// Run a prompt and collect all events into a channel.
    ///
    /// Returns the receiver end of the channel. Useful for UI integration
    /// where events need to be processed asynchronously.
    pub async fn run_prompt_channel(
        &self,
        prompt: impl Into<String>,
    ) -> Result<(mpsc::Receiver<AgentEvent>, String)> {
        let (tx, rx) = mpsc::channel(256);
        let prompt = prompt.into();

        let agent = Arc::clone(&self.agent);
        // Spawn the agent run on a separate task
        tokio::spawn(async move {
            let _ = agent.run_with_channel(prompt, tx).await;
        });

        // We can't know the final response here without collecting events,
        // so return empty string and let the caller collect from rx
        Ok((rx, String::new()))
    }

    // ── Tool Registration ───────────────────────────────────────────

    /// Register a custom tool with the agent.
    ///
    /// The tool must implement [`AgentTool`].
    pub fn register_tool(&self, tool: Arc<dyn AgentTool>) {
        self.agent.tools().register(tool);
    }

    /// Register a boxed tool (helper for ergonomic registration).
    pub fn register_boxed_tool(&self, tool: Box<dyn AgentTool>) {
        self.agent.tools().register(tool.into());
    }

    /// Get a reference to the tool registry for inspection or advanced registration.
    pub fn tool_registry(&self) -> Arc<ToolRegistry> {
        self.agent.tools()
    }

    /// Enable built-in tools by name.
    ///
    /// Valid names: `"read"`, `"bash"`, `"edit"`, `"write"`, `"grep"`, `"find"`, `"ls"`.
    pub fn enable_tools(&mut self, names: &[&str]) {
        for name in names {
            if !self.active_tool_names.contains(&name.to_string()) {
                self.active_tool_names.push(name.to_string());
            }
        }
    }

    /// Disable tools by name.
    pub fn disable_tools(&mut self, names: &[&str]) {
        let name_set: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        self.active_tool_names
            .retain(|n| !name_set.contains(n));
    }

    /// Get the list of currently active tool names.
    pub fn active_tools(&self) -> &[String] {
        &self.active_tool_names
    }

    // ── Model Management ────────────────────────────────────────────

    /// Switch the model used for future LLM calls.
    ///
    /// The model ID should be in `provider/model` format (e.g., `"anthropic/claude-sonnet-4-20250514"`).
    pub fn switch_model(&self, model_id: &str) -> Result<()> {
        self.agent.switch_model(model_id)
    }

    /// Get the current model ID.
    pub fn model_id(&self) -> String {
        self.agent.model_id()
    }

    /// Get the list of scoped models available for cycling.
    pub fn scoped_models(&self) -> &[ScopedModel] {
        &self.scoped_models
    }

    /// Set the scoped models for cycling.
    pub fn set_scoped_models(&mut self, models: Vec<ScopedModel>) {
        self.scoped_models = models;
    }

    // ── Session Management ──────────────────────────────────────────

    /// Get the session ID.
    pub fn session_id(&self) -> uuid::Uuid {
        self.session_id
    }

    /// Get a reference to the session manager.
    pub fn session_manager(&self) -> &SessionManager {
        &self.session_manager
    }

    /// Get a mutable reference to the session manager.
    pub fn session_manager_mut(&mut self) -> &mut SessionManager {
        &mut self.session_manager
    }

    /// Reset the conversation (clear messages but keep the agent).
    pub fn reset(&self) {
        self.agent.reset();
    }

    /// Get the working directory.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Get the settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Get a reference to the underlying agent.
    pub fn agent(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    /// Get the agent state (messages, model, etc.).
    pub fn agent_state(&self) -> oxi_agent::AgentState {
        self.agent.state()
    }

    /// Get the model registry.
    pub fn model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }

    /// Get the resource loader.
    pub fn resource_loader(&self) -> &DefaultResourceLoader {
        &self.resource_loader
    }

    // ── System Prompt ───────────────────────────────────────────────

    /// Update the system prompt.
    pub fn set_system_prompt(&self, prompt: impl Into<String>) {
        self.agent.set_system_prompt(prompt);
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Get the default agent directory (`~/.oxi`).
fn default_agent_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".oxi")
}

/// Resolve the model to use based on config, settings, and available models.
///
/// Returns `(Some(model_id), None)` on success, or `(None, Some(warning))` if
/// no model could be resolved.
fn resolve_model(
    config: &SdkConfig,
    settings: &Settings,
    model_registry: &ModelRegistry,
    has_existing_session: bool,
    existing_session: &crate::session::SessionContext,
) -> (Option<String>, Option<String>) {
    // 1. Explicit model from config
    if let Some(ref model) = config.model {
        return (Some(model.clone()), None);
    }

    // 2. Try to restore model from existing session
    if has_existing_session {
        if let Some(ref session_model) = existing_session.model {
            // Try to find this model in the registry
            let parts: Vec<&str> = session_model.split('/').collect();
            if parts.len() >= 2 {
                if let Some(_found) = model_registry.find_model(parts[0], parts[1..].join("/").as_str()) {
                    return (Some(session_model.clone()), None);
                }
            }
            // Model not available — fall through with a warning
            let warning = format!(
                "Could not restore model {}. Falling back to default.",
                session_model
            );
            // Continue to find an alternative
            let fallback = find_default_model(settings, model_registry);
            return match fallback {
                Some(id) => (Some(id), Some(warning)),
                None => (None, Some(warning)),
            };
        }
    }

    // 3. Use settings default or first available
    let fallback = find_default_model(settings, model_registry);
    match fallback {
        Some(id) => (Some(id), None),
        None => (None, Some("No models available. Please configure an API key.".to_string())),
    }
}

/// Find the default model from settings or registry.
fn find_default_model(settings: &Settings, model_registry: &ModelRegistry) -> Option<String> {
    // Try settings default first
    if let Some(ref model) = settings.default_model {
        return Some(model.clone());
    }

    // Try default provider
    if let Some(ref provider) = settings.default_provider {
        // Find first available model from this provider
        if let Some(model) = model_registry.find_first_for_provider(provider) {
            return Some(model);
        }
    }

    // Fall back to first available model in registry
    model_registry.find_first_available()
}

/// Resolve the thinking level.
fn resolve_thinking_level(
    config_level: Option<ThinkingLevel>,
    settings: &Settings,
    has_existing_session: bool,
) -> ThinkingLevel {
    config_level
        .unwrap_or(settings.thinking_level)
}

/// Resolve tool configuration from SDK config.
fn resolve_tool_config(
    config: &SdkConfig,
    default_tools: &[String],
) -> (Vec<String>, Option<Vec<String>>) {
    match (&config.tools, &config.no_tools) {
        (Some(tools), _) => {
            // Explicit allowlist
            (tools.clone(), Some(tools.clone()))
        }
        (None, Some(NoTools::All)) => {
            // No tools at all
            (Vec::new(), Some(Vec::new()))
        }
        (None, Some(NoTools::Builtin)) => {
            // No builtin tools, but custom tools allowed
            (Vec::new(), None)
        }
        (None, None) => {
            // Default: enable builtin tools
            (default_tools.to_vec(), None)
        }
    }
}

/// Build the system prompt for the SDK agent.
fn build_sdk_system_prompt(settings: &Settings, thinking_level: &ThinkingLevel) -> String {
    let mut prompt = match thinking_level {
        ThinkingLevel::None => {
            String::from("You are a helpful AI assistant. Provide direct, concise answers.")
        }
        ThinkingLevel::Minimal => {
            String::from("You are a helpful AI assistant. Provide clear and helpful answers.")
        }
        ThinkingLevel::Standard => String::from(
            "You are a helpful AI coding assistant. Think through problems \
             step by step when helpful, but keep responses focused and actionable.",
        ),
        ThinkingLevel::Thorough => String::from(
            "You are an expert AI coding assistant. Take time to thoroughly \
             analyze problems, consider edge cases, and provide comprehensive \
             solutions with explanations. Think deeply before responding.",
        ),
    };

    // Append theme hint if non-default
    if settings.theme != "default" && !settings.theme.is_empty() {
        prompt.push_str(&format!("\n\nTheme: {}", settings.theme));
    }

    prompt
}

/// Create a provider for the given model ID.
///
/// The model ID should be in `provider/model` format.
fn create_provider_for_model(model_id: &str, _model_registry: &ModelRegistry) -> Result<Arc<dyn Provider>> {
    let parts: Vec<&str> = model_id.split('/').collect();
    if parts.len() < 2 {
        anyhow::bail!(
            "Invalid model ID '{}'. Expected format: 'provider/model'",
            model_id
        );
    }

    let provider_name = parts[0];
    get_provider(provider_name)
        .map(Arc::from)
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", provider_name))
}

/// Get attribution headers for a given model and settings.
///
/// Returns extra HTTP headers to send with provider requests for analytics/tracking.
#[allow(dead_code)]
fn get_attribution_headers(model_id: &str, settings: &Settings) -> Option<Vec<(String, String)>> {
    // Only send attribution if telemetry is enabled
    // For now, we check if the provider is OpenRouter or Cloudflare
    let parts: Vec<&str> = model_id.split('/').collect();
    let provider = parts.first()?;

    match *provider {
        "openrouter" => Some(vec![
            ("HTTP-Referer".to_string(), "https://oxi.dev".to_string()),
            ("X-Title".to_string(), "oxi".to_string()),
            ("X-Categories".to_string(), "cli-agent".to_string()),
        ]),
        "cloudflare-workers-ai" | "cloudflare-ai-gateway" => {
            Some(vec![("User-Agent".to_string(), "oxi-coding-agent".to_string())])
        }
        _ => None,
    }
}

// =============================================================================
// Session Context (for model restoration)
// =============================================================================

/// Lightweight session context for model restoration during session resume.
///
/// This is a simplified version of the full session context, containing
/// just enough information to restore model and thinking level.
#[derive(Debug, Clone, Default)]
pub struct SessionContextInfo {
    /// Messages in the session.
    pub messages: Vec<crate::session::AgentMessage>,
    /// Model ID that was used when the session was last active.
    pub model: Option<String>,
    /// Thinking level that was active.
    pub thinking_level: Option<ThinkingLevel>,
}

// =============================================================================
// Factory Functions
// =============================================================================

/// Create a new SDK with minimal configuration.
///
/// Uses all defaults: current directory, settings from disk, first available model.
pub async fn create_sdk() -> Result<SdkCreateResult> {
    Sdk::new(SdkConfig::default()).await
}

/// Create a new SDK with a specific model.
///
/// # Arguments
/// * `model_id` - Model in `provider/model` format (e.g., `"anthropic/claude-sonnet-4-20250514"`)
pub async fn create_sdk_with_model(model_id: impl Into<String>) -> Result<SdkCreateResult> {
    let config = SdkConfig {
        model: Some(model_id.into()),
        ..Default::default()
    };
    Sdk::new(config).await
}

/// Create a new SDK with a specific working directory.
pub async fn create_sdk_with_cwd(cwd: impl Into<PathBuf>) -> Result<SdkCreateResult> {
    let config = SdkConfig {
        cwd: Some(cwd.into()),
        ..Default::default()
    };
    Sdk::new(config).await
}

/// Create a new SDK in read-only mode (no bash, edit, or write tools).
pub async fn create_readonly_sdk() -> Result<SdkCreateResult> {
    let config = SdkConfig {
        tools: Some(vec!["read".to_string(), "grep".to_string(), "find".to_string(), "ls".to_string()]),
        ..Default::default()
    };
    Sdk::new(config).await
}
