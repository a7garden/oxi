//! Tests for the RPC mode module.

use super::*;
use serde_json::Value;
use std::sync::Arc;

// ── JSONL Tests ────────────────────────────────────────────────

#[test]
fn test_serialize_json_line() {
    let value = serde_json::json!({"type": "test", "data": 42});
    let line = protocol::serialize_json_line(&value);
    assert!(line.ends_with('\n'));
    assert!(!line.contains("\r\n"));
    let parsed: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["type"], "test");
    assert_eq!(parsed["data"], 42);
}

#[test]
fn test_parse_json_line() {
    let line = r#"{"type":"test"}"#;
    let value = protocol::parse_json_line(line).unwrap();
    assert_eq!(value["type"], "test");
}

#[test]
fn test_parse_json_line_trailing_cr() {
    let line = "{\"type\":\"test\"}\r";
    let value = protocol::parse_json_line(line).unwrap();
    assert_eq!(value["type"], "test");
}

#[test]
fn test_jsonl_line_reader_basic() {
    let mut reader = protocol::JsonlLineReader::new();
    let lines = reader.feed("{\"a\":1}\n{\"b\":2}\n");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "{\"a\":1}");
    assert_eq!(lines[1], "{\"b\":2}");
}

#[test]
fn test_jsonl_line_reader_partial() {
    let mut reader = protocol::JsonlLineReader::new();
    let lines = reader.feed("{\"a\":1");
    assert!(lines.is_empty());
    let lines2 = reader.feed("}\n");
    assert_eq!(lines2.len(), 1);
    assert_eq!(lines2[0], "{\"a\":1}");
}

#[test]
fn test_jsonl_line_reader_flush() {
    let mut reader = protocol::JsonlLineReader::new();
    reader.feed("{\"partial\":true");
    assert!(reader.has_buffered_data());
    let final_line = reader.flush();
    assert_eq!(final_line, Some("{\"partial\":true".to_string()));
    assert!(!reader.has_buffered_data());
}

#[test]
fn test_jsonl_line_reader_empty_lines() {
    let mut reader = protocol::JsonlLineReader::new();
    let lines = reader.feed("\n\n\n");
    assert!(lines.is_empty());
}

// ── JSON-RPC 2.0 Tests ────────────────────────────────────────

#[test]
fn test_jsonrpc_method_mapping() {
    let cmd = protocol::jsonrpc_to_command(
        "prompt",
        Some(serde_json::json!({"message": "hello"})),
        Some(Value::String("1".to_string())),
    );
    assert!(cmd.is_some());
    let cmd = cmd.unwrap();
    assert_eq!(cmd["type"], "prompt");
    assert_eq!(cmd["message"], "hello");
    assert_eq!(cmd["id"], "1");
}

#[test]
fn test_jsonrpc_method_mapping_unknown() {
    let cmd = protocol::jsonrpc_to_command("nonexistent", None, Some(Value::String("1".to_string())));
    assert!(cmd.is_none());
}

#[test]
fn test_jsonrpc_to_command_all_methods() {
    let methods = [
        "prompt", "steer", "follow_up", "abort", "new_session",
        "get_state", "set_model", "cycle_model", "get_available_models",
        "set_thinking_level", "cycle_thinking_level",
        "set_steering_mode", "set_follow_up_mode",
        "compact", "set_auto_compaction",
        "set_auto_retry", "abort_retry",
        "bash", "abort_bash",
        "get_session_stats", "export_html", "switch_session",
        "fork", "clone", "get_fork_messages",
        "get_last_assistant_text", "set_session_name",
        "get_messages", "get_commands",
    ];
    for method in methods {
        let cmd = protocol::jsonrpc_to_command(method, None, Some(Value::Number(1.into())));
        assert!(cmd.is_some(), "Method {} should map to a command", method);
    }
}

