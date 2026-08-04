//! RPC actor and stdio entry point.

use crate::App;
use crate::app::agent_session::{AgentSession, AgentSessionHandle};
use crate::store::session::SessionManager;
use crate::store::settings::Settings;
use anyhow::{Context, Result};
use oxicode_agent::{Agent, AgentEvent, AgentHooks, ToolExecutionMode};
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
    agent: Arc<Agent>,
    settings: Settings,
    cwd: String,
    active_bash: Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    /// Shared with every swapped-in session so /steer, /follow_up, and
    /// Ctrl+C continue to work after `/new`, `/resume`, `/fork`, etc.
    session_state: crate::SessionState,
}

/// Run the RPC server over JSON Lines on stdin/stdout.
pub async fn run_rpc_mode(app: App) -> Result<()> {
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .into_owned();
    let agent = app.agent();
    let settings = app.settings().clone();
    let session_state = app.session_state().clone();
    let session = AgentSession::new(
        Arc::clone(&agent),
        settings.clone(),
        SessionManager::create(&cwd, None),
        cwd.clone(),
        session_state.clone(),
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
        agent,
        settings,
        cwd,
        active_bash: Arc::new(parking_lot::Mutex::new(None)),
        session_state,
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

/// Install the cli-owned session hooks on a freshly-constructed session.
///
/// The session has ALREADY had the shared state wired into it via the
/// `SessionState` we passed to `AgentSession::new`. This call now just
/// re-installs the SAME closures so even sessions constructed outside
/// the `App` flow (legacy paths, tests) observe Ctrl+C and queues.
/// Once Task 9 drops the `install_runtime_hooks` call site entirely,
/// this helper becomes dead and should be removed.
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
    // Extract the request ID before `from_value` consumes `value`. Without
    // this, parse-error responses carry `id: null` and the RPC client's
    // response-matcher (`send_and_wait`) never matches them — causing the
    // client to hang until timeout (F-3 test, 2026-07-25).
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    serde_json::from_value(value)
        .map_err(|error| error_response(id, "parse", format!("Parse error: {error}")))
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
            } => match build_user_message(message, &images) {
                Ok(msg) => {
                    self.session.steer_sync_message(msg);
                    self.respond(success_response(id, "steer", None));
                }
                Err(e) => self.respond(error_response(id, "steer", e)),
            },
            RpcCommand::FollowUp {
                id,
                message,
                images,
            } => match build_user_message(message, &images) {
                Ok(msg) => {
                    self.session.follow_up_sync_message(msg);
                    self.respond(success_response(id, "follow_up", None));
                }
                Err(e) => self.respond(error_response(id, "follow_up", e)),
            },
            RpcCommand::Abort { id } => {
                self.session.abort().await;
                self.session.agent_ref().cancel();
                self.respond(success_response(id, "abort", None));
            }
            RpcCommand::NewSession { id, .. } => {
                let handle = self
                    .swap_session(SessionManager::create(&self.cwd, None))
                    .await;
                self.respond(success_response(
                    id,
                    "new_session",
                    Some(serde_json::json!({ "session_id": handle.session_id() })),
                ));
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
            RpcCommand::CycleModel { id } => match self.session.cycle_model() {
                Some(model) => self.respond(success_response(
                    id,
                    "cycle_model",
                    Some(serde_json::json!({ "model": model })),
                )),
                None => self.respond(error_response(
                    id,
                    "cycle_model",
                    "no scoped models configured; use set_model to pick a model",
                )),
            },
            RpcCommand::GetAvailableModels { id } => {
                let models: Vec<_> = oxicode_sdk::get_all_models()
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
            RpcCommand::SetSteeringMode { id, mode } => {
                self.session.set_steering_mode(mode);
                self.respond(success_response(id, "set_steering_mode", None));
            }
            RpcCommand::SetFollowUpMode { id, mode } => {
                self.session.set_follow_up_mode(mode);
                self.respond(success_response(id, "set_follow_up_mode", None));
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
            RpcCommand::SetAutoCompaction { id, enabled } => {
                self.session.set_auto_compaction(enabled);
                self.respond(success_response(id, "set_auto_compaction", None));
            }
            RpcCommand::SetAutoRetry { id, enabled } => {
                self.session.set_auto_retry(enabled);
                self.respond(success_response(id, "set_auto_retry", None));
            }
            RpcCommand::AbortRetry { id } => {
                self.session.cancel_auto_retry();
                self.respond(success_response(id, "abort_retry", None));
            }
            RpcCommand::Bash { id, command } => self.run_bash(id, command),
            RpcCommand::AbortBash { id } => {
                let sender = self.active_bash.lock().take();
                match sender {
                    Some(tx) => {
                        let _ = tx.send(());
                        self.respond(success_response(id, "abort_bash", None));
                    }
                    None => self.respond(error_response(
                        id,
                        "abort_bash",
                        "no bash command is running",
                    )),
                }
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
                        if let oxicode_sdk::Message::Assistant(message) = message {
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
                let commands: Vec<_> =
                    crate::tui_vt::slash::registry::SlashRegistry::builtin_commands()
                        .into_iter()
                        .map(|(name, description, aliases)| {
                            serde_json::json!({
                                "name": name,
                                "description": description,
                                "aliases": aliases,
                            })
                        })
                        .collect();
                self.respond(success_response(
                    id,
                    "get_commands",
                    Some(serde_json::json!({ "commands": commands })),
                ));
            }
            RpcCommand::ExportHtml { id, output_path } => match output_path {
                Some(path) => match self.session.export_html() {
                    Ok(html) => match std::fs::write(&path, &html) {
                        Ok(()) => self.respond(success_response(
                            id,
                            "export_html",
                            Some(serde_json::json!({ "path": path })),
                        )),
                        Err(e) => self.respond(error_response(
                            id,
                            "export_html",
                            format!("failed to write {path}: {e}"),
                        )),
                    },
                    Err(e) => self.respond(error_response(id, "export_html", e.to_string())),
                },
                None => self.respond(error_response(id, "export_html", "output_path is required")),
            },
            RpcCommand::SwitchSession { id, session_path } => {
                if session_path.is_empty() || !std::path::Path::new(&session_path).exists() {
                    self.respond(error_response(
                        id,
                        "switch_session",
                        format!("session file not found: {session_path}"),
                    ));
                } else {
                    let sm = SessionManager::open(&session_path, None, Some(&self.cwd));
                    let handle = self.swap_session(sm).await;
                    self.respond(success_response(
                        id,
                        "switch_session",
                        Some(serde_json::json!({ "session_id": handle.session_id() })),
                    ));
                }
            }
            RpcCommand::Fork { id, entry_id } => {
                // `branch_from_entry` delegates to the live session manager
                // (with its current entries) and returns the path of a new
                // session file containing entries up to and including
                // `entry_id`. Open that file and `swap_session` to it so
                // future commands operate on the fork.
                match self.session.branch_from_entry(&entry_id) {
                    Ok(new_path) => {
                        let new_sm = SessionManager::open(&new_path, None, Some(&self.cwd));
                        let handle = self.swap_session(new_sm).await;
                        self.respond(success_response(
                            id,
                            "fork",
                            Some(serde_json::json!({ "session_id": handle.session_id() })),
                        ));
                    }
                    Err(e) => self.respond(error_response(id, "fork", e)),
                }
            }
            RpcCommand::Clone { id } => match self.session.session_file() {
                Some(path) => match SessionManager::fork_from(&path, &self.cwd, None) {
                    Ok(sm) => {
                        let handle = self.swap_session(sm).await;
                        self.respond(success_response(
                            id,
                            "clone",
                            Some(serde_json::json!({ "session_id": handle.session_id() })),
                        ));
                    }
                    Err(e) => self.respond(error_response(id, "clone", e)),
                },
                None => self.respond(error_response(
                    id,
                    "clone",
                    "no current session file to clone",
                )),
            },
            RpcCommand::GetForkMessages { id } => {
                self.respond(success_response(
                    id,
                    "get_fork_messages",
                    Some(serde_json::json!({ "messages": self.session.messages() })),
                ));
            }
        }
    }
    async fn swap_session(&mut self, session_manager: SessionManager) -> AgentSessionHandle {
        let new_session = AgentSession::new(
            Arc::clone(&self.agent),
            self.settings.clone(),
            session_manager,
            self.cwd.clone(),
            self.session_state.clone(),
        );
        install_session_hooks(&new_session);
        let handle = new_session.clone_handle();
        // Replace the active handle. The previous session is dropped here;
        // any in-flight prompts continue running on their cloned handles.
        self.session = handle.clone();
        handle
    }

    fn prompt(&mut self, id: Option<String>, message: String, images: Option<Vec<ImageData>>) {
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

        let session = self.session.clone_handle();
        let prompt_message = match build_user_message(message.clone(), &images) {
            Ok(m) => m,
            Err(e) => {
                self.respond(error_response(id, "prompt", e));
                return;
            }
        };
        self.session.persist_user_message(message.clone());
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
        let session = self.session.clone_handle();

        let agent_run = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to build RPC agent runtime")?;
            runtime.block_on(async {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(agent.run_with_channel_message(prompt_message, event_tx))
                    .await
            })
        });
        let output = self.output.clone();
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

    fn run_bash(&self, id: Option<String>, command: String) {
        if is_dangerous_rpc_command(&command) {
            tracing::warn!("RPC bash command contains dangerous pattern: {:?}", command);
        }

        // Reject a fresh bash command while another is still in flight — the
        // RPC protocol only exposes a single active_bash slot for abort.
        {
            let mut slot = self.active_bash.lock();
            if slot.is_some() {
                self.respond(error_response(
                    id,
                    "bash",
                    "another bash command is already running",
                ));
                return;
            }
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            *slot = Some(tx);
            // Drop the lock before spawning to release the mutex.
            drop(slot);
            self.spawn_bash(id, command, rx);
        }
    }

    fn spawn_bash(
        &self,
        id: Option<String>,
        command: String,
        abort_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let output = self.output.clone();
        let active_bash = Arc::clone(&self.active_bash);
        tokio::spawn(async move {
            use std::process::Stdio;
            use tokio::io::AsyncReadExt;
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg(&command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => {
                    // Clear the active slot before responding so a future
                    // bash command can run.
                    active_bash.lock().take();
                    let _ = output.send(WriterFrame::Response(error_response(
                        id,
                        "bash",
                        error.to_string(),
                    )));
                    return;
                }
            };
            // Detach piped handles BEFORE the select and drain them concurrently
            // with `wait()`. Otherwise, a command that writes more than the OS
            // pipe buffer (~64 KB) would block on write while we wait, deadlocking
            // forever. On the abort path `start_kill()` + `wait()` closes the
            // pipes so the read tasks see EOF and finish with whatever is buffered.
            let mut stdout_pipe = child.stdout.take();
            let mut stderr_pipe = child.stderr.take();
            let stdout_task = tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                if let Some(pipe) = stdout_pipe.as_mut() {
                    let _ = pipe.read_to_end(&mut buf).await;
                }
                buf
            });
            let stderr_task = tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                if let Some(pipe) = stderr_pipe.as_mut() {
                    let _ = pipe.read_to_end(&mut buf).await;
                }
                buf
            });
            let aborted;
            let exit_status = tokio::select! {
                biased;
                _ = abort_rx => {
                    aborted = true;
                    // Abort requested: kill the child and reap it so the
                    // piped streams see EOF and the read tasks finish.
                    let _ = child.start_kill();
                    child.wait().await
                }
                status = child.wait() => {
                    aborted = false;
                    status
                }
            };
            // Collect the concurrent read tasks (they always complete after
            // the child exits / is killed, which closes the pipes).
            let stdout_bytes = stdout_task.await.unwrap_or_default();
            let stderr_bytes = stderr_task.await.unwrap_or_default();
            // Clear the slot regardless of outcome so the next bash can run.
            active_bash.lock().take();
            let response = match exit_status {
                Ok(status) => success_response(
                    id,
                    "bash",
                    Some(serde_json::json!({
                        "stdout": String::from_utf8_lossy(&stdout_bytes),
                        "stderr": String::from_utf8_lossy(&stderr_bytes),
                        "exit_code": status.code(),
                        "aborted": aborted,
                    })),
                ),
                Err(error) => error_response(id, "bash", error.to_string()),
            };
            let _ = output.send(WriterFrame::Response(response));
        });
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
            "steering_mode": self.session.steering_mode(),
            "follow_up_mode": self.session.follow_up_mode(),
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

