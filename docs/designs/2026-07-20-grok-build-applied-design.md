# grok-build → oxi 적용 설계

- 작성일: 2026-07-20
- 소스 동기 커밋: `grok-build@98c3b24` (SOURCE_REV)
- 적용 후보 4건: typed 도구 시그니처 / toolbus delta over MCP / 컴팩션 reminder / 샌드박스 추출
- 적용 방식: grok 설계를 차용하되 oxi의 기존 컴포지션(`oxi-sdk` 15-port + `oxi-agent::McpManager` 풀)과 정합하게 슬라이스
- 검증 기준: 각 후보마다 명세한 회귀 테스트 + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo nextest run --workspace` 통과

---

## 0. 전제 정정 — 분산 toolbus 후보의 진짜 delta

이전 보고서(`docs/ref-porter/2026-07-20-xai-org-grok-build.md`)에서 "oxi는 외부 노출 가능한 toolbus 프로토콜이 없다"고 기술했지만 **이는 부정확**. oxi는 이미 원격 도구 프로토콜을 가지고 있음:

- `oxi-agent::mcp::McpTransport` (`oxi-agent/src/mcp/transport/mod.rs:48`) — stdio + HTTP+SSE 양쪽 트랜스포트 추상화
- `McpManager` (`oxi-agent/src/mcp/mod.rs:104`) — 서버별 클라이언트 풀, 캐시, 콘센트, lifecycle, OAuth(`auth.rs`)
- `McpClient` (`oxi-agent/src/mcp/client.rs:98`) — JSON-RPC 2.0 클라이언트, 알림/요청-응답 correlation
- 4,084 LOC의 성숙한 구현

따라서 본 설계에서 grok의 `xai-computer-hub-sdk`를 차용하는 부분은 **MCP가 다루지 않는 delta**로 한정. 다음 표는 MCP와 hub의 기능 비교와 본 설계에서의 채택 여부.

| 기능 | MCP | grok hub | 본 설계 |
|---|---|---|---|
| JSON-RPC 프레이밍 | O (stdio + http) | O (WebSocket) | **채택 안 함** — MCP로 충분, WebSocket 추가는 신규 트랜스포트 |
| 인증 | OAuth bearer (단일 토큰) | `Principal { user_id, session_ids, scopes, audiences }` | **채택** — 멀티 세션 자격증명 모델만 |
| 연결 풀/공유 | 서버별 `McpManager` | `(url, principal)` 키 풀 | **채택 안 함** — `McpManager` 풀과 중복 |
| 자동 재연결 | 단순 backoff (`mod.rs:644-647`) | `ReconnectCallback` + serve replay | **채택** — replay 의미론만 |
| oxi-as-server (도구 노출) | X (클라이언트 only) | `ToolServer` + `ToolServerHandler` | **채택** — oxi가 외부에서 도구를 받을 수 있는 채널 |
| Hooks 양방향 | 클라이언트→서버 알림만 | 서버→클라이언트 hook + reply | **채택** — server→client hook 의미론만 |
| OIDC 디스커버리 | X | O | 채택 안 함 |

**결론**: hub 전체를 복제하지 않고, **WebSocket transport + 자동 재연결/replay + oxi-as-server + hooks** 4개 delta만 차용.

---

## 1. 후보 A — Typed 도구 시그니처 (트레이트 경계 보존)

### 목표

`oxi-agent::tools::AgentTool` 트레이트의 `params: serde_json::Value` 시그니처는 **변경 불가** — `ToolRegistry.tools: HashMap<String, Arc<dyn AgentTool>>` (`oxi-agent/src/tools.rs:855`)와 `Vec<Box<dyn AgentTool>>` 빌드 (`tools.rs:998`)로 박혀 있어 연관 타입을 추가하면 `dyn` 호환성이 깨진다.

grok의 `Tool<Args: Deserialize + JsonSchema, Output>` (`crates/common/xai-tool-runtime/src/tool.rs:36-47`) 패턴은 **개념만** 차용. 신규 **병렬 generic 트레이트** `TypedTool`을 추가하고 **이미 존재하는 `AgentTool`을 dyn 소거 표면**으로 재사용하는 어댑터로 연결한다. 트레이트 경계(`execute(params: Value)`)는 그대로 유지 — 32개 기존 도구 / `ToolRegistry` / `AgentLoop` 호출 지점 전부 무수정.

### 문제 분석

- **컴파일 타임 안전성 부재**: `params["path"]` 인덱싱이 32개 도구 전부에 분산. 새 도구 추가 시 누락 위험.
- **중복 보일러플레이트**: `parameters_schema()`가 32개 도구 각각에서 수동 JSON Schema 문자열 반환 — `schemars::JsonSchema` derive로 대체 가능.
- **스트리밍 부재**: `AgentTool`은 `on_progress(ProgressCallback)` (tools.rs:714-716)로 단일 문자열 진행만 지원. `ToolStream<T>` (Progress N + Terminal 1) 의미론 없음.
- **기존 `ToolDefinitionLike`는 typed 도구용이 아님**: `tool_definition_wrapper.rs:39-44, 124-129, 158-165` 모두 `params: Value` 경계. dynamic/closure 기반 도구 wrapping 용도. **확장 대상이 아니다.**

### 설계

#### A.1 신규 generic 트레이트 — `oxi-agent/src/tools/typed.rs`

```rust
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

