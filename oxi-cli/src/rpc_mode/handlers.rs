//! RPC command handlers and the main event loop.

use crate::App;
use anyhow::Result;
use serde_json::Value;
use std::io::{BufRead, Write};
use std::sync::Arc;

use super::protocol::*;
use super::state::RpcServer;

// ============================================================================
// RPC Server Entry Point
// ============================================================================

/// Run the RPC server in stdio mode.
///
/// Reads JSONL commands from stdin, processes them, and writes
/// JSONL responses/events to stdout.
pub async fn run_rpc_mode(app: App) -> Result<()> {
    let mut server = Arc::new(RpcServer::new(0)); // 0 = stdio mode
    let output = RpcOutput::new();

    // Spawn event forwarding task
    let output_clone = output.clone();
    // Take the event receiver - server was just created so Arc hasn't been shared
    let rx = Arc::get_mut(&mut server)
        .expect("server Arc must be unique at construction")
        .take_event_receiver();
    let mut event_rx = rx.expect("event receiver must be available at construction");
    let event_handle = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let json = serde_json::to_value(&event).unwrap_or_default();
            output_clone.write_line(&json);
        }
    });

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line_reader = JsonlLineReader::new();
    let _raw_buf = [0u8; 4096];

    loop {
        // Check for shutdown
        if server.is_shutdown_requested() {
            break;
        }

        // Read from stdin (blocking)
        let mut read_buf = String::new();
        match input.read_line(&mut read_buf) {
            Ok(0) => {
                // EOF reached — flush any remaining buffered data
                if let Some(final_line) = line_reader.flush() {
                    process_line(&final_line, &server, &app, &output).await;
                }
                break;
            }
            Ok(_) => {
                let lines = line_reader.feed(&read_buf);
                for line in lines {
                    process_line(&line, &server, &app, &output).await;
                }
            }
            Err(e) => {
                eprintln!("Error reading stdin: {}", e);
                break;
            }
        }
    }

    // Cleanup
    event_handle.abort();
    Ok(())
}

/// Process a single JSONL input line.
async fn process_line(
    line: &str,
    server: &Arc<RpcServer>,
    app: &App,
    output: &RpcOutput,
) {
    let value = match parse_json_line(line) {
        Ok(v) => v,
        Err(e) => {
            let error_response = RpcResponse::Response {
                id: None,
                command: "parse".to_string(),
                success: false,
                data: None,
                error: Some(format!("Failed to parse command: {}", e)),
            };
            output.write_obj(&error_response);
            return;
        }
    };

    // Check if this is a JSON-RPC 2.0 request
    if let Some(obj) = value.as_object() {
        if obj.contains_key("jsonrpc") {
            handle_jsonrpc_request(obj, server, app, output);
            return;
        }
    }

    // Handle extension UI responses
    if let Some(obj) = value.as_object() {
        if obj.get("type").and_then(|v| v.as_str()) == Some("extension_ui_response") {
            if let Ok(response) = serde_json::from_value::<RpcExtensionUiResponse>(value.clone()) {
                let id = match &response {
                    RpcExtensionUiResponse::ExtensionUiResponse { id, .. } => id.clone(),
                };
                server.resolve_extension_request(&id, response).await;
            }
            return;
        }
    }

    // Handle as native RPC command
    match serde_json::from_value::<RpcCommand>(value) {
        Ok(command) => {
            let response = execute_command(server, app, command);
            output.write_obj(&response);
        }
        Err(e) => {
            let error_response = RpcResponse::Response {
                id: None,
                command: "parse".to_string(),
                success: false,
                data: None,
                error: Some(format!("Parse error: {}", e)),
            };
            output.write_obj(&error_response);
        }
    }
}

