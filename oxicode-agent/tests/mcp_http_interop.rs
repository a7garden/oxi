//! Mock Streamable HTTP interop test for the MCP v2.1 transport.
//!
//! Spawns a minimal in-test TCP server that speaks just enough of the
//! Streamable HTTP transport (2025-03-26) to complete an MCP handshake
//! and serve `tools/list`, then drives oxicode's `StreamableHttpTransport`
//! through `McpClient::connect_with_transport`.
//!
//! This is the regression guard for G2 (Streamable HTTP). The mock is
//! dependency-free (raw `tokio::net::TcpListener`, no httpmock/http
//! crate) so it adds no dev-dependency surface.
//!
//! Run with: `cargo test -p oxicode-agent --test mcp_http_interop -- --ignored`

use oxicode_agent::mcp::client::McpClient;
use oxicode_agent::mcp::transport::http::StreamableHttpTransport;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn spawn_mock_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{}", addr);

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => return,
            };
            tokio::spawn(handle_conn(stream));
        }
    });

    base
}

async fn handle_conn(mut stream: TcpStream) {
    let request_bytes = match read_http_request(&mut stream).await {
        Ok(b) => b,
        Err(_) => return,
    };

    let mut content_length = 0usize;
    let mut body_start = 0usize;
    if let Some(pos) = find_double_crlf(&request_bytes) {
        body_start = pos + 4;
        content_length = parse_content_length(&request_bytes[..pos]);
    }
    let body = request_bytes.get(body_start..body_start + content_length);

    let request_str = std::str::from_utf8(&request_bytes).unwrap_or("");
    let method = request_str
        .split("\r\n")
        .next()
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("");

    let response = match (method, body) {
        ("POST", Some(b))
            if std::str::from_utf8(b)
                .unwrap_or("")
                .contains("\"initialize\"") =>
        {
            let id = extract_id(b);
            build_response(
                200,
                "application/json",
                Some("test-session-123"),
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"protocolVersion":"2025-03-26","serverInfo":{{"name":"mock","version":"0.0.1"}},"capabilities":{{"tools":{{"listChanged":true}}}}}}}}"#,
                    id
                ),
            )
        }
        ("POST", Some(b))
            if std::str::from_utf8(b)
                .unwrap_or("")
                .contains("\"tools/list\"") =>
        {
            let id = extract_id(b);
            build_response(
                200,
                "application/json",
                None,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"tools":[{{"name":"echo","description":"mock echo","inputSchema":{{"type":"object","properties":{{"text":{{"type":"string"}}}}}}}}]}}}}"#,
                    id
                ),
            )
        }
        ("POST", _) => build_response(202, "application/json", None, ""),
        ("DELETE", _) => build_response(204, "", None, ""),
        _ => build_response(405, "text/plain", None, "method not allowed"),
    };
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_double_crlf(&buf) {
            let header_end = pos + 4;
            let content_length = parse_content_length(&buf[..header_end]);
            if buf.len() >= header_end + content_length {
                buf.truncate(header_end + content_length);
                return Ok(buf);
            }
        }
    }
    Ok(buf)
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> usize {
    let s = std::str::from_utf8(headers).unwrap_or("");
    for line in s.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            return rest.trim().parse().unwrap_or(0);
        }
        if let Some(rest) = line.strip_prefix("content-length:") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn extract_id(body: &[u8]) -> u64 {
    let s = std::str::from_utf8(body).unwrap_or("");
    let after = match s.find("\"id\":") {
        Some(p) => &s[p + 5..],
        None => return 1,
    };
    let trimmed = after.trim_start();
    let num: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().unwrap_or(1)
}

fn build_response(status: u16, content_type: &str, session_id: Option<&str>, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let mut s = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        status,
        reason,
        body.len()
    );
    if !content_type.is_empty() {
        s.push_str(&format!("Content-Type: {}\r\n", content_type));
    }
    if let Some(sid) = session_id {
        s.push_str(&format!("Mcp-Session-Id: {}\r\n", sid));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s
}

#[tokio::test]
#[ignore]
async fn http_interop_with_mock_streamable_server() {
    let base = spawn_mock_server().await;
    let endpoint = format!("{}/mcp", base);

    let transport =
        StreamableHttpTransport::new("mock", &endpoint, None, 5_000).expect("build transport");

    let mut client = tokio::time::timeout(
        Duration::from_secs(10),
        McpClient::connect_with_transport(Box::new(transport)),
    )
    .await
    .expect("initialize timed out")
    .expect("initialize should succeed against the mock Streamable HTTP server");

    assert_eq!(client.server_info.name, "mock");

    let tools = tokio::time::timeout(Duration::from_secs(10), client.list_tools())
        .await
        .expect("tools/list timed out")
        .expect("tools/list should succeed");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    client.close().await.ok();
}