/// 타입 안전 도구 트레이트.
///
/// Generic + 연관 타입 → **dyn 호환 안 됨**. [`TypedToolAdapter`]가
/// [`AgentTool`]을 구현해 `Arc<dyn AgentTool>`로 소거한다.
pub trait TypedTool: Send + Sync + 'static {
    /// LLM 에서 넘어오는 JSON 인자의 타입. `DeserializeOwned + JsonSchema` 둘 다 필수.
    type Args: DeserializeOwned + JsonSchema + Send + 'static;

    fn name(&self) -> &str;
    fn label(&self) -> &str { self.name() }
    fn description(&self) -> &str;
    fn essential(&self) -> bool { false }

    /// Typed execution — 인자는 이미 deserialized.
    async fn execute_typed(
        &self,
        tool_call_id: &str,
        args: Self::Args,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError>;
}
```

이 트레이트는 generic이고 `Self::Args`가 있어서 `dyn TypedTool`으로 쓸 수 없다. 어댑터가 소거 책임을 진다.

#### A.2 어댑터 — `TypedToolAdapter<T>`

`DefinitionWrapper<T>` (`tool_definition_wrapper.rs:138-167`) 의 형태를 그대로 본떠서, `TypedTool` → `AgentTool` 어댑터를 둔다. dyn 표면은 **기존 `AgentTool`** — `ToolDyn` 같은 별도 트레이트를 만들지 않는다.

```rust
/// [`TypedTool`]을 [`AgentTool`]로 소거한다. `parameters_schema()`는
/// `schemars::schema_for!(<T as TypedTool>::Args)` 결과를, `execute`는
/// 내부에서 `serde_json::from_value`로 deserialize 한 뒤
/// [`TypedTool::execute_typed`]를 호출한다.
///
/// [`DefinitionWrapper`]와 같은 모양이지만 인자 deserialize 지점이
/// `Value → T::Args`로 강타입화되어 있다는 점이 다르다.
pub struct TypedToolAdapter<T: TypedTool>(Arc<T>);

impl<T: TypedTool> std::fmt::Debug for TypedToolAdapter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedToolAdapter")
            .field("name", &self.0.name())
            .finish()
    }
}

#[async_trait]
impl<T: TypedTool> AgentTool for TypedToolAdapter<T> {
    fn name(&self) -> &str { self.0.name() }
    fn label(&self) -> &str { self.0.label() }
    fn description(&self) -> &str { self.0.description() }
    fn essential(&self) -> bool { self.0.essential() }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(<T as TypedTool>::Args))
            .unwrap_or_else(|_| serde_json::json!({"type": "object"}))
    }

    fn execution_mode(&self) -> ToolExecutionMode { ToolExecutionMode::ParallelSafe }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let tool_name = self.0.name();
        let args = <T as TypedTool>::Args::deserialize(params)
            .map_err(|e| ToolError::InvalidArgs(
                format!("invalid args for '{tool_name}': {e}")
            ))?;
        self.0.execute_typed(tool_call_id, args, signal, ctx).await
    }
}

