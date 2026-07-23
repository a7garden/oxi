//! RPC actor and stdio entry point.

use crate::App;
use crate::app::agent_session::{AgentSession, AgentSessionHandle};
use crate::store::session::SessionManager;
use anyhow::{Context, Result};
use oxi_agent::{AgentEvent, AgentHooks, ToolExecutionMode};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::protocol::*;

type OutputSender = mpsc::UnboundedSender<WriterFrame>;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OutputFrame {
    Ready,
}

enum WriterFrame {
    Response(RpcResponse),
    Event(RpcEvent),
    Json(Value),
}

struct RpcActor {
    session: AgentSessionHandle,
    output: mpsc::UnboundedSender<WriterFrame>,
    active_run: Option<tokio::task::JoinHandle<Result<()>>>,
}

/// Run the RPC server over JSON Lines on stdin/stdout.
pub async fn run_rpc_mode(app: App) -> Result<()> {
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .into_owned();
    let session = AgentSession::new(
        app.agent(),
        app.settings().clone(),
        SessionManager::create(&cwd, None),
        cwd,
    );
    install_session_hooks(&session);
    let session = session.clone_handle();

    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<RpcCommand>();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<WriterFrame>();

    let reader = tokio::spawn(read_commands(command_tx, output_tx.clone()));
    let writer = tokio::spawn(write_frames(output_rx));

    output_tx
        .send(WriterFrame::Json(
            serde_json::to_value(OutputFrame::Ready).expect("ready frame is serializable"),
        ))
        .map_err(|_| anyhow::anyhow!("RPC stdout writer stopped during startup"))?;

    let mut actor = RpcActor {
        session,
        output: output_tx,
        active_run: None,
    };

    while let Some(command) = command_rx.recv().await {
        actor.handle(command).await;
    }

    actor.abort_active_run().await;
    drop(actor.output);
    reader.abort();
    writer.await.context("RPC stdout writer task failed")??;
    Ok(())
}

fn install_session_hooks(session: &AgentSession) {
    let steering = session.steering_queue();
    let follow_up = session.follow_up_queue();
    let should_stop = session.should_stop_flag();
    session.agent_ref().set_hooks(AgentHooks {
        should_stop_after_turn: Some(Arc::new(move |_| should_stop.load(Ordering::SeqCst))),
        get_steering_messages: Some(Arc::new(move || steering.write().drain(..).collect())),
        get_follow_up_messages: Some(Arc::new(move || follow_up.write().drain(..).collect())),
        tool_execution: ToolExecutionMode::Sequential,
        ..Default::default()
    });
}

async fn read_commands(command_tx: mpsc::UnboundedSender<RpcCommand>, output: OutputSender) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => continue,
            Ok(Some(line)) => match parse_command_line(&line) {
                Ok(command) => {
                    if command_tx.send(command).is_err() {
                        break;
                    }
                }
                Err(response) => {
                    if output.send(WriterFrame::Response(response)).is_err() {
                        break;
                    }
                }
            },
            Ok(None) => break,
            Err(error) => {
                let _ = output.send(WriterFrame::Response(error_response(
                    None,
                    "read",
                    format!("Failed to read stdin: {error}"),
                )));
                break;
            }
        }
    }
}

pub(crate) fn parse_command_line(line: &str) -> std::result::Result<RpcCommand, RpcResponse> {
    let value = parse_json_line(line).map_err(|error| {
        error_response(None, "parse", format!("Failed to parse command: {error}"))
    })?;
    if value.get("jsonrpc").is_some() {
        return Err(error_response(
            value.get("id").map(Value::to_string),
            "jsonrpc",
            "JSON-RPC framing is not supported by the actor RPC protocol; send native JSONL commands",
        ));
    }
    if value.get("type").and_then(Value::as_str) == Some("extension_ui_response") {
        return Err(error_response(
            None,
            "extension_ui_response",
            "extension UI responses are not yet supported in RPC mode",
        ));
    }
    serde_json::from_value(value)
        .map_err(|error| error_response(None, "parse", format!("Parse error: {error}")))
}

async fn write_frames(mut rx: mpsc::UnboundedReceiver<WriterFrame>) -> Result<()> {
    let mut stdout = tokio::io::stdout();
    while let Some(frame) = rx.recv().await {
        let line = match frame {
            WriterFrame::Response(response) => serialize_json_line_obj(&response),
            WriterFrame::Event(event) => serialize_json_line_obj(&event),
            WriterFrame::Json(value) => serialize_json_line(&value),
        };
        stdout.write_all(line.as_bytes()).await?;
        stdout.flush().await?;
    }
    Ok(())
}

