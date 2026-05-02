//! oxi: CLI coding harness
//!
//! This crate provides the main application logic for the oxi CLI.

pub mod settings;

use anyhow::{Error, Result};
use oxi_agent::{Agent, AgentConfig, AgentEvent};
use oxi_ai::{get_model, get_provider};
use settings::{Settings, ThinkingLevel};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Application state and entry point
pub struct App {
    agent: Arc<Agent>,
    settings: Settings,
}

/// Chat message for display
#[derive(Debug, Clone)]
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
pub struct InteractiveSession {
    pub messages: Vec<ChatMessage>,
    pub thinking: bool,
    pub current_response: String,
}

impl Default for InteractiveSession {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            thinking: false,
            current_response: String::new(),
        }
    }
}

impl InteractiveSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(ChatMessage::user(content));
    }

    pub fn add_assistant_message(&mut self, content: String) {
        self.messages.push(ChatMessage::assistant(content));
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
}

/// Build the system prompt based on thinking level
fn build_system_prompt(thinking_level: ThinkingLevel) -> String {
    match thinking_level {
        ThinkingLevel::None => String::from(
            "You are a helpful AI assistant. Provide direct, concise answers.",
        ),
        ThinkingLevel::Minimal => String::from(
            "You are a helpful AI assistant. Provide clear and helpful answers.",
        ),
        ThinkingLevel::Standard => String::from(
            "You are a helpful AI coding assistant. Think through problems \
             step by step when helpful, but keep responses focused and actionable.",
        ),
        ThinkingLevel::Thorough => String::from(
            "You are an expert AI coding assistant. Take time to thoroughly \
             analyze problems, consider edge cases, and provide comprehensive \
             solutions with explanations. Think deeply before responding.",
        ),
    }
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

        // Build agent config
        let system_prompt = build_system_prompt(settings.thinking_level);
        let config = AgentConfig {
            name: "oxi".to_string(),
            description: Some("oxi CLI agent".to_string()),
            model_id: model_id.clone(),
            system_prompt: Some(system_prompt),
            max_iterations: 10,
            timeout_seconds: 300,
            temperature: None,
            max_tokens: None,
        };

        let agent = Arc::new(Agent::new(Arc::from(provider), config));

        Ok(Self { agent, settings })
    }

    /// Get the current settings
    pub fn settings(&self) -> &Settings {
        &self.settings
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
    pub async fn run_interactive(&self) -> Result<InteractiveLoop> {
        let session = InteractiveSession::new();
        Ok(InteractiveLoop {
            app: self,
            session,
        })
    }

    /// Reset the conversation
    pub fn reset(&self) {
        self.agent.reset();
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
        
        // Spawn the agent
        let agent = Arc::clone(&self.app.agent);
        let handle = tokio::spawn(async move {
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
                    self.session.append_to_response(&format!("[Error: {}]", message));
                    self.session.finish_response();
                    self.session.thinking = false;
                }
                _ => {}
            }
        }

        // Wait for agent to finish
        let _ = handle.await;

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
}