/// 등록 헬퍼 — `register_arc`에 그대로 넘기면 `HashMap`에 dyn 표면으로 들어간다.
pub fn wrap_typed<T: TypedTool>(tool: T) -> Arc<dyn AgentTool> {
    Arc::new(TypedToolAdapter(Arc::new(tool)))
}
```

핵심: **dyn 소거 표면은 `AgentTool` 그대로**. `ToolRegistry` / `Vec<Box<dyn AgentTool>>` / `with_builtins_cwd` 빌드 경로 무수정.

#### A.3 스트리밍은 후속 시리즈로 분리

`ToolStream<T>` 의미론은 AgentTool 의 `on_progress(ProgressCallback)` 와 의미가 다름. 동시 도입은 위험. 첫 시리즈에서는 **typed 인자만** 도입하고 `on_progress(ProgressCallback)` 그대로 유지. 후속 PR 에서 별도 `StreamingTypedTool` 트레이트 도입 — 그때 progress 콜백 → `ToolStream::Progress` 매핑.

#### A.4 `ToolRegistry` / `AgentLoop` / 기존 도구 — 전부 무수정

- `ToolRegistry.tools` (`tools.rs:855`) — `HashMap<String, Arc<dyn AgentTool>>` 그대로.
- `register_arc(Arc<dyn AgentTool>)` (`tools.rs:894`) — `wrap_typed(...)` 결과를 그대로 넘기면 됨.
- `with_builtins_cwd` 빌드 (`tools.rs:998-1054`) — 기존 32개 도구 무수정. 점진적으로 새 도구 추가 시 `wrap_typed(...)` 한 줄로 교체 가능.
- `AgentLoop` 호출 지점 (`agent_loop/mod.rs`에서 `Arc<dyn AgentTool>::execute(...)`) — 무수정.

#### A.5 단계적 마이그레이션 순서

| PR | 내용 | 위험 |
|---|---|---|
| **PR-A1** | `TypedTool` 트레이트 + `TypedToolAdapter<T>` + `wrap_typed<T>` 추가. `oxi-agent/src/tools/typed.rs` 신규 모듈. **`schemars` 신규 의존성 추가**. 단위 테스트: schema 생성 / 정상 deserialize / `InvalidArgs` 반환. **기존 32개 도구 / ToolRegistry / AgentLoop 무수정**. | 낮음 |
| **PR-A2** | `ClosureTool` (`oxi-sdk/src/closure_tool.rs`) 에 typed variant 추가. 기존 `new_sync/new_async` API 보존, 신규 `ClosureTool::new_typed::<Args>()` 추가. → SDK 사용자 코드가 typed 사용 선택 가능. | 낮음 |
| **PR-A3** | 첫 내부 도구 1개 (`grep`) 를 `TypedTool` 로 재작성. 회귀 테스트 동일 결과 확인. | 낮음 — 단일 도구 |
| **PR-A4..N** | 나머지 도구 순차 이식. 우선순위: essential (read/write/edit/bash/grep/find/ls) 먼저, 이후 옵션. **각 도구마다 PR + 회귀 테스트**. 32개 동시 변경 금지. | 낮음 |

#### A.6 검증

- **PR-A1**: `cargo nextest run -p oxi-agent -- typed` — 신규 단위 테스트 통과.
- **PR-A1 회귀**: `cargo nextest run -p oxi-agent` — 32개 기존 도구 영향 없음.
- **PR-A2**: `cargo nextest run -p oxi-sdk closure` — ClosureTool typed variant 동등성.
- **PR-A3**: `cargo nextest run -p oxi-agent -- grep` + `cargo test --doc` — grep 결과 정확성.
- **clippy**: 각 PR에서 `cargo clippy --workspace --all-targets -- -D warnings` 게이트.

#### A.7 위험과 mitigation

- **`schemars` 신규 의존성**: oxi-agent Cargo.toml 에 `schemars = "0.8"` 추가. 트리 크기 약간 증가 → `cargo deny check` 영향. 현재 oxi 가 이미 `jsonschema 0.30` 의존 (grok Cargo.toml:164) — `schemars` 와는 다른 라이브러리(스키마 생성 vs 스키마 검증).
- **`ToolError::InvalidArgs` variant 부재**: 현재 도구 에러 타입에 `InvalidArgs` variant 가 없을 가능성 → `oxi-agent/src/error.rs` 에 추가 필요. PR-A1 에서 함께.
- **dyn dispatch 비용**: 어댑터가 `Arc<dyn AgentTool>` 한 단계 더 감싸므로 `tools.get(name).execute(...)` 호출에 `Arc::clone` + vtable lookup 추가. 기존 `register` 경로와 동일한 dyn 비용.
- **`JsonSchema` derive 누락**: `T::Args` 가 `JsonSchema` derive 가 없는 타입이면 컴파일 에러. 회피책: 사용자가 derive 추가, 또는 `parameters_schema` 를 override 하도록 어댑터에 hook 포인트 제공 (후속 PR).
- **`DeserializeOwned` 강제**: `Self::Args: DeserializeOwned` — `'static` lifetime 필요. JSON 만 받으면 충분하므로 OK. 단, `&'static str` 같은 borrowed string 못 씀 → 도구 인자 타입은 owned 로 작성.
- **스트리밍 지연 도입**: PR-A 시리즈에서는 typed 인자만. `on_progress(ProgressCallback)` 그대로 유지. 후속 PR 에서 `StreamingTypedTool` 도입.

---

## 2. 후보 B — MCP 위 toolbus delta (WebSocket transport + oxi-as-server + 재연결/replay + hooks)

### 목표

oxi의 MCP 풀(`oxi-agent/src/mcp/`)이 다루지 않는 4개 delta만 추가:

1. **WebSocket transport** — stdio/http 외에 영구 양방향 채널
2. **자동 재연결 + serve replay** — 단순 backoff(`mod.rs:644-647`)를 넘어서 자동 복구
3. **oxi-as-server** — oxi가 외부 에이전트에게 도구를 노출하는 채널
4. **Server→client hooks** — 서버가 클라이언트에게 알림/요청

전체 hub 복제 금지. MCP 위 얇은 레이어.

### 설계

#### B.1 신규 크레이트 — `oxi-agent/src/mcp/ws_transport.rs`

```rust
pub struct WebSocketTransport {
    url: Url,
    credential: Arc<dyn McpCredentialProvider>,
    /// Auto-reconnect state machine.
    state: Arc<parking_lot::Mutex<WsState>>,
    /// Outbound mpsc to writer task; inbound unbounded from reader task.
    outbound_tx: mpsc::UnboundedSender<String>,
    inbound_rx: Arc<parking_lot::Mutex<Option<broadcast::Receiver<RawJsonRpcMessage>>>>,
    /// Pending requests: id → oneshot sender.
    pending: Arc<parking_lot::Mutex<HashMap<u64, oneshot::Sender<RawJsonRpcMessage>>>>,
    /// Replay buffer (capped) for in-flight requests across reconnect.
    replay_buf: Arc<parking_lot::Mutex<VecDeque<(u64, String)>>>,
}

