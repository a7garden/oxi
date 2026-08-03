# Advisor + Agent Hub 통합 설계

> **날짜**: 2026-07-29
> **상태**: 설계 확정, 구현 대기
> **상위 문서**: `omp-realignment-design.md` (P0–P4 완료 후속)
> **omp 소스**: `agent-hub.ts` (566), `agent-transcript-viewer.ts` (461), `transcript-recorder.ts` (159)
> **대상 크레이트**: `oxicode-cli/`, `oxicode-agent/`, `oxicode-tui/`

---

## 1. 문제 정의

### 1.1 advisor 시스템은 완성, 시각화만 부재

oxicode는 `oxicode-agent/src/advisor/` (1,846줄)에 advisor 엔진을 이미 포팅했다:
- `AdvisorRuntime` — primary turn을 shadow해서 비동기 advise
- `AdviseTool` — `nit`/`concern`/`blocker` severity와 함께 advise를 큐에 enqueue
- `AdvisorEmissionGuard` — 중복/무의미 advise 차단
- `AdvisorTranscriptRecorder` — `<session_dir>/__advisor.jsonl`에 모든 advise turn 기록
- `assemble_advisor_system_prompt` — AGENTS.md/WATCHDOG.md 컨텍스트 주입
- `AgentSession::build_advisor` — runtime 인스턴스화 + 도구 등록
- `forward_event_to_extensions` — primary turn 종료 시 advisor에 push
- `/advisor` slash 명령 — on/off/status

**빠진 것**: advisor가 뭘 advise하고 있는지 TUI에서 볼 수 없음. 현재 aside 채널 advise는 토스트로만 표시되고 사라짐. transcript 파일은 디스크에 있지만 아무도 읽지 않음.

### 1.2 subagent 모니터링은 처음부터 부재

`oxicode-sdk/src/lifecycle/AgentPool`/`AgentHandle` 데이터 계층은 있으나, oxicode-cli는 한 번도 `agent_pool: Some(...)`로 설정한 적이 없다. subagent는 out-of-process CLI로 spawn되어 자체 `session_file`에 기록되지만, 부모 TUI에서는 그 파일을 발견할 방법이 없다.

### 1.3 목표

**두 종류의 shadow agent(advisor + subagent)를 한 화면에서 모니터링하고, 각각의 live transcript를 볼 수 있게 한다.** omp의 Agent Hub를 oxicode의 tape 모델 + 기존 overlay 인프라에 맞게 이식.

---

## 2. 범위

### 2.1 in-scope

1. `AgentHandle`에 표시용 필드 추가: `display_name`, `kind`, `last_activity_ms`, `current_task`, `session_file`
2. `AgentSession`에 `AgentPool` 연결 + advisor/subagent 등록 훅
3. `AgentHubOverlay` (fullscreen alt-screen) — table view + transcript view
4. `/agents` slash 명령 (또는 `/hub`) — overlay open
5. 키 바인딩: `Ctrl+h` (또는 `F4`) — toggle hub
6. advisor: aside channel advise가 transcript 카드 (SystemMessage 외 추가) + 토스트로 영속 표시
7. transcript polling: session_dir의 `<id>.jsonl` + `__advisor.jsonl`을 250ms mtime 폴링

### 2.2 out-of-scope (이번 PR)

- collab guest/remote hub (omp `AgentHubRemote`) — 네트워크 계층 필요
- Agent Dashboard (omp 1,206줄) — 별도 PR
- advisor `concern`/`blocker` 시 primary 자동 인터럽트 (이미 `AdvisorDeliveryState`에 구현됨, 변경 없음)
- subagent의 out-of-process → in-process 전환 (Oxios 임베더 작업, 별도)
- TTSR/SoftReq/Approval 같은 다른 advisor 기능

---

## 3. 아키텍처

### 3.1 데이터 모델