#[test]
fn test_rpc_response_to_jsonrpc_success() {
    let response = protocol::RpcResponse::Response {
        id: Some("1".to_string()),
        command: "get_state".to_string(),
        success: true,
        data: Some(serde_json::json!({"session_id": "abc"})),
        error: None,
    };
    let json = protocol::rpc_response_to_jsonrpc(&response, Value::String("1".to_string()));
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], "1");
    assert_eq!(parsed["result"]["session_id"], "abc");
}

#[test]
fn test_rpc_response_to_jsonrpc_error() {
    let response = protocol::RpcResponse::Response {
        id: Some("1".to_string()),
        command: "test".to_string(),
        success: false,
        data: None,
        error: Some("Something failed".to_string()),
    };
    let json = protocol::rpc_response_to_jsonrpc(&response, Value::String("1".to_string()));
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["error"]["code"], protocol::JSONRPC_INTERNAL_ERROR);
    assert_eq!(parsed["error"]["message"], "Something failed");
}

// ── RPC Server tests ─────────────────────────────────────────────

#[test]
fn test_rpc_server_new() {
    let server = state::RpcServer::new(8080);
    assert_eq!(server.port(), 8080);
    assert!(!server.is_shutdown_requested());
}

#[test]
fn test_rpc_server_shutdown() {
    let server = Arc::new(state::RpcServer::new(0));
    assert!(!server.is_shutdown_requested());
    server.request_shutdown();
    assert!(server.is_shutdown_requested());
}

#[test]
fn test_rpc_server_session_state() {
    let server = Arc::new(state::RpcServer::new(0));
    let state_val = server.get_session_state();
    assert_eq!(state_val.message_count, 0);
    assert_eq!(state_val.thinking_level, "default");
    assert!(state_val.auto_compaction_enabled);

    server.update_session_state(|s| {
        s.message_count = 10;
        s.thinking_level = "high".to_string();
    });

    let new_state = server.get_session_state();
    assert_eq!(new_state.message_count, 10);
    assert_eq!(new_state.thinking_level, "high");
}

#[test]
fn test_rpc_server_model_state() {
    let server = Arc::new(state::RpcServer::new(0));
    assert!(server.get_session_state().model.is_none());

    server.update_session_state(|s| {
        s.model = Some(protocol::ModelInfo {
            provider: "anthropic".to_string(),
            id: "claude-3-opus".to_string(),
        });
    });

    let state_val = server.get_session_state();
    assert!(state_val.model.is_some());
    let model = state_val.model.unwrap();
    assert_eq!(model.provider, "anthropic");
    assert_eq!(model.id, "claude-3-opus");
}

#[test]
fn test_parse_images_data_uri() {
    let images = vec![protocol::ImageData {
        source: "data:image/png;base64,iVBORw0KGgo=".to_string(),
        media_type: "image/png".to_string(),
    }];
    let parsed = state::RpcServer::parse_images(Some(images));
    assert_eq!(parsed.len(), 1);
    assert!(!parsed[0].data.is_empty());
    assert_eq!(parsed[0].mime_type, "image/png");
}

#[test]
fn test_parse_images_empty() {
    let parsed = state::RpcServer::parse_images(None);
    assert!(parsed.is_empty());
}

#[test]
fn test_parse_images_non_data_uri() {
    let images = vec![protocol::ImageData {
        source: "https://example.com/image.png".to_string(),
        media_type: "image/png".to_string(),
    }];
    let parsed = state::RpcServer::parse_images(Some(images));
    assert!(parsed.is_empty());
}

// ── Session Handoff Tests ─────────────────────────────────────────

#[test]
fn test_session_handoff_round_trip() {
    let state_val = protocol::SessionState {
        model: Some(protocol::ModelInfo {
            provider: "test".to_string(),
            id: "model-1".to_string(),
        }),
        thinking_level: "high".to_string(),
        is_streaming: false,
        is_compacting: false,
        steering_mode: "all".to_string(),
        follow_up_mode: "all".to_string(),
        session_file: Some("/tmp/session.json".to_string()),
        session_id: "abc-123".to_string(),
        session_name: Some("Test Session".to_string()),
        auto_compaction_enabled: true,
        message_count: 42,
        pending_message_count: 0,
    };

    let handoff = protocol::SessionHandoff::from_state(&state_val);
    assert_eq!(handoff.session_id, "abc-123");
    assert_eq!(handoff.message_count, 42);
    assert_eq!(handoff.model_id, Some("test/model-1".to_string()));

    let json = handoff.to_json().unwrap();
    let decoded = protocol::SessionHandoff::from_json(&json).unwrap();
    assert_eq!(decoded.session_id, "abc-123");
    assert_eq!(decoded.message_count, 42);
}