const DEFAULT_PENDING_TIMEOUT: Duration = Duration::from_secs(30);
const REPLAY_BUFFER_CAP: usize = 128;

#[async_trait]
impl McpTransport for WebSocketTransport {
    async fn request(&mut self, id: u64, json: &str) -> Result<RawJsonRpcMessage> {
        // (1) Save into replay buffer (for retry on reconnect).
        self.replay_buf.lock().push_back((id, json.to_string()));
        if self.replay_buf.lock().len() > REPLAY_BUFFER_CAP { ... 트림 ... }
        // (2) Register pending.
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        // (3) Send (or fail if disconnected — caller decides retry).
        self.outbound_tx.send(json.to_string()).await?;
        // (4) Await with timeout; on timeout return Err + cleanup pending.
        tokio::time::timeout(DEFAULT_PENDING_TIMEOUT, rx).await
            .map_err(|_| ToolError::Transport("request timeout".into()))?
            .map_err(|_| ToolError::Transport("cancelled".into()))
    }
    async fn notify(&mut self, json: &str) -> Result<()> {
        // Notify 는 replay 안 함 (best-effort).
        self.outbound_tx.send(json.to_string()).await?;
        Ok(())
    }
    fn set_inbound_handler(&mut self, handler: InboundHandler) { ... }
    async fn close(&mut self) -> Result<()> { ... }
    fn is_connected(&self) -> bool { ... }
}
```

연결 상태 머신: `Disconnected → Connecting → Connected → Reconnecting → Connected`.

**재연결 시 replay 동작**:
1. `tokio-tungstenite` 연결 재수립
2. `initialize` 핸드셰이크 재실행 (서버가 새 세션 ID 부여 가능)
3. replay buffer의 in-flight request들을 **id는 그대로** 재전송 (서버가 idempotent id 처리를 가정)
4. 응답이 오면 기존 `pending`에 전달

**서버측 구현**: `WebSocketServer`가 핸드셰이크에서 핸드셰이크 응답으로 `session_id`를 부여할 때 **같은 연결의 in-flight id는 보존**해야 함. 이는 단순하지만 서버 측 처리 명세가 있어야 함.

#### B.2 신규 크레이트 — `oxi-agent/src/mcp/oxi_as_server.rs`

`xai-computer-hub-sdk`의 `ToolServer`/`ToolServerHandler`(grok `crates/common/xai-computer-hub-sdk/src/server.rs:160-203`) 차용.

```rust
/// oxi 를 외부 에이전트에게 도구 노출자로 만든다.
/// MCP 의 server side 와 동등하지만, oxi 도구(AgentTool/Tool) 를 그대로 노출.
pub struct ToolServer {
    registry: Arc<ToolRegistry>,
    handlers: RwLock<HashMap<String, Arc<dyn ToolServerHandler>>>,
    bind_rx: broadcast::Receiver<BindEvent>,
}

#[async_trait]
pub trait ToolServerHandler: Send + Sync + 'static {
    fn tool_id(&self) -> &str;
    fn description(&self) -> String;
    fn input_schema(&self) -> Option<Value> { None }
    async fn handle_call(
        &self,
        ctx: ToolCallContext,
        args: Value,
    ) -> Pin<Box<dyn Stream<Item = ToolStreamItem<Value>> + Send>>;
    async fn handle_hook(&self, session_id: &str, event: HookEvent) {}
}

/// ToolRegistry의 typed + legacy 도구를 자동으로 ToolServerHandler로 노출.
pub struct RegistryAdapter(Arc<ToolRegistry>);

#[async_trait]
impl ToolServerHandler for RegistryAdapter {
    fn tool_id(&self) -> &str { self.0.find_first().await /* 첫 도구 — multi-tool은 별도 구현 */ }
    // ...
}
```

실제 oxi-as-server는 **tool-id-별 handler 동적 라우팅**이 필요. 단일 핸드셰이크에서 `tools/list`로 노출 목록을 응답하고, `tools/call`에서 tool-id로 라우팅하는 어댑터가 필요. 이는 본 설계 범위 외 — 별도 디자인으로 분리. 본 PR의 범위는 **프레임워크만** (handler trait + server skeleton).

#### B.3 Hooks — server→client 의미론

```rust
pub enum HookEvent {
    Cancel { call_id: String, reason: String },
    Pause { session_id: String },
    Resume { session_id: String },
    SessionEnded { session_id: String, reason: String },
    Custom { kind: String, payload: Value },
}

