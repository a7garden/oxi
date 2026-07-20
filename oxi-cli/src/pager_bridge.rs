//! AgentSession → oxi-pager bridge.
//!
//! Subscribes to AgentSession events, converts them to pager BackgroundEvents,
//! and runs the pager event loop.

use crate::app::agent_session::{AgentSession, SessionEvent};
use oxi_agent::AgentEvent;
use oxi_pager::{BackgroundEvent, run as run_pager};
use tokio::sync::mpsc;

/// Run the grok-quality TUI pager with an AgentSession backend.
pub async fn run_pager_with_session(
    mut session: AgentSession,
    initial_message: Option<String>,
) -> anyhow::Result<()> {
    let (bg_tx, bg_rx) = mpsc::unbounded_channel::<BackgroundEvent>();

    // Clone tx for the callback
    let tx = bg_tx.clone();

    // Subscribe to session events — forward to pager channel
    let _guard = session.subscribe(Box::new(move |event: &SessionEvent| {
        if let SessionEvent::Agent(agent_event) = event {
            let bg = agent_to_background(agent_event);
            if bg.is_some() {
                let _ = tx.send(bg);
            }
        }
    }));

    // Send initial user message if provided
    if let Some(msg) = initial_message {
        session.submit(&msg).await?;
    }

    // Run the pager event loop (blocks until user quits)
    run_pager(session, bg_rx).await
}

/// Convert an AgentEvent to a BackgroundEvent for the pager.
fn agent_to_background(event: &AgentEvent) -> Option<BackgroundEvent> {
    match event {
        AgentEvent::MessageUpdate {
            message: _,
            delta,
        } => {
            if let Some(text) = delta {
                Some(BackgroundEvent::AssistantDelta(text.clone()))
            } else {
                None
            }
        }
        AgentEvent::MessageStart { message } => {
            if message.role == oxi_ai::Role::User {
                Some(BackgroundEvent::UserMessage(message.content_text()))
            } else {
                None
            }
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
            ..
        } => Some(BackgroundEvent::ToolCall {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
            params: serde_json::to_string_pretty(args).unwrap_or_default(),
        }),
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name: _,
            result,
            ..
        } => Some(BackgroundEvent::ToolResult {
            id: tool_call_id.clone(),
            content: result.clone().unwrap_or_default(),
        }),
        _ => None,
    }
}
