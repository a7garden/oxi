//! PagerEvent — normalized input from agent / user / tick / background.
//!
//! BackgroundEvent bridges oxi-agent's `AgentEvent` into pager domain types.

use crate::status::StatusState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Events sent from the background agent worker to the TUI.
#[derive(Debug, Clone)]
pub enum BackgroundEvent {
    /// Streaming text delta from assistant.
    AssistantDelta(String),
    /// Assistant finished current message.
    AssistantDone,
    /// Tool call initiated.
    ToolCall {
        id: String,
        name: String,
        params: String,
    },
    /// Tool call result.
    ToolResult {
        id: String,
        content: String,
    },
    /// User message from agent loop.
    UserMessage(String),
    /// Status bar update.
    StatusUpdate(StatusState),
    /// Agent finished streaming.
    StreamDone,
}

/// Resolved keyboard input.
#[derive(Debug, Clone)]
pub struct ResolvedKey {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl From<KeyEvent> for ResolvedKey {
    fn from(k: KeyEvent) -> Self {
        Self {
            code: k.code,
            modifiers: k.modifiers,
        }
    }
}

/// Top-level events routed to the reducer.
#[derive(Debug, Clone)]
pub enum PagerEvent {
    Agent(Box<oxi_agent::events::AgentEvent>),
    Input(ResolvedKey),
    Tick,
    Background(BackgroundEvent),
}