```rust
// oxicode-sdk/src/lifecycle/supervisor.rs (확장)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    Main,        // 이 세션의 메인 에이전트
    Subagent,    // subagent 도구로 spawn된 자식
    Advisor,     // read-only reviewer (advisor)
}

pub struct AgentHandle {
    // 기존 필드 (그대로)
    pub agent_id: String,
    pub status: Arc<AtomicU8>,
    pub agent: Arc<oxicode_agent::Agent>,
    pub config: Arc<RwLock<AgentConfig>>,
    pub metrics: Arc<AgentMetrics>,
    pub lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    pub created_at_ms: u64,
    pub parent_id: Option<String>,
    pub routing: RoutingControl,

    // ── Hub 표시용 (신규) ──
    pub display_name: Arc<RwLock<String>>,         // 기본: agent_id
    pub kind: AgentKind,                            // 결정 시점 고정
    pub last_activity_ms: Arc<AtomicU64>,
    pub current_task: Arc<RwLock<Option<String>>>,
    pub session_file: Arc<RwLock<Option<PathBuf>>>, // transcript 읽기 경로
}

impl AgentHandle {
    pub fn touch_activity(&self) {
        self.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    }
    pub fn set_current_task(&self, task: Option<String>) {
        *self.current_task.write() = task;
        self.touch_activity();
    }
    pub fn hub_status(&self) -> HubStatus {
        match self.status.load(Ordering::Relaxed) {
            STATUS_RUNNING => HubStatus::Running,
            STATUS_IDLE => HubStatus::Idle,
            STATUS_STOPPED => HubStatus::Parked,
            STATUS_FAILED => HubStatus::Aborted,
            _ => HubStatus::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubStatus { Running, Idle, Parked, Aborted }
```

### 3.2 AgentPool 연결

```rust
pub struct AgentSession {

    // ... 기존 필드 ...
    pool: Arc<AgentPool>,  // session 생성 시 빌드, self 등록
}

impl AgentSession {
    pub fn new(...) -> Result<Arc<Self>> {
        let pool = Arc::new(AgentPool::new());
        let main_handle = AgentHandle::for_main(self.agent.clone(), self.session_id());
        pool.insert("main".into(), main_handle);

        let session = Arc::new(Self { pool, ... });

        // Advisor 등록 (build_advisor 성공 시)
        if let Some(advisor_rt) = session.build_advisor() {
            // AdvisorRuntime은 자체 Agent를 가짐 — 그걸로 handle 만들어 등록
            let advisor_handle = AgentHandle::for_advisor(
                advisor_rt.agent(),
                session.advisor_transcript_path(),
            );
            session.pool.insert("advisor".into(), advisor_handle);
        }
        Ok(session)
    }

    /// Subagent spawn 후 호출 — oxicode-sdk의 subagent 도구가 ToolContext에서
    /// session.pool을 얻어 호출. out-of-process 경로는 디렉토리 스캔으로
    /// 복원 (3.4).
    pub fn register_subagent(&self, name: String, session_file: PathBuf) {
        let handle = AgentHandle::for_subagent(name.clone(), session_file);
        self.pool.insert(name, handle);
    }
}
```

### 3.3 AdvisorRuntime 변경

`AdvisorRuntime`은 자체 `Agent`를 보유하지만, transcript를 `AdvisorTranscriptRecorder`로 파일에 쓴다. 변경 최소화:

```rust
// oxicode-agent/src/advisor/runtime.rs (변경 없음, hook만 추가)

impl AdvisorRuntime {
    /// Returns the transcript file path (or None if no session file).
    /// Used by AgentSession to register advisor in AgentPool.
    pub fn transcript_path(&self) -> Option<PathBuf> { ... }
}
```

advisor transcript는 이미 `<session_dir>/__advisor.jsonl`로 기록되므로 (advisor_context.rs:130) — **Hub는 그 파일을 그대로 읽으면 됨.** 추가 코드 없음.

### 3.4 Transcript 풀링 (pull-based)

**핵심 결정: push가 아니라 mtime 폴링.** omp도 동일 (`DATA_CHANGE_RENDER_COALESCE_MS = 100ms`).

```rust
// oxicode-cli/src/tui/overlay/agent_hub/transcript_reader.rs (신규)

pub struct TranscriptReader {
    /// Path to the .jsonl file (or __advisor.jsonl).
    path: PathBuf,
    /// Last seen mtime + size. If unchanged since last read, returns cached.
    last_mtime: Option<SystemTime>,
    last_size: u64,
    /// Parsed lines, cached.
    lines: Vec<TranscriptLine>,
}

impl TranscriptReader {
    pub fn new(path: PathBuf) -> Self { ... }

    /// Re-read only if mtime/size changed. Cheap when idle.
    pub fn refresh(&mut self) -> bool {
        let meta = match std::fs::metadata(&self.path) { Ok(m) => m, Err(_) => return false };
        let mtime = meta.modified().ok();
        let size = meta.len();
        if Some((mtime, size)) == Some((self.last_mtime, self.last_size)) {
            return false;
        }
        self.last_mtime = mtime;
        self.last_size = size;
        // Read from the last known byte offset (incremental) or full re-parse.
        // omp does full re-parse: "files may be in-place rewritten (SessionManager)".
        // We mirror that for v1.
        self.lines = parse_jsonl(&self.path);
        true
    }

    pub fn lines(&self) -> &[TranscriptLine] { &self.lines }
}
```