fn build_user_message(
    text: String,
    images: &Option<Vec<ImageData>>,
) -> Result<oxicode_ai::Message, String> {
    let mut blocks: Vec<oxicode_ai::ContentBlock> = Vec::new();
    if !text.is_empty() {
        blocks.push(oxicode_ai::ContentBlock::Text(
            oxicode_ai::TextContent::new(text),
        ));
    }
    if let Some(images) = images.as_ref() {
        for image in images {
            // Accept either raw base64 or a `data:<mime>;base64,...` prefix.
            let raw = image.source.as_str();
            let (mime, payload) = if let Some(rest) = raw.strip_prefix("data:") {
                match rest.split_once(";base64,") {
                    Some((mime, b64)) => (mime.to_string(), b64.to_string()),
                    None => {
                        return Err(format!(
                            "image source for {} is not a base64 data URL",
                            image.media_type
                        ));
                    }
                }
            } else {
                let mime = if image.media_type.is_empty() {
                    "image/png".to_string()
                } else {
                    image.media_type.clone()
                };
                (mime, raw.to_string())
            };
            blocks.push(oxicode_ai::ContentBlock::Image(
                oxicode_ai::ImageContent::new(payload, mime),
            ));
        }
    }
    if blocks.is_empty() {
        return Err("steer/follow_up requires a non-empty message".to_string());
    }
    Ok(oxicode_ai::Message::User(oxicode_ai::UserMessage::new(
        blocks,
    )))
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