/// Handle a JSON-RPC 2.0 request.
fn handle_jsonrpc_request(
    obj: &serde_json::Map<String, Value>,
    server: &Arc<RpcServer>,
    app: &App,
    output: &RpcOutput,
) {
    let jsonrpc = obj.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
    if jsonrpc != "2.0" {
        let err = JsonRpcErrorResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::Null,
            error: JsonRpcError {
                code: JSONRPC_INVALID_REQUEST,
                message: "Invalid jsonrpc version".to_string(),
                data: None,
            },
        };
        output.write_obj(&err);
        return;
    }

    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let method = match obj.get("method").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => {
            let err = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id,
                error: JsonRpcError {
                    code: JSONRPC_INVALID_REQUEST,
                    message: "Missing method".to_string(),
                    data: None,
                },
            };
            output.write_obj(&err);
            return;
        }
    };

    let params = obj.get("params").cloned();

    // Map to internal command
    let cmd_value = match jsonrpc_to_command(method, params, Some(id.clone())) {
        Some(v) => v,
        None => {
            let err = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id,
                error: JsonRpcError {
                    code: JSONRPC_METHOD_NOT_FOUND,
                    message: format!("Method not found: {}", method),
                    data: None,
                },
            };
            output.write_obj(&err);
            return;
        }
    };

    // Parse and execute
    match serde_json::from_value::<RpcCommand>(cmd_value) {
        Ok(command) => {
            let response = execute_command(server, app, command);
            let jsonrpc_response = rpc_response_to_jsonrpc(&response, id);
            if !jsonrpc_response.is_empty() {
                output.write_raw(&format!("{}\n", jsonrpc_response));
            }
        }
        Err(e) => {
            let err = JsonRpcErrorResponse {
                jsonrpc: "2.0".to_string(),
                id,
                error: JsonRpcError {
                    code: JSONRPC_INVALID_PARAMS,
                    message: format!("Invalid params: {}", e),
                    data: None,
                },
            };
            output.write_obj(&err);
        }
    }
}

