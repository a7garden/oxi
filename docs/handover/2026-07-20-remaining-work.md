# 인수인계서: oxi grok-build 적용 잔여 작업

**작성일**: 2026-07-20
**작성자**: coding-agent 세션 (b38f3693 ~ e6fed893)
**대상**: TypedTool 마이그레이션 + grok-build-applied-design 미완료 항목

---

## 현재 상태 요약

| 영역 | 진행률 | 완료된 파일 | 미완료 |
|------|--------|-------------|--------|
| A-Tools (TypedTool) | 16/25개 도구 | grep, read, write, bash, find, ls, edit, memory_recall, memory_reflect, memory_retain, memory_edit, web_search, get_search_results, generate_image, github_search, context7×2 | ask, lsp, browse(4), subagent, commit, todo, github |
| B-Transport | B1/B2/B3/B4 완료 | ws_transport (skeleton + I/O), oxi_as_server (server + hooks) | — (구조 완료) |
| C-Reminder | C1 완료 (data+builder+tests) | reminder.rs | C2-C4: agent loop wiring |
| D-Sandbox | D1 완료 (skeleton) | SandboxProfile, PolicyRules, SandboxError, SandboxManager | D2+: platform impl |
| WS Transport | B2 기본 I/O 완료 | connect/disconnect/read/write | reconnect + replay + handler dispatch |

**최종 검증**: 3448/3448 tests pass, `cargo clippy -D warnings` clean.

---

## 1. Non-essential tools TypedTool 마이그레이션 (~9개 도구)

### 마이그레이션 패턴 (13개 도구에서 검증됨)

각 도구 파일에 대해 다음 4단계를 수행:

```
1. IMPORTS 추가
   - use schemars::JsonSchema;
   - use serde::Deserialize;
   - use crate::tools::typed::TypedTool;

2. ARGS STRUCT 추가 (도구 struct 직전)
   #[derive(Deserialize, JsonSchema)]
   pub struct XxxArgs {
       // parameters_schema()의 모든 필드를 정확히 매핑
       // #[serde(default)] 또는 #[serde(default = "fn")]로 기본값 처리
       // #[serde(rename = "camelCase")]로 JSON 필드명 매핑
   }

3. TYPEDTOOL IMPL 추가 (AgentTool impl 직후)
   #[async_trait]
   impl TypedTool for XxxTool {
       type Args = XxxArgs;
       async fn execute_typed(...) {
           // AgentTool::execute()의 로직을 그대로 복사,
           // params.get("field") 대신 args.field 사용
       }
   }

4. EXECUTE DELEGATE (AgentTool::execute 본체 교체)
   async fn execute(&self, ..., params: Value, ...) {
       let args: XxxArgs = serde_json::from_value(params)
           .map_err(|e| format!("invalid params: {e}"))?;
       self.execute_typed(..., args, ...).await
   }
```

### 중요 규칙

- **TypedTool에는 `type Args`와 `execute_typed()`만 정의** — name/label/description/essential/parameters_schema/execution_mode는 AgentTool에 그대로 둠
- **AgentTool impl은 절대 건드리지 않음** (parameters_schema hand-crafted JSON 유지)
- **ToolError = String** (type alias) — `format!(...)`으로 에러 생성
- 테스트에서 `tool.execute(...)` 직접 호출은 AgentTool impl이 남아있어야 동작

### 도구별 상세

#### 1. ask (ask.rs, ~779 lines)
- **스키마**: `questions: Vec<Question>`, `recommended: Option<usize>`, `multi: Option<bool>`
- **execute**: `parse_questions(&params)` 호출 → bridge 저장 → `select_with_abort` 대기
- **특이사항**: `AskBridge` 필드 + `on_progress` 없음. `parse_questions()`는 free function
- **TypedTool 접근**: args → `serde_json::to_value(&args)` → `parse_questions()` 재사용

#### 2. lsp (lsp.rs, ~166 lines)
- **스키마**: `action: String` (enum, required), `file`, `old_path`, `new_path`, `line`, `symbol`, `new_name`, `apply: bool`, `query`
- **execute**: `parse_lsp_action(&params)` → `provider.execute_action(&action)`
- **참고**: 이전 세션에서 1회 시도했으나 stale anchor로 복구됨 (git checkout). 최초 상태로 돌아감

#### 3. browse_tool (browse/browse_tool.rs)
- **스키마**: `url: String` (required), `action: Option<String>`, `timeout: Option<u64>`
- **import**: `use crate::tools::typed::TypedTool;`

