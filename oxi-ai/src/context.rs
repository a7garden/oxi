//! Conversation context management

use super::{Message, Tool};
use serde::{Deserialize, Serialize};

/// Conversation context for LLM interactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    /// System prompt sent with each request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Conversation history
    pub messages: Vec<Message>,

    /// Available tools for this context
    #[serde(default)]
    pub tools: Vec<Tool>,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    /// Create a new empty context
    ///
    /// # Examples
    ///
    /// ```
    /// use oxi_ai::Context;
    /// let mut ctx = Context::new();
    /// ctx.set_system_prompt("You are a helpful assistant.");
    /// ```
    pub fn new() -> Self {
        Self {
            system_prompt: None,
            messages: Vec::new(),
            tools: Vec::new(),
        }
    }

    /// Create a context with a system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Add a message to the context
    ///
    /// # Examples
    ///
    /// ```
    /// use oxi_ai::{Context, Message, UserMessage};
    /// let mut ctx = Context::new();
    /// ctx.add_message(Message::User(UserMessage::new("Hello!")));
    /// assert_eq!(ctx.len(), 1);
    /// ```
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Get a message by index
    pub fn message(&self, index: usize) -> Option<&Message> {
        self.messages.get(index)
    }

    /// Get the last message
    ///
    /// # Examples
    ///
    /// ```
    /// use oxi_ai::{Context, Message, UserMessage};
    /// let mut ctx = Context::new();
    /// ctx.add_message(Message::User(UserMessage::new("First")));
    /// ctx.add_message(Message::User(UserMessage::new("Second")));
    /// assert!(ctx.last_message().is_some());
    /// ```
    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Check if context has any messages
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get number of messages
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Set the system prompt
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    /// Clear the system prompt
    pub fn clear_system_prompt(&mut self) {
        self.system_prompt = None;
    }

    /// Set available tools
    ///
    /// # Examples
    ///
    /// ```
    /// use oxi_ai::{Context, Tool};
    /// let mut ctx = Context::new();
    /// let tool = Tool::new(
    ///     "search",
    ///     "Search the web",
    ///     serde_json::json!({"type": "object", "properties": {}}),
    /// );
    /// ctx.set_tools(vec![tool]);
    /// assert_eq!(ctx.tools.len(), 1);
    /// ```
    pub fn set_tools(&mut self, tools: Vec<Tool>) {
        self.tools = tools;
    }

    /// Add a tool
    pub fn add_tool(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    /// Get the system prompt for this context.
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Serialize context to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize a context from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Duplicate this context
    pub fn duplicate(&self) -> Self {
        Self {
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: self.tools.clone(),
        }
    }
}

impl From<Vec<Message>> for Context {
    fn from(messages: Vec<Message>) -> Self {
        Self {
            system_prompt: None,
            messages,
            tools: Vec::new(),
        }
    }
}

impl From<Message> for Context {
    fn from(message: Message) -> Self {
        Self {
            system_prompt: None,
            messages: vec![message],
            tools: Vec::new(),
        }
    }
}