```rust
pub struct TranscriptLine {
    pub timestamp_ms: u64,
    pub role: String,        // "user" | "assistant" | "tool"
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_status: Option<String>, // "running" | "completed" | "error"
}
```

**파일 형식**: oxicode의 session JSONL은 각 entry가 `SessionEntry` (timestamp + role + content). advisor transcript는 `AdvisorTranscriptRecorder`가 자체 JSONL을 쓰지만 형식이 다르다 (line 184-188):
```json
{"ts": 1234, "messages": ["...", "..."]}
```

→ 두 형식을 모두 처리하는 `parse_jsonl` 필요. 또는 transcript reader를 `SessionEntry` 형식과 `advisor` 형식 둘 다 파싱.

### 3.5 AgentHubOverlay 구조

```rust
// oxicode-cli/src/tui/overlay/agent_hub/ (신규 디렉토리, ~500 LOC)

pub enum HubView { Table, Transcript { agent_id: String } }

pub struct AgentHubOverlay {
    pool: Arc<AgentPool>,
    /// Snapshot of pool rows (refreshed on poll). Stable order while open.
    rows: Vec<HubRow>,
    row_order: HashMap<String, usize>,
    selected: usize,
    view: HubView,
    /// Active transcript readers, keyed by agent_id.
    readers: HashMap<String, TranscriptReader>,
    /// Scroll offset for transcript view.
    transcript_scroll: usize,
    transcript_follow: bool,    // default true — tail following
}

pub struct HubRow {
    pub id: String,
    pub display_name: String,
    pub kind: AgentKind,
    pub status: HubStatus,
    pub current_task: Option<String>,
    pub last_activity_ms: u64,
    pub age_text: String,         // "3s ago" — precomputed for render
}

impl AgentHubOverlay {
    pub fn new(pool: Arc<AgentPool>) -> Self { ... }
}
```

**Table view (default)**: 
- 렌더: status 배지 + display_name + kind + current_task + age
- 정렬: running > idle > parked > aborted, then last_activity desc
- 키: j/k nav, Enter → transcript, Esc/Q → close
- 하단 힌트: `j/k: nav  Enter: transcript  Esc: close`

**Transcript view (Enter)**:
- 풀스크린 transcript 표시 (table 위에 overlay, omp `openChat` 패턴)
- 활성 reader의 마지막 N줄 표시
- tail following (기본 on, 스크롤 업 시 off)
- 키: j/k/G/g/PgUp/PgDn + Esc → table로 복귀

### 3.6 풀스크린 vs popup

**결정: 풀스크린 alt-screen** (omp 검증, transcript 보기엔 좁은 popup 부적합).

oxicode의 tape 모델과 양립:
- 일반 채팅 → main screen (tape)
- Hub 열기 → `EnterAlternateScreen` → Hub가 alt-screen 차지
- Hub 닫기 → `LeaveAlternateScreen` → main screen 복귀
- 이미 `oxicode-cli/src/tui/terminal_host.rs:94`가 alt-screen enter/leave를 처리 중

**Transcript viewer는 nested 풀스크린이 아니라 table view의 풀스크린을 그대로 사용** (omp `openChat`은 nested overlay지만, oxicode에서는 별도 overlay로 분리해 단순화). Hub의 풀스크린 안에서 table ↔ transcript를 토글.

### 3.7 Advisor 메시지 영속화 (transcript 카드)

현재 aside channel advise는 `SessionEvent::Advisor` → `UiEvent::SystemMessage` → 토스트. **추가**: 동일 이벤트로 transcript 카드(스크롤백에 영속)를 emit.

```rust
// oxicode-cli/src/tui/handlers.rs (확장)

SessionEvent::Advisor { channel, body, severity } => {
    // 기존 토스트
    let _ = ui_tx.send(UiEvent::SystemMessage(format!("Advisor ({:?}): {body}", channel)));

    // 신규: transcript 카드 (스크롤백에 남음)
    if matches!(channel, AdvisorDeliveryChannel::Aside | AdvisorDeliveryChannel::Preserve) {
        let _ = ui_tx.send(UiEvent::AdvisorCard {
            body: body.clone(),
            severity: severity.unwrap_or(AdvisorSeverity::Nit),
            timestamp_ms: now_ms(),
        });
    }
}
```

advisor 카드는 기존 chat transcript의 `ContentBlock::Advisory`로 추가 — primary agent의 tool 결과처럼 자연스럽게 표시.

