use oxi::rpc_mode::{RpcClient, RpcClientConfig, RpcResponse};

#[test]
fn rpc_client_round_trips_ready_and_real_state() {
    let mut client = RpcClient::new(RpcClientConfig {
        binary_path: env!("CARGO_BIN_EXE_oxi").to_string(),
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
        ..Default::default()
    });
    client.start().unwrap();

    let error = client.set_auto_retry(false).unwrap_err().to_string();
    assert!(error.contains("set_auto_retry is not yet supported"));
}