impl RpcActor {
    async fn handle(&mut self, command: RpcCommand) {
        match command {
            RpcCommand::Prompt {
                id,
                message,
                images,
                streaming_behavior: _,
            } => self.prompt(id, message, images),
            RpcCommand::Steer {
                id,
                message,
                images,
            } => {
                if images.as_ref().is_some_and(|images| !images.is_empty()) {
                    self.respond(unsupported_response(id, "steer with images"));
                } else {
                    self.session.steer_sync(message);
                    self.respond(success_response(id, "steer", None));
                }
            }
            RpcCommand::FollowUp {
                id,
                message,
                images,
            } => {
                if images.as_ref().is_some_and(|images| !images.is_empty()) {
                    self.respond(unsupported_response(id, "follow_up with images"));
                } else {
                    self.session.follow_up_sync(message);
                    self.respond(success_response(id, "follow_up", None));
                }
            }
            RpcCommand::Abort { id } => {
                self.session.abort().await;
                self.session.agent_ref().cancel();
                // The session hook and Agent cancel flag stop the dedicated local runtime.
                // Keep the join task alive so it can persist final state and clear streaming.
                self.respond(success_response(id, "abort", None));
            }
            RpcCommand::NewSession { id, .. } => {
                self.respond(unsupported_response(id, "new_session"));
            }
            RpcCommand::GetState { id } => self.respond(success_response(
                id,
                "get_state",
                Some(self.session_state_value()),
            )),
            RpcCommand::SetModel {
                id,
                provider,
                model_id,
            } => {
                let full_model_id = if model_id.contains('/') {
                    model_id
                } else {
                    format!("{provider}/{model_id}")
                };
                match self.session.set_model(&full_model_id) {
                    Ok(()) => self.respond(success_response(
                        id,
                        "set_model",
                        Some(serde_json::json!({ "model": full_model_id })),
                    )),
                    Err(error) => self.respond(error_response(id, "set_model", error.to_string())),
                }
            }
            RpcCommand::CycleModel { id } => self.respond(unsupported_response(id, "cycle_model")),
            RpcCommand::GetAvailableModels { id } => {
                let models: Vec<_> = oxi_sdk::get_all_models()
                    .map(|entry| {
                        serde_json::json!({
                            "provider": entry.provider,
                            "id": entry.id,
                        })
                    })
                    .collect();
                self.respond(success_response(
                    id,
                    "get_available_models",
                    Some(serde_json::json!({ "models": models })),
                ));
            }
            RpcCommand::SetThinkingLevel { id, level } => {
                match crate::store::settings::parse_thinking_level(&level) {
                    Some(level) => {
                        self.session.set_thinking_level(level);
                        self.respond(success_response(id, "set_thinking_level", None));
                    }
                    None => self.respond(error_response(
                        id,
                        "set_thinking_level",
                        format!("invalid thinking level: {level}"),
                    )),
                }
            }
            RpcCommand::CycleThinkingLevel { id } => match self.session.cycle_thinking_level() {
                Some(level) => self.respond(success_response(
                    id,
                    "cycle_thinking_level",
                    Some(serde_json::json!({ "level": format_thinking_level(level) })),
                )),
                None => self.respond(error_response(
                    id,
                    "cycle_thinking_level",
                    "the active model does not support another thinking level",
                )),
            },
            RpcCommand::SetSteeringMode { id, .. } => {
                self.respond(unsupported_response(id, "set_steering_mode"));
            }
            RpcCommand::SetFollowUpMode { id, .. } => {
                self.respond(unsupported_response(id, "set_follow_up_mode"));
            }
            RpcCommand::Compact {
                id,
                custom_instructions,
            } => match self.session.compact(custom_instructions).await {
                Ok(result) => self.respond(success_response(
                    id,
                    "compact",
                    Some(serde_json::json!({
                        "tokens_before": result.tokens_before,
                        "message_count": self.session.messages().len(),
                    })),
                )),
                Err(error) => self.respond(error_response(id, "compact", error.to_string())),
            },
            RpcCommand::SetAutoCompaction { id, .. } => {
                self.respond(unsupported_response(id, "set_auto_compaction"));
            }
            RpcCommand::SetAutoRetry { id, .. } => {
                self.respond(unsupported_response(id, "set_auto_retry"));
            }
            RpcCommand::AbortRetry { id } => {
                self.respond(unsupported_response(id, "abort_retry"));
            }
            RpcCommand::Bash { id, command } => self.run_bash(id, command).await,
            RpcCommand::AbortBash { id } => {
                self.respond(unsupported_response(id, "abort_bash"));
            }
            RpcCommand::GetSessionStats { id } => {
                let state = self.session.state();
                let stats = self.session.session_stats();
                self.respond(success_response(
                    id,
                    "get_session_stats",
                    Some(serde_json::json!({
                        "session_id": stats.session_id,
                        "message_count": stats.total_messages,
                        "user_messages": stats.user_messages,
                        "assistant_messages": stats.assistant_messages,
                        "tool_calls": stats.tool_calls,
                        "tool_results": stats.tool_results,
                        "token_count": state.estimate_tokens(),
                    })),
                ));
            }
            RpcCommand::GetLastAssistantText { id } => {
                let text = self
                    .session
                    .state()
                    .messages
                    .iter()
                    .rev()
                    .find_map(|message| {
                        if let oxi_sdk::Message::Assistant(message) = message {
                            Some(message.text_content())
                        } else {
                            None
                        }
                    });
                self.respond(success_response(
                    id,
                    "get_last_assistant_text",
                    Some(serde_json::json!({ "text": text })),
                ));
            }
            RpcCommand::SetSessionName { id, name } => {
                self.session.set_session_name(name);
                self.respond(success_response(id, "set_session_name", None));
            }
            RpcCommand::GetMessages { id } => self.respond(success_response(
                id,
                "get_messages",
                Some(serde_json::json!({ "messages": self.session.messages() })),
            )),
            RpcCommand::GetCommands { id } => {
                self.respond(unsupported_response(id, "get_commands"));
            }
            RpcCommand::ExportHtml { id, .. } => {
                self.respond(unsupported_response(id, "export_html"));
            }
            RpcCommand::SwitchSession { id, .. } => {
                self.respond(unsupported_response(id, "switch_session"));
            }
            RpcCommand::Fork { id, .. } => self.respond(unsupported_response(id, "fork")),
            RpcCommand::Clone { id } => self.respond(unsupported_response(id, "clone")),
            RpcCommand::GetForkMessages { id } => {
                self.respond(unsupported_response(id, "get_fork_messages"));
            }
        }
    }

