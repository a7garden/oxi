//! Agent session runtime — service container and runtime factory.
//!
//! Originally inspired by pi-mono's agent-session-runtime and agent-session-services.
//!
//! This module provides:
//!
//! - **`AgentSessionServices`**: A coherent set of cwd-bound runtime services
//!   (auth storage, settings, model registry, resource loader).
//! - **`AgentSessionRuntime`**: Owns the current `AgentSession` plus its services,
//!   and provides session lifecycle methods (`new_session`, `switch_session`,
//!   `fork`, `import_from_jsonl`, `dispose`).
//! - **`create_agent_session_services`**: Factory that builds services for a given cwd.
//! - **`create_agent_session_from_services`**: Factory that creates an `AgentSession`
//!   from pre-built services.
//!
//! # Architecture
//!
//! ```text
//!   CLI / interactive / print / rpc
//!            │
//!            ▼
//!   AgentSessionRuntime       ← this module
//!     ├─ AgentSessionServices  (cwd-bound infra)
//!     └─ AgentSession          (session wrapper around Agent)
//!            │
//!            ▼
//!   oxi_agent::Agent
//!            │
//!            ▼
//!   oxi_ai::Provider
//! ```

use crate::agent_session::{AgentSession, AgentSessionHandle, ScopedModel};
use oxi_store::auth_storage::AuthStorage;
use oxi_store::model_registry::ModelRegistry;
use crate::resource_loader::ResourceLoader;
use oxi_store::session::SessionManager;
use oxi_store::session_cwd::{assert_session_cwd_exists, SessionCwdSource};
use oxi_store::settings::{Settings, ThinkingLevel};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ═══════════════════════════════════════════════════════════════════════════
// Diagnostics
// ═══════════════════════════════════════════════════════════════════════════

/// Non-fatal issues collected while creating services or sessions.
///
/// Runtime creation returns diagnostics to the caller instead of printing
/// or exiting. The app layer decides whether warnings should be shown and
/// whether errors should abort startup.
#[derive(Debug, Clone)]
pub struct AgentSessionRuntimeDiagnostic {
/// pub.
    pub severity: DiagnosticSeverity,
/// pub.
    pub message: String,
}

/// Severity level for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
/// info variant.
    Info,
/// warning variant.
    Warning,
/// error variant.
    Error,
}

impl std::fmt::Display for DiagnosticSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticSeverity::Info => write!(f, "info"),
            DiagnosticSeverity::Warning => write!(f, "warning"),
            DiagnosticSeverity::Error => write!(f, "error"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Session services
// ═══════════════════════════════════════════════════════════════════════════

/// Coherent cwd-bound runtime services for one effective session cwd.
///
/// This is infrastructure only. The `AgentSession` itself is created
/// separately so session options can be resolved against these services first.
pub struct AgentSessionServices {
    /// Current working directory (may change on session switch).
    pub cwd: PathBuf,
    /// Agent data directory (typically `~/.oxi`).
    pub agent_dir: PathBuf,
    /// Auth storage for API keys / OAuth tokens.
    pub auth_storage: Arc<AuthStorage>,
    /// Settings (layered configuration).
    pub settings: Arc<Settings>,
    /// Model registry (built-in + custom models).
    pub model_registry: Arc<ModelRegistry>,
    /// Resource loader (skills, extensions, themes, context files).
    pub resource_loader: Arc<ResourceLoader>,
    /// Diagnostics collected during service creation.
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

/// Options for creating cwd-bound runtime services.
pub struct CreateAgentSessionServicesOptions {
    /// Working directory for the session.
    pub cwd: PathBuf,
    /// Override agent data directory (default: `~/.oxi`).
    pub agent_dir: Option<PathBuf>,
    /// Override auth storage (default: auto-detected from agent_dir).
    pub auth_storage: Option<Arc<AuthStorage>>,
    /// Override settings (default: loaded from cwd + agent_dir).
    pub settings: Option<Arc<Settings>>,
    /// Override model registry (default: `agent_dir/models.json`).
    pub model_registry: Option<Arc<ModelRegistry>>,
    /// Override resource loader (default: created from cwd + agent_dir).
    pub resource_loader: Option<Arc<ResourceLoader>>,
}

impl CreateAgentSessionServicesOptions {
    /// Create options with minimal required fields.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            agent_dir: None,
            auth_storage: None,
            settings: None,
            model_registry: None,
            resource_loader: None,
        }
    }
}