// ── Event Tests ─────────────────────────────────────────────────

#[test]
fn test_rpc_event_serialization() {
    let event = protocol::RpcEvent::TextChunk {
        text: "hello".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"text_chunk\""));
    assert!(json.contains("\"text\":\"hello\""));
}

#[test]
fn test_rpc_event_agent_start() {
    let event = protocol::RpcEvent::AgentStart;
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"agent_start\""));
}

#[test]
fn test_rpc_event_agent_end() {
    let event = protocol::RpcEvent::AgentEnd;
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"agent_end\""));
}

// ── Paste handler tests ──────────────────────────────────────────

#[test]
fn test_paste_handler_initial_state() {
    let handler = utils::PasteHandler::new();
    assert_eq!(handler.state(), utils::PasteState::Normal);
    assert!(handler.buffer().is_empty());
}

#[test]
fn test_paste_handler_reset() {
    let mut handler = utils::PasteHandler::new();
    handler.buffer.push(b't');
    handler.state = utils::PasteState::Pasting;
    handler.reset();
    assert_eq!(handler.state(), utils::PasteState::Normal);
    assert!(handler.buffer().is_empty());
}

#[test]
fn test_paste_handler_paste_start_sequence() {
    let mut handler = utils::PasteHandler::new();

    assert!(handler.process_byte(0x1B).is_none());
    assert_eq!(handler.state(), utils::PasteState::Normal);
    assert!(handler.process_byte(0x5B).is_none());
    assert!(handler.process_byte(0x32).is_none());
    assert!(handler.process_byte(0x30).is_none());
    assert!(handler.process_byte(0x30).is_none());
    assert!(handler.process_byte(0x7E).is_none());

    assert_eq!(handler.state(), utils::PasteState::Pasting);
}

#[test]
fn test_paste_handler_extract_image_png() {
    let mut handler = utils::PasteHandler::new();
    handler.buffer = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
        0x00, 0x00, 0x00, 0x0D,
        0x49, 0x48, 0x44, 0x52,
    ];
    let result = handler.extract_image_data();
    assert!(result.is_some());
    assert!(result.unwrap().starts_with(&[0x89, 0x50, 0x4E, 0x47]));
}

#[test]
fn test_paste_handler_extract_image_jpeg() {
    let mut handler = utils::PasteHandler::new();
    handler.buffer = vec![
        0xFF, 0xD8, 0xFF, 0xE0,
        0x00, 0x10, 0x4A, 0x46,
    ];
    let result = handler.extract_image_data();
    assert!(result.is_some());
    assert!(result.unwrap().starts_with(&[0xFF, 0xD8, 0xFF]));
}

#[test]
fn test_paste_handler_extract_image_none() {
    let mut handler = utils::PasteHandler::new();
    handler.buffer = b"hello world".to_vec();
    let result = handler.extract_image_data();
    assert!(result.is_none());
}

#[test]
fn test_paste_handler_extract_image_short_buffer() {
    let handler = utils::PasteHandler::new();
    let result = handler.extract_image_data();
    assert!(result.is_none());
}

#[test]
fn test_paste_handler_extract_image_with_nulls() {
    let mut handler = utils::PasteHandler::new();
    let mut buffer = vec![0u8; 100];
    for i in 10..20 {
        buffer[i] = 0;
    }
    handler.buffer = buffer;
    let result = handler.extract_image_data();
    assert!(result.is_some());
}

// ── Session state serialization tests ───────────────────────────