/// McpTransport 위에서 동작하는 HookReceiver.
pub struct HookReceiver {
    inbound_handler: Arc<parking_lot::Mutex<Option<HookHandler>>>,
}
```

구현: `McpTransport::set_inbound_handler`가 이미 알림 + server→client 요청을 받음. 기존 인프라로 가능. 클라이언트측 dispatch만 추가.

#### B.4 단계적 마이그레이션 순서

| PR | 내용 | 위험 |
|---|---|---|
| **PR-B1** | `WebSocketTransport` 트레잇 구현. McpConfig에 `url` + `transport = "websocket"` 추가. 기존 MCP 동작과 분리 (feature flag `mcp-ws` 또는 cargo feature). | 중간 — WebSocket 라이브러리 의존성 추가 (oxi는 이미 `tokio-tungstenite` 안 씀 → 신규 의존) |
| **PR-B2** | 자동 재연결 + replay buffer. WebSocketTransport 내부. | 중간 |
| **PR-B3** | `ToolServer` + `ToolServerHandler` 스켈레톤. 서버 바이너리 모드 신규 (예: `oxi serve --bind 127.0.0.1:9000`). | 높음 — 새로운 네트워크 표면 |
| **PR-B4** | `HookEvent` 클라이언트 디스패치. `McpClient`의 `inbound_handler` 확장. | 낮음 |

#### B.5 검증

- **PR-B1**: WebSocket MCP 서버(별도 프로세스) 띄우고 `cargo nextest run -p oxi-agent -- mcp::ws` 로 round-trip.
- **PR-B2**: 서버 강제 종료 후 자동 재연결 확인. replay buffer 차면 oldest drop.
- **PR-B3**: `oxi serve` 모드에서 WebSocket 클라이언트(테스트 harness)로 `tools/list` + `tools/call` 라운드트립.
- **PR-B4**: 서버→클라이언트 hook (예: cancel) 발사 시 클라이언트가 `CancelOnDrop` 동작.
- **clippy**: cargo feature 추가 시 `cargo clippy -p oxi-agent --features mcp-ws -- -D warnings`.

#### B.6 위험과 mitigation

- **WebSocket 신규 의존성**: `tokio-tungstenite` (grok Cargo.toml:94). 트리 크기 약간 증가 — 의존성 그래프 변경은 `cargo deny check` 게이트 영향.
- **서버 모드의 보안 표면**: `oxi serve`는 외부 입력을 받아 도구를 실행. 기존 CLI의 bash 도구/파일 도구가 그대로 노출되면 위험. → 서버 모드 시작 시 **기본 도구 화이트리스트** 적용 또는 `--allow-tools` 명시 요구. 본 PR-B3 자체는 스켈레톤만 (기본 allowlist 적용).
- **replay 의미론**: 서버가 idempotent id 처리를 가정. oxi-as-server의 본 PR-B3 구현이 동일한 의미론을 따라야 함 (스펙 문서화 필요).
- **MCP 표준 호환성**: WebSocket transport는 MCP 표준 외. 향후 MCP가 WS를 추가하면 그대로 사용 가능.

---

## 3. 후보 C — 컴팩션 reminder 시스템

### 목표

`xai-grok-compaction/src/reminder.rs:87`의 `ActiveAgentReminderState` 패턴 차용. oxi는 현재 `CompactionStrategy`(`oxi-ai/src/compaction.rs:335-377`)가 단순 압축 비율/턴수/토큰 임계값만 보고 컴팩션 시점을 결정. **활성 todo + 서브에이전트 + 백그라운드 작업 상태를 컴팩션 알림에 반영**해 LLM이 컨텍스트 손실 후에도 활성 작업을 인지.

### 문제 분석

- **컴팩션 후 todo 손실**: `oxi-ai::compaction.rs:31`의 `generate_branch_summary`는 메시지 텍스트에서 토픽/결정 추출. 활성 todo 상태는 무시됨.
- **컴팩션 시점 결정이 빈약**: `CompactionStrategy::Threshold`/`EveryNTurns`/`AbsoluteTokens`/`Snapcompact`(`compaction.rs:367-377`) 정적 결정. 활성 작업이 많을 때는 더 자주, 적을 때는 더 드물게 컴팩션 가능.
- **컴팩션 후 reminder 부재**: grok는 reminder prompt를 생성(`reminder.rs:519` LOC의 prompt builder)해서 컴팩션 후 LLM에게 재주입. oxi의 `compaction_instruction: Option<String>`(`oxi-agent/src/agent_loop/config.rs:21`)은 단순 정적 문자열.

### 설계

#### C.1 신규 모듈 — `oxi-agent/src/agent_loop/reminder.rs`

```rust
/// 컴팩션 시점에 활성 상태를 조회해 reminder 문자열을 생성한다.
/// Grok의 `ActiveAgentReminderState`(xai-grok-compaction/src/reminder.rs:87) 차용.
pub struct ActiveReminderInputs<'a> {
    pub todos: Option<&'a dyn TodoStateProvider>,
    pub subagent_state: Option<&'a SubagentRegistry>,  // 향후 추가 (현재 oxi 에는 없음)
    pub background_tasks: Option<&'a BackgroundTaskTracker>,  // 향후 추가
    pub last_n_assistant_turns: usize,
}

