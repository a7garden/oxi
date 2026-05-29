#![warn(missing_docs)]
#![warn(clippy::unwrap_used)]
#![allow(unknown_lints)]

//! oxi: CLI coding harness
//!
//! This crate provides the main application logic for the oxi CLI.

// ─── Root-level entry modules ───────────────────────────────────────────────
// cli must be pub for main.rs binary
pub mod cli;
pub mod print_mode;
pub mod setup_wizard;

// ─── Directory groups ───────────────────────────────────────────────────────
pub(crate) mod app;
pub(crate) mod context;
pub mod extensions; // public for main.rs
pub(crate) mod infra;
pub(crate) mod media;
pub(crate) mod prompt;
pub(crate) mod rpc_mode;
pub(crate) mod skills;
pub mod storage; // public for main.rs (packages)
                 // Re-exports from storage for main.rs
pub use storage::packages::PackageManager;
pub use storage::packages::ResourceKind;
pub mod tui; // public for main.rs
pub(crate) mod ui;
pub mod updater;
pub(crate) mod util;

// ─── oxi-store re-exports (shared persistent state) ─────────────────────────
pub use oxi_store::{
    auth_guidance, auth_storage, model_registry, model_resolver, session, session_cwd,
    session_navigation, settings, settings_validation, AgentMessage, AssistantContentBlock,
    AuthStorage, ContentBlock, ContentValue, ModelRegistry, SessionEntry, SessionManager,
    SessionTreeNode, Settings, ValidationReport,
};

/// Context for compaction operations, passed to extension hooks
#[derive(Debug, Clone)]
pub struct CompactionContext {
    /// Messages being compacted
    pub messages_count: usize,
    /// Estimated tokens before compaction
    pub tokens_before: usize,
    /// Target token count after compaction
    pub target_tokens: usize,
    /// Strategy being used
    pub strategy: String,
}

impl CompactionContext {
    /// Create a new compaction context
    pub fn new(
        messages_count: usize,
        tokens_before: usize,
        target_tokens: usize,
        strategy: impl Into<String>,
    ) -> Self {
        Self {
            messages_count,
            tokens_before,
            target_tokens,
            strategy: strategy.into(),
        }
    }

    /// Get expected compression ratio
    pub fn compression_ratio(&self) -> f32 {
        if self.tokens_before == 0 {
            return 1.0;
        }
        self.target_tokens as f32 / self.tokens_before as f32
    }
}

// ─── Module-level imports ────────────────────────────────────────────────────
use anyhow::{Error, Result};
use oxi_agent::{Agent, AgentConfig, AgentEvent};
use oxi_sdk::OxiBuilder;
use parking_lot::RwLock;
use skills::SkillManager;
use std::sync::Arc;
use uuid::Uuid;

// ─── Application state ───────────────────────────────────────────────────────

/// Application state and entry point
pub struct App {
    /// SDK engine for provider/model resolution
    #[allow(dead_code)]
    engine: oxi_sdk::Oxi,
    agent: Arc<Agent>,
    settings: Settings,
    skills: RwLock<SkillManager>,
    active_skills: RwLock<Vec<String>>,
    wasm_ext: Option<std::sync::Arc<crate::extensions::WasmExtensionManager>>,
    questionnaire_bridge:
        Option<std::sync::Arc<oxi_agent::tools::questionnaire::QuestionnaireBridge>>,
}

/// Chat message for display
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    /// Role of the message sender (e.g. "user" or "assistant").
    pub role: String,
    /// Text content of the message.
    pub content: String,
    /// Timestamp when the message was created.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ChatMessage {
    /// Create a new user chat message.
    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Create a new assistant chat message.
    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Interactive session state
#[derive(Debug, Clone, Default)]
pub struct InteractiveSession {
    /// Chat messages exchanged so far.
    pub messages: Vec<ChatMessage>,
    /// Whether the assistant is currently generating a response.
    pub thinking: bool,
    /// Partial response text accumulated during streaming.
    pub current_response: String,
    /// Unique session identifier.
    pub session_id: Option<Uuid>,
    /// Optional human-readable session name.
    pub name: Option<String>,
    /// Raw session entries for persistence and tree navigation.
    pub entries: Vec<SessionEntry>,
}

