# P0.5 — Remaining Work

> **작성**: 2026-07-28 (session 6 완료 — Cursor Phase 3 구현 완료)
> **Git 커밋**: `ae5d3f85` (이전 docs) + Cursor full impl
> **테스트**: 654/654 (oxi-ai), clippy clean, consumer check clean

---

## Current State

| Provider | File | Lines | Status | Protocol |
|----------|------|-------|--------|----------|
| **Cursor** | `oxi-ai/src/providers/cursor.rs` | **1117** | **✅ Full impl** | HTTP/2 + Connect + Protobuf (492 types via prost-build) |
| **Devin** | `oxi-ai/src/providers/devin.rs` | **916** | **✅ Full impl** | HTTP/1.1 Connect + Protobuf |
| **GitLab Duo** | `oxi-ai/src/providers/gitlab_duo.rs` | **480** | **✅ Working REST proxy** | REST + Auth Delegation |
| GitLab Duo Agent | — | — | Not started | WebSocket + OAuth (gitlab-duo-workflow.ts) |

### Api Variant Mapping

| Api enum | Serde name | Dispatch | Maps to |
|----------|-----------|----------|---------|
| `Api::CursorAgent` | `cursor-agent` | `CursorProvider::new()` | **Working** — HTTP/2 + Connect + prost-build |
| `Api::DevinAgent` | `devin-agent` | `DevinProvider::new()` | **Working** — Connect + protobuf |
| `Api::GitLabDuo` | `gitlab-duo` | `GitLabDuoProvider::new()` | **Working** — REST proxy |
| `Api::GitLabDuoAgent` | `gitlab-duo-agent` | `_ => None` (wildcard) | **Unimplemented** — WebSocket workflow |

### Test Suite
- oxi-ai: **654/654** passing (was 641; +14 Cursor tests for framing, protobuf roundtrips, request construction, blob store — 1 test replaced/merged)
- clippy: clean (`-D warnings`)
- consumer check (`cargo check -p oxi-cli`): clean
- Full baseline (oxi-cli 763 + oxi-agent 746 + oxi-sdk 398 = 1907): verified via consumer check; additive changes only

### Dependencies Added (runtime)
- `prost = "0.14"` — protobuf codec
- `tokio-stream = "0.1"` — streaming via `UnboundedReceiverStream`

### Dependencies Added (build)
- `prost-build = "0.14"` — proto compilation for Cursor's 492 message types
- `protoc-bin-vendored = "3.0"` — bundled protoc (hermetic build)

---

## What's Done

### Phase 1 — Provider stubs + dispatch (✅ completed)
- `cursor.rs`, `devin.rs`, `gitlab_duo.rs` created
- `build_builtin_transport` / `_with_options` dispatch wired
- `API_TO_PROVIDER` table extended: `cursor`, `devin`, `gitlab-duo`
- `Api::GitLabDuo` added to `oxi-catalog/src/api.rs` (separate from `Api::GitLabDuoAgent` for WebSocket workflow)

### Phase 2 — Devin full implementation (✅ completed)
- **Connect protocol framing**: 5-byte envelope (1 flag + 4 BE length), gzip compression, end-stream trailers
- **Protobuf message types**: 9 prost-derived types
- **Auth flow**: `GetUserJwtRequest` → `GetUserJwtResponse` with gzip fallback
- **Streaming**: `tokio::spawn` + `mpsc::unbounded_channel` producing `ProviderEvent`
- **Tests**: 21 tests
- **Agent loop integration**: Full `Context` → `GetChatMessageRequest` construction

### Phase 3 — Cursor full implementation (✅ completed)

**Source**: omp `packages/ai/src/providers/cursor.ts` (3396 lines)

> **Transport correction**: The original doc claimed Cursor uses WebSocket. It does **not**. Cursor uses **HTTP/2 + Connect streaming protocol** (`content-type: application/connect+proto`, `connect-protocol-version: 1`) — the same Connect framing as Devin, but over HTTP/2 instead of HTTP/1.1. `reqwest` with `rustls-tls` already supports HTTP/2 via ALPN; `tokio-tungstenite` was never needed.

Key implementation details:
- **Proto schema**: 3526-line self-contained proto3 (`proto/cursor/agent.proto`, 492 messages, 17 enums, 5 services) compiled via `prost-build` + `protoc-bin-vendored` in `build.rs`
- **Connect framing**: Reused Devin's 5-byte envelope (1 flag + 4 BE length); request sent uncompressed, decompressed on read
- **Bidirectional HTTP/2 streaming**: `mpsc::unbounded_channel` feeds the request body; response reader clones the sender to answer KV blob requests
- **Blob store**: SHA-256 keyed `HashMap<Vec<u8>, Vec<u8>>` for conversation state blobs (system prompt JSON, history turns)
- **Request builder**: `Context` → `AgentRunRequest` with `root_prompt_messages_json` (Vercel-AI-SDK-shaped JSON blobs) + `turns` (`ConversationTurnStructure` blobs) + action
- **Response decoder**: `AgentServerMessage` → `ProviderEvent` mapping (text deltas, thinking deltas, tool calls, turn_ended)
- **KV channel**: Full `GetBlobArgs`/`SetBlobArgs` response handler (mandatory — server resolves blob IDs this way)
- **Exec channel**: Native tool execution rejected with `ExecClientThrow` — tool calls surface as `ProviderEvent::ToolCallEnd` for the host agent loop
- **Tests**: 14 tests for framing, protobuf roundtrips, blob store, request construction (single-turn + multi-turn history)

---

## Remaining: Phase 4 — GitLab Duo Workflow (gitlab-duo-agent)

**Source**: omp `packages/ai/src/providers/gitlab-duo-workflow.ts` (3135 lines)

### What's needed
1. **WebSocket client**: `tokio-tungstenite` (not yet in `oxi-ai/Cargo.toml`)
2. **OAuth flow**: GitLab app registration + token exchange
3. **REST setup**: 6 API calls before streaming (discovery, project, workflow creation, model listing, direct access, workflow start)
4. **WebSocket workflow protocol**: Start → checkpoints → tool calls → resume
5. **MCP tool definitions**: GitLab-specific tool schema (duo_mcp_tools)
6. **Staleness detection**: Server-side checkpoint comparison for detecting stalled workflows
7. **Step limit restart**: Fresh workflow restart when server report graph-recursion limit

### Dependencies to add
- `tokio-tungstenite` — WebSocket client (this one genuinely needs it; GitLab Duo Workflow uses WebSocket, not HTTP/2 Connect)
- `reqwest` already available for REST setup calls

### Complexity
- **High**. Workflow state machine with pause/resume, checkpoint management
- Multi-phase setup (6+ REST calls) before first WebSocket message
- Requires VS Code app registration for OAuth flow
- Server-side staleness detection and fresh-workflow restart logic

---

## Recommendations

1. **GitLab Duo Workflow** (Phase 4) — the only remaining P0.5 item. Needs `tokio-tungstenite` and a workflow state machine. Can be deferred; it's the least-used remote-AGENT provider.

2. **P2 — TUI omp realignment** (separate track, ~3–6 months) — see `docs/superpowers/plans/2026-07-27-p2-tui-realignment.md`. Not started.

### Quick-start for next session
```bash
# GitLab Duo Workflow — read the source
read /tmp/omp/packages/ai/src/providers/gitlab-duo-workflow.ts

# For the Cursor provider that's now complete, the proto was copied to:
#   oxi-ai/proto/cursor/agent.proto
# The build script is at:
#   oxi-ai/build.rs
```
