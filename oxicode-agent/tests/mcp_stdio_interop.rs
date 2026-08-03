//! Real-server stdio interop test for the MCP v2 transport redesign.
//!
//! Spawns the official [`@modelcontextprotocol/server-everything`] reference
//! server and verifies that oxicode's JSONL framing + new request/notify
//! transport complete the initialize handshake and `tools/list`/`ping`.
//!
//! Requires `npx` (Node.js) on PATH. Marked `#[ignore]` so CI does not
//! download npm packages; run locally with:
//!
//! ```text
//! cargo test -p oxicode-agent --test mcp_stdio_interop -- --ignored
//! ```
//!
//! This is the regression guard for the G1 framing fix (see
//! `docs/designs/2026-06-19-mcp-v2-conformance-transports.md`).
//!
//! [`@modelcontextprotocol/server-everything`]: https://www.npmjs.com/package/@modelcontextprotocol/server-everything

use oxicode_agent::mcp::client::McpClient;
use std::collections::HashMap;
use std::time::Duration;

const COLD_START: Duration = Duration::from_secs(120);
const ROUND_TRIP: Duration = Duration::from_secs(30);

#[tokio::test]
#[ignore]
async fn stdio_interop_with_server_everything() {
    // 1. Spawn the reference server and run the initialize handshake.
    //    This is the operation that the old Content-Length framing broke:
    //    server-everything expects newline-delimited JSON and ignores
    //    Content-Length-framed messages entirely.
    let mut client = tokio::time::timeout(
        COLD_START,
        McpClient::connect(
            "npx",
            &[
                "-y".to_string(),
                "@modelcontextprotocol/server-everything".to_string(),
                "stdio".to_string(),
            ],
            &HashMap::new(),
            None,
            false,
        ),
    )
    .await
    .expect("initialize timed out (likely npx download on cold cache; retry)")
    .expect("initialize handshake with server-everything should succeed (G1 fix)");

    assert!(
        !client.server_info.name.is_empty(),
        "server name should be set after initialize"
    );

    // 2. List tools — exercises the JSONL recv path and id correlation.
    let tools = tokio::time::timeout(ROUND_TRIP, client.list_tools())
        .await
        .expect("tools/list timed out")
        .expect("tools/list should succeed");

    assert!(
        !tools.is_empty(),
        "server-everything exposes tools; an empty list means the framing or handshake regressed"
    );

    // 3. Round-trip a ping — confirms the id counter advances and a second
    //    request after `tools/list` reuses the connection cleanly.
    tokio::time::timeout(ROUND_TRIP, client.ping())
        .await
        .expect("ping timed out")
        .expect("ping should succeed");

    client.close().await.ok();
}