#[test]
fn test_session_state_serialization() {
    let state_val = protocol::SessionState {
        model: Some(protocol::ModelInfo {
            provider: "anthropic".to_string(),
            id: "claude-3-opus".to_string(),
        }),
        thinking_level: "high".to_string(),
        is_streaming: true,
        is_compacting: false,
        steering_mode: "all".to_string(),
        follow_up_mode: "one_at_a_time".to_string(),
        session_file: Some("/tmp/session.json".to_string()),
        session_id: "test-123".to_string(),
        session_name: Some("Test Session".to_string()),
        auto_compaction_enabled: true,
        message_count: 42,
        pending_message_count: 1,
    };

    let json = serde_json::to_string(&state_val).unwrap();
    assert!(json.contains("\"thinking_level\":\"high\""));
    assert!(json.contains("\"is_streaming\":true"));
    assert!(json.contains("\"message_count\":42"));
    assert!(json.contains("\"session_file\":\"/tmp/session.json\""));
}

#[test]
fn test_session_state_skips_optional_fields() {
    let state_val = protocol::SessionState {
        model: None,
        thinking_level: "default".to_string(),
        is_streaming: false,
        is_compacting: false,
        steering_mode: "all".to_string(),
        follow_up_mode: "all".to_string(),
        session_file: None,
        session_id: "test".to_string(),
        session_name: None,
        auto_compaction_enabled: true,
        message_count: 0,
        pending_message_count: 0,
    };

    let json = serde_json::to_string(&state_val).unwrap();
    assert!(!json.contains("\"model\""));
    assert!(!json.contains("\"session_file\""));
    assert!(!json.contains("\"session_name\""));
}

#[test]
fn test_command_info_serialization() {
    let cmd = protocol::CommandInfo {
        name: "test".to_string(),
        description: Some("Test command".to_string()),
        source: "builtin".to_string(),
        source_info: None,
    };

    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"name\":\"test\""));
    assert!(json.contains("\"source\":\"builtin\""));
    assert!(!json.contains("\"source_info\""));
}

#[test]
fn test_command_info_with_source_info() {
    let cmd = protocol::CommandInfo {
        name: "skill:deploy".to_string(),
        description: Some("Deploy skill".to_string()),
        source: "skill".to_string(),
        source_info: Some(protocol::SourceInfo {
            path: Some("~/.oxi/skills/deploy.md".to_string()),
            origin: Some("user".to_string()),
        }),
    };

    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"source_info\""));
    assert!(json.contains("deploy.md"));
}

// ── Image data tests ─────────────────────────────────────────────

#[test]
fn test_image_data_deserialization() {
    let json = r#"{"source":"data:image/png;base64,ABC123","type":"image/png"}"#;
    let data: protocol::ImageData = serde_json::from_str(json).unwrap();
    assert_eq!(data.source, "data:image/png;base64,ABC123");
    assert_eq!(data.media_type, "image/png");
}

// ── RPC Response tests ───────────────────────────────────────────

#[test]
fn test_rpc_response_success() {
    let response = protocol::RpcResponse::Response {
        id: Some("123".to_string()),
        command: "test".to_string(),
        success: true,
        data: Some(serde_json::json!({"result": "ok"})),
        error: None,
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"success\":true"));
    assert!(json.contains("\"command\":\"test\""));
}

#[test]
fn test_rpc_response_error() {
    let response = protocol::RpcResponse::Response {
        id: Some("123".to_string()),
        command: "test".to_string(),
        success: false,
        data: None,
        error: Some("Something went wrong".to_string()),
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"success\":false"));
    assert!(json.contains("\"error\":\"Something went wrong\""));
}

#[test]
fn test_rpc_extension_ui_request_select() {
    let request = protocol::RpcExtensionUiRequest::Select {
        id: "req-123".to_string(),
        title: "Select an option".to_string(),
        options: vec!["Option 1".to_string(), "Option 2".to_string()],
        timeout: Some(5000),
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"method\":\"select\""));
    assert!(json.contains("\"title\":\"Select an option\""));
}