#### 4. browse_extract_tool (browse/browse_extract_tool.rs)
- **스키마**: `url: String` (required), `selector: Option<String>`, `attribute: Option<String>`

#### 5. browse_script_tool (browse/browse_script_tool.rs)
- **스키마**: `code: String` (required), `url: Option<String>`, `timeout: Option<u64>`

#### 6. browse_session_tool (browse/browse_session_tool.rs)
- **스키마**: `action: String` (enum: open/close/run), `url: Option<String>`, `code: Option<String>`, `name: Option<String>`

#### 7. subagent (subagent.rs)
- **스키마**: `task: String` (required), `agent: Option<String>`, `schema: Option<Value>`, `isolated: Option<bool>`, `handle: Option<bool>`
- **execute**: `SubagentRunner::run_isolated()` 호출. 복잡한 옵션 처리 로직

#### 8. commit (commit.rs, ~1300 lines)
- **스키마**: `type: String` (required, enum), `scope: Option<String>`, `title: String` (required), `body: Option<String>`, `breaking: Option<bool>`, `issues: Option<Vec<String>>`, `edit: Option<String>`
- **execute**: LLM 분석 + git commit 생성. 복잡한 다단계 로직

#### 9. todo (todo.rs)
- **스키마**: `op: String` (enum: init/start/done/drop/rm/append/view), `task: Option<String>`, `phase: Option<String>`, `items: Option<Vec<String>>`, `list: Option<Vec<{phase,items}>>`
- **execute**: op-dispatch 패턴. 각 op마다 다른 params

#### 10. github (github.rs, ~1400 lines)
- **스키마**: `op: String` (required), `repo`, `branch`, `path`, `pr`, `force`, `title`, `body`, `base`, `head`, `draft`, `fill`, `reviewer`, `assignee`, `label`, `query`, `since`, `until`, `limit`, `run`, `tail` 등 20+ params
- **execute**: op-dispatch. `gh` CLI 호출 또는 REST API 호출
- **특이사항**: 가장 큰 파일. 다수의 sub-command

### 검증 방법
```bash
cargo check -p oxi-agent
cargo nextest run -p oxi-agent
```

---

## 2. C2-C4: Compaction Reminder Wiring

### 현재 상태
- `oxi-agent/src/agent_loop/reminder.rs` — 완전히 구현됨 (PR-C1)
  - `ActiveReminderInputs`, `CompactionReminderBuilder`, `todo_summary()`, `combine_instruction()`
  - 136 lines of tests
- `oxi-agent/src/agent_loop/mod.rs` — `pub mod reminder;` 선언됨

### 해야 할 일

#### C2: agent_loop/mod.rs에 reminder wiring
파일: `oxi-agent/src/agent_loop/mod.rs`

1. `super::reminder::{ActiveReminderInputs, CompactionReminderBuilder, combine_instruction}` import
2. `AgentLoopConfig`에 `todo_provider: Option<Arc<dyn TodoStateProvider>>` 필드 추가
3. compaction/build_compaction_instruction 함수에서 compaction 시점에 `CompactionReminderBuilder::new().build(&inputs)` 호출
4. 결과를 compaction instruction에 추가

```rust
// mod.rs 내 compaction 로직 위치
// "compaction" 또는 "compact"로 검색하여 찾을 수 있음
// 대략 500-700 line 영역
```

#### C3: oxi-cli composition root에서 wiring
파일: `oxi-cli/src/bootstrap.rs` 또는 `oxi-cli/src/app/agent_session_*.rs`

1. `AgentLoopConfig` 구성 시 `todo_provider` 설정
2. 세션의 `TodoStateProvider` 구현체 전달

#### C4: 테스트
```bash
cargo nextest run -p oxi-agent -- reminder
```

---

## 3. D2+: oxi-sandbox Platform Implementation

### 현재 상태
- `/Volumes/MERCURY/PROJECTS/oxi/oxi-sandbox/` — skeleton crate
- `SandboxProfile` (enum: ReadOnly, WorkspaceWrite, NetworkRestricted, Custom)
- `PolicyRules` (struct: allowed_paths, blocked_paths, network_access, etc.)
- `SandboxError` (enum: NotAvailable, Execution, Config)
- `SandboxManager` (struct with `wrap_command()` stub)

### 해야 할 일

#### D2: macOS sandbox-exec 구현
파일: `oxi-sandbox/src/macos.rs` (신규)
```rust
use crate::{SandboxError, SandboxProfile};

pub struct Sandbox;

impl Sandbox {
    pub fn run(profile: &SandboxProfile, command: &str, args: &[&str])
        -> Result<CommandOutput, SandboxError>
    {
        // sandbox-exec로 프로세스 래핑
        // profile에 따라 sandbox 프로파일 생성
        // fallback: unsandboxed 실행
    }
}
```

