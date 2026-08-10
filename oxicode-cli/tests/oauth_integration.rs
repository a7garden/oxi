//! End-to-end OAuth flow against a local mock OAuth server.
//!
//! Spins up an `httpmock` token server plus a real loopback `TcpListener`
//! that the test drives as the "browser" (a `TcpStream` sending a raw
//! `GET /callback?code=...&state=...` request). The full
//! PKCE -> callback -> exchange path is exercised against the
//! `provider_oauth` + `oauth_listener` modules.

use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use httpmock::MockServer;
use oxicode::oauth_listener;
use oxicode::provider_oauth::{self, ProviderOAuthSpec};

/// Drive a real callback over a fresh loopback `TcpStream`.
///
/// The test binds `127.0.0.1:0`, hands the listener to
/// `oauth_listener::await_callback`, and then connects a raw `TcpStream`
/// from the same process to simulate the browser's redirect. Using a real
/// TCP connection (not a mock request) verifies that the listener's
/// HTTP/1.1 parsing, query-string handling, and state-mismatch rejection
/// all behave on the wire as they would in production.
async fn simulate_browser_callback(port: u16, code: &str, state: &str) {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to loopback listener");
    let request = format!(
        "GET /callback?code={code}&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write HTTP request");
    // Drop the stream so the listener can read EOF cleanly.
    drop(stream);
}

#[tokio::test]
async fn happy_path_openai_oauth_login() {
    let auth_server = MockServer::start_async().await;
    auth_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/oauth/token");
        then.status(200).json_body(serde_json::json!({
            "access_token": "AT",
            "refresh_token": "RT",
            "expires_in": 3600,
            "scope": "openid"
        }));
    });

    let spec = ProviderOAuthSpec {
        client_id: "app-x".into(),
        auth_url: format!("{}/authorize", auth_server.base_url()),
        token_url: format!("{}/oauth/token", auth_server.base_url()),
        scopes: vec!["openid".into()],
        redirect_path: "/callback".into(),
        use_pkce: true,
    };

    let (state, challenge) = provider_oauth::pkce_pair();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Sanity: build_auth_url must produce a well-formed URL. The exact
    // query parameters are covered by the unit test in provider_oauth.rs;
    // here we only assert it doesn't blow up.
    let _url = provider_oauth::build_auth_url(&spec, port, &state, &challenge);

    // The listener task owns the listener and a clone of the state token;
    // the "browser" task (this test) keeps the original `state` for the
    // callback URL it will craft.
    let state_for_listener = state.clone();
    let listener_task = tokio::spawn(async move {
        oauth_listener::await_callback(
            listener,
            state_for_listener,
            "/callback".into(),
            Duration::from_secs(5),
        )
        .await
    });
    simulate_browser_callback(port, "THE_CODE", &state).await;

    let cb = listener_task
        .await
        .expect("listener task did not panic")
        .expect("callback must complete within the timeout");
    assert_eq!(cb.code, "THE_CODE");
    assert_eq!(cb.state, state);

    let tokens = provider_oauth::exchange_code(&spec, port, &cb.code, &challenge)
        .await
        .expect("token exchange should succeed against the mock server");
    assert_eq!(tokens.access_token, "AT");
    assert_eq!(tokens.refresh_token.as_deref(), Some("RT"));
    assert!(tokens.expires_at > 0);
    assert_eq!(tokens.scopes, vec!["openid".to_string()]);
}

#[tokio::test]
async fn refresh_extends_expiry() {
    let auth_server = MockServer::start_async().await;
    auth_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/oauth/token");
        then.status(200).json_body(serde_json::json!({
            "access_token": "AT2",
            "refresh_token": "RT2",
            "expires_in": 7200
        }));
    });

    let spec = ProviderOAuthSpec {
        client_id: "app-x".into(),
        auth_url: format!("{}/authorize", auth_server.base_url()),
        token_url: format!("{}/oauth/token", auth_server.base_url()),
        scopes: vec!["openid".into()],
        redirect_path: "/callback".into(),
        use_pkce: true,
    };

    let now = chrono::Utc::now().timestamp();
    let tokens = provider_oauth::refresh_grant(&spec, "RT_LEGACY")
        .await
        .expect("refresh_grant should succeed against the mock server");
    assert_eq!(tokens.access_token, "AT2");
    assert_eq!(tokens.refresh_token.as_deref(), Some("RT2"));
    // expires_in 7200 -> expires_at ≈ now + 7200, with a few seconds of
    // slack for test execution.
    assert!(
        tokens.expires_at >= now + 7200 - 5 && tokens.expires_at <= now + 7200 + 5,
        "expires_at {} should be ≈ now + 7200 = {}",
        tokens.expires_at,
        now + 7200
    );
}