    fn prompt(&mut self, id: Option<String>, message: String, images: Option<Vec<ImageData>>) {
        if images.as_ref().is_some_and(|images| !images.is_empty()) {
            self.respond(unsupported_response(id, "prompt with images"));
            return;
        }
        if self.session.is_streaming() {
            self.respond(error_response(
                id,
                "prompt",
                "an agent run is already active; use steer or follow_up",
            ));
            return;
        }

        self.session.reset_should_stop();
        self.session.agent_ref().reset_cancel();
        self.session.streaming_flag().store(true, Ordering::SeqCst);
        self.session.persist_user_message(message.clone());

        let session = self.session.clone_handle();
        let output = self.output.clone();
        let agent = session.agent_ref();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<AgentEvent>();
        let forwarder = tokio::task::spawn_blocking(move || {
            while let Ok(event) = event_rx.recv() {
                session.forward_event_to_extensions(&event);
                if let AgentEvent::MessageEnd { message } = &event {
                    session.persist_event_message(message);
                }
                if let Some(event) = agent_event_to_rpc(&event)
                    && output.send(WriterFrame::Event(event)).is_err()
                {
                    break;
                }
            }
        });
        let output = self.output.clone();
        let session = self.session.clone_handle();
        let agent_run = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to build RPC agent runtime")?;
            runtime.block_on(async {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(agent.run_with_channel(message, event_tx))
                    .await
            })
        });
        self.active_run = Some(tokio::spawn(async move {
            let result = agent_run.await;
            let _ = forwarder.await;
            session.persist();
            session.streaming_flag().store(false, Ordering::SeqCst);
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    let _ = output.send(WriterFrame::Event(RpcEvent::Error {
                        message: error.to_string(),
                    }));
                }
                Err(error) => {
                    let _ = output.send(WriterFrame::Event(RpcEvent::Error {
                        message: format!("RPC agent task failed: {error}"),
                    }));
                }
            }
            Ok(())
        }));

        self.respond(success_response(
            id,
            "prompt",
            Some(serde_json::json!({ "accepted": true })),
        ));
    }

    async fn run_bash(&self, id: Option<String>, command: String) {
        if is_dangerous_rpc_command(&command) {
            tracing::warn!("RPC bash command contains dangerous pattern: {:?}", command);
        }
        let result = tokio::task::spawn_blocking(move || {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
        })
        .await;
        let response = match result {
            Ok(Ok(output)) => success_response(
                id,
                "bash",
                Some(serde_json::json!({
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                    "exit_code": output.status.code(),
                })),
            ),
            Ok(Err(error)) => error_response(id, "bash", error.to_string()),
            Err(error) => error_response(id, "bash", format!("bash task failed: {error}")),
        };
        self.respond(response);
    }

    fn session_state_value(&self) -> Value {
        let state = self.session.state();
        let model_id = self.session.model_id();
        let (provider, id) = model_id
            .split_once('/')
            .map(|(provider, id)| (provider.to_string(), id.to_string()))
            .unwrap_or_else(|| (String::new(), model_id));
        serde_json::json!({
            "model": ModelInfo { provider, id },
            "thinking_level": format_thinking_level(self.session.thinking_level()),
            "is_streaming": self.session.is_streaming(),
            "is_compacting": self.session.is_compacting(),
            "steering_mode": "all",
            "follow_up_mode": "all",
            "session_id": self.session.session_id(),
            "auto_compaction_enabled": self.session.auto_compaction_enabled(),
            "message_count": state.messages.len(),
            "pending_message_count": self.session.pending_message_count(),
            "iteration": state.iteration,
            "stop_reason": state.stop_reason,
        })
    }

    fn respond(&self, response: RpcResponse) {
        let _ = self.output.send(WriterFrame::Response(response));
    }

    async fn abort_active_run(&mut self) {
        self.session.abort().await;
        self.session.agent_ref().cancel();
        if let Some(handle) = self.active_run.take() {
            let _ = handle.await;
        }
    }
}