---

## 4. 통합 포인트

### 4.1 키 바인딩 (oxicode-tui)

```rust
// oxicode-tui/src/keybindings/registry.rs

pub enum Action {
    // ... 기존 ...
    ToggleAgentHub,  // 신규
}

// init_defaults:
(ToggleAgentHub, vec!["Ctrl+h"]),

// parse_action:
"toggleagenthub" => Some(ToggleAgentHub),
```

`Ctrl+h`는 omp의 `app.agents.hub` 기본값과 동일.

### 4.2 슬래시 명령 (oxicode-cli)

```rust
// oxicode-cli/src/tui/slash/builtin/agents.rs (신규)

pub(crate) struct AgentsCommand;
impl SlashCommand for AgentsCommand {
    fn name(&self) -> &str { "agents" }
    fn aliases(&self) -> &[&str] { &["hub"] }
    fn description(&self) -> &str { "Open the agent hub overlay (advisor + subagents)" }
    fn execute(&self, _args, ctx) -> SlashOutcome {
        let pool = ctx.session.pool().clone();  // AgentSession에 getter 추가
        ctx.state.overlay_state = Some(Box::new(AgentHubOverlay::new(pool)));
        SlashOutcome::Handled
    }
}
```

`mod.rs`의 `register_builtin_slash_commands`에 등록.

### 4.3 Handler dispatch (oxicode-cli)

```rust
// oxicode-cli/src/tui/handlers.rs

KAction::ToggleAgentHub => {
    let pool = session.pool().clone();
    state.overlay_state = Some(Box::new(AgentHubOverlay::new(pool)));
    None
}
```

`dispatch_action` match arm에 추가.

### 4.4 Subagent 등록 (oxicode-agent)

subagent 도구가 spawn 후 `ToolContext`의 `agent_pool`을 얻어 등록. oxicode-cli의 out-of-process 경로는 디렉토리 스캔으로 복원 (3.5).

```rust
// oxicode-agent/src/tools/subagent.rs (변경)

async fn execute(...) {
    // ... spawn logic ...
    if let Some(pool) = ctx.agent_pool {
        pool.register_subagent(name, session_file);  // AgentPool에 메서드 추가
    }
}
```

`ToolContext::agent_pool`는 이미 SDK에 존재 (`oxicode-sdk/src/agent_loop/...`) — oxicode-cli의 `App::from_oxicode`에서 `agent_pool: Some(session.pool().clone())`로 전달.

**v1 한정**: out-of-process subagent는 `__advisor.jsonl`과 동일하게 **세션 디렉토리 스캔**으로 복원. 스캔 규칙:
- `<session_dir>/*.jsonl` (subagent transcript)
- `<session_dir>/__advisor.jsonl` (advisor transcript)
- 메인 session.jsonl 자체는 제외

세션 시작 시 한 번 스캔 → `register_persisted_subagents(pool, session_dir)`.

---

## 5. 디렉토리/파일 변경

### 신규

```
oxicode-cli/src/tui/overlay/agent_hub/
├── mod.rs              # AgentHubOverlay struct + OverlayComponent impl
├── state.rs            # HubRow, HubView, sort logic
├── table.rs            # render_table, status_badge, format_age
├── transcript.rs       # TranscriptReader + TranscriptLine + parse_jsonl
└── keys.rs             # handle_key, key hints

oxicode-cli/src/tui/slash/builtin/agents.rs   # /agents slash command
oxicode-cli/src/app/agent_hub_bridge.rs       # AgentPool wiring in AgentSession
```

### 변경

```
oxicode-sdk/src/lifecycle/supervisor.rs        # AgentKind, HubStatus, AgentHandle 필드 5개 + impl
oxicode-sdk/src/lifecycle/agent_pool.rs       # register_subagent, for_each_row
oxicode-tui/src/keybindings/registry.rs        # ToggleAgentHub action
oxicode-tui/src/widgets/chat/state.rs          # ContentBlock::Advisory variant (card)
oxicode-tui/src/widgets/chat/markdown.rs        # Advisory variant render (severity-colored card)
oxicode-tui/src/widgets/chat/render.rs          # route ContentBlock::Advisory through transcript
oxicode-agent/src/advisor/runtime.rs           # transcript_path() getter
oxicode-agent/src/advisor/agent_advisor.rs     # (변경 없음, hook만 노출)
oxicode-cli/src/app/agent_session.rs           # pool field, register_advisor, register_subagent
oxicode-cli/src/tui/handlers.rs                # ToggleAgentHub dispatch + SessionEvent::Advisor → UiEvent::AdvisorCard
oxicode-cli/src/tui/app.rs                     # UiEvent::AdvisorCard → ContentBlock::Advisory emit
oxicode-cli/src/tui/slash/builtin/mod.rs       # AgentsCommand 등록
oxicode-cli/src/tui/overlay/mod.rs             # agent_hub module 등록
```