impl InteractiveSession {
    /// Create a new empty interactive session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a user message to the session.
    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(ChatMessage::user(content.clone()));
        let entry = SessionEntry::new(AgentMessage::User {
            content: ContentValue::String(content),
        });
        self.entries.push(entry);
    }

    /// Add an assistant message to the session.
    pub fn add_assistant_message(&mut self, content: String) {
        self.messages.push(ChatMessage::assistant(content.clone()));
        let entry = SessionEntry::new(AgentMessage::Assistant {
            content: vec![AssistantContentBlock::Text { text: content }],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        });
        self.entries.push(entry);
        self.current_response.clear();
    }

    /// Append text to the current partial streaming response.
    pub fn append_to_response(&mut self, text: &str) {
        self.current_response.push_str(text);
    }

    /// Finalize the current streaming response into a full assistant message.
    pub fn finish_response(&mut self) {
        if !self.current_response.is_empty() {
            let response = std::mem::take(&mut self.current_response);
            self.add_assistant_message(response);
        }
    }

    /// Get all entries in the session
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Get entry at a specific index
    pub fn get_entry(&self, index: usize) -> Option<&SessionEntry> {
        self.entries.get(index)
    }

    /// Get entry by ID
    pub fn get_entry_by_id(&self, id: &str) -> Option<&SessionEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Truncate entries at a given index (for branching)
    pub fn truncate_at(&mut self, index: usize) {
        self.entries.truncate(index + 1);
    }
}

// ─── System prompt builder ───────────────────────────────────────────────────

// TODO: This build_system_prompt duplicates the one in
// app/agent_session_runtime.rs. Both delegate to prompt::system_prompt::build_system_prompt
// but with different options (this one passes skills; the other passes tool_snippets).
// Unify into a single shared utility that accepts all options.
fn build_system_prompt(
    thinking_level: oxi_store::settings::ThinkingLevel,
    skill_contents: &[String],
) -> String {
    let custom_prompt = match thinking_level {
        oxi_store::settings::ThinkingLevel::Off => Some(String::from(
            "You are a helpful AI assistant. Provide direct, concise answers.",
        )),
        oxi_store::settings::ThinkingLevel::Minimal => Some(String::from(
            "You are a helpful AI assistant. Provide clear and helpful answers.",
        )),
        oxi_store::settings::ThinkingLevel::Low => Some(String::from(
            "You are a helpful AI assistant. Provide brief, actionable responses.",
        )),
        oxi_store::settings::ThinkingLevel::Medium => Some(String::from(
            "You are a helpful AI coding assistant. Think through problems \
             step by step when helpful, but keep responses focused and actionable.",
        )),
        oxi_store::settings::ThinkingLevel::High => Some(String::from(
            "You are an expert AI coding assistant. Take time to thoroughly \
             analyze problems, consider edge cases, and provide comprehensive \
             solutions with explanations. Think deeply before responding.",
        )),
        oxi_store::settings::ThinkingLevel::XHigh => Some(String::from(
            "You are an expert AI coding assistant. Use maximum reasoning depth. \
             Consider all alternatives, edge cases, and potential implications. \
             Provide the most thorough, comprehensive analysis possible.",
        )),
    };

    let skills: Vec<prompt::system_prompt::Skill> = skill_contents
        .iter()
        .enumerate()
        .map(|(i, content)| prompt::system_prompt::Skill {
            name: format!("skill-{}", i),
            content: content.clone(),
        })
        .collect();

    let options = prompt::system_prompt::BuildSystemPromptOptions {
        custom_prompt,
        skills,
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        ..Default::default()
    };

    prompt::system_prompt::build_system_prompt(&options)
}

// ─── App implementation ─────────────────────────────────────────────────────

