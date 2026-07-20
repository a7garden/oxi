# xai-org/grok-build → oxi 비교 분석

- 작성일: 2026-07-20
- 보고서 언어: 한국어 (`$language=ko`)
- 분석 초점: `$focus=architecture` (구조 + SDK 적합성)
- 소스 동기 커밋: `grok-build@98c3b24` (`SOURCE_REV` 기록 — monorepo에서 주기 동기)

## 판정

**Port partially** — grok-build는 (1) 분산 toolbus 프로토콜과 (2) typed tool signature로 oxi와 다른 차원의 두 강점을 보유. oxi의 단일 프로세스 + JSON-Value tool 설계로는 이 둘을 표현할 수 없음. 다만 전체 이식이 아닌 두 곳의 **설계 차용**이 가성비 최적.

---

## 요약

`xai-org/grok-build`는 SpaceXAI가 x.ai/cli로 배포하는 **터미널 기반 AI 코딩 에이전트**의 Rust 소스 공개본. TUI(`xai-grok-pager`) + 셸(`xai-grok-shell`) + 도구(`xai-grok-tools`) + 작업공간(`xai-grok-workspace`)으로 구성된 단일 Rust 워크스페이스이며 78개 크레이트를 가짐(Cargo.toml:6-85). 가장 핵심은 `crates/common/xai-computer-hub-sdk/` — **JSON-RPC over WebSocket으로 toolbus를 외부에 노출하는 양방향 SDK**. Tool은 `xai-tool-runtime::Tool`(`tool.rs:36`)의 **연관 타입(typed `Args` + typed `Output`) 트레잇**으로 표현되며 `ToolServer`/`ToolHarness`가 로컬 우선 디스패치 + 원격 fallback(`harness.rs:7-15`)을 제공.

oxi는 동일 카테고리(TUI 코딩 에이전트, Rust, MIT 라이선스)이지만 oxi-cli + oxi-tui + oxi-sdk의 3-tier 컴포지션 구조이며 외부 노출 가능한 toolbus 프로토콜은 없음. 핵심 가치는 **port 트레이트 15개**(oxi-sdk/src/ports/mod.rs)를 통한 in-process 컴포지션 단순성.

---

## oxi 현재 상태

### 모듈 구조

| 영역 | oxi | grok-build |
|---|---|---|
| 워크스페이스 | 6 crates (`AGENTS.md` 정의) | 78 crates (Cargo.toml:6-85) |
| 컴포지션 루트 | `oxi-cli/src/bootstrap.rs` | `crates/codegen/xai-grok-pager-bin` |
| TUI | `oxi-tui` (독립, ratatui 위주) | `xai-grok-pager` (2,650+ LOC의 server.rs 포함) |
| 셸/런타임 | `oxi-agent::agent_loop` | `xai-grok-shell`, `xai-grok-shell-base` |
| 도구 | `oxi-agent::tools/*.rs` (32개 모듈) | `xai-grok-tools/{implementations,computer,notification,registry,persistence,...}` |
| 작업공간 | `oxi-cli/src/store/` | `xai-grok-workspace` (VCS/체크포인트/실행) |
| **외부 노출 SDK** | `oxi-sdk` (port traits, in-process) | **`xai-computer-hub-sdk` (JSON-RPC over WS)** |
| LLM 추상 | `oxi-ai::providers::trait_def::Provider` | `xai-grok-models` (async-openai 위주) |
| 캐탈로그 | `oxi-ai::catalog` (3-layer, models.dev) | 없음 (`async-openai` 직접 의존 Cargo.toml:106) |
| 샌드박스 | `oxi-cli::bootstrap` (부분) | `xai-grok-sandbox` (2,577 LOC, bubblewrap+network policy) |

### 도구 트레이트 표면

**oxi** — `oxi-agent/src/tools.rs:200` 부근의 `AgentTool`:

```rust
async fn execute(
    &self,
    tool_call_id: &str,
    params: Value,                          // serde_json::Value, untyped
    signal: Option<oneshot::Receiver<()>>,
    ctx: &ToolContext,
) -> Result<AgentToolResult, ToolError>;
```