---

## 6. 테스트 계획

```rust
// oxicode-cli/src/tui/overlay/agent_hub/transcript.rs
#[cfg(test)]
mod tests {
    #[test]
    fn parses_session_jsonl() { ... }
    #[test]
    fn parses_advisor_jsonl() { ... }
    #[test]
    fn refresh_skips_when_unchanged() { ... }
    #[test]
    fn refresh_reruns_on_mtime_change() { ... }
    #[test]
    fn incremental_growth_does_not_reparse() { ... } // append-only
}

// oxicode-cli/src/tui/overlay/agent_hub/state.rs
#[cfg(test)]
mod tests {
    #[test]
    fn rows_sort_running_first() { ... }
    #[test]
    fn frozen_order_on_subsequent_refresh() { ... }
    #[test]
    fn age_formats_correctly() { ... }
}

// oxicode-sdk/src/lifecycle/supervisor.rs
#[cfg(test)]
mod tests {
    #[test]
    fn handle_touch_updates_activity() { ... }
    #[test]
    fn hub_status_maps_correctly() { ... }
    #[test]
    fn pool_register_and_list() { ... }
}
```

**PTY 테스트 추가**: `cargo nextest run -p oxicode-cli --test pty_e2e test_pty_hub_opens_and_lists_advisor` — PTY에서 `/agents` 입력 → overlay 표시 → advisor 행 1개 확인 → Esc로 닫기.

---

## 7. 성공 기준

- [ ] `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` + `cargo fmt --all -- --check` + `cargo nextest run --workspace` 모두 green
- [ ] `Ctrl+h` 또는 `/agents` 입력 시 AgentHubOverlay 열림
- [ ] Advisor가 활성화된 세션에서 `__advisor.jsonl`이 Hub table에 "advisor" 행으로 표시
- [ ] 그 행에서 Enter → advisor의 live transcript 표시 (tail following)
- [ ] subagent (out-of-process) spawn 후 Hub에 해당 transcript 파일이 행으로 추가
- [ ] advisor aside advise가 토스트 + transcript 카드 양쪽으로 표시
- [ ] PTY 인수 테스트 통과
- [ ] `oxicode-sdk/src/lifecycle/AgentPool` 이 oxicode-cli에서 한 번 이상 사용됨 (현재 0)

---

## 8. 위험 & 완화

| 위험 | 완화 |
|---|---|
| 풀스크린 alt-screen이 tape 모델과 충돌 | `EnterAlternateScreen`/`LeaveAlternateScreen`이 한 쌍 (open/close) — 비용 동일 |
| mtime 폴링 비용 (250ms) | `refresh_skips_when_unchanged` — 변경 없으면 syscall 0회 |
| `__advisor.jsonl` 형식이 SessionEntry와 다름 | 별도 `parse_jsonl` 분기 (format discriminator) |
| Transcript viewer가 너무 좁으면 table이 안 보임 | omp 패턴: fullscreen toggle, table ↔ transcript 라우팅 |
| Subagent out-of-process IPC 부재 | 세션 디렉토리 스캔으로 동일 효과 (omp 검증) |
| AgentPool 사이클 (subagent가 session을 참조) | 약참조 (`Weak<AgentSession>`) + `Arc<AgentPool>` 분리 |

---

## 9. 마일스톤

| 순서 | 산출물 | 의존 |
|:-:|---|---|
| M1 | `AgentHandle` 필드 + `HubStatus` + 테스트 | — |
| M2 | `AgentPool` 메서드 (`for_each_row`, `register_subagent`) + 테스트 | M1 |
| M3 | `AgentSession` pool 필드 + advisor 등록 + 세션 디렉토리 스캔 | M1, M2 |
| M4 | `TranscriptReader` + `parse_jsonl` + 테스트 | — |
| M5 | `AgentHubOverlay` table view + `ToggleAgentHub` 키 + `/agents` 슬래시 | M1, M2, M3, M4 |
| M6 | Transcript view + tail following | M5 |
| M7 | Advisor card (transcript 영속) + PTY 테스트 | M5 |

M1–M4가 인프라, M5–M7이 사용자 가시 부분. M5에서 이미 hub 테이블은 작동함 (transcript는 M6에서 추가).
