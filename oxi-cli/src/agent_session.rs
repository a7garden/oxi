//! AgentSession — session wrapper around Agent.
//!
//! This is the core session abstraction shared between all run modes
//! (interactive, print, RPC). It encapsulates:
//!
//! - Agent state access and event subscription
//! - Automatic session persistence on each agent event
//! - Model and thinking-level management with cycling
//! - Auto-compaction (threshold-based and overflow-recovery)
//! - Auto-retry on transient / rate-limit errors
//! - Steering / follow-up message queueing
//! - Extension event forwarding hooks
//!
//! # Architecture
//!
//! ```text
//! interactive.rs / print_mode.rs / rpc_mode.rs
//!        │
//!        ▼
//!  AgentSession   ← this module
//!        │
//!        ▼
//!  oxi_agent::Agent
//!        │
//!        ▼
//!  oxi_ai::Provider  (streaming LLM calls)
//! ```

use crate::auto_compaction::CompactionConfig;
use crate::extensions::{ExtensionContext, ExtensionContextBuilder, ExtensionRunner, InputEvent as ExtInputEvent, InputEventResult as ExtInputEventResult, SessionShutdownEvent, SessionShutdownReason};
use crate::session::{AgentMessage, SessionManager};
use crate::settings::{Settings, ThinkingLevel};
use anyhow::{Context, Result};
use oxi_agent::{Agent, AgentEvent, AgentState};
use oxi_ai::Message;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, Notify};
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════════════════
// Session-level events (extends AgentEvent with session concerns)
// ═══════════════════════════════════════════════════════════════════════════

/// Events emitted by [`AgentSession`] in addition to the underlying
/// [`AgentEvent`]s.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// A steering or follow-up queue changed.
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    /// Compaction started.
    CompactionStart {
        reason: CompactionReason,
    },
    /// Compaction finished (or failed / was aborted).
    CompactionEnd {
        reason: CompactionReason,
        result: Option<CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    /// Auto-retry started.
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: String,
    },
    /// Auto-retry ended.
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        final_error: Option<String>,
    },
    /// Session display name changed.
    SessionInfoChanged {
        name: Option<String>,
    },
    /// Thinking level changed.
    ThinkingLevelChanged {
        level: ThinkingLevel,
    },
    /// Passthrough agent event.
    Agent(AgentEvent),
}

/// Why compaction was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    /// User ran `/compact`.
    Manual,
    /// Context exceeded threshold percentage.
    Threshold,
    /// LLM returned a context-overflow error.
    Overflow,
}

/// Result of a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: Option<Uuid>,
    pub tokens_before: usize,
    pub details: Option<serde_json::Value>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Model cycling
// ═══════════════════════════════════════════════════════════════════════════

/// Scoped model entry for Ctrl+P cycling.
#[derive(Debug, Clone)]
pub struct ScopedModel {
    pub provider: String,
    pub model_id: String,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Result from [`AgentSession::cycle_model`].
#[derive(Debug, Clone)]
pub struct ModelCycleResult {
    pub provider: String,
    pub model_id: String,
    pub thinking_level: ThinkingLevel,
    pub is_scoped: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// Prompt options
// ═══════════════════════════════════════════════════════════════════════════

/// Options for [`AgentSession::prompt`].
#[derive(Debug, Clone)]
pub struct PromptOptions {
    /// Whether to expand file-based prompt templates (default: true).
    pub expand_templates: bool,
    /// Image attachments.
    pub images: Vec<oxi_ai::ImageContent>,
    /// How to queue when agent is streaming: steer (interrupt) or follow-up (wait).
    pub streaming_behavior: Option<StreamingBehavior>,
    /// Source of input (for extension hooks).
    pub source: InputSource,
}

impl Default for PromptOptions {
    fn default() -> Self {
        Self {
            expand_templates: true,
            images: Vec::new(),
            streaming_behavior: None,
            source: InputSource::Interactive,
        }
    }
}

/// How to queue a message when the agent is already streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingBehavior {
    /// Inject as a steering message.
    Steer,
    /// Append as a follow-up.
    FollowUp,
}

/// Source of user input (for extension hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Interactive,
    Extension,
    Rpc,
}

impl Default for InputSource {
    fn default() -> Self {
        Self::Interactive
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Session statistics
// ═══════════════════════════════════════════════════════════════════════════

/// Statistics returned by [`AgentSession::session_stats`].
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub session_id: String,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub total_messages: usize,
    pub tokens: TokenStats,
    pub cost: f64,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default)]
pub struct TokenStats {
    pub input: usize,
    pub output: usize,
    pub total: usize,
}

// ═══════════════════════════════════════════════════════════════════════════
// Retry settings (session-level, read from Settings)
// ═══════════════════════════════════════════════════════════════════════════

/// Retry settings derived from the global [`Settings`].
#[derive(Debug, Clone)]
pub struct SessionRetrySettings {
    pub enabled: bool,
    pub max_retries: u32,
    pub base_delay_ms: u64,
}