#### D3: Linux bwrap 구현
파일: `oxi-sandbox/src/linux.rs` (신규)
```rust
use crate::{SandboxError, SandboxProfile};

pub struct Sandbox;

impl Sandbox {
    pub fn run(profile: &SandboxProfile, command: &str, args: &[&str])
        -> Result<CommandOutput, SandboxError>
    {
        // bwrap(1) 명령어로 sandbox 구성
        // --ro-bind /usr /usr
        // --proc /proc
        // --dev /dev
        // 필요시 --unshare-net
    }
}
```

#### D4: Fallback noop 구현
파일: `oxi-sandbox/src/noop.rs` (신규)
```rust
pub struct Sandbox;
impl Sandbox {
    pub fn run(_profile: &SandboxProfile, command: &str, args: &[&str])
        -> Result<CommandOutput, SandboxError>
    {
        // 그냥 실행 (sandbox 없음)
    }
}
```

#### D5: lib.rs 수정
```rust
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), path = "noop.rs")]
mod platform;
pub use platform::Sandbox;
```

### 의존성
- `nono` crate — Linux bwrap 바인딩 (optional, feature: `linux-bwrap`)
- macOS는 stdlib만으로 가능

### 검증
```bash
cargo check -p oxi-sandbox
cargo check -p oxi-sandbox --features linux-bwrap  # Linux 전용
```

---

## 4. WS Transport: reconnect + replay + handler dispatch

### 현재 상태
- `oxi-agent/src/mcp/transport/ws_transport.rs` — 기본 I/O 구현됨 (PR-B2)
- `WebSocketTransport { url, state: Arc<Mutex<WsState>> }`
- `connect()` — spawn reader/writer tasks with tokio-tungstenite
- `request()` — broadcast 기반 response matching
- `notify()` — mpsc 기반 전송
- Feature gate: `ws-transport` (tokio-tungstenite 의존)

### 해야 할 일

#### WS-1: InboundHandler dispatch
`request()` 메서드에서 non-matching 메시지를 `InboundHandler`로 라우팅

```rust
// 현재 request()는 non-matching 메시지를 드롭
// 수정: handler에 dispatch
if msg.id == Some(id) {
    return Ok(msg);
} else if let Some(ref mut handler) = s.handler {
    // handler(msg);  // InboundHandler 호출
}
```

#### WS-2: Auto-reconnect on disconnect
Reader/writer task 종료 감지 → 자동 재연결

```rust
// connect_inner() 내부
// reader/writer task가 종료되면 state 업데이트
// WebSocketTransport::ensure_connected() → 재연결 시도
pub async fn ensure_connected(&self) -> Result<()> {
    if !self.is_connected() {
        Self::connect_inner(&self.state, &self.url).await?;
    }
    Ok(())
}
```

#### WS-3: Replay buffer 재전송
재연결 시 in-flight request 재전송
```rust
// replay_buf에 저장된 (id, json) 쌍을 재전송
for (id, json) in s.replay_buf.iter() {
    s.outbound_tx.send(json.clone()).ok();
}
```

#### WS-4: 테스트
```bash
cargo check -p oxi-agent --features ws-transport
# 실제 ws 서버 필요시 integration test
```

---

## 아키텍처 참고

### 의존성 그래프
```
oxi-pager ← oxi-cli ← oxi-sdk ← oxi-agent ← oxi-ai
                                      └── mcp/
                                          ├── ws_transport.rs (feature-gated)
                                          └── oxi_as_server.rs
```

### 키 파일 위치

| 관심사 | 파일 |
|--------|------|
| TypedTool trait + adapter | `oxi-agent/src/tools/typed.rs` |
| ToolRegistry + AgentTool trait | `oxi-agent/src/tools.rs` |
| MCP transport trait | `oxi-agent/src/mcp/transport/mod.rs` |
| MCP ws_transport (PR-B2) | `oxi-agent/src/mcp/transport/ws_transport.rs` |
| MCP oxi-as-server (PR-B3/B4) | `oxi-agent/src/mcp/oxi_as_server.rs` |
| Compaction reminder | `oxi-agent/src/agent_loop/reminder.rs` |
| Sandbox crate | `oxi-sandbox/src/lib.rs` |
| Reminder wiring target | `oxi-agent/src/agent_loop/mod.rs` |

### 빌드 명령어
```bash
cargo check --workspace                       # 전체 빌드
cargo check -p oxi-agent --features ws-transport  # WS transport 포함
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```