pub struct CompactionReminderBuilder {
    /// 컴팩션 prompt 의 "focus areas:" 섹션에 들어갈 문자열을 만든다.
    pub fn build(&self, inputs: &ActiveReminderInputs, ctx: &ReminderContext) -> String {
        let mut sections = Vec::new();
        // (1) todo 요약 — is_actionable 인 phase 의 첫 항목 + 카운트
        if let Some(todos) = inputs.todos {
            if let Some(phase_list) = todos.get_phases_blocking() {
                let active = phase_list.iter().filter(|p| p.is_active()).take(5).count();
                if active > 0 {
                    sections.push(format!("Active todos: {} phases with actionable items", active));
                }
            }
        }
        // (2) sub-agent 활성 카운트
        if let Some(subs) = inputs.subagent_state {
            let active = subs.active_count();
            if active > 0 {
                sections.push(format!("Active subagents: {}", active));
            }
        }
        // (3) background tasks
        if let Some(bg) = inputs.background_tasks {
            let running = bg.running_count();
            if running > 0 {
                sections.push(format!("Running background tasks: {}", running));
            }
        }
        // (4) 마지막 N 어시스턴트 턴에서 작업 키워드 추출 (현재 generate_branch_summary 의 로직 차용)
        // ...
        sections.join("\n")
    }
}
```

#### C.2 `compaction_instruction` 자동 주입 — `oxi-agent/src/agent_loop/config.rs:21`

```rust
impl AgentLoopConfig {
    /// Effective compaction instruction — `compaction_instruction` (정적) +
    /// `CompactionReminderBuilder::build(...)` (동적). 두 개가 모두 있을 때
    /// 동적 reminder 가 정적 instruction 뒤에 append 된다.
    pub fn effective_compaction_instruction(
        &self,
        inputs: &ActiveReminderInputs,
        reminder_ctx: &ReminderContext,
    ) -> Option<String> {
        let static_part = self.compaction_instruction.clone();
        let dynamic_part = self.reminder_builder.as_ref()
            .map(|b| b.build(inputs, reminder_ctx))
            .filter(|s| !s.is_empty());
        match (static_part, dynamic_part) {
            (None, None) => None,
            (Some(s), None) => Some(s),
            (None, Some(d)) => Some(d),
            (Some(s), Some(d)) => Some(format!("{}\n\nFocus areas after compaction:\n{}", s, d)),
        }
    }
}
```

#### C.3 호출 지점 — `oxi-agent/src/agent_loop/mod.rs:1160` 부근

기존 `if let Some(ref hook) = self.config.on_compaction { ... }` 직전에 `effective_compaction_instruction` 호출. 호출에 필요한 `TodoStateProvider`는 `AgentLoopConfig.todo: Option<Arc<dyn TodoStateProvider>>`(`config.rs:61`)에서 가져옴 — **이미 있음**.

#### C.4 단계적 마이그레이션 순서

| PR | 내용 | 위험 |
|---|---|---|
| **PR-C1** | `reminder.rs` 신규 모듈. `ActiveReminderInputs` + `CompactionReminderBuilder` 추가. **todo 만 구현**. 테스트: 활성 todo 0/1/5/10 케이스에서 reminder 문자열 형식 확인. | 낮음 — 기존 동작 보존 |
| **PR-C2** | `AgentLoopConfig::effective_compaction_instruction` 추가. `compaction_instruction` 와 OR 결합. | 낮음 |
| **PR-C3** | `AgentLoop` 의 컴팩션 호출 지점에서 reminder 사용. `TodoStateProvider` 는 기존 wire-up (config.rs:61 + bootstrap.rs 에서 `oxi-cli::store::todo_state::TodoState`) 그대로 사용. | 중간 — reminder 가 LLM 컨텍스트에 들어가므로 토큰 비용 검증 필요 |
| **PR-C4 (옵션)** | 서브에이전트 + background tasks 추가. oxi 현재 미존재 → 별도 디자인 필요. | 높음 |

#### C.5 검증

- **PR-C1 회귀**: `cargo nextest run -p oxi-agent reminder` — reminder 단위 테스트 4개 케이스.
- **PR-C3 회귀**: 통합 테스트 — todo 3개 활성 상태에서 컴팩션 트리거 → 컴팩션 prompt 에 reminder 포함 확인. `cargo nextest run -p oxi-agent -- agent_loop::compaction`.
- **토큰 비용**: 컴팩션 prompt 가 `target_ratio` 슬라이더 (`oxi-ai/src/compaction.rs:147`) 안에 머무는지 확인. reminder 가 200 토큰 초과하지 않도록 cap.
- **clippy**: 모듈 추가뿐, 기존 동작 보존 — 자연 통과.

#### C.6 위험과 mitigation

- **TodoStateProvider 부재 시**: `Option<Arc<dyn TodoStateProvider>>` — `None`이면 reminder 빈 문자열. oxi-cli 가 아닌 다른 consumer(oxios 등)는 None 가능. fallback 처리 필수.
- **스레드 안전성**: `TodoStateProvider::get_phases` 가 `Vec<TodoPhase>` 반환 (tools/todo.rs:101). 컴팩션은 컴팩션 태스크에서 호출 — `RwLock` read lock만 잡으면 충분.
- **LLM 의 reminder 신뢰도**: AGENTS.md 의 Pitfalls 항목 — 채널 정책이 강한 기본일 뿐 100% 강제 아님. reminder 도 같은 카테고리. → "MUST" 가 아닌 "should focus on" 톤으로.
- **컴팩션 instruction 충돌**: 사용자 정의 `compaction_instruction` 이 있으면 정적 + 동적 concat. 사용자가 reminder 비활성화 옵션 (`AgentLoopConfig.compaction_reminder_enabled: bool`).

---

## 4. 후보 D — 샌드박스 추출

### 목표

`oxi-cli::bootstrap` 안에 인라인으로 박혀있는 (있다면) sandbox 설정을 `oxi-sandbox` 크레이트로 추출. grok의 `xai-grok-sandbox`(2,577 LOC: lib.rs + profiles.rs + network_policy.rs + child_net.rs) 패턴 차용.

### 문제 분석

- **현재 위치 추정**: `oxi-cli::bootstrap` (`oxi-cli/src/bootstrap.rs:18`) 에서 `build_app(args)` 가 sandbox 설정하는지 grep 결과 — `nono`/`bwrap` 매치 없음. sandbox 정책은 oxi-sdk `AccessGate` port (`oxi-sdk/src/ports/mod.rs:643`) 에 추상화돼 있을 가능성. 검증 필요.
- **`AccessGate` 현재 사용**: grep 결과 `SimpleAccessGate` 또는 `AllowAllAccessGate` 정도일 듯. macOS bubblewrap 미지원(`bwrap`는 Linux 전용).
- **oxi 의 macOS-only 매트릭스**: AGENTS.md Pitfalls — `test.yml` macOS-only. Linux bubblewrap 적용 범위 확인 필요.

### 설계

#### D.1 신규 크레이트 — `oxi-sandbox/`

Cargo.toml 신규:
```toml
[package]
name = "oxi-sandbox"
edition.workspace = true

