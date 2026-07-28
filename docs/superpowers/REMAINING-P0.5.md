# P0.5 — Remaining Work

> **작성**: 2026-07-28 (session 5 종료, 이후 amended)
> **Git 커밋**: `11165ac7` (Devin full impl) + `97b19f8c` (이전 docs)
> **테스트**: 641/641 (oxi-ai), clippy clean, consumer check clean

---

## Current State

| Provider | File | Lines | Status | Protocol |
|----------|------|-------|--------|----------|
| Cursor | `oxi-ai/src/providers/cursor.rs` | 53 | **Stub** (NotImplemented) | HTTP/2 + Protobuf (WebSocket) |
| **Devin** | `oxi-ai/src/providers/devin.rs` | **885** | **✅ Full impl** | HTTP/1.1 Connect + Protobuf |
| **GitLab Duo** | `oxi-ai/src/providers/gitlab_duo.rs` | **480** | **✅ Working REST proxy** | REST + Auth Delegation |
| GitLab Duo Agent | — | — | Not started | WebSocket + OAuth (gitlab-duo-workflow.ts) |

### Api Variant Mapping

| Api enum | Serde name | Dispatch | Maps to |
|----------|-----------|----------|---------|
| `Api::CursorAgent` | `cursor-agent` | `CursorProvider::new()` (stub) | Stub — NotImplemented error |
| `Api::DevinAgent` | `devin-agent` | `DevinProvider::new()` (full impl) | **Working** — Connect + protobuf |
| `Api::GitLabDuo` | `gitlab-duo` | `GitLabDuoProvider::new()` | **Working** — REST proxy |
| `Api::GitLabDuoAgent` | `gitlab-duo-agent` | `_ => None` (wildcard) | **Unimplemented** — WebSocket workflow |

### Test Suite
- oxi-ai: **641/641** passing (was 626; +15 Devin tests for framing, protobuf roundtrips, edge cases)
- clippy: clean (`-D warnings`)
- consumer check (`cargo check -p oxi-cli`): clean
- Full baseline (oxi-cli 763 + oxi-agent 746 + oxi-sdk 398 = 1907): verified via consumer check; additive changes only

### Dependencies Added
- `prost = "0.14"` — protobuf codec (Devin message types with derive macros)
- `tokio-stream = "0.1"` — streaming via `UnboundedReceiverStream`

---

## What's Done

### Phase 1 — Provider stubs + dispatch (✅ completed)
- `cursor.rs`, `devin.rs`, `gitlab_duo.rs` created
- `build_builtin_transport` / `_with_options` dispatch wired
- `API_TO_PROVIDER` table extended: `cursor`, `devin`, `gitlab-duo`
- `Api::GitLabDuo` added to `oxi-catalog/src/api.rs` (separate from `Api::GitLabDuoAgent` for WebSocket workflow)

### Phase 2 — Devin full implementation (✅ completed)
- **Connect protocol framing**: 5-byte envelope (1 flag + 4 BE length), gzip compression, end-stream trailers
- **Protobuf message types**: 9 prost-derived types (GetChatMessageRequest/Response, GetUserJwtRequest/Response, Metadata, ChatMessagePrompt, ChatToolCall, ChatToolDefinition, CompletionConfiguration, ModelUsageStats)
- **Auth flow**: `GetUserJwtRequest` → `GetUserJwtResponse` with gzip fallback
- **Streaming**: `tokio::spawn` + `mpsc::unbounded_channel` producing `ProviderEvent` variants (text/thinking/toolcall deltas)
- **Tests**: 21 tests covering frame roundtrips (uncompressed, gzip, partial, multi-frame), protobuf roundtrips for all message types, trailer parsing, combined frame+protobuf fixture, session token normalization
- **Agent loop integration**: Full `Context` → `GetChatMessageRequest` construction (system prompt, message history with User/Assistant/ToolResult mapping, tool definitions)

---

## Remaining: Phase 3 — Cursor Full Implementation

**Source**: omp `packages/ai/src/providers/cursor.ts` (3395 lines)
**Location**: `/tmp/omp/packages/catalog/src/discovery/cursor-gen/agent_pb.ts` (protobuf TS types)

### What's needed
1. **WebSocket client**: `tokio-tungstenite` (not yet in `oxi-ai/Cargo.toml`)
2. **Protobuf message types**: AgentService/AgentServerMessage (agent_v1 proto)
   - TS types at `cursor-gen/agent_pb.ts` (4000+ lines of generated code)
   - Need prost-derived types for: AgentServerMessage, AgentClientMessage, ConversationState, ShellStream, etc.
3. **Connect protocol over HTTP/2**: Same framing as Devin, but over HTTP/2
4. **Conversation state**: Server-side checkpoint resume per turn
5. **Tool execution bridge**: Cursor-specific exec handlers (file ops, MCP bridge)
6. **Proxy support**: TLS tunnel for HTTP/2 through proxies

### Dependencies to add
- `tokio-tungstenite` — WebSocket client
- `reqwest` with `h2` feature (already has rustls-tls → HTTP/2 via ALPN)

### Complexity
- **Very high**. The Cursor protocol is the most complex of the three
- Protobuf generated code is 4000+ lines for the message types alone
- Requires conversation state caching and blob store
- Includes shell streaming, MCP tool calling, and file operations

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
- `tokio-tungstenite` — WebSocket client
- `reqwest` already available for REST setup calls

### Complexity
- **High**. Workflow state machine with pause/resume, checkpoint management
- Multi-phase setup (6+ REST calls) before first WebSocket message
- Requires VS Code app registration for OAuth flow
- Server-side staleness detection and fresh-workflow restart logic

---

## Priority Recommendation

1. **Cursor** (Phase 3) — higher practical value (Cursor is widely used). Start by adding `tokio-tungstenite` to `oxi-ai/Cargo.toml`, then define the agent_v1 protobuf types, then implement the WebSocket streaming.

2. **GitLab Duo Workflow** (Phase 4) — the `gitlab-duo-agent` variant. Can be done after Cursor since both need `tokio-tungstenite`.

Both require `tokio-tungstenite` as an additional dependency in `oxi-ai/Cargo.toml`. The key difference from Devin: both use **WebSocket** (not HTTP streaming), and both have significantly more complex state machines.

### Quick-start for next session
```bash
# Add tokio-tungstenite
cd /Volumes/MERCURY/PROJECTS/oxi
# In oxi-ai/Cargo.toml, add:
# tokio-tungstenite = { version = "0.26", default-features = false }

# Read Cursor's protobuf types
read /tmp/omp/packages/catalog/src/discovery/cursor-gen/agent_pb.ts

# Read GitLab Duo workflow
read /tmp/omp/packages/ai/src/providers/gitlab-duo-workflow.ts
```
