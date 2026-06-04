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
pub mod services;
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
pub(crate) mod util;

// ─── oxi-store re-exports (shared persistent state) ─────────────────────────
pub use oxi_store::{
    auth_guidance, auth_storage, model_registry, model_resolver, session, session_cwd,
    session_navigation, settings, settings_validation, AgentMessage, AssistantContentBlock,
    AuthStorage, ContentBlock, ContentValue, ModelRegistry, SessionEntry, SessionManager,
    SessionTreeNode, Settings, ValidationReport,
};

/// Build an `Oxi` engine wired with file-based port implementations.
///
/// This is the **new entry point** for oxi-cli run modes. It uses
/// `oxi-fs` adapters and `OxiBuilder::with_port_*` to construct an
/// `Oxi` with persistence, auth, config, and skills wired. The legacy
/// `App::new` path is still used by the interactive TUI during the
/// migration period.
///
/// # Example
///
/// ```no_run
/// use oxi::build_oxi_engine;
/// # fn _example() -> anyhow::Result<()> {
/// let oxi = build_oxi_engine()?;
/// println!("providers: {}", oxi.providers().names().len());
/// # Ok(()) }
/// ```
pub fn build_oxi_engine() -> anyhow::Result<oxi_sdk::Oxi> {
    let paths = services::OxiPaths::default_paths()?;
    services::build_oxi(&paths)
}

/// Self-check the wired port implementations. Prints a one-line summary
/// per port and returns `Ok(())` if all are reachable.
///
/// Triggered by the `OXI_PORT_CHECK=1` environment variable from
/// `oxi-cli/src/main.rs`. Useful for verifying the new composition root
/// without disturbing the legacy `App::new` path.
pub async fn run_port_check() -> anyhow::Result<()> {
    let oxi = build_oxi_engine()?;
    let ports = oxi.ports();

    // State
    let entries = ports.state.list("").await?;
    println!("[state]    entries: {}", entries.len());

    // Auth
    let providers = ports.auth.list_providers().await?;
    println!("[auth]     providers with credentials: {:?}", providers);

    // Config
    let keys = ports.config.list()?;
    println!("[config]   keys: {}", keys.len());

    // Skills
    let skills = ports.skills.list().await?;
    println!("[skills]   {} skill(s) discovered", skills.len());
    for s in &skills {
        println!("           - {}: {}", s.name, s.description);
    }

    // Event bus / memory / etc — all noop unless registered
    let _ = ports.event_bus.publish(&"port-check".to_string(), serde_json::json!({"ok": true})).await;
    println!("[event-bus] publish ok (noop bus if not registered)");

    println!("\nport check: ok");
    Ok(())
}

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
use parking_lot::RwLock;
use skills::SkillManager;
use std::sync::Arc;

// ─── Application state ───────────────────────────────────────────────────────

/// Application state and entry point.
///
/// Holds an `Oxi` engine (composition root) and a single `Agent` built
/// from it. The legacy `App::new(settings)` constructor is **gone**;
/// use [`App::from_oxi`] with a wired `Oxi` from
/// [`build_oxi_engine`].
pub struct App {
    oxi: oxi_sdk::Oxi,
    agent: Arc<Agent>,
    settings: Settings,
    skills: RwLock<SkillManager>,
    active_skills: RwLock<Vec<String>>,
    wasm_ext: Option<std::sync::Arc<crate::extensions::WasmExtensionManager>>,
    questionnaire_bridge:
        Option<std::sync::Arc<oxi_agent::tools::questionnaire::QuestionnaireBridge>>,
}

/// Context for compaction operations, passed to extension hooks// ─── System prompt builder ───────────────────────────────────────────────────

fn build_system_prompt(
    thinking_level: oxi_store::settings::ThinkingLevel,
    skill_contents: &[String],
) -> String {
    let skills: Vec<prompt::system_prompt::Skill> = skill_contents
        .iter()
        .enumerate()
        .map(|(i, content)| prompt::system_prompt::Skill {
            name: format!("skill-{}", i),
            content: content.clone(),
        })
        .collect();

    let options = prompt::system_prompt::BuildSystemPromptOptions {
        custom_prompt: prompt::system_prompt::thinking_level_prompt(thinking_level),
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
    /// Build an `App` from a wired `Oxi` engine and a settings object.
    ///
    /// The `Oxi` should be created via [`build_oxi_engine`] (or
    /// `services::build_oxi`) so that all 11 ports are wired. The
    /// settings hold the user's runtime configuration (model, thinking
    /// level, etc.).
    pub async fn from_oxi(oxi: oxi_sdk::Oxi, settings: Settings) -> Result<Self> {
        let model_id = settings.effective_model(None).unwrap_or_default();
        let provider_name = settings
            .effective_provider(None)
            .unwrap_or_else(|| model_id.split('/').next().unwrap_or("").to_string());

        // Pull the API key from the wired port, not from oxi_store.
        let api_key = oxi.ports().auth.get_api_key(&provider_name).await?;

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
            oxi_sdk::CompactionStrategy::Threshold(0.8)
        } else {
            oxi_sdk::CompactionStrategy::Disabled
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
            api_key,
            workspace_dir: Some(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            ),
            output_mode: None,
            provider_options: None,
        };

        // Build the agent via the SDK's AgentBuilder — no manual wiring.
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let agent = oxi
            .agent(config)
            .workspace(cwd)
            .build()
            .map_err(|e| Error::msg(format!("agent build failed: {e}")))?;
        let agent = Arc::new(agent);

        let bridge =
            std::sync::Arc::new(oxi_agent::tools::questionnaire::QuestionnaireBridge::new());
        let questionnaire_tool =
            oxi_agent::tools::questionnaire::QuestionnaireTool::new(bridge.clone());
        agent
            .tools()
            .register_arc(std::sync::Arc::new(questionnaire_tool));

        Ok(Self {
            oxi,
            agent,
            settings,
            skills: RwLock::new(skills),
            active_skills: RwLock::new(Vec::new()),
            wasm_ext: None,
            questionnaire_bridge: Some(bridge),
        })
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
            if let oxi_sdk::Message::Assistant(a) = msg {
                return Ok(a.text_content());
            }
        }
        Ok(String::new())
    }

    /// Reset the conversation
    pub fn reset(&self) {
        self.agent.reset();
    }

    /// Switch the model used for future LLM calls.
    pub async fn switch_model(&self, model_id: &str) -> anyhow::Result<()> {
        let parts: Vec<&str> = model_id.split('/').collect();
        let provider = parts
            .first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "anthropic".to_string());
        let api_key = self.oxi.ports().auth.get_api_key(&provider).await?;
        self.agent.switch_model(model_id, api_key);
        Ok(())
    }

    /// Get the current model ID
    pub fn model_id(&self) -> String {
        self.agent.model_id()
    }
}
