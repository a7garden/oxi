/// Core agent implementation

use crate::config::AgentConfig;
use crate::config::ShouldStopAfterTurnContext;
use crate::events::AgentEvent;
use crate::state::{AgentState, SharedState};
use crate::tools::{AgentTool, ToolRegistry};
use crate::types::{Response, StopReason};
use anyhow::{Error, Result};
use oxi_ai::{
    transform_for_provider, CompactionManager, CompactionStrategy,
    LlmCompactor, Provider,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Mutable agent internals protected by a read-write lock.

struct AgentInner {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
}

/// Agent runtime.
///
/// Manages provider, tool registry, state, and compaction, providing an
/// agentic loop for prompt execution, model switching, tool calls, and fallback.
pub struct Agent {
    inner: RwLock<AgentInner>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
    hooks: parking_lot::RwLock<crate::config::AgentHooks>,
    /// Guard: true while a run is in progress. Prevents concurrent runs.
    is_running: AtomicBool,
}

impl Agent {
    /// Create a new agent with the given provider, config, and tool registry.
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig, tools: Arc<ToolRegistry>) -> Self {
        let mut compaction_manager =
            CompactionManager::new(config.compaction_strategy.clone(), config.context_window);

        // Pre-initialize the LLM compactor if compaction is enabled
        if config.compaction_strategy != CompactionStrategy::Disabled {
            let model = crate::model_id::resolve_model_from_id(&config.model_id);

            if let Some(model) = model {
                let llm_compactor =
                    Arc::new(LlmCompactor::new(model.clone(), Arc::clone(&provider)));
                compaction_manager.set_compactor(llm_compactor);
            }
        }

        Self {
            inner: RwLock::new(AgentInner { config, provider }),
            tools,
            state: SharedState::new(),
            compaction_manager,
            hooks: parking_lot::RwLock::new(crate::config::AgentHooks::default()),
            is_running: AtomicBool::new(false),
        }
    }

    /// Create an agent with an empty tool registry.
    pub fn new_empty(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
        Self::new(provider, config, Arc::new(ToolRegistry::new()))
    }

    /// Get the agent configuration (read guard)
    fn config(&self) -> parking_lot::RwLockReadGuard<'_, AgentInner> {
        self.inner.read()
    }

    /// Get a write guard for the agent inner state
    fn inner_mut(&self) -> parking_lot::RwLockWriteGuard<'_, AgentInner> {
        self.inner.write()
    }

    /// Get the current model ID
    pub fn model_id(&self) -> String {
        self.config().config.model_id.clone()
    }

    /// Switch the model used for future LLM calls.
    ///
    /// If the new model uses a different provider API, the conversation
    /// history is automatically transformed for cross-provider compatibility
    /// (e.g. thinking blocks are converted to `<thinking>` tags).
    ///
    /// # Arguments
    /// * `model_id` - New model ID in `provider/model` format
    ///
    /// # Returns
    /// `Ok(())` on success, or an error if the model/provider is unknown
    pub fn switch_model(&self, model_id: &str) -> Result<()> {
        let new_model = crate::model_id::resolve_model_from_id(model_id)
            .ok_or_else(|| Error::msg(format!("Model '{}' not found", model_id)))?;

        // Create the new provider
        let new_provider = oxi_ai::get_provider(&new_model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", new_model.provider)))?;

        // Detect API change and transform messages if needed
        {
            let inner = self.config();
            let old_model_id = &inner.config.model_id;
            let old_api = crate::model_id::resolve_model_from_id(old_model_id)
                .map(|m| m.api)
                .unwrap_or(oxi_ai::Api::AnthropicMessages);

            if old_api != new_model.api {
                // Transform existing messages for the new provider
                let messages = self.state.get_state().messages.clone();
                let transformed = transform_for_provider(&messages, &old_api, &new_model.api);
                self.state.update(|s| {
                    s.replace_messages(transformed);
                });
            }
        }

        // Update config and provider atomically
        let mut inner = self.inner_mut();
        inner.config.model_id = model_id.to_string();
        inner.provider = Arc::from(new_provider);

        Ok(())
    }

    /// Switch the model using a pre-resolved `Model` object.
    ///
    /// This is useful when the caller has already looked up the model
    /// and optionally created the provider.
    pub fn switch_to_model(&self, model: &oxi_ai::Model) -> Result<()> {
        let model_id = format!("{}/{}", model.provider, model.id);
        let new_provider = oxi_ai::get_provider(&model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", model.provider)))?;

        // Detect API change and transform messages if needed
        {
            let inner = self.config();
            let old_api = crate::model_id::resolve_model_from_id(&inner.config.model_id)
                .map(|m| m.api)
                .unwrap_or(oxi_ai::Api::AnthropicMessages);

            if old_api != model.api {
                let messages = self.state.get_state().messages.clone();
                let transformed = transform_for_provider(&messages, &old_api, &model.api);
                self.state.update(|s| {
                    s.replace_messages(transformed);
                });
            }
        }

        let mut inner = self.inner_mut();
        inner.config.model_id = model_id;
        inner.provider = Arc::from(new_provider);

        Ok(())
    }

    /// Get a handle to the tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    /// Get a snapshot of the current agent state.
    pub fn state(&self) -> AgentState {
        self.state.get_state()
    }

    /// Reset agent state for a new conversation
    pub fn reset(&self) {
        self.state.reset();
    }

    /// Register a tool that the agent can invoke during a run.
    pub fn add_tool<T: AgentTool + 'static>(&self, tool: T) {
        self.tools.register(tool);
    }

    /// Update the system prompt for future interactions.
    pub fn set_system_prompt(&self, prompt: String) {
        self.inner_mut().config.system_prompt = Some(prompt);
    }

    /// Get the compaction manager
    pub fn compaction_manager(&self) -> &CompactionManager {
        &self.compaction_manager
    }

    /// Run the agent with a prompt, collecting all events into a vector.
    ///
    /// Convenience wrapper around [`run_with_channel`] that gathers every
    /// [`AgentEvent`] produced during the run.
    pub async fn run(&self, prompt: String) -> Result<(Response, Vec<AgentEvent>)> {
        let mut events = Vec::new();
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let result = self.run_with_channel(prompt, tx).await;
        while let Ok(event) = rx.recv() {
            events.push(event);
        }
        result.map(|r| (r, events))
    }

    /// Run the agent, delivering events through the provided channel.
    ///
    /// Delegates to [`AgentLoop`] which implements the same 2-level agentic
    /// loop matching pi-mono's architecture:
    ///
    /// ```text
    /// AgentLoop.run_messages()
    ///   Outer loop (follow-up messages):
    ///     Inner loop (tool calls + steering):
    ///       1. Inject pending messages (steering)
    ///       2. Compaction check
    ///       3. Stream LLM response (with accumulated partial messages)
    ///       4. Execute tool calls if any
    ///       5. Emit turn_end
    ///       6. Check shouldStopAfterTurn
    ///       7. Poll steering messages
    ///     Check follow-up messages
    ///     Exit
    /// ```
    pub async fn run_with_channel(
        &self,
        prompt: String,
        tx: std::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        // pi-mono: Agent.prompt() throws if activeRun exists.
        // Prevent concurrent runs that would corrupt shared state.
        if self.is_running.compare_exchange(
            false, true,
            Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
            return Err(Error::msg("Agent is already running"));
        }

        let result = self.run_with_channel_inner(prompt, tx).await;

        // Always clear the running flag
        self.is_running.store(false, Ordering::SeqCst);
        result
    }

    /// Inner implementation of run_with_channel, called after the running guard is set.
    async fn run_with_channel_inner(
        &self,
        prompt: String,
        tx: std::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        use crate::agent_loop::AgentLoop;

        let inner = self.inner.read();
        let provider: Arc<dyn Provider> = Arc::clone(&inner.provider);
        let max_iterations = inner.config.max_iterations;
        let system_prompt = inner.config.system_prompt.clone();
        let temperature = inner.config.temperature;
        let max_tokens = inner.config.max_tokens;
        let compaction_strategy = inner.config.compaction_strategy.clone();
        let context_window = inner.config.context_window;
        let api_key = inner.config.api_key.clone();
        let workspace_dir = inner.config.workspace_dir.clone();
        drop(inner); // release read lock

        // Build AgentLoopConfig from Agent's config
        let loop_config = crate::agent_loop::config::AgentLoopConfig {
            model_id: self.model_id(),
            system_prompt,
            max_iterations,
            temperature: temperature.unwrap_or(1.0) as f32,
            max_tokens: max_tokens.unwrap_or(4096) as u32,
            tool_execution: crate::config::ToolExecutionMode::Sequential,
            compaction_strategy,
            compaction_instruction: None,
            context_window,
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            auto_retry_enabled: true,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 1000,
            api_key,
            workspace_dir,
        };

        // Create AgentLoop. We give it a NEW SharedState and sync back after.
        // (SharedState is not Clone, so we create a fresh one from current state)
        let fresh_state = crate::state::SharedState::new();
        let current = self.state.get_state();
        fresh_state.update(|s| {
            *s = current;
        });

        let agent_loop = AgentLoop::new(
            provider,
            loop_config,
            Arc::clone(&self.tools),
            fresh_state,
        );

        // Pre-populate steering/follow-up from hooks
        let hooks = self.hooks.read();
        let al = agent_loop;

        if let Some(ref get_steering) = hooks.get_steering_messages {
            for msg_text in get_steering() {
                al.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new(msg_text)));
            }
        }
        if let Some(ref get_follow_up) = hooks.get_follow_up_messages {
            for msg_text in get_follow_up() {
                al.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(msg_text)));
            }
        }

        // Wire should_stop_after_turn hook: share AgentLoop's external_stop
        // Arc with the emit callback. When the hook fires (Ctrl+C detected),
        // it sets ext_stop. AgentLoop checks this in should_stop_after_turn().
        //
        // Arc<dyn Fn> can be cloned, so we read it without consuming.
        let maybe_hook = {
            drop(hooks);
            let hooks_r = self.hooks.read();
            hooks_r.should_stop_after_turn.clone()
        };
        let ext_stop = al.external_stop().clone();

        // Create emit callback that sends through the channel.
        // AgentLoop calls this synchronously. UnboundedSender::send() is
        // non-blocking and never drops events (unlike try_send on bounded).
        let tx_emit = tx.clone();

        // Run the agent loop
        tracing::info!("[AGENT] Starting agent run with channel");
        let result = al.run(prompt.clone(), move |event: AgentEvent| {
            // Forward event to channel (std::sync::mpsc — send from sync context)
            tracing::info!("[AGENT-EMIT] Event: {:?}", std::mem::discriminant(&event));
            if let Err(e) = tx_emit.send(event.clone()) {
                tracing::error!("[AGENT-EMIT] Failed to send agent event to channel: {:?}", e);
            } else {
                tracing::info!("[AGENT-EMIT] Successfully sent event");
            }

            // On TurnEnd, poll the should_stop_after_turn hook to detect Ctrl+C.
            // The hook wraps an AtomicBool (should_stop_flag from AgentSession).
            // We can't pass real context here, but the TUI hook only checks
            // the AtomicBool anyway: |ctx| should_stop_flag.load(SeqCst).
            if let Some(ref hook) = maybe_hook {
                if let AgentEvent::TurnEnd { ref assistant_message, ref tool_results, .. } = event {
                    // Build real context from actual turn data
                    let asst = match assistant_message {
                        oxi_ai::Message::Assistant(a) => a.clone(),
                        _ => {
                            // Can't extract assistant message, just check the hook with empty ctx
                            let ctx = ShouldStopAfterTurnContext {
                                message: oxi_ai::AssistantMessage::new(
                                    oxi_ai::Api::OpenAiCompletions, "agent", "agent-model",
                                ),
                                tool_results: Vec::new(),
                                iteration: 0,
                            };
                            if hook(&ctx) {
                                ext_stop.store(true, Ordering::SeqCst);
                            }
                            return;
                        }
                    };
                    let ctx = ShouldStopAfterTurnContext {
                        message: asst,
                        tool_results: tool_results.clone(),
                        iteration: 0,
                    };
                    if hook(&ctx) {
                        ext_stop.store(true, Ordering::SeqCst);
                    }
                }
            }
        }).await;

        match result {
            Ok(_events) => {
                // Sync state back from AgentLoop
                let loop_state = al.state().get_state();
                self.state.update(|s| {
                    *s = loop_state;
                });

                // Extract final response text from state
                let state = self.state.get_state();
                let final_text = state.messages.iter().rev()
                    .find_map(|m| match m {
                        oxi_ai::Message::Assistant(a) => {
                            a.content.iter().find_map(|b| match b {
                                oxi_ai::ContentBlock::Text(t) => Some(t.text.clone()),
                                _ => None,
                            })
                        }
                        _ => None,
                    })
                    .unwrap_or_default();

                let stop_reason = state.stop_reason.unwrap_or(StopReason::Stop);

                Ok(Response {
                    content: final_text,
                    stop_reason,
                })
            }
            Err(e) => Err(e),
        }
    }

    // ── Helper methods for the agentic loop ────────────────────────

    /// Set hooks for the agent loop.
    pub fn set_hooks(&self, hooks: crate::config::AgentHooks) {
        let mut h = self.hooks.write();
        *h = hooks;
    }

    /// Run the agent, invoking `on_event` for each [`AgentEvent`] produced.
    ///
    /// Blocking convenience wrapper suitable for callers that prefer a
    /// callback-based API over a channel.
    pub async fn run_streaming<F>(&self, prompt: String, mut on_event: F) -> Result<Response>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let result = self.run_with_channel(prompt, tx).await;
        while let Ok(event) = rx.recv() {
            on_event(event);
        }
        result
    }

}
