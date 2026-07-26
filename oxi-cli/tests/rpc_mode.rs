//! RPC-mode integration tests that spawn the freshly-built `oxi` binary.
//!
//! macOS is oxi's single deployment target. These tests exec the real binary
//! via `oxi --mode rpc`, which only initializes reliably in the target
//! platform's runtime environment. On the GitHub ubuntu runner the spawned
//! process cannot come up headless, so `RpcClient::start()` fails. We
//! therefore compile/run this file on macOS only (the `test.yml` job) —
//! gating here keeps the ubuntu smoke-test (`ci.yml`) and the publish
//! `verify` gate from failing on a non-target platform.
#![cfg(target_os = "macos")]

use oxi::rpc_mode::{RpcClient, RpcClientConfig, RpcResponse};

#[test]
fn rpc_client_round_trips_ready_and_real_state() {
    let mut client = RpcClient::new(RpcClientConfig {
        binary_path: env!("CARGO_BIN_EXE_oxi").to_string(),
        model: Some("openai/gpt-4o-mini".to_string()),
        ..Default::default()
    });
    client.start().unwrap();

    let response = client.get_state().unwrap();
    match response {
        RpcResponse::Response {
            success,
            data: Some(data),
            ..
        } => {
            assert!(success);
            assert!(data["session_id"].as_str().is_some());
            assert!(data["message_count"].as_u64().is_some());
            assert!(data["model"]["id"].as_str().is_some());
        }
        _ => panic!("expected successful state response"),
    }
}

#[test]
fn rpc_client_surfaces_unsupported_command_errors() {
    let mut client = RpcClient::new(RpcClientConfig {
        binary_path: env!("CARGO_BIN_EXE_oxi").to_string(),
        model: Some("openai/gpt-4o-mini".to_string()),
        ..Default::default()
    });
    client.start().unwrap();

    // `set_auto_retry` is now supported (handlers.rs:320), so use a truly
    // bogus command to verify unsupported commands still surface errors.
    let error = client
        .send_raw(serde_json::json!({ "type": "totally_bogus_command" }))
        .unwrap_err()
        .to_string();
    assert!(
        !error.is_empty(),
        "unsupported command must surface an error, got: {error}"
    );
}