impl App {
    /// Create a new App instance
    pub async fn new(settings: Settings) -> Result<Self> {
        let model_id = settings.effective_model(None).unwrap_or_default();
        let provider_name = settings
            .effective_provider(None)
            .unwrap_or_else(|| model_id.split('/').next().unwrap_or("").to_string());

        let (provider_name, model_name) = if model_id.contains('/') {
            let parts: Vec<&str> = model_id.split('/').collect();
            (parts[0].to_string(), parts[1..].join("/"))
        } else if !model_id.is_empty() {
            (provider_name.clone(), model_id.clone())
        } else {
            (String::new(), String::new())
        };

        // Build SDK engine with built-in providers and models
        let engine = OxiBuilder::new().with_builtins().build();

        // Resolve model via SDK (validation only)
        if !provider_name.is_empty() && !model_name.is_empty() {
            let _ = engine.resolve_model(&format!("{}/{}", provider_name, model_name));
        }

        // Resolve provider via SDK
        let provider: Arc<dyn oxi_ai::Provider> = if !provider_name.is_empty() {
            engine
                .create_provider(&provider_name)
                .map_err(|e| Error::msg(format!("{}", e)))?
        } else {
            engine
                .create_provider("anthropic")
                .map_err(|e| Error::msg(format!("{}", e)))?
        };

        let skills_dir = SkillManager::skills_dir().unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".oxi")
                .join("skills")
        });
        let skills = SkillManager::load_from_dir(&skills_dir).unwrap_or_else(|e| {
            tracing::debug!("Skills not loaded: {}", e);
            SkillManager::new()
        });

        let system_prompt = build_system_prompt(settings.thinking_level, &[]);
        let compaction_strategy = if settings.auto_compaction {
            oxi_ai::CompactionStrategy::Threshold(0.8)
        } else {
            oxi_ai::CompactionStrategy::Disabled
        };
        let auth = oxi_store::auth_storage::shared_auth_storage();
        let api_key = auth.get_api_key(&provider_name);

        let config = AgentConfig {
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
            workspace_dir: Some(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            ),
            output_mode: None,
            provider_options: None,
        };

        let agent = Arc::new(Agent::new(
            provider,
            config,
            Arc::new(oxi_agent::ToolRegistry::new()),
        ));

        let bridge =
            std::sync::Arc::new(oxi_agent::tools::questionnaire::QuestionnaireBridge::new());
        let questionnaire_tool =
            oxi_agent::tools::questionnaire::QuestionnaireTool::new(bridge.clone());
        agent
            .tools()
            .register_arc(std::sync::Arc::new(questionnaire_tool));

        Ok(Self {
            engine,
            agent,
            settings,
            skills: RwLock::new(skills),
            active_skills: RwLock::new(Vec::new()),
            wasm_ext: None,
            questionnaire_bridge: Some(bridge),
        })
    }

    /// Access the SDK engine (for provider/model resolution)
    #[allow(dead_code)]
    pub(crate) fn engine(&self) -> &oxi_sdk::Oxi {
        &self.engine
    }

    /// Get the current settings
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Set the WASM extension manager
    pub fn set_wasm_ext(
        &mut self,
        ext: Option<std::sync::Arc<crate::extensions::WasmExtensionManager>>,
    ) {
        self.wasm_ext = ext;
    }

    /// Get the WASM extension manager
    pub fn wasm_ext(&self) -> Option<&std::sync::Arc<crate::extensions::WasmExtensionManager>> {
        self.wasm_ext.as_ref()
    }

    /// Get a reference to the underlying agent.
    pub fn agent(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    /// Get the tool registry (for registering extension tools)
    pub fn agent_tools(&self) -> Arc<oxi_agent::ToolRegistry> {
        self.agent.tools()
    }

    /// Get the questionnaire bridge, if initialized.
    pub fn questionnaire_bridge(
        &self,
    ) -> Option<&std::sync::Arc<oxi_agent::tools::questionnaire::QuestionnaireBridge>> {
        self.questionnaire_bridge.as_ref()
    }

    /// Get a reference to the skill manager
    pub fn skills(&self) -> parking_lot::RwLockReadGuard<'_, SkillManager> {
        self.skills.read()
    }

    /// Activate a skill by name. Returns an error string if not found.
    pub fn activate_skill(&self, name: &str) -> Result<(), String> {
        {
            let skills = self.skills.read();
            if skills.get(name).is_none() {
                return Err(format!("Skill '{}' not found", name));
            }
        }
        let name_lower = name.to_lowercase();
        {
            let mut active = self.active_skills.write();
            if !active.contains(&name_lower) {
                active.push(name_lower);
            }
        }
        self.rebuild_system_prompt();
        Ok(())
    }

    /// Deactivate a skill by name.
    pub fn deactivate_skill(&self, name: &str) {
        let name_lower = name.to_lowercase();
        {
            let mut active = self.active_skills.write();
            active.retain(|n| n != &name_lower);
        }
        self.rebuild_system_prompt();
    }

    /// List currently active skill names
    pub fn active_skills(&self) -> Vec<String> {
        self.active_skills.read().clone()
    }

    /// Rebuild the system prompt with current active skills
    fn rebuild_system_prompt(&self) {
        let active = self.active_skills.read();
        let skills = self.skills.read();
        let contents: Vec<String> = active
            .iter()
            .filter_map(|name| skills.get(name).map(|s| s.content.clone()))
            .collect();
        let prompt = build_system_prompt(self.settings.thinking_level, &contents);
        self.agent.set_system_prompt(prompt);
    }

    /// Get a clone of the current state
    pub fn agent_state(&self) -> oxi_agent::AgentState {
        self.agent.state()
    }

    /// Run a single prompt and return the response
    pub async fn run_prompt(&self, prompt: String) -> Result<String> {
        let (response, _events) = self.agent.run(prompt).await?;
        Ok(response.content)
    }

    /// Run a prompt with event callback
    pub async fn run_prompt_with_events<F>(&self, prompt: String, on_event: F) -> Result<String>
    where
        F: FnMut(AgentEvent) + Send + 'static,
    {
        self.agent.run_streaming(prompt, on_event).await?;
        let state = self.agent_state();
        for msg in state.messages.iter().rev() {
            if let oxi_ai::Message::Assistant(a) = msg {
                return Ok(a.text_content());
            }
        }
        Ok(String::new())
    }

    /// Run in interactive mode, returning an event stream
    pub async fn run_interactive(&self) -> Result<InteractiveLoop<'_>> {
        let session = InteractiveSession::new();
        Ok(InteractiveLoop { app: self, session })
    }

    /// Reset the conversation
    pub fn reset(&self) {
        self.agent.reset();
    }

    /// Switch the model used for future LLM calls.
    pub fn switch_model(&self, model_id: &str) -> anyhow::Result<()> {
        let parts: Vec<&str> = model_id.split('/').collect();
        let provider = parts
            .first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "anthropic".to_string());
        let api_key = oxi_store::auth_storage::shared_auth_storage().get_api_key(&provider);
        self.agent.switch_model(model_id, api_key)
    }

    /// Get the current model ID
    pub fn model_id(&self) -> String {
        self.agent.model_id()
    }
}