/// Execute an RPC command and return the response.
fn execute_command(server: &Arc<RpcServer>, _app: &App, command: RpcCommand) -> RpcResponse {
    match command {
        // ── Prompting ──────────────────────────────────────────────
        RpcCommand::Prompt {
            id,
            message: _,
            images,
            streaming_behavior: _,
        } => {
            let _image_sources = RpcServer::parse_images(images);

            server.update_session_state(|s| {
                s.is_streaming = true;
                s.pending_message_count += 1;
            });

            server.emit_event(RpcEvent::AgentStart);

            RpcResponse::Response {
                id,
                command: "prompt".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::Steer {
            id,
            message: _,
            images: _,
        } => {
            server.update_session_state(|s| {
                s.steering_mode = "one_at_a_time".to_string();
            });

            RpcResponse::Response {
                id,
                command: "steer".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::FollowUp {
            id,
            message: _,
            images: _,
        } => {
            server.update_session_state(|s| {
                s.follow_up_mode = "one_at_a_time".to_string();
            });

            RpcResponse::Response {
                id,
                command: "follow_up".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::Abort { id } => {
            server.update_session_state(|s| {
                s.is_streaming = false;
            });
            server.emit_event(RpcEvent::AgentEnd);

            RpcResponse::Response {
                id,
                command: "abort".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::NewSession {
            id,
            parent_session: _,
        } => {
            server.update_session_state(|s| {
                s.session_id = uuid::Uuid::new_v4().to_string();
                s.message_count = 0;
                s.pending_message_count = 0;
            });

            RpcResponse::Response {
                id,
                command: "new_session".to_string(),
                success: true,
                data: Some(serde_json::json!({ "cancelled": false })),
                error: None,
            }
        }

        // ── State ──────────────────────────────────────────────────
        RpcCommand::GetState { id } => {
            let state = server.get_session_state();

            RpcResponse::Response {
                id,
                command: "get_state".to_string(),
                success: true,
                data: Some(serde_json::to_value(&state).expect("state should be serializable")),
                error: None,
            }
        }

        // ── Model ──────────────────────────────────────────────────
        RpcCommand::SetModel {
            id,
            provider,
            model_id,
        } => {
            // In a real implementation, this would switch the model
            server.update_session_state(|s| {
                s.model = Some(ModelInfo {
                    provider: provider.clone(),
                    id: model_id.clone(),
                });
            });

            RpcResponse::Response {
                id,
                command: "set_model".to_string(),
                success: true,
                data: Some(serde_json::json!({
                    "provider": provider,
                    "id": model_id
                })),
                error: None,
            }
        }

        RpcCommand::CycleModel { id } => RpcResponse::Response {
            id,
            command: "cycle_model".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "model": null,
                "thinking_level": "default",
                "is_scoped": false
            })),
            error: None,
        },

        RpcCommand::GetAvailableModels { id } => RpcResponse::Response {
            id,
            command: "get_available_models".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "models": []
            })),
            error: None,
        },

        // ── Thinking ───────────────────────────────────────────────
        RpcCommand::SetThinkingLevel { id, level } => {
            server.update_session_state(|s| {
                s.thinking_level = level;
            });

            RpcResponse::Response {
                id,
                command: "set_thinking_level".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::CycleThinkingLevel { id } => {
            let current = server.get_session_state().thinking_level;
            let next = match current.as_str() {
                "off" => "default",
                "default" => "medium",
                "medium" => "high",
                _ => "off",
            };

            server.update_session_state(|s| {
                s.thinking_level = next.to_string();
            });

            RpcResponse::Response {
                id,
                command: "cycle_thinking_level".to_string(),
                success: true,
                data: Some(serde_json::json!({ "level": next })),
                error: None,
            }
        }

        // ── Queue modes ────────────────────────────────────────────
        RpcCommand::SetSteeringMode { id, mode } => {
            server.update_session_state(|s| {
                s.steering_mode = mode;
            });

            RpcResponse::Response {
                id,
                command: "set_steering_mode".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        RpcCommand::SetFollowUpMode { id, mode } => {
            server.update_session_state(|s| {
                s.follow_up_mode = mode;
            });

            RpcResponse::Response {
                id,
                command: "set_follow_up_mode".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        // ── Compaction ─────────────────────────────────────────────
        RpcCommand::Compact {
            id,
            custom_instructions: _,
        } => {
            server.update_session_state(|s| {
                s.is_compacting = true;
            });

            // Simulate compaction
            let state = server.get_session_state();
            let result = CompactionResult {
                original_count: state.message_count,
                compacted_count: (state.message_count as f32 * 0.7) as usize,
                tokens_saved: Some(1000),
            };

            server.update_session_state(|s| {
                s.is_compacting = false;
                s.message_count = result.compacted_count;
            });

            RpcResponse::Response {
                id,
                command: "compact".to_string(),
                success: true,
                data: Some(serde_json::to_value(&result).expect("compact result should be serializable")),
                error: None,
            }
        }

        RpcCommand::SetAutoCompaction { id, enabled } => {
            server.update_session_state(|s| {
                s.auto_compaction_enabled = enabled;
            });

            RpcResponse::Response {
                id,
                command: "set_auto_compaction".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        // ── Retry ─────────────────────────────────────────────────
        RpcCommand::SetAutoRetry { id, enabled: _ } => RpcResponse::Response {
            id,
            command: "set_auto_retry".to_string(),
            success: true,
            data: None,
            error: None,
        },

        RpcCommand::AbortRetry { id } => RpcResponse::Response {
            id,
            command: "abort_retry".to_string(),
            success: true,
            data: None,
            error: None,
        },

        // ── Bash ───────────────────────────────────────────────────
        RpcCommand::Bash { id, command } => {
            if is_dangerous_rpc_command(&command) {
                tracing::warn!(
                    "RPC bash command contains dangerous pattern: {:?}",
                    command
                );
            }
            let output_result = std::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output();

            match output_result {
                Ok(output) => RpcResponse::Response {
                    id,
                    command: "bash".to_string(),
                    success: true,
                    data: Some(serde_json::json!({
                        "stdout": String::from_utf8_lossy(&output.stdout),
                        "stderr": String::from_utf8_lossy(&output.stderr),
                        "exit_code": output.status.code()
                    })),
                    error: None,
                },
                Err(e) => RpcResponse::Response {
                    id,
                    command: "bash".to_string(),
                    success: false,
                    data: None,
                    error: Some(e.to_string()),
                },
            }
        }

        RpcCommand::AbortBash { id } => RpcResponse::Response {
            id,
            command: "abort_bash".to_string(),
            success: true,
            data: None,
            error: None,
        },

        // ── Session ────────────────────────────────────────────────
        RpcCommand::GetSessionStats { id } => {
            let state = server.get_session_state();
            let stats = SessionStats {
                message_count: state.message_count,
                token_count: None,
                last_activity: None,
            };

            RpcResponse::Response {
                id,
                command: "get_session_stats".to_string(),
                success: true,
                data: Some(serde_json::to_value(&stats).expect("session stats should be serializable")),
                error: None,
            }
        }

        RpcCommand::ExportHtml { id, output_path: _ } => RpcResponse::Response {
            id,
            command: "export_html".to_string(),
            success: true,
            data: Some(serde_json::json!({ "path": "session.html" })),
            error: None,
        },

        RpcCommand::SwitchSession {
            id,
            session_path: _,
        } => {
            server.update_session_state(|s| {
                s.session_id = uuid::Uuid::new_v4().to_string();
                s.message_count = 0;
            });

            RpcResponse::Response {
                id,
                command: "switch_session".to_string(),
                success: true,
                data: Some(serde_json::json!({ "cancelled": false })),
                error: None,
            }
        }

        RpcCommand::Fork { id, entry_id: _ } => RpcResponse::Response {
            id,
            command: "fork".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "text": "",
                "cancelled": false
            })),
            error: None,
        },

        RpcCommand::Clone { id } => RpcResponse::Response {
            id,
            command: "clone".to_string(),
            success: true,
            data: Some(serde_json::json!({ "cancelled": false })),
            error: None,
        },

        RpcCommand::GetForkMessages { id } => RpcResponse::Response {
            id,
            command: "get_fork_messages".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "messages": []
            })),
            error: None,
        },

        RpcCommand::GetLastAssistantText { id } => RpcResponse::Response {
            id,
            command: "get_last_assistant_text".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "text": null
            })),
            error: None,
        },

        RpcCommand::SetSessionName { id, name } => {
            server.update_session_state(|s| {
                s.session_name = if name.is_empty() { None } else { Some(name) };
            });

            RpcResponse::Response {
                id,
                command: "set_session_name".to_string(),
                success: true,
                data: None,
                error: None,
            }
        }

        // ── Messages ───────────────────────────────────────────────
        RpcCommand::GetMessages { id } => RpcResponse::Response {
            id,
            command: "get_messages".to_string(),
            success: true,
            data: Some(serde_json::json!({
                "messages": []
            })),
            error: None,
        },

        // ── Commands ────────────────────────────────────────────────
        RpcCommand::GetCommands { id } => {
            let commands = vec![
                CommandInfo {
                    name: "compact".to_string(),
                    description: Some("Compact context".to_string()),
                    source: "builtin".to_string(),
                    source_info: None,
                },
                CommandInfo {
                    name: "clear".to_string(),
                    description: Some("Clear conversation".to_string()),
                    source: "builtin".to_string(),
                    source_info: None,
                },
            ];

            RpcResponse::Response {
                id,
                command: "get_commands".to_string(),
                success: true,
                data: Some(serde_json::json!({ "commands": commands })),
                error: None,
            }
        }
    }
}

/// Check for dangerous patterns in RPC bash commands.
fn is_dangerous_rpc_command(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    lower.contains("/etc/passwd")
        || lower.contains("id_rsa")
        || lower.contains("curl | nc")
        || lower.contains("/dev/tcp/")
        || lower.contains("rm -rf /")
        || lower.contains("> /etc/")
        || lower.contains("mkfifo")
}