impl SessionRetrySettings {
    /// Read from the global settings.
    pub fn from_settings(_settings: &Settings) -> Self {
        // For now, use reasonable defaults; the settings module can be extended.
        Self {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 1000,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AgentSession
// ═══════════════════════════════════════════════════════════════════════════

/// Session wrapper around [`Agent`] that adds:
///
/// - Model cycling and thinking-level management
/// - Steering / follow-up message queues
/// - Auto-compaction after responses
/// - Auto-retry on transient errors
/// - Session persistence (auto-save on each event)
/// - Extension event forwarding hooks
pub struct AgentSession {
    // ── Core ──────────────────────────────────────────────────────────
    agent: Arc<Agent>,
    settings: Arc<RwLock<Settings>>,
    session_manager: Arc<RwLock<SessionManager>>,

    // ── Event listeners ──────────────────────────────────────────────
    listeners: Arc<RwLock<Vec<Box<dyn Fn(&SessionEvent) + Send + Sync>>>>,
    event_tx: mpsc::UnboundedSender<SessionEvent>,

    // ── Model / thinking state ───────────────────────────────────────
    scoped_models: Arc<RwLock<Vec<ScopedModel>>>,

    // ── Queues ───────────────────────────────────────────────────────
    steering_messages: Arc<RwLock<VecDeque<String>>>,
    follow_up_messages: Arc<RwLock<VecDeque<String>>>,

    // ── Compaction state ─────────────────────────────────────────────
    compaction_config: Arc<RwLock<CompactionConfig>>,
    compaction_abort: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    overflow_recovery_attempted: Arc<RwLock<bool>>,

    // ── Retry state ──────────────────────────────────────────────────
    retry_settings: Arc<RwLock<SessionRetrySettings>>,
    retry_attempt: Arc<RwLock<u32>>,
    retry_abort: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    retry_notify: Arc<Notify>,

    // ── Session persistence ──────────────────────────────────────────
    session_id: Arc<RwLock<String>>,

    // ── CWD ──────────────────────────────────────────────────────────
    cwd: String,

    // ── Extensions ───────────────────────────────────────────────────
    extension_runner: Arc<RwLock<Option<ExtensionRunner>>>,
}

impl AgentSession {
    /// Create a new session wrapping the given [`Agent`].
    pub fn new(
        agent: Arc<Agent>,
        settings: Settings,
        session_manager: SessionManager,
        cwd: String,
    ) -> Self {
        let session_id = session_manager.get_session_id();
        let retry_settings = SessionRetrySettings::from_settings(&settings);
        let compaction_config = CompactionConfig {
            enabled: settings.auto_compaction,
            ..CompactionConfig::default()
        };

        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        Self {
            agent,
            settings: Arc::new(RwLock::new(settings)),
            session_manager: Arc::new(RwLock::new(session_manager)),
            listeners: Arc::new(RwLock::new(Vec::new())),
            event_tx,
            scoped_models: Arc::new(RwLock::new(Vec::new())),
            steering_messages: Arc::new(RwLock::new(VecDeque::new())),
            follow_up_messages: Arc::new(RwLock::new(VecDeque::new())),
            compaction_config: Arc::new(RwLock::new(compaction_config)),
            compaction_abort: Arc::new(Mutex::new(None)),
            overflow_recovery_attempted: Arc::new(RwLock::new(false)),
            retry_settings: Arc::new(RwLock::new(retry_settings)),
            retry_attempt: Arc::new(RwLock::new(0)),
            retry_abort: Arc::new(Mutex::new(None)),
            retry_notify: Arc::new(Notify::new()),
            session_id: Arc::new(RwLock::new(session_id)),
            cwd,
            extension_runner: Arc::new(RwLock::new(None)),
        }
    }

    // ══════════════════════════════════════════════════════════════════
    // Read-only state access
    // ══════════════════════════════════════════════════════════════════

    /// Get the current model ID (`provider/model`).
    pub fn model_id(&self) -> String {
        self.agent.model_id()
    }

    /// Get the current agent state.
    pub fn state(&self) -> AgentState {
        self.agent.state()
    }

    /// Current thinking level.
    pub fn thinking_level(&self) -> ThinkingLevel {
        self.settings.read().thinking_level
    }

    /// Whether the agent is currently streaming.
    pub fn is_streaming(&self) -> bool {
        // The agent doesn't expose is_streaming yet; approximate from state.
        // When streaming is in progress, there will be an active run.
        false // TODO: wire up when Agent exposes is_streaming
    }

    /// All messages in the agent state.
    pub fn messages(&self) -> Vec<Message> {
        self.agent.state().messages
    }

    /// Current session ID.
    pub fn session_id(&self) -> String {
        self.session_manager.read().get_session_id()
    }

    /// Whether compaction is in progress.
    pub fn is_compacting(&self) -> bool {
        self.compaction_abort.try_lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Whether auto-retry is in progress.
    pub fn is_retrying(&self) -> bool {
        *self.retry_attempt.read() > 0
    }

    /// Get the current session stats.
    pub fn session_stats(&self) -> SessionStats {
        let state = self.agent.state();
        let mut user_messages = 0usize;
        let mut assistant_messages = 0usize;
        let mut tool_results = 0usize;
        let mut tool_calls = 0usize;
        let input_tokens = 0usize;
        let output_tokens = 0usize;

        for msg in &state.messages {
            match msg {
                Message::User(_) => user_messages += 1,
                Message::Assistant(a) => {
                    assistant_messages += 1;
                    // Count tool-use content blocks
                    for block in &a.content {
                        if matches!(block, oxi_ai::ContentBlock::ToolCall(_)) {
                            tool_calls += 1;
                        }
                    }
                    let _ = &a; // suppress unused warning
                }
                Message::ToolResult(_) => tool_results += 1,
            }
        }

        SessionStats {
            session_id: self.session_id(),
            user_messages,
            assistant_messages,
            tool_calls,
            tool_results,
            total_messages: state.messages.len(),
            tokens: TokenStats {
                input: input_tokens,
                output: output_tokens,
                total: input_tokens + output_tokens,
            },
            cost: 0.0,
        }
    }

    /// Get the number of pending messages (steering + follow-up).
    pub fn pending_message_count(&self) -> usize {
        self.steering_messages.read().len() + self.follow_up_messages.read().len()
    }

    /// Get pending steering messages.
    pub fn steering_messages(&self) -> Vec<String> {
        self.steering_messages.read().iter().cloned().collect()
    }

    /// Get pending follow-up messages.
    pub fn follow_up_messages(&self) -> Vec<String> {
        self.follow_up_messages.read().iter().cloned().collect()
    }

    /// Current working directory.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Get scoped models for cycling.
    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        self.scoped_models.read().clone()
    }

    /// Check if auto-compaction is enabled.
    pub fn auto_compaction_enabled(&self) -> bool {
        self.compaction_config.read().enabled
    }

    /// Check if auto-retry is enabled.
    pub fn auto_retry_enabled(&self) -> bool {
        self.retry_settings.read().enabled
    }

    // ══════════════════════════════════════════════════════════════════
    // Event subscription
    // ══════════════════════════════════════════════════════════════════

    /// Subscribe to session events. Returns a guard that, when dropped,
    /// unsubscribes the listener.
    ///
    /// **Note:** The listener is called synchronously on the event-processing
    /// thread; keep it fast. For async processing, forward to a channel.
    pub fn subscribe(&self, listener: Box<dyn Fn(&SessionEvent) + Send + Sync>) -> SessionListenerGuard {
        let key = {
            let mut listeners = self.listeners.write();
            listeners.push(listener);
            listeners.len() - 1
        };
        SessionListenerGuard {
            listeners: Arc::clone(&self.listeners),
            key,
        }
    }

    /// Subscribe via an unbounded channel. Returns the receiver.
    pub fn subscribe_channel(&self) -> mpsc::UnboundedReceiver<SessionEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe(Box::new(move |event| {
            let _ = tx.send(event.clone());
        }));
        rx
    }

    /// Emit a session event to all listeners.
    fn emit(&self, event: SessionEvent) {
        let listeners = self.listeners.read();
        for listener in listeners.iter() {
            listener(&event);
        }
        // Also send to the internal channel
        let _ = self.event_tx.send(event);
    }

    /// Emit a queue update event.
    fn emit_queue_update(&self) {
        self.emit(SessionEvent::QueueUpdate {
            steering: self.steering_messages(),
            follow_up: self.follow_up_messages(),
        });
    }

    // ══════════════════════════════════════════════════════════════════
    // Prompting
    // ══════════════════════════════════════════════════════════════════

    /// Send a prompt to the agent.
    ///
    /// If the agent is already streaming and `streaming_behavior` is set,
    /// the message is queued as steering or follow-up instead.
    ///
    /// After the agent finishes, auto-compaction and auto-retry are
    /// checked automatically.
    pub async fn prompt(&self, text: String, options: PromptOptions) -> Result<()> {
        // When streaming, queue the message instead
        if self.is_streaming() {
            return match options.streaming_behavior {
                Some(StreamingBehavior::Steer) => {
                    self.steer(text).await
                }
                Some(StreamingBehavior::FollowUp) => {
                    self.follow_up(text).await
                }
                None => {
                    anyhow::bail!(
                        "Agent is already processing. Specify streaming_behavior to queue the message."
                    );
                }
            };
        }

        // Validate model
        let model_id = self.model_id();
        if model_id.is_empty() {
            anyhow::bail!("No model selected");
        }

        // Run the agent and collect events
        let (_response, events) = self.agent.run(text.clone()).await?;

        // Process events for session persistence, compaction, and retry
        self.process_events(events).await?;

        Ok(())
    }

    /// Run a prompt and get a channel of events for streaming display.
    ///
    /// **Note:** Due to the Agent's internal locks not being `Send`, this
    /// uses `tokio::task::LocalSet` internally. Callers should prefer
    /// [`prompt`] when full streaming control is not needed.
    pub fn prompt_streaming(
        &self,
        text: String,
    ) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (_agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(100);

        // Clone tx for the forwarding task
        let tx_clone = tx.clone();

        // Forward agent events through our channel
        tokio::spawn(async move {
            while let Some(event) = agent_rx.recv().await {
                let _ = tx_clone.send(event);
            }
        });

        // Run the agent in a LocalSet (because Agent's internal RwLock is !Send)
        let agent_clone = Arc::clone(&self.agent);
        let _session_clone = self.clone_handle();
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let local = tokio::task::LocalSet::new();
                let _ = local.run_until(async move {
                    let (inner_tx, mut inner_rx) = mpsc::channel(100);
                    // Use agent.run_with_channel inside LocalSet
                    let agent_for_task = Arc::clone(&agent_clone);
                    let handle = tokio::task::spawn_local(async move {
                        let _ = agent_for_task.run_with_channel(text, inner_tx).await;
                    });

                    // Forward events from inner to outer
                    while let Some(event) = inner_rx.recv().await {
                        let _ = tx.send(event);
                    }

                    let _ = handle.await;
                });
            });
        });

        rx
    }

    /// Queue a steering message (delivered after current turn's tool calls).
    pub async fn steer(&self, text: String) -> Result<()> {
        {
            let mut queue = self.steering_messages.write();
            queue.push_back(text.clone());
        }
        self.emit_queue_update();

        // Inject into agent state as a user message
        self.agent.state().add_user_message(text);

        Ok(())
    }

    /// Queue a follow-up message (processed after agent finishes).
    pub async fn follow_up(&self, text: String) -> Result<()> {
        {
            let mut queue = self.follow_up_messages.write();
            queue.push_back(text.clone());
        }
        self.emit_queue_update();

        Ok(())
    }

    /// Abort current operation.
    pub async fn abort(&self) {
        self.abort_retry();
        // Agent abort is not yet exposed; best-effort
        tracing::debug!("AgentSession::abort() requested");
    }

    /// Clear all queued messages and return them.
    pub fn clear_queue(&self) -> (Vec<String>, Vec<String>) {
        let steering: Vec<String> = self.steering_messages.write().drain(..).collect();
        let follow_up: Vec<String> = self.follow_up_messages.write().drain(..).collect();
        self.emit_queue_update();
        (steering, follow_up)
    }

    // ══════════════════════════════════════════════════════════════════
    // Model management
    // ══════════════════════════════════════════════════════════════════

    /// Switch model mid-conversation.
    pub fn set_model(&self, model_id: &str) -> Result<()> {
        self.agent.switch_model(model_id)?;

        // Persist model change to session
        {
            let mut sm = self.session_manager.write();
            let parts: Vec<&str> = model_id.split('/').collect();
            if parts.len() >= 2 {
                sm.append_model_change(parts[0], &parts[1..].join("/"));
            }
        }

        // Update settings default
        {
            let mut settings = self.settings.write();
            let parts: Vec<&str> = model_id.split('/').collect();
            if parts.len() >= 2 {
                settings.default_provider = Some(parts[0].to_string());
                settings.default_model = Some(parts[1..].join("/"));
            } else {
                settings.default_model = Some(model_id.to_string());
            }
        }

        Ok(())
    }

    /// Cycle to the next/previous model.
    ///
    /// Uses scoped models (from `--models` flag) if available,
    /// otherwise cycles through well-known defaults.
    pub fn cycle_model(&self, direction: CycleDirection) -> Option<ModelCycleResult> {
        let scoped = self.scoped_models.read().clone();

        if !scoped.is_empty() {
            return self.cycle_scoped_model(&scoped, direction);
        }

        // Fall back to a hardcoded list of popular models
        let defaults = default_model_list();
        if defaults.len() <= 1 {
            return None;
        }
        self.cycle_default_model(&defaults, direction, false)
    }

    fn cycle_scoped_model(
        &self,
        scoped: &[ScopedModel],
        direction: CycleDirection,
    ) -> Option<ModelCycleResult> {
        if scoped.len() <= 1 {
            return None;
        }

        let current_id = self.model_id();
        let current_index = scoped
            .iter()
            .position(|m| format!("{}/{}", m.provider, m.model_id) == current_id)
            .unwrap_or(0);

        let len = scoped.len();
        let next_index = match direction {
            CycleDirection::Forward => (current_index + 1) % len,
            CycleDirection::Backward => (current_index + len - 1) % len,
        };

        let next = &scoped[next_index];
        let new_id = format!("{}/{}", next.provider, next.model_id);

        if let Err(e) = self.set_model(&new_id) {
            tracing::warn!("Failed to switch to scoped model {}: {}", new_id, e);
            return None;
        }

        // Apply thinking level
        if let Some(level) = next.thinking_level {
            self.set_thinking_level(level);
        }

        Some(ModelCycleResult {
            provider: next.provider.clone(),
            model_id: next.model_id.clone(),
            thinking_level: self.thinking_level(),
            is_scoped: true,
        })
    }

    fn cycle_default_model(
        &self,
        models: &[(&str, &str)],
        direction: CycleDirection,
        _is_scoped: bool,
    ) -> Option<ModelCycleResult> {
        let current_id = self.model_id();
        let current_index = models
            .iter()
            .position(|(p, m)| format!("{}/{}", p, m) == current_id)
            .unwrap_or(0);

        let len = models.len();
        let next_index = match direction {
            CycleDirection::Forward => (current_index + 1) % len,
            CycleDirection::Backward => (current_index + len - 1) % len,
        };

        let (provider, model) = models[next_index];
        let new_id = format!("{}/{}", provider, model);

        if let Err(e) = self.set_model(&new_id) {
            tracing::warn!("Failed to switch to model {}: {}", new_id, e);
            return None;
        }

        Some(ModelCycleResult {
            provider: provider.to_string(),
            model_id: model.to_string(),
            thinking_level: self.thinking_level(),
            is_scoped: false,
        })
    }

    /// Set scoped models for cycling.
    pub fn set_scoped_models(&self, models: Vec<ScopedModel>) {
        *self.scoped_models.write() = models;
    }

    // ══════════════════════════════════════════════════════════════════
    // Thinking level management
    // ══════════════════════════════════════════════════════════════════

    /// Set thinking level, clamped to model capabilities.
    pub fn set_thinking_level(&self, level: ThinkingLevel) {
        let old_level = self.thinking_level();
        if level == old_level {
            return;
        }

        {
            let mut settings = self.settings.write();
            settings.thinking_level = level;
        }

        // Persist to session
        {
            let mut sm = self.session_manager.write();
            sm.append_thinking_level_change(&format!("{:?}", level).to_lowercase());
        }

        self.emit(SessionEvent::ThinkingLevelChanged { level });
    }

    /// Cycle to the next thinking level.
    pub fn cycle_thinking_level(&self) -> Option<ThinkingLevel> {
        let levels = [
            ThinkingLevel::None,
            ThinkingLevel::Minimal,
            ThinkingLevel::Standard,
            ThinkingLevel::Thorough,
        ];
        let current = self.thinking_level();
        let current_index = levels.iter().position(|l| *l == current).unwrap_or(0);
        let next_index = (current_index + 1) % levels.len();
        let next = levels[next_index];
        self.set_thinking_level(next);
        Some(next)
    }

    // ══════════════════════════════════════════════════════════════════
    // Auto-compaction
    // ══════════════════════════════════════════════════════════════════

    /// Manually trigger compaction.
    pub async fn compact(&self, custom_instructions: Option<String>) -> Result<CompactionResult> {
        self.emit(SessionEvent::CompactionStart {
            reason: CompactionReason::Manual,
        });

        let result = self.run_compaction(custom_instructions).await;

        match &result {
            Ok(r) => self.emit(SessionEvent::CompactionEnd {
                reason: CompactionReason::Manual,
                result: Some(r.clone()),
                aborted: false,
                will_retry: false,
                error_message: None,
            }),
            Err(e) => self.emit(SessionEvent::CompactionEnd {
                reason: CompactionReason::Manual,
                result: None,
                aborted: false,
                will_retry: false,
                error_message: Some(e.to_string()),
            }),
        }

        result
    }

    /// Check auto-compaction after a response and trigger if needed.
    async fn check_auto_compaction(&self) {
        let config = self.compaction_config.read().clone();
        if !config.enabled {
            return;
        }

        let state = self.agent.state();
        let messages = &state.messages;
        if messages.is_empty() {
            return;
        }

        // Estimate token count
        let context_json = serde_json::to_string(messages).unwrap_or_default();
        let estimated_tokens = oxi_ai::estimate_tokens(&context_json);

        // Get context window from agent config (default 128k)
        let context_window = 128_000;

        // Check threshold
        let threshold = config.threshold as usize;
        if estimated_tokens > (context_window * threshold / 100) {
            tracing::info!(
                "Auto-compaction triggered: {} tokens > {}% of {}",
                estimated_tokens,
                threshold,
                context_window,
            );

            self.emit(SessionEvent::CompactionStart {
                reason: CompactionReason::Threshold,
            });

            let result = self.run_compaction(None).await;

            match result {
                Ok(r) => self.emit(SessionEvent::CompactionEnd {
                    reason: CompactionReason::Threshold,
                    result: Some(r),
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                }),
                Err(e) => {
                    tracing::warn!("Auto-compaction failed: {}", e);
                    self.emit(SessionEvent::CompactionEnd {
                        reason: CompactionReason::Threshold,
                        result: None,
                        aborted: false,
                        will_retry: false,
                        error_message: Some(format!("Auto-compaction failed: {}", e)),
                    });
                }
            }
        }
    }

    /// Internal compaction execution.
    async fn run_compaction(&self, _custom_instructions: Option<String>) -> Result<CompactionResult> {
        let state = self.agent.state();
        let messages = state.messages.clone();

        if messages.len() < 3 {
            anyhow::bail!("Nothing to compact (session too small)");
        }

        // Use the agent's built-in compaction manager
        let compacted = self
            .agent
            .compaction_manager()
            .compact_if_needed(&messages, None, state.estimate_tokens(), state.iteration)
            .await
            .context("Compaction failed")?;

        match compacted {
            Some(ctx) => {
                let tokens_before = state.estimate_tokens();
                let compacted_count = ctx.compacted_count;

                // Replace messages in agent state
                self.agent.state().replace_messages(ctx.kept_messages.clone());

                // Persist to session
                self.persist_session();

                Ok(CompactionResult {
                    summary: ctx.summary.clone(),
                    first_kept_entry_id: None,
                    tokens_before,
                    details: Some(serde_json::json!({
                        "compacted_count": compacted_count,
                        "summary_length": ctx.summary.len(),
                    })),
                })
            }
            None => {
                anyhow::bail!("Nothing to compact");
            }
        }
    }

    /// Abort in-progress compaction.
    pub async fn abort_compaction(&self) {
        let mut guard = self.compaction_abort.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
        }
    }

    /// Enable or disable auto-compaction.
    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        self.compaction_config.write().enabled = enabled;
        self.settings.write().auto_compaction = enabled;
    }

    // ══════════════════════════════════════════════════════════════════
    // Auto-retry
    // ══════════════════════════════════════════════════════════════════

    /// Check if the last response indicates a retryable error and retry if so.
    async fn check_auto_retry(&self, events: &[AgentEvent]) -> bool {
        let retry_settings = self.retry_settings.read().clone();
        if !retry_settings.enabled {
            return false;
        }

        // Look for error events
        let has_error = events.iter().any(|e| matches!(e, AgentEvent::Error { .. }));
        if !has_error {
            // Reset retry counter on success
            let mut attempt = self.retry_attempt.write();
            if *attempt > 0 {
                self.emit(SessionEvent::AutoRetryEnd {
                    success: true,
                    attempt: *attempt,
                    final_error: None,
                });
                *attempt = 0;
            }
            return false;
        }

        // Extract error message
        let error_msg = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::Error { message } => Some(message.clone()),
                _ => None,
            })
            .unwrap_or_default();

        // Check if error is retryable
        let retryable = is_retryable_error(&error_msg);
        if !retryable {
            return false;
        }

        let mut attempt = self.retry_attempt.write();
        *attempt += 1;

        if *attempt > retry_settings.max_retries {
            self.emit(SessionEvent::AutoRetryEnd {
                success: false,
                attempt: *attempt - 1,
                final_error: Some(error_msg),
            });
            *attempt = 0;
            return false;
        }

        let current_attempt = *attempt;
        let delay_ms = retry_settings.base_delay_ms * 2u64.pow(current_attempt - 1);

        self.emit(SessionEvent::AutoRetryStart {
            attempt: current_attempt,
            max_attempts: retry_settings.max_retries,
            delay_ms,
            error_message: error_msg.clone(),
        });

        // Wait with exponential backoff
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

        tracing::info!(
            "Auto-retry attempt {}/{}",
            current_attempt,
            retry_settings.max_retries
        );

        true
    }

    /// Abort in-progress retry.
    pub fn abort_retry(&self) {
        *self.retry_attempt.write() = 0;
        self.retry_notify.notify_one();
    }

    /// Wait for any in-progress retry to complete.
    pub async fn wait_for_retry(&self) {
        if *self.retry_attempt.read() > 0 {
            self.retry_notify.notified().await;
        }
    }

    /// Enable or disable auto-retry.
    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        self.retry_settings.write().enabled = enabled;
    }

    // ══════════════════════════════════════════════════════════════════
    // Session persistence
    // ══════════════════════════════════════════════════════════════════

    /// Persist the current agent state to the session manager.
    fn persist_session(&self) {
        let state = self.agent.state();
        let mut sm = self.session_manager.write();

        for msg in &state.messages {
            match msg {
                Message::User(u) => {
                    let content = match &u.content {
                        oxi_ai::MessageContent::Text(t) => t.clone(),
                        oxi_ai::MessageContent::Blocks(blocks) => {
                            blocks
                                .iter()
                                .filter_map(|b| b.as_text())
                                .collect::<Vec<_>>()
                                .join("")
                        }
                    };
                    sm.append_message(AgentMessage::User {
                        content: crate::session::ContentValue::String(content),
                    });
                }
                Message::Assistant(a) => {
                    let content_str = a
                        .content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("");
                    sm.append_message(AgentMessage::Assistant {
                        content: vec![],
                        provider: None,
                        model_id: None,
                        usage: None,
                        stop_reason: None,
                    });
                    let _ = content_str; // will be used when AssistantMessage
                    // variant supports storing text content
                }
                Message::ToolResult(t) => {
                    let content = t
                        .content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("");
                    sm.append_message(AgentMessage::ToolResult {
                        content: crate::session::ContentValue::String(content),
                        tool_call_id: t.tool_call_id.clone(),
                    });
                }
            }
        }
    }

    /// Process a batch of agent events for session concerns.
    async fn process_events(&self, events: Vec<AgentEvent>) -> Result<()> {
        // Forward all events to listeners and extensions
        for event in &events {
            self.emit(SessionEvent::Agent(event.clone()));

            // Forward to extension runner for typed hooks
            let guard = self.extension_runner.read();
            if let Some(runner) = guard.as_ref() {
                runner.registry().emit_event(event);

                // Dispatch typed hooks
                match event {
                    AgentEvent::ToolCall { tool_call } => {
                        runner.emit_tool_call(&tool_call.name, &tool_call.arguments);
                    }
                    AgentEvent::ToolExecutionStart { tool_name, args, .. } => {
                        runner.emit_tool_call(tool_name, args);
                    }
                    AgentEvent::ToolExecutionEnd { tool_name, result, .. } => {
                        let tool_result = oxi_agent::AgentToolResult::success(&result.content);
                        runner.emit_tool_result_event(tool_name, &tool_result);
                    }
                    AgentEvent::Error { message } => {
                        let err = anyhow::anyhow!("{}", message);
                        runner.registry().emit_error(&err);
                    }
                    _ => {}
                }
            }
        }

        // Check for retryable errors
        let will_retry = self.check_auto_retry(&events).await;
        if will_retry {
            // Retry was initiated; the retry will produce new events
            return Ok(());
        }

        // Check auto-compaction after successful completion
        let has_complete = events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::AgentEnd { .. } | AgentEvent::Complete { .. }
            )
        });
        if has_complete {
            self.check_auto_compaction().await;

            // Process follow-up queue if any
            let follow_ups: Vec<String> = self.follow_up_messages.write().drain(..).collect();
            if !follow_ups.is_empty() {
                self.emit_queue_update();
                // Submit follow-ups as new prompts
                for msg in follow_ups {
                    let _ = self.agent.run(msg).await;
                }
            }
        }

        // Persist to session
        self.persist_session();

        Ok(())
    }

    // ══════════════════════════════════════════════════════════════════
    // Session management
    // ══════════════════════════════════════════════════════════════════

    /// Set a display name for the current session.
    pub fn set_session_name(&self, name: String) {
        let mut sm = self.session_manager.write();
        sm.append_session_info(&name);
        self.emit(SessionEvent::SessionInfoChanged {
            name: Some(name),
        });
    }

    /// Reset the agent state for a new conversation.
    pub fn reset(&self) {
        self.agent.reset();
        *self.overflow_recovery_attempted.write() = false;
        self.clear_queue();
    }

    /// Get a reference to the underlying [`Agent`].
    ///
    /// Use this when you need direct agent access (e.g., `run_with_channel`).
    pub fn agent_ref(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    /// Get a cheap cloneable handle that references the same underlying session.
    pub fn clone_handle(&self) -> AgentSessionHandle {
        AgentSessionHandle {
            inner: Arc::new(self.clone_inner()),
        }
    }

    // Internal: produce a Self with the same arcs (doesn't actually clone Agent).
    fn clone_inner(&self) -> Self {
        Self {
            agent: Arc::clone(&self.agent),
            settings: Arc::clone(&self.settings),
            session_manager: Arc::clone(&self.session_manager),
            listeners: Arc::clone(&self.listeners),
            event_tx: self.event_tx.clone(),
            scoped_models: Arc::clone(&self.scoped_models),
            steering_messages: Arc::clone(&self.steering_messages),
            follow_up_messages: Arc::clone(&self.follow_up_messages),
            compaction_config: Arc::clone(&self.compaction_config),
            compaction_abort: Arc::clone(&self.compaction_abort),
            overflow_recovery_attempted: Arc::clone(&self.overflow_recovery_attempted),
            retry_settings: Arc::clone(&self.retry_settings),
            retry_attempt: Arc::clone(&self.retry_attempt),
            retry_abort: Arc::clone(&self.retry_abort),
            retry_notify: Arc::clone(&self.retry_notify),
            session_id: Arc::clone(&self.session_id),
            cwd: self.cwd.clone(),
            extension_runner: Arc::clone(&self.extension_runner),
        }
    }

    // ══════════════════════════════════════════════════════════════════
    // Extension integration
    // ══════════════════════════════════════════════════════════════════

    /// Set or replace the [`ExtensionRunner`] used by this session.
    ///
    /// This is called by the runtime after CLI parsing to inject the
    /// extension runner. If a runner was already set, its extensions are
    /// unloaded first via `emit_session_shutdown`.
    pub fn set_extension_runner(&self, runner: ExtensionRunner) {
        // If there is an existing runner, notify its extensions about shutdown
        {
            let guard = self.extension_runner.read();
            if let Some(existing) = guard.as_ref() {
                let session_id = self.session_id();
                let shutdown_event = SessionShutdownEvent {
                    reason: SessionShutdownReason::Reload,
                    target_session_file: None,
                };
                existing.emit_session_shutdown_event(&shutdown_event);
                existing.registry().emit_session_end(&session_id);
                existing.registry().emit_unload();
            }
        }

        // Install the new runner
        {
            let mut guard = self.extension_runner.write();
            *guard = Some(runner);
        }

        // Fire lifecycle hooks on the new runner
        {
            let guard = self.extension_runner.read();
            if let Some(runner) = guard.as_ref() {
                let ctx = self.build_extension_context();
                runner.registry().emit_load(&ctx);
                let session_id = self.session_id();
                runner.registry().emit_session_start(&session_id);
            }
        }

        tracing::debug!("ExtensionRunner installed into AgentSession");
    }

    /// Get a reference to the current [`ExtensionRunner`], if any.
    pub fn extension_runner(&self) -> parking_lot::RwLockReadGuard<'_, Option<ExtensionRunner>> {
        self.extension_runner.read()
    }

    /// Take the [`ExtensionRunner`] out of this session, shutting down extensions first.
    pub fn take_extension_runner(&self) -> Option<ExtensionRunner> {
        {
            let guard = self.extension_runner.read();
            if let Some(runner) = guard.as_ref() {
                let session_id = self.session_id();
                let shutdown_event = SessionShutdownEvent {
                    reason: SessionShutdownReason::Quit,
                    target_session_file: None,
                };
                runner.emit_session_shutdown_event(&shutdown_event);
                runner.registry().emit_session_end(&session_id);
                runner.registry().emit_unload();
            }
        }
        self.extension_runner.write().take()
    }

    /// Build an [`ExtensionContext`] for the current session state.
    ///
    /// The context provides extensions with access to settings, tools,
    /// session state, and other host capabilities.
    pub fn build_extension_context(&self) -> ExtensionContext {
        ExtensionContextBuilder::new(PathBuf::from(&self.cwd))
            .settings(Arc::clone(&self.settings))
            .session_id(self.session_id())
            .build()
    }

    /// Forward an agent event to the extension system.
    ///
    /// If an [`ExtensionRunner`] is installed, the event is broadcast to
    /// all enabled extensions. The event is *also* emitted as a
    /// [`SessionEvent::Agent`] to regular session listeners.
    pub fn forward_event_to_extensions(&self, event: &AgentEvent) {
        // Always emit to session listeners
        self.emit(SessionEvent::Agent(event.clone()));

        // Forward to extension runner if installed
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.registry().emit_event(event);

            // Dispatch to typed hooks based on event variant
            match event {
                AgentEvent::ToolCall { tool_call } => {
                    runner.emit_tool_call(&tool_call.name, &tool_call.arguments);
                }
                AgentEvent::ToolExecutionStart { tool_name, args, .. } => {
                    runner.emit_tool_call(tool_name, args);
                }
                AgentEvent::ToolExecutionEnd { tool_name, result, .. } => {
                    let tool_result = oxi_agent::AgentToolResult::success(&result.content);
                    runner.emit_tool_result_event(tool_name, &tool_result);
                }
                _ => {}
            }
        }
    }

    /// Check if extension handlers are registered for an event type.
    pub fn has_extension_handlers(&self, event_type: &str) -> bool {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.has_handlers(event_type)
        } else {
            false
        }
    }

    /// Collect all tools contributed by extensions.
    ///
    /// Returns an empty vector when no extension runner is installed.
    pub fn extension_tools(&self) -> Vec<Arc<dyn oxi_agent::AgentTool>> {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.all_tools()
        } else {
            Vec::new()
        }
    }

    /// Collect all commands contributed by extensions.
    ///
    /// Returns an empty vector when no extension runner is installed.
    pub fn extension_commands(&self) -> Vec<crate::extensions::Command> {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.all_commands()
        } else {
            Vec::new()
        }
    }

    /// Emit a before-tool-call event to extensions.
    ///
    /// Extensions may block the tool call by returning an error.
    /// Returns the [`ToolCallEmitResult`] with blocking status.
    pub fn emit_before_tool_call(
        &self,
        tool_name: &str,
        params: &serde_json::Value,
    ) -> crate::extensions::ToolCallEmitResult {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.emit_tool_call(tool_name, params)
        } else {
            crate::extensions::ToolCallEmitResult::default()
        }
    }

    /// Emit an after-tool-result event to extensions.
    ///
    /// Extensions can inspect and log tool results.
    pub fn emit_after_tool_result(
        &self,
        tool_name: &str,
        result: &oxi_agent::AgentToolResult,
    ) -> crate::extensions::ToolResultEmitResult {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.emit_tool_result_event(tool_name, result)
        } else {
            crate::extensions::ToolResultEmitResult::default()
        }
    }

    /// Process user input through extension hooks before agent processing.
    ///
    /// Extensions may transform or handle the input. Returns the final
    /// [`InputEventResult`](ExtInputEventResult).
    pub fn process_input_through_extensions(
        &self,
        text: &str,
        source: InputSource,
    ) -> ExtInputEventResult {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            let ext_source = match source {
                InputSource::Interactive => crate::extensions::InputSource::Interactive,
                InputSource::Extension => crate::extensions::InputSource::Extension,
                InputSource::Rpc => crate::extensions::InputSource::Rpc,
            };
            let mut event = ExtInputEvent {
                text: text.to_string(),
                source: ext_source,
            };
            runner.emit_input_event(&mut event)
        } else {
            ExtInputEventResult::Continue
        }
    }

    /// Notify extensions that a message was sent.
    pub fn notify_extensions_message_sent(&self, msg: &str) {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.registry().emit_message_sent(msg);
        }
    }

    /// Notify extensions that a message was received.
    pub fn notify_extensions_message_received(&self, msg: &str) {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            runner.registry().emit_message_received(msg);
        }
    }

    /// Notify extensions that settings have changed.
    pub fn notify_extensions_settings_changed(&self) {
        let guard = self.extension_runner.read();
        if let Some(runner) = guard.as_ref() {
            let settings = self.settings.read().clone();
            runner.registry().emit_settings_changed(&settings);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Listener guard
// ═══════════════════════════════════════════════════════════════════════════

/// RAII guard that removes a session event listener when dropped.
pub struct SessionListenerGuard {
    listeners: Arc<RwLock<Vec<Box<dyn Fn(&SessionEvent) + Send + Sync>>>>,
    key: usize,
}

impl Drop for SessionListenerGuard {
    fn drop(&mut self) {
        let mut listeners = self.listeners.write();
        if self.key < listeners.len() {
            // Replace with a no-op
            listeners[self.key] = Box::new(|_| {});
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Handle (cheap clone)
// ═══════════════════════════════════════════════════════════════════════════

/// A cheaply-clonable handle to an [`AgentSession`].
///
/// Use this when you need to share the session across tasks / threads.
#[derive(Clone)]
pub struct AgentSessionHandle {
    inner: Arc<AgentSession>,
}

impl std::ops::Deref for AgentSessionHandle {
    type Target = AgentSession;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Cycling direction
// ═══════════════════════════════════════════════════════════════════════════

/// Direction for model cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleDirection {
    Forward,
    Backward,
}

impl Default for CycleDirection {
    fn default() -> Self {
        Self::Forward
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Default list of popular models for cycling when no scoped models are set.
fn default_model_list() -> Vec<(&'static str, &'static str)> {
    vec![
        ("anthropic", "claude-sonnet-4-20250514"),
        ("anthropic", "claude-haiku-4-20250414"),
        ("openai", "gpt-4o"),
        ("openai", "gpt-4o-mini"),
        ("google", "gemini-2.0-flash"),
    ]
}

/// Check if an error message indicates a retryable condition.
fn is_retryable_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    // Context overflow is NOT retryable (handled by compaction)
    if lower.contains("context") && lower.contains("overflow") {
        return false;
    }
    // Match common retryable patterns
    lower.contains("overloaded")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("429")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("service unavailable")
        || lower.contains("server error")
        || lower.contains("internal error")
        || lower.contains("network error")
        || lower.contains("connection error")
        || lower.contains("connection refused")
        || lower.contains("fetch failed")
        || lower.contains("timeout")
        || lower.contains("timed out")
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_error() {
        assert!(is_retryable_error("Error 429: Too many requests"));
        assert!(is_retryable_error("server error 503"));
        assert!(is_retryable_error("overloaded_error"));
        assert!(is_retryable_error("connection refused"));
        assert!(is_retryable_error("fetch failed"));
        assert!(is_retryable_error("request timed out"));

        // Context overflow is NOT retryable
        assert!(!is_retryable_error("context overflow"));
        assert!(!is_retryable_error("Context length exceeded"));

        // Normal errors are not retryable
        assert!(!is_retryable_error("Invalid tool call"));
        assert!(!is_retryable_error("Unknown command"));
    }

    #[test]
    fn test_default_model_list() {
        let models = default_model_list();
        assert!(!models.is_empty());
        assert!(
            models
                .iter()
                .any(|(p, m)| *p == "anthropic" && *m == "claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn test_session_retry_settings_default() {
        let settings = Settings::default();
        let retry = SessionRetrySettings::from_settings(&settings);
        assert!(retry.enabled);
        assert_eq!(retry.max_retries, 3);
        assert_eq!(retry.base_delay_ms, 1000);
    }

    #[test]
    fn test_cycle_direction_default() {
        assert_eq!(CycleDirection::default(), CycleDirection::Forward);
    }

    #[test]
    fn test_thinking_level_ordering() {
        let levels = [
            ThinkingLevel::None,
            ThinkingLevel::Minimal,
            ThinkingLevel::Standard,
            ThinkingLevel::Thorough,
        ];
        // Ensure we can cycle through all levels
        let mut current = 0;
        for _ in 0..levels.len() {
            current = (current + 1) % levels.len();
        }
        assert_eq!(current, 0); // Wraps back to start
    }

    #[test]
    fn test_scoped_model() {
        let model = ScopedModel {
            provider: "anthropic".to_string(),
            model_id: "claude-sonnet-4-20250514".to_string(),
            thinking_level: Some(ThinkingLevel::Standard),
        };
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.model_id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_compaction_reason() {
        assert_eq!(CompactionReason::Manual, CompactionReason::Manual);
        assert_ne!(CompactionReason::Manual, CompactionReason::Threshold);
        assert_ne!(CompactionReason::Threshold, CompactionReason::Overflow);
    }

    #[test]
    fn test_model_cycle_result() {
        let result = ModelCycleResult {
            provider: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            thinking_level: ThinkingLevel::Standard,
            is_scoped: false,
        };
        assert!(!result.is_scoped);
    }

    #[test]
    fn test_session_stats_default() {
        let stats = SessionStats {
            session_id: "test".to_string(),
            user_messages: 0,
            assistant_messages: 0,
            tool_calls: 0,
            tool_results: 0,
            total_messages: 0,
            tokens: TokenStats::default(),
            cost: 0.0,
        };
        assert_eq!(stats.total_messages, 0);
    }

    #[test]
    fn test_streaming_behavior() {
        assert_eq!(StreamingBehavior::Steer, StreamingBehavior::Steer);
        assert_ne!(StreamingBehavior::Steer, StreamingBehavior::FollowUp);
    }

    #[test]
    fn test_input_source_default() {
        assert_eq!(InputSource::default(), InputSource::Interactive);
    }

    #[test]
    fn test_prompt_options_default() {
        let opts = PromptOptions::default();
        assert!(opts.expand_templates);
        assert!(opts.images.is_empty());
        assert!(opts.streaming_behavior.is_none());
        assert_eq!(opts.source, InputSource::Interactive);
    }
}