인자/출력 모두 `serde_json::Value`. `parameters_schema()`는 `Value`로 JSON Schema를 런타임에 노출.

**grok** — `crates/common/xai-tool-runtime/src/tool.rs:36-112`:

```rust
pub trait Tool: Send + Sync {
    type Args: for<'de> Deserialize<'de> + JsonSchema + Send + 'static;
    type Output: Serialize + ToolOutput + Send + 'static;
    fn id(&self) -> ToolId;
    fn description(&self, _ctx: &ListToolsContext) -> ToolDescription;
    fn capabilities(&self) -> ToolCapabilities { ... }
    fn should_list(&self, _ctx: &ListToolsContext) -> bool { true }
    fn execute(&self, ctx: ToolCallContext, args: Self::Args)
        -> impl Future<Output = ToolStream<Self::Output>> + Send;
    fn run(&self, ...) -> impl Future<...> { ... } // default = not_implemented
}
```

**연관 타입으로 강타입 signature** + `JsonSchema` derive로 JSON Schema 자동 생성. 스트리밍은 `ToolStream<T>` (`tool.rs:116`) — Progress N개 + Terminal 1개.

### SDK 진입점 비교

| 항목 | oxi-sdk (`builder.rs:23-43`) | grok computer-hub-sdk (`lib.rs`) |
|---|---|---|
| 핵심 타입 | `Oxi`, `OxiBuilder`, `AgentBuilder` | `ToolServer`, `ToolHarness`, `HubConnectionPool` |
| 컴포지션 방식 | Builder 메서드 (`with_state`, `with_auth`, ...) | Builder + WebSocket 풀 + 핸들러 등록 |
| 트랜스포트 | in-process (포트 → 컴포지션 루트) | JSON-RPC over WebSocket (원격 toolbus) |
| 다중 연결 | 없음 (단일 `Oxi` 인스턴스) | `HubConnectionPool` (`pool.rs:71`) — `(url, principal)` 키 공유 |
| 로컬 우선 디스패치 | 트레잇 객체 직접 디스패치 | `LocalRegistry` (`harness.rs:124-211`) → 미스 시 wire RPC |
| 권한/세션 | `AuthProvider` port (단일 자격증명) | `Principal { user_id, session_ids, scopes, audiences }` (transport.rs:24-40) — 멀티 세션 |
| 재연결 | N/A (프로세스 내부) | `HubConnection::ReconnectCallback` + 자동 replay (lib.rs:7-8) |
| 컴플라이언스 | `Cargo.toml` 전체 lint, workspace 단일 | 크레이트별 lint + workspace lint |

### 컴팩션

- **oxi** `oxi-ai/src/compaction.rs` — `CompactionConfig`, `CompactionStrategy`, `CompactedContext`. `generate_branch_summary` 단일 함수. 메시지 기반 압축.
- **grok** `crates/common/xai-grok-compaction/` (1288 LOC) — `lib.rs` + `item.rs` + `select.rs` + `reminder.rs`(519 LOC) + `sampler.rs` + `token.rs` + `prompt.rs`. **`ActiveAgentReminderState` 구조체**(reminder.rs:87)로 todo + 서브에이전트 + 백그라운드 작업의 활성 상태를 컴팩션 알림에 반영. 단순 요약보다 **LLM에게 다시 던지는 reminder prompt**를 생성하는 차원.

### 샌드박스

- **oxi**: `oxi-cli/src/bootstrap.rs`에 부분 통합. `nono`/`bwrap` 의존성 없음.
- **grok** `xai-grok-sandbox` (2,577 LOC) — `lib.rs`(`SandboxManager`), `profiles.rs`(823 LOC), `network_policy.rs`(501 LOC), `child_net.rs`. `bwrap` 재실행 패턴(`lib.rs:249`) + 네트워크 차단 + devbox 프로파일 + 자동 allow 정책. 프로덕션 수준.

### 외부 도구 통합 (MCP 외)