[features]
default = []
linux-bwrap = ["dep:nono"]  # Linux bubblewrap
macos-sandbox-exec = []      # macOS sandbox-exec wrapper

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }

nono = { version = "0.1", optional = true }
```

#### D.2 구조

```
oxi-sandbox/src/
├── lib.rs              # SandboxManager, ProfileName
├── profiles.rs         # ReadOnly, WorkspaceWrite, NetworkRestricted
├── policy.rs           # Path/network/command rules
├── command.rs          # Command wrapping (bwrap re-exec / sandbox-exec)
├── metrics.rs          # violation counter (Prometheus optional)
└── platform/
    ├── linux.rs        # bwrap integration
    └── macos.rs        # sandbox-exec integration
```

#### D.3 핵심 타입

```rust
pub enum SandboxProfile {
    ReadOnly,           // file reads only
    WorkspaceWrite,     // workspace reads/writes only
    NetworkRestricted,  // workspace + limited outbound
    Custom(PolicyRules),
}

pub struct PolicyRules {
    pub allowed_read_paths: Vec<PathBuf>,
    pub allowed_write_paths: Vec<PathBuf>,
    pub allowed_network_hosts: Vec<String>,
    pub blocked_env_vars: Vec<String>,  // 기존 MCP stdio.rs:30-35 와 동일
}

pub struct SandboxManager {
    profile: SandboxProfile,
    workspace_root: PathBuf,
    state: parking_lot::RwLock<SandboxState>,
}

impl SandboxManager {
    pub fn new(profile: SandboxProfile, workspace_root: PathBuf) -> Self { ... }
    /// Wrap a Command to run inside the sandbox.
    pub fn wrap(&self, cmd: &mut tokio::process::Command) -> Result<()> { ... }
    pub fn log_violation(&self, target: &str, op: &str) { ... }
}
```

#### D.4 oxi-sdk `AccessGate` port 와의 통합

`oxi_sdk::ports::AccessGate`(`oxi-sdk/src/ports/mod.rs:643`)는 pre-execution policy. 본 sandbox 는 **`AccessGate` 의 구현체**로 들어감:

```rust
// oxi-sdk/src/ports/sandbox_gate.rs (신규)
pub struct SandboxAccessGate {
    manager: Arc<oxi_sandbox::SandboxManager>,
}

