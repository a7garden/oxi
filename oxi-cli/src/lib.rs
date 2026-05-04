//! oxi: CLI coding harness
//!
//! This crate provides the main application logic for the oxi CLI.

pub mod export;
pub mod extensions;
pub mod interactive;
pub mod oauth_server;
pub mod packages;
pub mod print_mode;
pub mod session;
pub mod settings;
pub mod skills;
pub mod templates;
pub mod tui_components;
pub mod tui_interactive;

// RPC mode for headless operation
pub mod rpc_mode;

// Image processing modules
pub mod clipboard_image;
pub mod image_convert;
pub mod image_resize;
pub mod file_processor;

// Utility modules
pub mod auth_storage;
pub mod auto_compaction;
pub mod branch_summarization;
pub mod bash_executor;
pub mod changelog;
pub mod child_process;
pub mod cli;
pub mod diagnostics;
pub mod error_recovery;
pub mod fs_watch;
pub mod mime_detect;
pub mod paths;
pub mod sleep;

// Re-exports for extension hooks
pub use crate::error_recovery::{RetryConfig, RetryableError};

// Note: RetryStrategy is available in error_recovery::RetryStrategy

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
pub mod event_bus;
pub mod footer_data;
pub mod git_utils;
pub mod keybindings;
pub mod messages;
pub mod model_registry;
pub mod model_resolver;
pub mod resource_loader;
pub mod frontmatter;
pub mod resource_loader_compat;
pub mod output_guard;
pub mod tmux_detect;
pub mod tools_manager;
pub mod version_check;

// Core modules ported from pi-mono
pub mod defaults;
pub mod provider_display_names;
pub mod session_cwd;
pub mod slash_commands;
pub mod agent_session;
// pub mod agent_session_runtime;  // TODO: incomplete, needs SessionManager::create_branched_session
pub mod compaction_utils;
pub mod source_info;
pub mod system_prompt;
pub mod telemetry;
pub mod timings;

use anyhow::{Error, Result};
use oxi_agent::{Agent, AgentConfig, AgentEvent};
use oxi_ai::{get_model, get_provider};
use parking_lot::RwLock;
use settings::{Settings, ThinkingLevel};
use skills::SkillManager;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Application state and entry point
pub struct App {
    agent: Arc<Agent>,
    settings: Settings,
    skills: RwLock<SkillManager>,
    active_skills: RwLock<Vec<String>>,
}

/// Chat message for display
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Interactive session state
#[derive(Debug, Clone)]
pub struct InteractiveSession {
    pub messages: Vec<ChatMessage>,
    pub thinking: bool,
    pub current_response: String,
    pub session_id: Option<Uuid>,
    pub name: Option<String>,
    pub entries: Vec<session::SessionEntry>,
}

impl Default for InteractiveSession {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            thinking: false,
            current_response: String::new(),
            session_id: None,
            name: None,
            entries: Vec::new(),
        }
    }
}

impl InteractiveSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(ChatMessage::user(content.clone()));
        // Also add to entries for session persistence
        let entry = session::SessionEntry::new(session::AgentMessage::User {
            content: session::ContentValue::String(content),
        });
        self.entries.push(entry);
    }

    pub fn add_assistant_message(&mut self, content: String) {
        self.messages.push(ChatMessage::assistant(content.clone()));
        // Also add to entries for session persistence
        let entry = session::SessionEntry::new(session::AgentMessage::Assistant {
            content: vec![session::AssistantContentBlock::Text { text: content }],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        });
        self.entries.push(entry);
        self.current_response.clear();
    }

    pub fn append_to_response(&mut self, text: &str) {
        self.current_response.push_str(text);
    }

    pub fn finish_response(&mut self) {
        if !self.current_response.is_empty() {
            let response = std::mem::take(&mut self.current_response);
            self.add_assistant_message(response);
        }
    }

    /// Get all entries in the session
    pub fn entries(&self) -> &[session::SessionEntry] {
        &self.entries
    }

    /// Get entry at a specific index
    pub fn get_entry(&self, index: usize) -> Option<&session::SessionEntry> {
        self.entries.get(index)
    }

    /// Get entry by ID
    pub fn get_entry_by_id(&self, id: &str) -> Option<&session::SessionEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Truncate entries at a given index (for branching)
    pub fn truncate_at(&mut self, index: usize) {
        self.entries.truncate(index + 1);
    }
}