- **oxi**: MCP는 `oxi-agent::mcp`로 통합, 그 외 외부 프로토콜 없음.
- **grok**: ACP(`agent-client-protocol` Cargo.toml:92, `xai-acp-lib` 크레이트) — **IDE 임베딩을 1등 시민으로 다룸**. README.md:16에서 "embedded in editors via the Agent Client Protocol (ACP)" 명시. `xai-computer-hub-sdk` 자체가 분산 toolbus — 도구 제공자가 별도 프로세스/머신에서 동작 가능.

### 컴팩션/캐탈로그 부재

grok는 `async-openai`(Cargo.toml:106)에 직접 의존 — `oxi-ai`처럼 Layer 1 (정적 TOML) + Layer 2.5 (models.dev) + Layer 3 (런타임 `/v1/models`) 구조가 없음. 이는 **oxi의 강점**(모델 메타데이터 정밀도)으로 그 자리에 대응 가능.

---

## 적용 후보

- **[high]** **Typed tool signature로의 트레잇 마이그레이션** — `oxi-agent/src/tools.rs`의 `AgentTool` 트레잇이 `params: Value`를 받는 현재 설계는 32개 도구 모두 수동 JSON Schema + 수동 deserialize를 강제. grok의 `Tool<Args, Output>` 패턴(tool.rs:36-47)을 채택하면 (1) `schemars` derive로 JSON Schema 자동 생성 (grok는 `JsonSchema` 강제), (2) 컴파일 타임 인자 검증, (3) `ToolStream<Output>` 통일된 스트리밍 인터페이스로 `ToolProgress` 도입 가능. 위험은 모든 도구 시그니처 재작성 — 32 모듈 동시 변경은 PR 한 건으로 불가. **PR 1: 트레잇 추가 + 호환 어댑터(`TypedToolAdapter`로 기존 `AgentTool`을 새 트레잇으로 감쌈)**, **PR 2: 도구별 점진 이식**.

- **[high]** **분산 toolbus 프로토콜 옵트인** — `xai-computer-hub-sdk`(lib.rs:7-21)의 `ToolServer`/`ToolHarness` 설계는 oxi의 `oxi-sdk` 위에 얹을 수 있는 별도 어댑터 크레이트로 적합. `oxi-sdk`의 port 트레잇이 in-process 컴포지션을 보장하는 동안, `oxi-toolbus`(신규) 또는 `oxi-sdk`에 추가 모듈로 **opt-in JSON-RPC transport**를 제공. `agent-client-protocol`(Cargo.toml:92) 통합은 grok의 ACP 사용례(README.md:16)에서 직접 차용. oxi가 MCP만 지원하는 비대칭을 해소.

- **[medium]** **컴팩션 reminder 시스템 차용** — `xai-grok-compaction/src/reminder.rs:87`의 `ActiveAgentReminderState`가 todo/subagent/background task 상태를 LLM에게 재주입하는 패턴은 oxi의 `oxi-ai/src/compaction.rs`에 없는 차원. `compaction.rs`는 단순 `generate_branch_summary`. 차용처: `oxi-ai::compaction`에 `reminder_budget(state, slots) -> String` 추가, `oxi-agent::tools::todo`(현재 트레잇 보유)와 `oxi-agent::subagent_runner`를 호출. 토큰 비용은 동일 컨텍스트에서 reminder만 추가되므로 `CompactionConfig::target_ratio` 슬라이더에 흡수 가능.

- **[medium]** **샌드박스 프로파일 + 네트워크 정책 모듈화** — `xai-grok-sandbox` (2,577 LOC, bwrap 재실행 + 프로파일 + 네트워크 차단 + 자동 allow)는 oxi의 컴포지션 루트(`oxi-cli/src/bootstrap.rs`)에 인라인으로 들어가 있는 부분을 자체 크레이트 `oxi-sandbox`로 추출. grok의 `lib.rs:62-228`의 `GlobalSandboxState` + `ProfileName` + `SandboxMetrics` 패턴이 그대로 차용 가능. 단, `nono`/`bwrap` 의존성은 외부 시스템 호출이 필요하므로 macOS 우선(현 `oxi-cli`가 macOS-only 매트릭스 — AGENTS.md에 명시).