/// Interactive loop handle
pub struct InteractiveLoop<'a> {
    app: &'a App,
    session: InteractiveSession,
}

impl<'a> InteractiveLoop<'a> {
    /// Add a user message and get the assistant response
    pub async fn send_message(&mut self, prompt: String) -> Result<()> {
        self.session.add_user_message(prompt.clone());
        self.session.thinking = true;

        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let agent = Arc::clone(&self.app.agent);

        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            let _ = agent.run_with_channel(prompt, tx).await;
        });

        while let Ok(event) = rx.recv() {
            match event {
                AgentEvent::TextChunk { text } => {
                    self.session.append_to_response(&text);
                }
                AgentEvent::Thinking => {}
                AgentEvent::Complete { .. } => {
                    self.session.finish_response();
                    self.session.thinking = false;
                }
                AgentEvent::Error { message, .. } => {
                    self.session
                        .append_to_response(&format!("[Error: {}]", message));
                    self.session.finish_response();
                    self.session.thinking = false;
                }
                _ => {}
            }
        }

        local.await;
        Ok(())
    }

    /// Get current messages
    pub fn messages(&self) -> &[ChatMessage] {
        &self.session.messages
    }

    /// Get the current partial response (while thinking)
    pub fn current_response(&self) -> &str {
        &self.session.current_response
    }

    /// Check if currently thinking
    pub fn is_thinking(&self) -> bool {
        self.session.thinking
    }

    /// Get session entries for tree navigation
    pub fn entries(&self) -> &[SessionEntry] {
        self.session.entries()
    }

    /// Get entry by ID
    pub fn get_entry(&self, id: Uuid) -> Option<&SessionEntry> {
        self.session.get_entry_by_id(&id.to_string())
    }

    /// Switch the model used for future LLM calls
    pub fn switch_model(&self, model_id: &str) -> anyhow::Result<()> {
        self.app.switch_model(model_id)
    }

    /// Get the current model ID
    pub fn model_id(&self) -> String {
        self.app.model_id()
    }
}