/// Build the system prompt based on thinking level and active skills
fn build_system_prompt(thinking_level: ThinkingLevel, skill_contents: &[String]) -> String {
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

    // Append active skill content
    for content in skill_contents {
        prompt.push_str("\n\n---\n# Active Skill\n\n");
        prompt.push_str(content);
    }

    prompt
}

impl App {
    /// Create a new App instance
    pub async fn new(settings: Settings) -> Result<Self> {
        let model_id = settings.effective_model(None);
        let provider_name = settings.effective_provider(None);

        // Parse model ID to get provider and model
        let parts: Vec<&str> = model_id.split('/').collect();
        let (provider_name, model_name) = if parts.len() >= 2 {
            (parts[0].to_string(), parts[1..].join("/"))
        } else {
            (provider_name.clone(), model_id.clone())
        };

        // Get the model
        let _model = get_model(&provider_name, &model_name)
            .ok_or_else(|| Error::msg(format!("Model '{}' not found", model_id)))?;

        // Create a provider for this model
        let provider = get_provider(&provider_name)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", provider_name)))?;

        // Load skills
        let skills_dir = SkillManager::skills_dir().unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".oxi")
                .join("skills")
        });
        let skills = SkillManager::load_from_dir(&skills_dir).unwrap_or_else(|e| {
            tracing::debug!("Skills not loaded: {}", e);
            SkillManager::load_from_dir(std::path::Path::new("/nonexistent")).unwrap()
        });

        // Build agent config from settings
        let system_prompt = build_system_prompt(settings.thinking_level, &[]);
        let compaction_strategy = if settings.auto_compaction {
            oxi_ai::CompactionStrategy::Threshold(0.8)
        } else {
            oxi_ai::CompactionStrategy::Disabled
        };
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
        };

        let agent = Arc::new(Agent::new(Arc::from(provider), config));

        Ok(Self {
            agent,
            settings,
            skills: RwLock::new(skills),
            active_skills: RwLock::new(Vec::new()),
        })
    }

    /// Get the current settings
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Get a reference to the underlying agent.
    pub fn agent(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    /// Get the tool registry (for registering extension tools)
    pub fn agent_tools(&self) -> Arc<oxi_agent::ToolRegistry> {
        self.agent.tools()
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
        // Get the last assistant message's text content
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
    ///
    /// See [`Agent::switch_model`] for details.
    pub fn switch_model(&self, model_id: &str) -> anyhow::Result<()> {
        self.agent.switch_model(model_id)
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
        // Add user message
        self.session.add_user_message(prompt.clone());
        self.session.thinking = true;

        // Run agent with channel
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);

        // Run the agent — we execute inline instead of spawning because
        // the agent's internal RwLockReadGuard is not Send-safe across
        // await points. We use a select-like approach: run the agent in a
        // local task that doesn't require Send.
        let agent = Arc::clone(&self.app.agent);

        // Use LocalSet to spawn a non-Send future
        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            let _ = agent.run_with_channel(prompt, tx).await;
        });

        // Collect events
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::TextChunk { text } => {
                    self.session.append_to_response(&text);
                }
                AgentEvent::Thinking => {
                    // Thinking state
                }
                AgentEvent::Complete { .. } => {
                    self.session.finish_response();
                    self.session.thinking = false;
                }
                AgentEvent::Error { message } => {
                    self.session
                        .append_to_response(&format!("[Error: {}]", message));
                    self.session.finish_response();
                    self.session.thinking = false;
                }
                _ => {}
            }
        }

        // Run local set to completion (drain remaining agent work)
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
    pub fn entries(&self) -> &[session::SessionEntry] {
        self.session.entries()
    }

    /// Get entry by ID
    pub fn get_entry(&self, id: Uuid) -> Option<&session::SessionEntry> {
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