- **[low]** **`HubConnectionPool` 차용** — `pool.rs:71-152`의 `(url, principal)` 키 풀 + idle reaper는 oxi의 MCP `McpManager`(transport별)가 이미 자체 풀을 가짐에도 **MCP 외 트랜스포트**(예: 향후 ACP) 도입 시 재사용 가능. 다만 즉시 가치 < 향후 가치.

### 이미 oxi가 잘 하는 것

- **port 트레잇 15개**(`StateStore` ~ `EmbeddingProvider`) — grok는 단일 `Transport` 트레잇(transport.rs:88)만 노출. oxi의 컴포지션 단순성이 우위.
- **모델 카탈로그 3-layer**(`oxi-ai::catalog` + models.dev enrichment) — grok에 없는 강점.
- **MIT 라이선스 + 외부 기여 수용** — grok는 Apache-2.0 + 외부 PR 거부(CONTRIBUTING.md:3-4).
- **`oxi-hashline`** — grok는 `xai-hunk-tracker` 크레이트로 대응하지만 hashline 표준이 아님.

---

## 위험 / 검증

### typed tool signature 이식 시

- **무엇이 깨지는가**: `AgentTool`을 구현한 외부 도구(`oxi-sdk::closure_tool::ClosureTool` — `oxi-sdk/src/closure_tool.rs`, `oxi-cli`의 32개 도구)가 모두 `params: Value`를 받음. 호환 어댑터 없이 트레잇을 교체하면 32+ 모듈 + 클로저 도구 + 테스트 mock 동시 수정 필요. 차용처: `oxi-hashline`의 `hashline_fs` 도구처럼 JSON Schema/JSON 입출력이 핵심인 도구는 어댑터로 우선 호환 유지.
- **최소 회귀 테스트**: `cargo nextest run -p oxi-agent --features integration-tools`. 도구 통합 테스트는 `oxi-agent/tests/agent_loop_full.rs`(AGENTS.md 참조). 트레잇 추가 후 기존 도구들이 동일 입력/출력으로 동작하는지 비교.
- **clippy**: `cargo clippy --workspace --all-targets -- -D warnings`로 트레잇 추가 PR은 검증 가능. 단, `schemars` derive 매크로가 생성하는 코드는 lint 대상에서 제외될 가능성 있음 — 정확히 검증하려면 derive 결과물에 대한 lint 통과 확인 필요.

### 분산 toolbus 추가 시

- **무엇이 깨지는가**: `xai-computer-hub-sdk`는 tokio + tokio-tungstenite + reqwest + OTel + prometheus로 거대한 의존성 그래프. oxi에 그대로 들이면 의존성 표면 폭증. 차용은 **설계 패턴 차용**(트레잇 + 풀) 수준으로 한정하고, 의존성은 `axum`(oxi-cli가 의존) 또는 `tonic`(gRPC) 같은 경량 대안 검토.
- **최소 회귀 테스트**: toolbus 프로토콜은 wire-level round-trip 테스트가 필수 — `toolbus/tests/wire_roundtrip.rs` 같은 통합 테스트로 인-프로세스 두 호스트(`ToolServer` + `ToolHarness`)를 나란히 띄우고 도구 호출/응답 검증.
- **clippy**: 1차 PR은 새 크레이트 추가는 clippy 게이트에 자연스럽게 포함.

### 컴팩션 reminder 이식 시

- **무엇이 깨지는가**: `oxi-agent::tools::todo`의 `TodoStateProvider` 트레잇(tools.rs:99-112)을 컴팩션 시점에 호출하면 `Agent` 컨텍스트 외부에서 호출됨 — `ToolContext` 의존성이 없는지 확인 필요. oxi는 컴팩션이 `oxi-ai` 레벨에서 일어나므로 `oxi-agent`로의 역방향 의존은 금지(`AGENTS.md` dependency flow).
- **최소 회귀 테스트**: `cargo nextest run -p oxi-ai compaction::` — reminder가 활성 todo 있을 때만 trigger되는지 결정성 검증.
- **clippy**: `oxi-ai` 크레이트 내부 변경이라 workspace lint로 잡힘.

