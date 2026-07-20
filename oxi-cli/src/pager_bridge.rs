//! AgentSession → oxi-pager BackgroundEvent bridge.
//!
//! Spawns the agent on a background thread via `Agent::run_with_channel`,
//! converts `AgentEvent` to `BackgroundEvent`, and feeds them into the
//! pager's event loop via a tokio mpsc channel.

use oxi_agent::{Agent, AgentEvent};
use oxi_pager::{BackgroundEvent, run as run_pager};
use std::sync::{Arc, mpsc};
use tokio::sync::mpsc as tokio_mpsc;

/// Run the grok-quality TUI pager with an Agent backend.
pub async fn run_pager_with_agent(agent: Arc<Agent>) -> anyhow::Result<()> {
    let (bg_tx, bg_rx) = tokio_mpsc::unbounded_channel::<BackgroundEvent>();

    // Pager needs a way to send user messages to the agent.
    // We use a separate channel: pager's submit → agent.run_with_channel
    let (user_tx, user_rx) = mpsc::channel::<String>();

    // Spawn agent worker thread: waits for user messages, runs agent, forwards events
    let agent_clone = Arc::clone(&agent);
    let bg = bg_tx.clone();
    std::thread::spawn(move || {
        while let Ok(prompt) = user_rx.recv() {
            let (agent_tx, agent_rx) = mpsc::channel();

            // Run agent in a blocking context
            let agent = Arc::clone(&agent_clone);
            let handle = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async { agent.run_with_channel(prompt, agent_tx).await })
            });

            // Forward agent events to pager
            while let Ok(event) = agent_rx.recv() {
                for bg_event in agent_to_background_events(&event) {
                    if bg.send(bg_event).is_err() {
                        return; // pager closed
                    }
                }
            }

            let _ = handle.join();
        }
    });

    // TODO: wire user_tx into the pager's submit handler
    // For now, pager submit just adds to scrollback

    // Run the pager event loop (blocks until user quits)
    run_pager(user_tx, bg_rx).await
}

/// Convert a single AgentEvent into zero or more BackgroundEvents.
fn agent_to_background_events(event: &AgentEvent) -> Vec<BackgroundEvent> {
    match event {
        AgentEvent::MessageUpdate { delta, .. } => {
            delta.as_ref().map(|text| {
                BackgroundEvent::AssistantDelta(text.clone())
            }).into_iter().collect()
        }
        AgentEvent::TextChunk { text } => {
            vec![BackgroundEvent::AssistantDelta(text.clone())]
        }
        AgentEvent::ThinkingDelta { text } => {
            vec![BackgroundEvent::AssistantDelta(format!("[think] {text}"))]
        }
        AgentEvent::MessageStart { message } => {
            if matches!(message, oxi_ai::Message::User(_)) {
                // User messages are handled by submit, not forwarded
                vec![]
            } else {
                vec![BackgroundEvent::AssistantDone]
            }
        }
        AgentEvent::MessageEnd { .. } => {
            vec![BackgroundEvent::StreamDone]
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
            ..
        } => {
            vec![BackgroundEvent::ToolCall {
                id: tool_call_id.clone(),
                name: tool_name.clone(),
                params: serde_json::to_string_pretty(args).unwrap_or_default(),
            }]
        }
        AgentEvent::ToolExecutionEnd {
            tool_call_id, result, ..
        } => {
            vec![BackgroundEvent::ToolResult {
                id: tool_call_id.clone(),
                content: result.content.clone(),
            }]
        }
        AgentEvent::ToolCall { tool_call } => {
            vec![BackgroundEvent::ToolCall {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                params: tool_call.arguments.to_string(),
            }]
        }
        AgentEvent::AgentEnd { .. } => {
            vec![BackgroundEvent::StreamDone]
        }
        _ => vec![],
    }
}