#[test]
fn test_rpc_extension_ui_request_notify() {
    let request = protocol::RpcExtensionUiRequest::Notify {
        id: "req-456".to_string(),
        message: "Hello!".to_string(),
        notify_type: Some("info".to_string()),
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"method\":\"notify\""));
    assert!(json.contains("\"message\":\"Hello!\""));
}

// ── Compaction result tests ──────────────────────────────────────

#[test]
fn test_compaction_result_serialization() {
    let result = protocol::CompactionResult {
        original_count: 100,
        compacted_count: 30,
        tokens_saved: Some(5000),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"original_count\":100"));
    assert!(json.contains("\"compacted_count\":30"));
}

#[test]
fn test_session_stats_serialization() {
    let stats = protocol::SessionStats {
        message_count: 50,
        token_count: Some(10000),
        last_activity: Some(1699000000),
    };

    let json = serde_json::to_string(&stats).unwrap();
    assert!(json.contains("\"message_count\":50"));
    assert!(json.contains("\"token_count\":10000"));
}

// ── RpcOutput tests ──────────────────────────────────────────────

#[test]
fn test_rpc_output_write_obj() {
    let output = protocol::RpcOutput::new();
    // This just tests it doesn't panic
    let response = protocol::RpcResponse::Response {
        id: Some("test".to_string()),
        command: "test".to_string(),
        success: true,
        data: None,
        error: None,
    };
    output.write_obj(&response);
}

// ── Command deserialization tests ─────────────────────────────────

#[test]
fn test_deserialize_prompt_with_streaming_behavior() {
    let json = r#"{
            "type": "prompt",
            "message": "hello",
            "streaming_behavior": "steer"
        }"#;
    let cmd: protocol::RpcCommand = serde_json::from_str(json).unwrap();
    match cmd {
        protocol::RpcCommand::Prompt {
            streaming_behavior, ..
        } => {
            assert_eq!(streaming_behavior, Some("steer".to_string()));
        }
        _ => panic!("Expected Prompt command"),
    }
}

#[test]
fn test_deserialize_prompt_without_streaming_behavior() {
    let json = r#"{"type": "prompt", "message": "hello"}"#;
    let cmd: protocol::RpcCommand = serde_json::from_str(json).unwrap();
    match cmd {
        protocol::RpcCommand::Prompt {
            streaming_behavior, ..
        } => {
            assert_eq!(streaming_behavior, None);
        }
        _ => panic!("Expected Prompt command"),
    }
}

// ── JSON-RPC error response tests ────────────────────────────────

#[test]
fn test_jsonrpc_error_response_serialization() {
    let resp = protocol::JsonRpcErrorResponse {
        jsonrpc: "2.0".to_string(),
        id: Value::Number(1.into()),
        error: protocol::JsonRpcError {
            code: protocol::JSONRPC_METHOD_NOT_FOUND,
            message: "Method not found".to_string(),
            data: None,
        },
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"code\":-32601"));
}

#[test]
fn test_jsonrpc_success_response_serialization() {
    let resp = protocol::JsonRpcSuccessResponse {
        jsonrpc: "2.0".to_string(),
        id: Value::String("abc".to_string()),
        result: serde_json::json!({"status": "ok"}),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"result\""));
    assert!(json.contains("\"status\":\"ok\""));
}

// ── RPC Client config tests ──────────────────────────────────────

#[test]
fn test_rpc_client_config_default() {
    let config = utils::RpcClientConfig::default();
    assert_eq!(config.binary_path, "oxi");
    assert!(config.cwd.is_none());
    assert!(config.provider.is_none());
    assert!(config.model.is_none());
    assert!(config.args.is_empty());
}

// ── Source Info tests ────────────────────────────────────────────

#[test]
fn test_source_info_serialization() {
    let info = protocol::SourceInfo {
        path: Some("/path/to/ext".to_string()),
        origin: Some("user".to_string()),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("/path/to/ext"));
}

#[test]
fn test_source_info_skip_empty() {
    let info = protocol::SourceInfo {
        path: None,
        origin: None,
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(!json.contains("path"));
    assert!(!json.contains("origin"));
}
