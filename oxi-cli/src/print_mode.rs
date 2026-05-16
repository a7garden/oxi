//! Print mode (non-interactive) for oxi.
//!
//! Used for:
//! - `oxi -p "prompt"` — text output (final response to stdout)
//! - `oxi --mode json "prompt"` — newline-delimited JSON event stream
//!
//! Reads prompt from stdin or args, runs the agent, prints the result,
//! and exits. No TUI rendering.

use crate::App;
use anyhow::Result;
use oxi_agent::{Agent, AgentEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Output format for print mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintMode {
    /// Output only the final assistant response as plain text.
    Text,
    /// Output all agent events as newline-delimited JSON.
    Json,
}

/// Options for running print mode.
#[derive(Debug)]
pub struct PrintModeOptions {
    /// Output mode: Text or Json.
    pub mode: PrintMode,
    /// Additional prompts to send after the initial message.
    pub messages: Vec<String>,
    /// The first prompt (may be provided via CLI or stdin).
    pub initial_message: Option<String>,
    /// When true, skip any stdin reading. Set by the caller (main.rs) when
    /// `--print` / `-p` is used so that print mode never blocks on a TTY.
    pub no_stdin: bool,
    /// Skip session creation entirely (for --print mode).
    pub no_session: bool,
    /// Suppress progress/stderr output.
    pub quiet: bool,
    /// Timeout in seconds for the entire operation.
    pub timeout: Option<u64>,
}

impl Default for PrintModeOptions {
    fn default() -> Self {
        Self {
            mode: PrintMode::Text,
            messages: Vec::new(),
            initial_message: None,
            no_stdin: false,
            no_session: false,
            quiet: false,
            timeout: None,
        }
    }
}

/// Run in print (single-shot) mode.
///
/// Sends prompts to the agent and outputs the result. Returns an exit code.
pub async fn run_print_mode(app: &App, options: PrintModeOptions) -> Result<i32> {
    let PrintModeOptions {
        mode,
        messages,
        initial_message,
        no_stdin,
        no_session: _,
        quiet,
        timeout,
    } = options;

    // If no_stdin is set, skip any stdin reading (prevents blocking on TTY).
    let _ = no_stdin;

    let agent: Arc<Agent> = app.agent();
    let mut exit_code = 0;

    // Register signal handlers for graceful shutdown
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    ctrlc_handler(shutdown_tx)?;

    // Build the core async work as a future so we can apply a timeout
    let work = async {
        // Process initial message
        if let Some(prompt) = initial_message {
            let result = run_single_prompt(&agent, &prompt, mode, quiet, &mut shutdown_rx).await;
            match result {
                Ok(()) => {}
                Err(PromptError::AgentError(msg)) => {
                    if mode == PrintMode::Text && !quiet {
                        eprintln!("Error: {}", msg);
                    }
                    exit_code = 1;
                }
                Err(PromptError::Shutdown) => {
                    exit_code = 130;
                    return;
                }
            }
        }

        // Process additional messages
        for message in messages {
            if shutdown_rx.try_recv().is_ok() {
                exit_code = 130;
                return;
            }

            let result = run_single_prompt(&agent, &message, mode, quiet, &mut shutdown_rx).await;
            match result {
                Ok(()) => {}
                Err(PromptError::AgentError(msg)) => {
                    if mode == PrintMode::Text && !quiet {
                        eprintln!("Error: {}", msg);
                    }
                    exit_code = 1;
                }
                Err(PromptError::Shutdown) => {
                    exit_code = 130;
                    return;
                }
            }
        }
    };

    // Apply timeout if specified
    if let Some(secs) = timeout {
        match tokio::time::timeout(Duration::from_secs(secs), work).await {
            Ok(()) => {}
            Err(_) => {
                if !quiet {
                    eprintln!("Timed out after {} seconds", secs);
                }
                exit_code = 124; // mirroring timeout(1) exit code
            }
        }
    } else {
        work.await;
    }

    Ok(exit_code)
}

/// Possible errors during a single prompt run.
enum PromptError {
    AgentError(String),
    Shutdown,
}

/// Run a single prompt through the agent, outputting events/results as appropriate.
async fn run_single_prompt(
    agent: &Arc<Agent>,
    prompt: &str,
    mode: PrintMode,
    quiet: bool,
    shutdown_rx: &mut mpsc::Receiver<()>,
) -> Result<(), PromptError> {
    let _ = quiet; // used by callers when handling PromptError
                   // Agent expects std::sync::mpsc, but we need async for tokio::select
                   // Use a sync mpsc channel inside spawn_blocking, bridge to tokio mpsc
    let (event_tx, event_rx) = std::sync::mpsc::channel::<AgentEvent>();
    let (async_tx, mut async_rx) = mpsc::channel::<AgentEvent>(256);

    // Spawn agent run on a LocalSet (non-Send futures)
    let agent_clone: Arc<Agent> = Arc::clone(agent);
    let prompt_owned = prompt.to_string();

    // Bridge thread: converts std::sync::mpsc events to tokio::mpsc.
    //
    // The bridge uses try_send (non-blocking) on the tokio channel instead
    // of rt.block_on(async_tx.send(...)) because the bridge thread has no
    // Tokio runtime. The tokio channel is bounded(256) so try_send may drop
    // events under extreme backpressure, but this is acceptable for print mode.
    let bridge_handle = std::thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            // Non-blocking send — no Tokio runtime needed
            let _ = async_tx.try_send(event);
        }
    });

    let agent_handle = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build agent runtime");
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local
                .run_until(async {
                    let _ = agent_clone.run_with_channel(prompt_owned, event_tx).await;
                })
                .await;
        });
    });

    // Stream events
    let mut last_text = String::new();
    let mut had_error = false;
    let mut error_message = String::new();
    let mut _stop_reason: Option<String> = None;

    loop {
        tokio::select! {
            event = async_rx.recv() => {
                match event {
                    Some(ev) => {
                        match &ev {
                            // Legacy TextChunk (from old Agent::run_with_channel)
                            AgentEvent::TextChunk { text } => {
                                last_text.push_str(text);
                            }
                            // AgentLoop MessageUpdate — extract text from snapshot
                            AgentEvent::MessageUpdate { message: oxi_ai::Message::Assistant(asst), .. } => {
                                // Only extract Text blocks (matching pi's print-mode.ts behavior).
                                // GLM fallback: if no Text blocks exist but Thinking
                                // blocks do, use Thinking content since GLM puts all
                                // output in reasoning_content.
                                let text_only: String = asst.content.iter()
                                    .filter_map(|b| match b {
                                        oxi_ai::ContentBlock::Text(t) => Some(t.text.as_str()),
                                        _ => None,
                                    })
                                    .collect();
                                if !text_only.is_empty() {
                                    last_text = text_only;
                                } else {
                                    // GLM fallback: use Thinking blocks when no Text
                                    let thinking_text: String = asst.content.iter()
                                        .filter_map(|b| match b {
                                            oxi_ai::ContentBlock::Thinking(t) => Some(t.thinking.as_str()),
                                            _ => None,
                                        })
                                        .collect();
                                    if !thinking_text.is_empty() {
                                        last_text = thinking_text;
                                    }
                                }
                            }
                            AgentEvent::MessageEnd { message: oxi_ai::Message::Assistant(asst) } => {
                                // Finalize last_text from the completed message snapshot
                                let text_only: String = asst.content.iter()
                                    .filter_map(|b| match b {
                                        oxi_ai::ContentBlock::Text(t) => Some(t.text.as_str()),
                                        _ => None,
                                    })
                                    .collect();
                                if !text_only.is_empty() {
                                    last_text = text_only;
                                } else {
                                    // GLM fallback
                                    let thinking_text: String = asst.content.iter()
                                        .filter_map(|b| match b {
                                            oxi_ai::ContentBlock::Thinking(t) => Some(t.thinking.as_str()),
                                            _ => None,
                                        })
                                        .collect();
                                    if !thinking_text.is_empty() {
                                        last_text = thinking_text;
                                    }
                                }
                            }
                            AgentEvent::Complete { .. } => {
                                _stop_reason = Some("complete".to_string());
                            }
                            AgentEvent::Error { message, .. } => {
                                had_error = true;
                                error_message = message.clone();
                                _stop_reason = Some("error".to_string());
                            }
                            _ => {}
                        }

                        if mode == PrintMode::Json {
                            if let Ok(json) = serde_json::to_string(&event_to_json(&ev)) {
                                println!("{}", json);
                                use std::io::Write;
                                std::io::stdout().flush().ok();
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = shutdown_rx.recv() => {
                return Err(PromptError::Shutdown);
            }
        }
    }

    // Wait for the agent thread to finish
    let _ = agent_handle.await;
    let _ = bridge_handle.join();

    if had_error {
        return Err(PromptError::AgentError(error_message));
    }

    // In text mode, print the final response
    if mode == PrintMode::Text && !last_text.is_empty() {
        println!("{}", last_text);
        use std::io::Write;
        std::io::stdout().flush().ok();
    }

    Ok(())
}

/// Extract text content from an oxi_ai::Message.
/// Prefers Text blocks; falls back to Thinking blocks if no Text
/// content exists (GLM models send all output as reasoning_content).
fn extract_text_from_message(msg: &oxi_ai::Message) -> String {
    match msg {
        oxi_ai::Message::Assistant(asst) => {
            let text_only: String = asst
                .content
                .iter()
                .filter_map(|b| match b {
                    oxi_ai::ContentBlock::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if !text_only.is_empty() {
                return text_only;
            }
            // GLM fallback: use Thinking blocks
            asst.content
                .iter()
                .filter_map(|b| match b {
                    oxi_ai::ContentBlock::Thinking(t) => Some(t.thinking.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        }
        _ => String::new(),
    }
}

/// Convert an AgentEvent to a JSON-serializable value for JSON mode.
fn event_to_json(event: &AgentEvent) -> serde_json::Value {
    match event {
        AgentEvent::Start { .. } => serde_json::json!({
            "type": "start"
        }),
        AgentEvent::Thinking => serde_json::json!({
            "type": "thinking"
        }),
        AgentEvent::TextChunk { text } => serde_json::json!({
            "type": "text_delta",
            "text": text,
        }),
        AgentEvent::ToolCall { tool_call } => serde_json::json!({
            "type": "tool_call",
            "id": tool_call.id,
            "name": tool_call.name,
            "arguments": tool_call.arguments.to_string(),
        }),
        AgentEvent::ToolStart {
            tool_name,
            tool_call_id,
            arguments: _,
        } => serde_json::json!({
            "type": "tool_start",
            "tool_name": tool_name,
            "tool_call_id": tool_call_id,
        }),
        AgentEvent::ToolComplete { result } => serde_json::json!({
            "type": "tool_complete",
            "content": result.content.chars().take(2000).collect::<String>(),
            "is_error": result.is_error(),
        }),
        AgentEvent::ToolError {
            error,
            tool_call_id,
        } => serde_json::json!({
            "type": "tool_error",
            "error": error,
            "tool_call_id": tool_call_id,
        }),
        AgentEvent::Complete { .. } => serde_json::json!({
            "type": "complete"
        }),
        AgentEvent::Error { message, .. } => serde_json::json!({
            "type": "error",
            "message": message,
        }),
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => serde_json::json!({
            "type": "usage",
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }),

        // AgentLoop events (new-style lifecycle)
        AgentEvent::AgentStart { .. } => serde_json::json!({
            "type": "agent_start"
        }),
        AgentEvent::AgentEnd { .. } => serde_json::json!({
            "type": "agent_end"
        }),
        AgentEvent::TurnStart { turn_number } => serde_json::json!({
            "type": "turn_start",
            "turn_number": turn_number,
        }),
        AgentEvent::TurnEnd { turn_number, .. } => serde_json::json!({
            "type": "turn_end",
            "turn_number": turn_number,
        }),
        AgentEvent::MessageStart { message } => {
            let text = extract_text_from_message(message);
            serde_json::json!({
                "type": "message_start",
                "text": text,
            })
        }
        AgentEvent::MessageUpdate { message, delta } => {
            let text = extract_text_from_message(message);
            serde_json::json!({
                "type": "message_update",
                "text": text,
                "delta": delta,
            })
        }
        AgentEvent::MessageEnd { message } => {
            let text = extract_text_from_message(message);
            serde_json::json!({
                "type": "message_end",
                "text": text,
            })
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => serde_json::json!({
            "type": "tool_execution_start",
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "args": args.to_string(),
        }),
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => serde_json::json!({
            "type": "tool_execution_end",
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "result": result.content.chars().take(2000).collect::<String>(),
            "is_error": is_error,
        }),

        // Everything else
        _ => serde_json::json!({
            "type": "unknown"
        }),
    }
}

/// Set up Ctrl+C handler to signal graceful shutdown.
fn ctrlc_handler(shutdown_tx: mpsc::Sender<()>) -> Result<()> {
    // Use tokio signal handling via a background thread.
    std::thread::spawn(move || {
        let _ = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|rt| {
                rt.block_on(async {
                    tokio::signal::ctrl_c().await.ok();
                    let _ = shutdown_tx.try_send(());
                });
            });
    });
    Ok(())
}

/// Read a prompt from stdin (for piping).
pub fn read_stdin_prompt() -> Result<String> {
    use std::io::{self, Read};
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    Ok(buffer.trim().to_string())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_to_json_start() {
        let event = AgentEvent::Start {
            prompt: "test".to_string(),
        };
        let json = event_to_json(&event);
        assert_eq!(json["type"], "start");
    }

    #[test]
    fn test_event_to_json_thinking() {
        let json = event_to_json(&AgentEvent::Thinking);
        assert_eq!(json["type"], "thinking");
    }

    #[test]
    fn test_event_to_json_text_chunk() {
        let event = AgentEvent::TextChunk {
            text: "Hello world".to_string(),
        };
        let json = event_to_json(&event);
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "Hello world");
    }

    #[test]
    fn test_event_to_json_tool_call() {
        let event = AgentEvent::ToolCall {
            tool_call: oxi_ai::ToolCall {
                content_type: oxi_ai::ToolCallType::ToolCall,
                id: "tc-1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/test.rs"}),
                thought_signature: None,
            },
        };
        let json = event_to_json(&event);
        assert_eq!(json["type"], "tool_call");
        assert_eq!(json["name"], "read_file");
        assert_eq!(json["id"], "tc-1");
    }

    #[test]
    fn test_event_to_json_error() {
        let event = AgentEvent::Error {
            message: "Something went wrong".to_string(),
            session_id: None,
        };
        let json = event_to_json(&event);
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "Something went wrong");
    }

    #[test]
    fn test_event_to_json_complete() {
        let event = AgentEvent::Complete {
            content: "done".to_string(),
            stop_reason: "end_turn".to_string(),
        };
        let json = event_to_json(&event);
        assert_eq!(json["type"], "complete");
    }

    #[test]
    fn test_event_to_json_tool_complete() {
        let event = AgentEvent::ToolComplete {
            result: oxi_ai::ToolResult {
                tool_call_id: "tc-1".to_string(),
                content: "file contents here".to_string(),
                status: "success".to_string(),
            },
        };
        let json = event_to_json(&event);
        assert_eq!(json["type"], "tool_complete");
        assert_eq!(json["is_error"], false);
    }

    #[test]
    fn test_print_mode_default_options() {
        let opts = PrintModeOptions::default();
        assert_eq!(opts.mode, PrintMode::Text);
        assert!(opts.messages.is_empty());
        assert!(opts.initial_message.is_none());
        assert!(!opts.no_stdin);
        assert!(!opts.no_session);
        assert!(!opts.quiet);
        assert!(opts.timeout.is_none());
    }

    #[test]
    fn test_print_mode_equality() {
        assert_eq!(PrintMode::Text, PrintMode::Text);
        assert_eq!(PrintMode::Json, PrintMode::Json);
        assert_ne!(PrintMode::Text, PrintMode::Json);
    }
}