#[async_trait]
impl AccessGate for SandboxAccessGate {
    async fn check(&self, action: &Action) -> Result<Decision> {
        match action {
            Action::FileRead(p) => self.manager.check_read(p).await,
            Action::FileWrite(p) => self.manager.check_write(p).await,
            Action::Command(cmd) => self.manager.check_command(cmd).await,
            Action::Network(host) => self.manager.check_network(host).await,
            _ => Ok(Decision::Allow),  // unimplemented action = allow (default)
        }
    }
}
```

#### D.5 단계적 마이그레이션 순서

| PR | 내용 | 위험 |
|---|---|---|
| **PR-D1** | `oxi-sandbox` 크레이트 스켈레톤. `SandboxProfile` + `PolicyRules` + `SandboxManager` 타입만. Linux/macOS feature 분리. **bwrap 호출은 stub** (테스트만 통과). | 낮음 |
| **PR-D2** | Linux bwrap 통합. `nono` 의존성 추가. macOS feature 게이트. | 중간 — Linux 빌드 매트릭스 추가 필요 |
| **PR-D3** | `SandboxAccessGate` 구현 + `oxi-sdk::ports` 에 노출. | 낮음 |
| **PR-D4** | `oxi-cli::bootstrap` 에서 기본 정책 자동 적용 (bash 도구 + edit 도구). TUI 세팅 overlay 에 toggle 추가. | 중간 |

#### D.6 검증

- **PR-D1**: `cargo nextest run -p oxi-sandbox` — 타입 단위 테스트.
- **PR-D2**: Linux CI 워커스에서 `cargo test -p oxi-sandbox --features linux-bwrap` — bwrap 실행 테스트 (Docker 가능).
- **PR-D3**: `cargo nextest run -p oxi-sdk -- sandbox` — AccessGate 구현 단위 테스트.
- **PR-D4**: `cargo nextest run -p oxi-cli` — 기존 TUI 진입 회귀 + sandbox-off 분기.
- **clippy**: 각 PR 표준 게이트.

#### D.7 위험과 mitigation

- **bwrap Linux 전용**: macOS 매트릭스만 가진 oxi 와 충돌. macOS 에서는 `sandbox-exec` 또는 Seatbelt 사용. feature gate 분리.
- **`AccessGate`가 pre-execution 검사만**: bash 도구의 자식 프로세스가 sandbox escape 시도 가능. → `SandboxManager::wrap` 으로 명령 자체를 감싸는 것이 본질. AccessGate 통합은 1차, 명령 wrapping 은 2차.
- **nono crate 안정성**: grok 는 `nono` 직접 의존 (Cargo.toml 미언급, 코드에서 import). 의존성 안정성 확인 필요.
- **CI 비용**: Linux 워커스 추가 → CI 시간 + 약 5분. `.github/workflows/test.yml` 수정.

---

## 5. 통합 일정 + 의존성

```
PR-A1 (TypedTool 트레잇 + 어댑터)
    ↓ (independent)
PR-A2 (ClosureTool typed variant)
    ↓
PR-A3..N (32개 도구 순차 이식, 우선 essential)
    ↓ (independent — typed 와 무관)
PR-B1 (WebSocket transport)
    ↓
PR-B2 (자동 재연결 + replay)
    ↓
PR-B3 (ToolServer)
    ↓
PR-B4 (Hooks)
    ↓
PR-C1..C3 (reminder)
    ↓ (independent)
PR-D1..D4 (sandbox)
```

**의존성**:
- A 는 B/C/D 와 완전 독립 — 가장 먼저 가능.
- B 는 C 와 독립 — 별도 트랙.
- C 는 A 와 독립 — AgentLoopConfig 만 의존.
- D 는 AccessGate port 와만 의존 — `oxi-sdk` 변경.

**권장 순서**: A1 → C1 (quick win) → A2 → D1 → B1 → C2 → A3..N (장기) → B2..B4 → D2..D4

---

## 6. 공통 검증 — 모든 PR 통과 기준

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` (AGENTS.md Pitfalls)
4. `cargo nextest run --workspace` (모든 테스트 통과)
5. `cargo audit` + `cargo deny check` (신규 의존성 추가 시)

**회귀 테스트 anchor**:
- 도구 통합: `oxi-agent/tests/agent_loop_full.rs` (AGENTS.md 참조)
- MCP 통합: `oxi-agent/src/mcp/` 의 기존 단위 테스트
- SDK 통합: `oxi-sdk/src/lib.rs:262 mod tests`

---

## 7. 메트릭 + 완료 기준

| 후보 | 완료 시 검증 가능한 효과 |
|---|---|
| **A** | 첫 이식된 도구(`grep`)의 `parameters_schema()` 가 `schemars` derive 로 생성됨. `tool_parameters()` 가 기존 수동 JSON Schema 와 의미 동등. 회귀 테스트 통과. |
| **B** | WebSocket MCP 서버에 oxi 가 클라이언트로 연결 + 도구 호출 라운드트립. 자동 재연결 + replay 동작 검증. |
| **C** | 컴팩션 후 todo 활성 상태 reminder 가 LLM prompt 에 포함됨 (integration test). |
| **D** | `oxi-cli --sandbox=workspace-write` 로 bash 명령 실행 시 workspace 외 접근 차단 (macOS sandbox-exec 또는 Linux bwrap). |

---

## 8. 라이선스 + 출처 표기

grok-build 는 Apache-2.0 (LICENSE). 본 설계는 **설계 차용**이므로 코드 복사가 아닌 패턴 채택. 향후 직접 구현 시 grok `crates/common/xai-computer-hub-sdk/src/{harness,server,pool}.rs` 와 `crates/common/xai-tool-runtime/src/tool.rs` 를 참고할 경우 — `THIRD-PARTY-NOTICES` 에 original source + Apache §4(b) "change notice" 추가. (grok 자체 THIRD-PARTY-NOTICES Cargo.toml:135-137 에 동일 관행 등재.)

oxi 는 MIT. Apache-2.0 코드와 호환 (양방향 호환).