/// Get the default agent directory (`~/.oxi`).
pub fn get_default_agent_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".oxi"))
        .unwrap_or_else(|| PathBuf::from(".oxi"))
}

/// Create cwd-bound runtime services.
///
/// Returns services plus diagnostics. It does **not** create an `AgentSession`.
pub fn create_agent_session_services(
    options: CreateAgentSessionServicesOptions,
) -> Result<AgentSessionServices> {
    let cwd = options.cwd;
    let agent_dir = options.agent_dir.unwrap_or_else(get_default_agent_dir);

    // Auth storage — both the service-level handle and the model registry
    // need auth access. Since AuthStorage is not Clone, we create two
    // instances (they both read from the same underlying file).
    let auth_storage = options
        .auth_storage
        .unwrap_or_else(|| Arc::new(AuthStorage::new()));

    // Settings — load from cwd + agent_dir
    let settings = options.settings.unwrap_or_else(|| {
        let s = Settings::load_from(&cwd).unwrap_or_default();
        Arc::new(s)
    });

    // Model registry — creates its own AuthStorage internally
    // (reads from the same default path)
    let model_registry = options.model_registry.unwrap_or_else(|| {
        Arc::new(ModelRegistry::create(
            AuthStorage::new(),
            Some(agent_dir.join("models.json")),
        ))
    });

    // Resource loader
    let resource_loader = options.resource_loader.unwrap_or_else(|| {
        Arc::new(ResourceLoader::with_paths(agent_dir.clone(), cwd.clone()))
    });

    Ok(AgentSessionServices {
        cwd,
        agent_dir,
        auth_storage,
        settings,
        model_registry,
        resource_loader,
        diagnostics: Vec::new(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Create session from services
// ═══════════════════════════════════════════════════════════════════════════

/// Options for creating an `AgentSession` from already-created services.
pub struct CreateAgentSessionFromServicesOptions {
    /// Pre-built services.
    pub services: Arc<AgentSessionServices>,
    /// Session manager (handles persistence).
    pub session_manager: SessionManager,
    /// Override model (e.g. from CLI `--model`).
    pub model_id: Option<String>,
    /// Override thinking level.
    pub thinking_level: Option<ThinkingLevel>,
    /// Scoped models for Ctrl+P cycling.
    pub scoped_models: Vec<ScopedModel>,
    /// Pre-configured tool registry to copy into the new agent.
    /// If None, builtin tools are registered automatically.
    pub tool_registry: Option<Arc<oxi_agent::ToolRegistry>>,
}

/// Result of creating an agent session.
pub struct CreateAgentSessionResult {
/// pub.
    pub session: AgentSession,
/// pub.
    pub model_fallback_message: Option<String>,
}

/// Create an `AgentSession` from previously created services.
///
/// This keeps session creation separate from service creation so callers
/// can resolve model, thinking, tools, and other session inputs against
/// the target cwd before constructing the session.
pub fn create_agent_session_from_services(
    options: CreateAgentSessionFromServicesOptions,
) -> Result<CreateAgentSessionResult> {
    let services = &options.services;
    let settings = services.settings.as_ref();
    let cwd = services.cwd.to_string_lossy().to_string();

    // Resolve model — no hardcoded default, must be configured
    let model_id = match options
        .model_id
        .or_else(|| settings.effective_model(None))
    {
        Some(id) if !id.is_empty() => {
            tracing::debug!("Model resolved: {} (default_model={:?}, last_used={:?})", id, settings.default_model, settings.last_used_model);
            id
        }
        other => {
            tracing::warn!("No model configured: effective_model={:?}", settings.effective_model(None));
            match other {
                Some(id) => id,
                None => String::new(),
            }
        }
    };

    // Resolve thinking level
    let thinking_level = options.thinking_level.unwrap_or(settings.thinking_level);

    // Get provider and model
    if model_id.is_empty() {
        // No model — return minimal session, TUI setup wizard will handle configuration
        let config = oxi_agent::AgentConfig {
            name: "oxi".to_string(),
            description: Some("oxi CLI agent".to_string()),
            model_id: String::new(),
            system_prompt: Some(build_system_prompt(thinking_level)),
            max_iterations: 10,
            timeout_seconds: settings.tool_timeout_seconds,
            temperature: settings.effective_temperature(),
            max_tokens: settings.effective_max_tokens(),
            compaction_strategy: if settings.auto_compaction {
                oxi_ai::CompactionStrategy::Threshold(0.8)
            } else {
                oxi_ai::CompactionStrategy::Disabled
            },
            compaction_instruction: None,
            context_window: 128_000,
            api_key: None,
        };
        // Use anthropic as a placeholder provider so the session can be created
        let provider = oxi_ai::get_provider("anthropic")
            .ok_or_else(|| anyhow::anyhow!("No provider available"))?;
        let agent = Arc::new(oxi_agent::Agent::new(Arc::from(provider), config));
        let session = AgentSession::new(agent, settings.clone(), options.session_manager, cwd);
        return Ok(CreateAgentSessionResult {
            session,
            model_fallback_message: None,
        });
    }

    let (provider_name, _model_name) = parse_model_id(&model_id);

    let provider = oxi_ai::get_provider(&provider_name)
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", provider_name))?;

    // Build agent config
    let system_prompt = build_system_prompt(thinking_level);
    let compaction_strategy = if settings.auto_compaction {
        oxi_ai::CompactionStrategy::Threshold(0.8)
    } else {
        oxi_ai::CompactionStrategy::Disabled
    };

    // Resolve API key from auth storage for the provider
    let api_key = services.auth_storage.get_api_key(&provider_name);

    let config = oxi_agent::AgentConfig {
        name: "oxi".to_string(),
        description: Some("oxi CLI agent".to_string()),
        model_id: model_id.clone(),
        system_prompt: Some(system_prompt),
        max_iterations: 10,
        timeout_seconds: settings.tool_timeout_seconds,
        temperature: settings.effective_temperature(),
        max_tokens: settings.effective_max_tokens(),
        compaction_strategy,
        compaction_instruction: None,
        context_window: 128_000,
        api_key,
    };

    let agent = Arc::new(oxi_agent::Agent::new(Arc::from(provider), config));

    // Register tools: use provided registry or fallback to builtins
    let registry = options.tool_registry.unwrap_or_else(|| {
        Arc::new(oxi_agent::ToolRegistry::with_builtins_cwd(
            PathBuf::from(&cwd),
            &services.settings.disabled_tools,
        ))
    });
    let agent_tools = agent.tools();
    for name in registry.names() {
        if let Some(tool) = registry.get(&name) {
            agent_tools.register_arc(tool);
        }
    }

    // Create the session
    let session = AgentSession::new(agent, settings.clone(), options.session_manager, cwd);

    // Set scoped models if provided
    if !options.scoped_models.is_empty() {
        session.set_scoped_models(options.scoped_models);
    }

    Ok(CreateAgentSessionResult {
        session,
        model_fallback_message: None,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Runtime factory
// ═══════════════════════════════════════════════════════════════════════════

/// Result returned by runtime creation.
pub struct CreateAgentSessionRuntimeResult {
/// pub.
    pub session: AgentSession,
/// pub.
    pub services: Arc<AgentSessionServices>,
/// pub.
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
/// pub.
    pub model_fallback_message: Option<String>,
}

/// Factory closure type that creates a runtime for a given cwd + session manager.
pub type CreateRuntimeFactory =
    dyn Fn(CreateRuntimeOptions) -> Result<CreateAgentSessionRuntimeResult> + Send + Sync;

/// Options passed to the runtime factory.
pub struct CreateRuntimeOptions {
/// pub.
    pub cwd: PathBuf,
/// pub.
    pub agent_dir: PathBuf,
/// pub.
    pub session_manager: SessionManager,
}

// ═══════════════════════════════════════════════════════════════════════════
// Session switch reason (for extension hooks / diagnostics)
// ═══════════════════════════════════════════════════════════════════════════

/// Why a session was replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSwitchReason {
    /// Fresh session created via /new.
    New,
    /// Switched to an existing session via /resume or /switch.
    Resume,
    /// Forked from an existing conversation.
    Fork,
    /// Imported from an external JSONL file.
    Import,
    /// Application is shutting down.
    Quit,
}

// ═══════════════════════════════════════════════════════════════════════════
// Import error
// ═══════════════════════════════════════════════════════════════════════════

/// Error when `/import` references a JSONL file that does not exist.
#[derive(Debug)]
pub struct SessionImportFileNotFoundError {
/// pub.
    pub file_path: PathBuf,
}

impl std::fmt::Display for SessionImportFileNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "File not found: {}", self.file_path.display())
    }
}

impl std::error::Error for SessionImportFileNotFoundError {}

// ═══════════════════════════════════════════════════════════════════════════
// AgentSessionRuntime
// ═══════════════════════════════════════════════════════════════════════════

/// Owns the current `AgentSession` plus its cwd-bound services.
///
/// Session replacement methods tear down the current runtime first, then
/// create and apply the next runtime. If creation fails, the error is
/// propagated to the caller.
pub struct AgentSessionRuntime {
    session: AgentSessionHandle,
    services: Arc<AgentSessionServices>,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
    create_runtime: Arc<CreateRuntimeFactory>,
}

impl AgentSessionRuntime {
    /// Create a new runtime wrapper.
    ///
    /// The same factory is stored and reused for later `/new`, `/resume`,
    /// `/fork`, and import flows.
    pub fn new(
        session: AgentSession,
        services: Arc<AgentSessionServices>,
        create_runtime: Arc<CreateRuntimeFactory>,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
        model_fallback_message: Option<String>,
    ) -> Self {
        Self {
            session: session.clone_handle(),
            services,
            diagnostics,
            model_fallback_message,
            create_runtime,
        }
    }

    // ── Accessors ────────────────────────────────────────────────────

    /// Current services.
    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    /// Current session handle.
    pub fn session(&self) -> &AgentSessionHandle {
        &self.session
    }

    /// Current working directory.
    pub fn cwd(&self) -> &Path {
        &self.services.cwd
    }

    /// Diagnostics from the last runtime creation.
    pub fn diagnostics(&self) -> &[AgentSessionRuntimeDiagnostic] {
        &self.diagnostics
    }

    /// Model fallback message (set when the requested model was unavailable).
    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }

    // ── Session lifecycle ────────────────────────────────────────────

    /// Switch to an existing session file.
    ///
    /// Validates that the session's cwd matches (or can be overridden),
    /// tears down the current session, and creates a new runtime.
    pub fn switch_session(
        &mut self,
        session_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<()> {
        // Open the target session
        let session_manager = SessionManager::open(session_path, None, cwd_override);

        // Validate CWD
        let cwd = session_manager.get_cwd();
        let adapter = SessionManagerCwdAdapter(&session_manager);
        assert_session_cwd_exists(&adapter, &cwd).map_err(|e| anyhow::anyhow!("{}", e))?;

        self.teardown_current(SessionSwitchReason::Resume);

        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: PathBuf::from(&cwd),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })?;

        self.apply(result);
        Ok(())
    }

    /// Create a new empty session.
    pub fn new_session(&mut self) -> Result<()> {
        let session_dir = get_default_session_dir();
        let session_manager = SessionManager::create(
            &self.services.cwd.to_string_lossy(),
            Some(&session_dir),
        );

        self.teardown_current(SessionSwitchReason::New);

        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: self.services.cwd.clone(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })?;

        self.apply(result);
        Ok(())
    }

    /// Fork from a specific entry.
    ///
    /// Creates a new session that branches from the given entry.
    /// Uses `SessionManager::fork_from` to create the forked session file,
    /// then branches to the specified entry within it.
    pub fn fork(&mut self, entry_id: &str, _position: ForkPosition) -> Result<()> {
        let session_dir = get_default_session_dir();
        let cwd_str = self.services.cwd.to_string_lossy().to_string();

        // Create a forked session from the current session file
        let mut session_manager = {
            // For an in-memory session, just create a new one
            let sm = SessionManager::create(&cwd_str, Some(&session_dir));
            sm
        };

        // Branch to the specified entry within the new session
        if let Err(e) = session_manager.branch(entry_id) {
            tracing::warn!("Branch to entry {} failed: {}", entry_id, e);
        }

        self.teardown_current(SessionSwitchReason::Fork);

        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: self.services.cwd.clone(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })?;

        self.apply(result);
        Ok(())
    }

    /// Import a session JSONL file and switch runtime state.
    ///
    /// # Errors
    ///
    /// Returns [`SessionImportFileNotFoundError`] when the input path does not
    /// exist. Returns other errors when the imported session's CWD cannot be
    /// resolved.
    pub fn import_from_jsonl(
        &mut self,
        input_path: &Path,
        cwd_override: Option<&str>,
    ) -> Result<()> {
        let resolved = if input_path.is_absolute() {
            input_path.to_path_buf()
        } else {
            std::env::current_dir()?.join(input_path)
        };

        if !resolved.exists() {
            return Err(SessionImportFileNotFoundError {
                file_path: resolved,
            }
            .into());
        }

        // Copy to session dir if needed
        let session_dir = get_default_session_dir();
        let dest_dir = Path::new(&session_dir);
        if !dest_dir.exists() {
            std::fs::create_dir_all(dest_dir)?;
        }

        let file_name = resolved.file_name().unwrap_or_default();
        let destination = dest_dir.join(file_name);

        if destination != resolved {
            std::fs::copy(&resolved, &destination)?;
        }

        let session_manager = SessionManager::open(
            &destination.to_string_lossy(),
            Some(&session_dir),
            cwd_override,
        );

        let cwd = session_manager.get_cwd();
        let adapter = SessionManagerCwdAdapter(&session_manager);
        assert_session_cwd_exists(&adapter, &cwd).map_err(|e| anyhow::anyhow!("{}", e))?;

        self.teardown_current(SessionSwitchReason::Import);

        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: PathBuf::from(&cwd),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })?;

        self.apply(result);
        Ok(())
    }

    /// Shut down the runtime gracefully.
    pub fn dispose(&mut self) {
        self.teardown_current(SessionSwitchReason::Quit);
    }

    // ── Internal ─────────────────────────────────────────────────────

    /// Teardown the current session.
    fn teardown_current(&mut self, _reason: SessionSwitchReason) {
        self.session.reset();
    }

    /// Apply a new runtime result.
    fn apply(&mut self, result: CreateAgentSessionRuntimeResult) {
        self.session = result.session.clone_handle();
        self.services = result.services;
        self.diagnostics = result.diagnostics;
        self.model_fallback_message = result.model_fallback_message;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Fork position
// ═══════════════════════════════════════════════════════════════════════════

/// Where to fork relative to a session entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkPosition {
    /// Fork at the specified entry (includes it).
    At,
    /// Fork before the specified entry (excludes it).
    Before,
}