### 샌드박스 추출 시

- **무엇이 깨지는가**: `oxi-cli::bootstrap`이 `nono` crate를 직접 사용하는 부분이 있는지 미확인. 추출 시 빌드 프로필에 따라 sandbox 비활성화 경로가 깨질 수 있음.
- **최소 회귀 테스트**: `cargo nextest run -p oxi-cli` 전체, 특히 TUI 진입 경로의 sandbox-off 분기.
- **clippy**: 새 크레이트라 자연 포함.

---

## SDK 형태 적합성 평가

질문의 핵심 — "oxi-sdk처럼 다른 에이전트를 위한 SDK로 제공될 수 있는가?" — 에 대한 답:

### grok의 `xai-computer-hub-sdk`가 더 멀리 갔음

`xai-computer-hub-sdk/src/lib.rs`는 **도구 제공자(server)와 도구 소비자(harness)를 모두 1등 시민으로 노출**:

```rust
pub use harness::{ LocalRegistry, ModelOutputExtractor, ToolHarness, ToolHarnessBuilder };
pub use server::{ ToolServer, ToolServerBuilder, ToolServerHandler };
pub use pool::HubConnectionPool;
```

→ 한 프로세스가 **동시에 tool 제공자이면서 다른 toolbus 클라이언트**가 될 수 있음. 멀티 에이전트 협업, 에이전트-IDE-플러그인-백엔드 분리 배포 모두 가능.

### oxi-sdk의 한계

`oxi-sdk/src/builder.rs:23-43`의 `Oxi`는 **인-프로세스 엔진**:
- 포트 트레잇(15개) 교체로 컴포지션 변경 가능
- 그러나 다른 프로세스/머신에서 실행 중인 oxi 인스턴스와 통신할 표준 프로토콜 없음
- ACP(Agent Client Protocol)도 미통합 — grok는 `agent-client-protocol 0.10.4`를 workspace dep으로 직접 보유(Cargo.toml:92)

### oxi-sdk를 grok급으로 끌어올리는 경로

1. **현재 강점 유지**: port 트레잇 15개를 통한 in-process 컴포지션은 변경하지 않음
2. **`oxi-sdk`에 opt-in transport 어댑터 추가**:
   - `OxiBuilder::with_toolbus(Transport)` — JSON-RPC over WebSocket 어댑터 (grok의 `Transport` 트레잇 차용)
   - 포트 트레잇 구현이 네트워크 너머에서 호출 가능하도록
3. **ACP 통합**: `oxi-cli`가 ACP 서버로 동작 — IDE(Zed, Neovim 등)에서 oxi를 호출. `agent-client-protocol` crate 채택, `oxi-cli/src/rpc_mode`에 ACP 모듈 추가
4. **`oxi-cli --mode acp-server`** 같은 새 모드 — grok README.md:16의 명시적 사용례와 정합

이렇게 하면 oxi-sdk는 **로컬 port + 원격 transport 양쪽을 노출하는 멀티-에이전트 SDK**가 됨. grok의 `xai-computer-hub-sdk`가 본질적으로 같은 자리.

### 라이선스 문제 없음

grok는 Apache-2.0(LICENSE 파일)이고 외부 기여 거부. **그러나 코드 차용은 Apache §4(b) "change notice" 의무**만 지키면 됨 — THIRD-PARTY-NOTICES(Cargo.toml:135-137)에 codex/opencode 포트가 이미 동일한 방식으로 등재되어 있어 관행 확인됨. oxi(MIT)는 Apache 코드와 호환.

---

## 클로징

이번 분기(Q3 2026)에는 **typed tool signature 이식(2 PR)** + **컴팩션 reminder 차용(1 PR)** 만 진행 권장. 분산 toolbus와 ACP는 oxi-cli의 모놀리식 단일 진입점 설계(AGENTS.md Pitfalls)와 충돌하므로 별 분기 검토. grok-build의 강점은 인정하되, **oxi의 단일 프로세스 + 15-port 컴포지션**이 oxi의 정체성 — 그 위에 얹는 것이지 대체가 아님.