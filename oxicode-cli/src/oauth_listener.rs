//! Single-shot HTTP listener for OAuth `authorization_code` callbacks.
//! Binds an ephemeral 127.0.0.1 port, accepts one connection, parses the
//! `GET <path>?<query>`, returns the `code` and `state`.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct CallbackReceived {
    pub code: String,
    pub state: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CallbackError {
    #[error("timeout waiting for OAuth callback")]
    Timeout,
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("state mismatch (expected {expected:?})")]
    StateMismatch { expected: String },
    #[error("missing `code` in callback")]
    MissingCode,
    #[error("path mismatch (expected {expected:?})")]
    PathMismatch { expected: String },
}

pub async fn await_callback(
    listener: TcpListener,
    expected_state: String,
    expected_path: String,
    timeout: Duration,
) -> Result<CallbackReceived, CallbackError> {
    implement(listener, expected_state, expected_path, timeout).await
}

async fn implement(
    listener: TcpListener,
    expected_state: String,
    expected_path: String,
    timeout: Duration,
) -> Result<CallbackReceived, CallbackError> {
    let accept = async {
        let (stream, _addr) = listener
            .accept()
            .await
            .map_err(|e| CallbackError::BadRequest(e.to_string()))?;
        Ok::<_, CallbackError>(stream)
    };
    let timeout_fut = tokio::time::sleep(timeout);
    tokio::pin!(timeout_fut);
    let mut stream = tokio::select! {
        biased;
        _ = &mut timeout_fut => return Err(CallbackError::Timeout),
        s = accept => s?,
    };

    let mut header_buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .map_err(|_| CallbackError::BadRequest("header read timeout".into()))?
            .map_err(|e| CallbackError::BadRequest(e.to_string()))?;
        if n == 0 {
            return Err(CallbackError::BadRequest("empty request".into()));
        }
        header_buf.extend_from_slice(&tmp[..n]);
        if header_buf.len() > 8192 {
            return Err(CallbackError::BadRequest("headers too large".into()));
        }
        if let Some(pos) = find_header_end(&header_buf) {
            break pos;
        }
    };

    let mut headers = [httparse::EMPTY_HEADER; 16];
    let mut req = httparse::Request::new(&mut headers);
    let parsed = req
        .parse(&header_buf)
        .map_err(|e| CallbackError::BadRequest(format!("header parse: {e}")))?;
    if !parsed.is_complete() {
        return Err(CallbackError::BadRequest("incomplete headers".into()));
    }

    let method = req
        .method
        .ok_or_else(|| CallbackError::BadRequest("missing method".into()))?;
    if method != "GET" {
        return Err(CallbackError::BadRequest(format!("expected GET, got {method}")));
    }
    let path_full = req
        .path
        .ok_or_else(|| CallbackError::BadRequest("missing path".into()))?;

    let (path, query) = match path_full.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_full, ""),
    };
    if path != expected_path {
        return Err(CallbackError::PathMismatch { expected: expected_path });
    }

    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            _ => {}
        }
    }
    let _ = header_end; // keep header_end alive for clarity; full body is discarded.

    let Some(received_state) = state else {
        return Err(CallbackError::BadRequest("missing `state` in callback".into()));
    };
    if received_state != expected_state {
        return Err(CallbackError::StateMismatch {
            expected: expected_state,
        });
    }
    let Some(code) = code else {
        return Err(CallbackError::MissingCode);
    };

    let body = "<!DOCTYPE html><html><body>Login complete. You may close this window.</body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    Ok(CallbackReceived { code, state: received_state })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    async fn drive_callback(
        request: &str,
        expected_state: &str,
        expected_path: &str,
        timeout: Duration,
    ) -> Result<CallbackReceived, CallbackError> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(await_callback(
            listener,
            expected_state.to_string(),
            expected_path.to_string(),
            timeout,
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut conn = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        conn.write_all(request.as_bytes()).await.unwrap();
        conn.flush().await.unwrap();
        task.await.unwrap()
    }

    #[tokio::test]
    async fn parses_valid_callback() {
        let req = "GET /callback?code=abc&state=ST HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let got = drive_callback(req, "ST", "/callback", Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(got.code, "abc");
        assert_eq!(got.state, "ST");
    }

    #[tokio::test]
    async fn rejects_state_mismatch() {
        let req = "GET /callback?code=abc&state=OTHER HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let err = drive_callback(req, "ST", "/callback", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, CallbackError::StateMismatch { .. }));
    }

    #[tokio::test]
    async fn rejects_missing_code() {
        let req = "GET /callback?state=ST HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let err = drive_callback(req, "ST", "/callback", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, CallbackError::MissingCode));
    }

    #[tokio::test]
    async fn rejects_missing_state_returns_bad_request() {
        let req = "GET /callback?code=abc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let err = drive_callback(req, "ST", "/callback", Duration::from_secs(2))
            .await
            .unwrap_err();
        match &err {
            CallbackError::BadRequest(msg) => assert!(
                msg.contains("state"),
                "expected BadRequest mentioning `state`, got {msg:?}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_path_mismatch() {
        let req = "GET /wrong?code=abc&state=ST HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let err = drive_callback(req, "ST", "/callback", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, CallbackError::PathMismatch { .. }));
    }

    #[tokio::test]
    async fn timeout_when_no_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let err = await_callback(
            listener,
            "ST".into(),
            "/callback".into(),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CallbackError::Timeout));
    }
}
