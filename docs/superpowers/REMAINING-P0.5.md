# P0.5 — Remaining Work

> **작성**: 2026-07-28 (session 5 종료 시점)
> **완료된 작업**: Provider stubs + dispatch entries + GitLab Duo REST proxy
> **Git 커밋**: `072a40bf` (feat(ai): P0.5 — remote-AGENT provider stubs + GitLab Duo REST proxy)

---

## Current State

| Provider | File | Lines | Status | Protocol |
|----------|------|-------|--------|----------|
| Cursor | `oxi-ai/src/providers/cursor.rs` | 53 | Stub (NotImplemented) | HTTP/2 + Protobuf |
| Devin | `oxi-ai/src/providers/devin.rs` | 67 | Stub (NotImplemented) | HTTP/1.1 Connect + Protobuf |
| **GitLab Duo** | `oxi-ai/src/providers/gitlab_duo.rs` | **480** | **Working REST proxy** | REST + Auth Delegation |
| GitLab Duo Agent | — | — | Not started | WebSocket + OAuth (gitlab-duo-workflow.ts) |

### What Works
- All three providers dispatch correctly through `build_builtin_transport`
- `GitLabDuoProvider` does full auth flow (token → direct access) + delegate to Anthropic/OpenAI
- Model mappings for 10 GitLab Duo models (duo-chat-*)
- API_TO_PROVIDER table extended with `cursor`, `devin`, `gitlab-duo-agent`
- `/gitlab-duo-agent` variant → `GitLabDuoProvider::new()` (currently returns NotImplemented for gitlab-duo-agent models)

### Test Suite
- oxi-ai: 626/626 passing
- clippy: clean (`-D warnings`)
- consumer check (oxi-cli): clean

---

## Remaining: Phase 2 — Devin Full Implementation

**Source**: omp `packages/ai/src/providers/devin.ts` (678 lines)
**Location**: `/tmp/omp/packages/ai/src/providers/devin/` (protobuf defs)

### What's needed
1. **Protobuf message types**: Codeium `GetChatMessageRequest` / `GetChatMessageResponse`
   - `.proto` files at `devin/proto/exa/chat_pb/`, `api_server_pb/`, etc.
   - Need reverse-engineered proto → Rust codegen (prost-build) or manual encoding
2. **Connect protocol framing**: Already written as utilities in `devin.rs` (stubbed out), needs completion
3. **Auth flow**: `GetUserJwtRequest` → `GetUserJwtResponse` (session token)
4. **Streaming response handling**: SSE-like delta parsing for `GetChatMessageResponse`

### Dependencies to add
- `prost` + `prost-types` — protobuf codec
- Possibly `prost-build` in build.rs for proto compilation

### Key types
```rust
// Connect frame: [flags: 1B][length: 4BE][payload: len]
struct ConnectFrame { flags: u8, payload: Vec<u8> }

// Auth
fn build_auth_request(api_key: &str) -> GetUserJwtRequest
fn parse_auth_response(data: &[u8]) -> Result<GetUserJwtResponse>

// Chat
fn build_chat_request(model, context, api_key, jwt) -> GetChatMessageRequest
fn parse_chat_response_frame(data: &[u8]) -> Vec<ProviderEvent>
```

### Protocol details
- HTTP/1.1 POST to `/exa.api_server_pb.ApiServerService/GetChatMessage`
- `content-type: application/connect+proto`
- Body: 5-byte frame header + gzip(payload)
- Response: stream of 5-byte framed protobuf messages
- Flag bits: 0x01=gzip, 0x02=end-of-stream(trailers)

---

## Remaining: Phase 3 — Cursor Full Implementation

**Source**: omp `packages/ai/src/providers/cursor.ts` (3395 lines)
**Location**: `/tmp/omp/packages/catalog/src/discovery/cursor-gen/agent_pb.ts` (protobuf TS types)

### What's needed
1. **HTTP/2 client**: `h2` or `reqwest` with HTTP/2 support
2. **Protobuf message types**: AgentService/AgentServerMessage (agent_v1 proto)
   - TS types at `cursor-gen/agent_pb.ts` (4000+ lines of generated code)
3. **Connect protocol over HTTP/2**: Same framing, different transport layer
4. **Conversation state**: Server-side checkpoint resume per turn
5. **Tool execution bridge**: Cursor-specific exec handlers (file ops, MCP)

### Dependencies to add
- `h2` — HTTP/2 client (or use reqwest with h2 feature)
- `prost` + proto compilation

### Complexity
- **Very high**. The Cursor protocol is the most complex of the three
- Protobuf generated code is 4000+ lines for the message types alone
- HTTP/2 is a new transport layer for oxi-ai
- No proxy support by default (bundled in Cursor's agent, not the provider)

---

## Remaining: Phase 4 — GitLab Duo Workflow (gitlab-duo-agent)

**Source**: omp `packages/ai/src/providers/gitlab-duo-workflow.ts` (3135 lines)

### What's needed
1. **WebSocket client**: `tokio-tungstenite`
2. **OAuth flow**: GitLab app registration + token exchange
3. **REST setup**: 6 API calls before streaming (discovery, project, workflow creation)
4. **WebSocket workflow protocol**: Start → checkpoints → tool calls → resume
5. **MCP tool definitions**: GitLab-specific tool schema

### Dependencies to add
- `tokio-tungstenite` — WebSocket client
- `reqwest` already available for REST setup

### Complexity
- **High**. Workflow state machine with pause/resume, checkpoint management
- Multi-phase setup (6+ REST calls) before first WebSocket message
- Requires VS Code app registration for OAuth flow
- Server-side staleness detection and fresh-workflow restart logic

---

## Priority Recommendation

1. **Devin** (Phase 2) — lowest complexity among remaining. HTTP/1.1, no WebSocket. The protobuf definitions and Connect framing are self-contained.

2. **GitLab Duo Workflow** (Phase 4) — the `gitlab-duo-agent` variant. Higher priority than Cursor because GitLab users exist.

3. **Cursor** (Phase 3) — highest complexity (HTTP/2 + protobuf). Defer until HTTP/2 client infra exists in oxi-ai.

All three require adding `prost` and potentially `tokio-tungstenite` to `oxi-ai/Cargo.toml`.