pub(crate) fn agent_event_to_rpc(event: &AgentEvent) -> Option<RpcEvent> {
    match event {
        AgentEvent::AgentStart { .. } | AgentEvent::Start { .. } => Some(RpcEvent::AgentStart),
        AgentEvent::AgentEnd { .. } | AgentEvent::Complete { .. } | AgentEvent::Cancelled => {
            Some(RpcEvent::AgentEnd)
        }
        AgentEvent::Thinking | AgentEvent::ThinkingDelta { .. } => Some(RpcEvent::Thinking),
        AgentEvent::ThinkingEnd => Some(RpcEvent::ThinkingEnd),
        AgentEvent::TextChunk { text } => Some(RpcEvent::TextChunk { text: text.clone() }),
        AgentEvent::MessageUpdate {
            delta: Some(text), ..
        } if !text.is_empty() => Some(RpcEvent::TextChunk { text: text.clone() }),
        AgentEvent::ToolCallDelta {
            tool_call_id,
            args_delta,
        } => Some(RpcEvent::ToolCallDelta {
            tool_call_id: tool_call_id.clone(),
            args_delta: args_delta.clone(),
        }),
        AgentEvent::ToolExecutionStart { tool_name, .. }
        | AgentEvent::ToolStart { tool_name, .. } => Some(RpcEvent::ToolStart {
            tool: tool_name.clone(),
        }),
        AgentEvent::ToolExecutionEnd { tool_name, .. } => Some(RpcEvent::ToolEnd {
            tool: tool_name.clone(),
        }),
        AgentEvent::Error { message, .. } | AgentEvent::ToolError { error: message, .. } => {
            Some(RpcEvent::Error {
                message: message.clone(),
            })
        }
        _ => None,
    }
}

fn success_response(id: Option<String>, command: &str, data: Option<Value>) -> RpcResponse {
    RpcResponse::Response {
        id,
        command: command.to_string(),
        success: true,
        data,
        error: None,
    }
}

fn error_response(id: Option<String>, command: &str, error: impl Into<String>) -> RpcResponse {
    RpcResponse::Response {
        id,
        command: command.to_string(),
        success: false,
        data: None,
        error: Some(error.into()),
    }
}

pub(crate) fn unsupported_response(id: Option<String>, command: &str) -> RpcResponse {
    error_response(
        id,
        command,
        format!("{command} is not yet supported in RPC mode"),
    )
}

fn format_thinking_level(level: crate::store::settings::ThinkingLevel) -> &'static str {
    match level {
        crate::store::settings::ThinkingLevel::Off => "off",
        crate::store::settings::ThinkingLevel::Minimal => "minimal",
        crate::store::settings::ThinkingLevel::Low => "low",
        crate::store::settings::ThinkingLevel::Medium => "medium",
        crate::store::settings::ThinkingLevel::High => "high",
        crate::store::settings::ThinkingLevel::XHigh => "xhigh",
    }
}

fn is_dangerous_rpc_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    lower.contains("/etc/passwd")
        || lower.contains("id_rsa")
        || lower.contains("curl | nc")
        || lower.contains("/dev/tcp/")
        || lower.contains("rm -rf /")
        || lower.contains("> /etc/")
        || lower.contains("mkfifo")
}