// ═══════════════════════════════════════════════════════════════════════════
// Session CWD adapter for SessionManager
// ═══════════════════════════════════════════════════════════════════════════

/// Adapter to use `SessionManager` with `assert_session_cwd_exists`.
struct SessionManagerCwdAdapter<'a>(&'a SessionManager);

impl SessionCwdSource for SessionManagerCwdAdapter<'_> {
    fn get_cwd(&self) -> Option<String> {
        let cwd = self.0.get_cwd();
        if cwd.is_empty() {
            None
        } else {
            Some(cwd)
        }
    }

    fn get_session_file(&self) -> Option<String> {
        self.0.get_session_file()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Top-level factory
// ═══════════════════════════════════════════════════════════════════════════

/// Create the initial runtime from a runtime factory and initial session target.
///
/// The same factory is stored on the returned `AgentSessionRuntime` and reused
/// for later `/new`, `/resume`, `/fork`, and import flows.
pub fn create_agent_session_runtime(
    create_runtime: Arc<CreateRuntimeFactory>,
    options: CreateRuntimeOptions,
) -> Result<AgentSessionRuntime> {
    // Validate CWD
    let adapter = SessionManagerCwdAdapter(&options.session_manager);
    let cwd = options.session_manager.get_cwd();
    assert_session_cwd_exists(&adapter, &cwd).map_err(|e| anyhow::anyhow!("{}", e))?;

    let result = create_runtime(options)?;

    Ok(AgentSessionRuntime::new(
        result.session,
        result.services,
        create_runtime,
        result.diagnostics,
        result.model_fallback_message,
    ))
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Parse a model ID string like `"anthropic/claude-sonnet-4-20250514"` into
/// `(provider, model)` parts.
fn parse_model_id(model_id: &str) -> (String, String) {
    let parts: Vec<&str> = model_id.splitn(2, '/').collect();
    if parts.len() >= 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("anthropic".to_string(), model_id.to_string())
    }
}

/// Build the system prompt based on thinking level.
///
/// Delegates to [`crate::system_prompt::build_system_prompt`].
fn build_system_prompt(thinking_level: ThinkingLevel) -> String {
    let custom_prompt = match thinking_level {
        ThinkingLevel::None => {
            Some("You are a helpful AI assistant. Provide direct, concise answers.".to_string())
        }
        ThinkingLevel::Minimal => {
            Some("You are a helpful AI assistant. Provide clear and helpful answers.".to_string())
        }
        ThinkingLevel::Standard => Some(
            "You are a helpful AI coding assistant. Think through problems \
             step by step when helpful, but keep responses focused and actionable."
                .to_string(),
        ),
        ThinkingLevel::Thorough => Some(
            "You are an expert AI coding assistant. Take time to thoroughly \
             analyze problems, consider edge cases, and provide comprehensive \
             solutions with explanations. Think deeply before responding."
                .to_string(),
        ),
    };

    let mut tool_snippets = std::collections::HashMap::new();
    tool_snippets.insert("read".into(), "Read file contents (text or image)".into());
    tool_snippets.insert("bash".into(), "Execute bash commands".into());
    tool_snippets.insert("edit".into(), "Edit files with exact text replacement".into());
    tool_snippets.insert("write".into(), "Write content to files".into());
    tool_snippets.insert("grep".into(), "Search file contents with regex".into());
    tool_snippets.insert("find".into(), "Find files by name/pattern".into());
    tool_snippets.insert("ls".into(), "List directory contents".into());
    tool_snippets.insert("web_search".into(), "Search the web (DuckDuckGo, Wikipedia, Bing, Brave)".into());

    let options = crate::system_prompt::BuildSystemPromptOptions {
        custom_prompt,
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        selected_tools: vec![
            "read".into(), "bash".into(), "edit".into(), "write".into(),
            "grep".into(), "find".into(), "ls".into(), "web_search".into(),
        ],
        tool_snippets,
        ..Default::default()
    };

    crate::system_prompt::build_system_prompt(&options)
}

/// Get the default sessions directory.
fn get_default_session_dir() -> String {
    format!(
        "{}/sessions",
        get_default_agent_dir().to_string_lossy()
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Default runtime factory
// ═══════════════════════════════════════════════════════════════════════════

/// Build a default runtime factory that creates services and sessions
/// using standard defaults.
///
/// This is the main entry point for callers that don't need custom
/// service injection.
pub fn default_create_runtime_factory() -> Arc<CreateRuntimeFactory> {
    Arc::new(|options: CreateRuntimeOptions| {
        // Create services for the target cwd
        let services = create_agent_session_services(
            CreateAgentSessionServicesOptions {
                cwd: options.cwd.clone(),
                agent_dir: Some(options.agent_dir.clone()),
                auth_storage: None,
                settings: None,
                model_registry: None,
                resource_loader: None,
            },
        )?;
        let services = Arc::new(services);

        let result = create_agent_session_from_services(
            CreateAgentSessionFromServicesOptions {
                services: services.clone(),
                session_manager: options.session_manager,
                model_id: None,
                thinking_level: None,
                scoped_models: Vec::new(),
                tool_registry: None,
            },
        )?;

        Ok(CreateAgentSessionRuntimeResult {
            session: result.session,
            services,
            diagnostics: Vec::new(),
            model_fallback_message: result.model_fallback_message,
        })
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_id_with_provider() {
        let (provider, model) = parse_model_id("anthropic/claude-sonnet-4-20250514");
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_parse_model_id_without_provider() {
        let (provider, model) = parse_model_id("claude-sonnet-4-20250514");
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_parse_model_id_nested() {
        let (provider, model) = parse_model_id("openai/gpt-4o");
        assert_eq!(provider, "openai");
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt(ThinkingLevel::None);
        assert!(prompt.contains("concise"));

        let prompt = build_system_prompt(ThinkingLevel::Standard);
        assert!(prompt.contains("coding"));

        let prompt = build_system_prompt(ThinkingLevel::Thorough);
        assert!(prompt.contains("comprehensive"));
    }

    #[test]
    fn test_diagnostic_severity_display() {
        assert_eq!(format!("{}", DiagnosticSeverity::Info), "info");
        assert_eq!(format!("{}", DiagnosticSeverity::Warning), "warning");
        assert_eq!(format!("{}", DiagnosticSeverity::Error), "error");
    }

    #[test]
    fn test_session_import_file_not_found_error() {
        let err = SessionImportFileNotFoundError {
            file_path: PathBuf::from("/tmp/nonexistent.jsonl"),
        };
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_get_default_agent_dir() {
        let dir = get_default_agent_dir();
        assert!(dir.to_string_lossy().contains(".oxi"));
    }

    #[test]
    fn test_fork_position() {
        assert_eq!(ForkPosition::At, ForkPosition::At);
        assert_ne!(ForkPosition::At, ForkPosition::Before);
    }

    #[test]
    fn test_session_switch_reason() {
        assert_eq!(SessionSwitchReason::New, SessionSwitchReason::New);
        assert_ne!(SessionSwitchReason::New, SessionSwitchReason::Resume);
        assert_ne!(SessionSwitchReason::Fork, SessionSwitchReason::Import);
        assert_ne!(SessionSwitchReason::Import, SessionSwitchReason::Quit);
    }

    #[test]
    fn test_create_agent_session_services_options() {
        let opts = CreateAgentSessionServicesOptions::new(PathBuf::from("/tmp"));
        assert_eq!(opts.cwd, PathBuf::from("/tmp"));
        assert!(opts.agent_dir.is_none());
        assert!(opts.auth_storage.is_none());
    }
}
