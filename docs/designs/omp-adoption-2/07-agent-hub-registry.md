# 세부 설계 ⑥ — Agent Hub / Registry (라이브 서브에이전트 모니터링)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §1·§11 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `modes/components/agent-hub.ts` (566줄), `agent-transcript-viewer.ts` (461줄), `agent-dashboard.ts` (1,206줄)
> oxi 기반: `oxi-sdk/src/lifecycle/` (Supervisor 1,068줄, AgentPool, AgentHandle)
> 후속: N2 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

omp의 **Agent Hub**는 등록된 모든 에이전트(메인 + 서브)를 한 화면에서 모니터링하는 풀스크린 오버레이다. 상태·비읽은 IRC 수·현재 작업·마지막 활동을 테이블로 보여주고, 각 에이전트의 트랜스크립트를 라이브로 열람할 수 있다.

**oxi의 핵심 자산**: `oxi-sdk/src/lifecycle/supervisor.rs`가 이미 `AgentHandle`(status, metrics, lifecycle events, parent_id)과 `AgentPool`을 제공한다. omp는 이것을 처음부터 만들었지만, oxi는 **TUI 레이어만 추가하면 된다**.

### omp가 검증한 가치
- **서브에이전트 가시성** — `task`로 띄운 서브에이전트의 진행·출력·비용을 실시간으로 확인.
- **주차(parked) 에이전트 부활** — 완료된 에이전트를 메모리에 유지, IRC로 다시 깨움.
- **트랜스크립트 테일** — 파일 기반 세션을 주기적으로 재읽어 라이브 업데이트.
- **advisor 가시화** — 관측 전용 advisor 에이전트의 트랜스크립트를 읽기 전용으로 표시.

---

## 1. omp 메커니즘

### 1.1 AgentHub 두 뷰 (`modes/components/agent-hub.ts`)

```
AgentHubOverlay
├── 테이블 뷰 (기본)
│   ┌──────────────────────────────────────────────────────┐
│   │ Agent         Status      Task              Activity  │
│   │ ● BuildLoader  running     Loading builds...  3s ago  │
│   │ ● TestRunner   running     Running cargo test 5s ago  │
│   │ ✓ AuthLoader   idle        Loaded auth module 1m ago  │
│   │ ○ DocScout     parked      Found 3 docs      5m ago  │
│   └──────────────────────────────────────────────────────┘
│   j/k: navigate  Enter: open chat  r: revive  x: abort
│
└── 채팅 뷰 (Enter 진입)
    ┌──────────────────────────────────────────────────────┐
    │ ● BuildLoader — running                               │
    │ ──────────────────────────────────────────────────    │
    │ [agent transcript — 실시간 tail]                       │
    │                                                        │
    │ > _                                                    │
    └──────────────────────────────────────────────────────┘
```

### 1.2 핵심 타입 (`registry/agent-registry.ts`)

```typescript
type AgentStatus = "running" | "idle" | "parked" | "aborted";
type AgentKind = "main" | "task" | "advisor" | "collab";

interface AgentRef {
    id: string;              // 고유 식별자
    displayName: string;
    kind: AgentKind;
    status: AgentStatus;
    sessionFile?: string;    // .jsonl 트랜스크립트
    parent?: string;
    lastActivity?: number;
    unreadIrcCount?: number;
    currentTask?: string;
}
```

### 1.3 상태 배지 (`agent-hub.ts:58`)

```typescript
function statusBadge(status: AgentStatus): string {
    switch (status) {
        case "running": return theme.fg("accent", `${theme.status.running} running`);
        case "idle":    return theme.fg("success", `${theme.status.enabled} idle`);
        case "parked":  return theme.fg("muted", `${theme.status.shadowed} parked`);
        case "aborted": return theme.fg("error", `${theme.status.aborted} aborted`);
    }
}
```

### 1.4 영속 서브에이전트 등록 (`agent-hub.ts:71`)

```typescript
function registerPersistedSubagents(registry: AgentRegistry, sessionFile: string | null) {
    if (!sessionFile?.endsWith(".jsonl")) return;
    const root = sessionFile.slice(0, -6);  // .jsonl 제거
    registerPersistedSubagentsFromDir(registry, root, undefined);
}

function registerPersistedSubagentsFromDir(registry, dir, parentId) {
    const entries = fs.readdirSync(dir, { withFileTypes: true });
    for (const entry of entries) {
        if (!entry.isFile() || !entry.name.endsWith(".jsonl")) continue;
        const sessionFile = path.join(dir, entry.name);
        // advisor 트랜스크립트는 관측 전용
        if (entry.name === ADVISOR_TRANSCRIPT_FILENAME) {
            registry.register({ id: `${owner}/advisor`, kind: "advisor", ... });
        } else {
            // 세션 파일에서 에이전트 메타데이터 추출
            registry.register({ id: entry.name, kind: "task", sessionFile, parent: parentId, ... });
        }
    }
}
```

### 1.5 트랜스크립트 뷰어 (`agent-transcript-viewer.ts`)

```typescript
// 파일 기반 트랜스크립트를 250ms마다 재통계 (mtime/size 변경 감지)
const POLL_MS = 250;

// ScrollView가 tail을 따라가다가 사용자가 스크롤 올리면 고정
#followBottom = true;

// 매 폴링마다 전체 세션 재구축 (증분 동기화 대신)
// — 파일이 in-place 재작성될 수 있으므로 (SessionManager)
#builder.rebuild();
```

---

## 2. oxi 기존 자산 분석

### 2.1 `oxi-sdk/src/lifecycle/supervisor.rs`

oxi는 이미 강력한 에이전트 라이프사이클 인프라를 갖추고 있다:

```rust
pub struct AgentHandle {
    agent_id: String,
    status: Arc<AtomicU8>,           // STATUS_CREATED/RUNNING/IDLE/STOPPED/FAILED
    agent: Arc<oxi_agent::Agent>,
    config: Arc<RwLock<AgentConfig>>,
    metrics: Arc<AgentMetrics>,       // 토큰/비용/지속시간
    lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    created_at_ms: u64,
    parent_id: Option<String>,
    routing: RoutingControl,
}
```

```rust
pub struct AgentPool {
    agents: parking_lot::RwLock<HashMap<String, Arc<AgentHandle>>>,
}

impl AgentPool {
    pub fn insert(&self, id: String, agent: Arc<AgentHandle>);
    pub fn get(&self, id: &str) -> Option<Arc<AgentHandle>>;
    pub fn list(&self) -> Vec<Arc<AgentHandle>>;
}
```

```rust
pub enum AgentLifecycleEvent {
    Started { agent_id: String, parent_id: Option<String> },
    Stopped { agent_id: String, reason: StopReason },
    // ...
}
```

### 2.2 oxi에 부족한 것

| omp 기능 | oxi 상태 | 갭 |
|---|---|---|
| `displayName` / `currentTask` | `config.name`만 있음 | 표시용 메타데이터 부재 |
| `unreadIrcCount` | IRC 미구현 (1차 계획에 없음) | IRC 카운트 불가 |
| `lastActivity` | `created_at_ms`만 있음 | 마지막 활동 타임스탬프 |
| `kind` (main/task/advisor) | parent_id만 있음 | 에이전트 유형 분류 |
| `sessionFile` (.jsonl tail) | 세션 영속은 있으나 Hub 연결 없음 | 파일 경로 연결 |
| `parked` 상태 | STOPPED만 있음 | 주차(메모리 유지) 상태 |
| 트랜스크립트 테일 | 세션 재로드는 있으나 라이브 폴링 없음 | mtime 폴링 + 재구축 |

---

## 3. oxi화 설계

### 3.1 AgentHandle 확장

`oxi-sdk/src/lifecycle/supervisor.rs`의 `AgentHandle`에 표시용 필드 추가:

```rust
#[derive(Clone)]
pub struct AgentHandle {
    // 기존 필드...
    
    // ── Agent Hub 표시용 (⑥ 추가) ──
    display_name: Arc<RwLock<String>>,
    kind: AgentKind,
    last_activity_ms: Arc<AtomicU64>,
    current_task: Arc<RwLock<Option<String>>>,
    session_file: Arc<RwLock<Option<PathBuf>>>,
    unread_irc: Arc<AtomicU32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Main,
    Task,       // task 도구로 생성된 서브에이전트
    Advisor,    // 관측 전용 (omp advisor 대응)
    Extension,  // 확장에서 생성
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHubStatus {
    Running,
    Idle,
    Parked,     // 완료되었으나 메모리에 유지 (IRC 대기)
    Aborted,    // 비정상 종료
}
```

```rust
impl AgentHandle {
    // Hub 표시용 접근자
    pub fn display_name(&self) -> String {
        self.display_name.read().clone()
    }
    
    pub fn kind(&self) -> AgentKind {
        self.kind
    }
    
    pub fn last_activity(&self) -> u64 {
        self.last_activity_ms.load(Ordering::Relaxed)
    }
    
    pub fn touch_activity(&self) {
        self.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    }
    
    pub fn current_task(&self) -> Option<String> {
        self.current_task.read().clone()
    }
    
    pub fn set_current_task(&self, task: Option<String>) {
        *self.current_task.write() = task;
        self.touch_activity();
    }
    
    pub fn hub_status(&self) -> AgentHubStatus {
        match self.status.load(Ordering::Relaxed) {
            STATUS_RUNNING => AgentHubStatus::Running,
            STATUS_IDLE => AgentHubStatus::Idle,
            STATUS_STOPPED => AgentHubStatus::Parked,  // 완료 = 주차
            STATUS_FAILED => AgentHubStatus::Aborted,
            _ => AgentHubStatus::Idle,
        }
    }
}
```

### 3.2 TUI 오버레이: `AgentHubOverlay`

`oxi-cli/src/tui/overlay/agent_hub.rs` (신규):

```rust
pub struct AgentHubOverlay {
    pool: Arc<AgentPool>,
    selected: usize,
    view: HubView,
    transcript_scroll: usize,
    transcript_follow: bool,
}

enum HubView {
    Table,      // 에이전트 목록
    Transcript { agent_id: String },  // 개별 트랜스크립트
}

impl AgentHubOverlay {
    pub fn new(pool: Arc<AgentPool>) -> Self {
        Self {
            pool,
            selected: 0,
            view: HubView::Table,
            transcript_scroll: 0,
            transcript_follow: true,
        }
    }
}
```

### 3.3 테이블 렌더

```rust
impl Overlay for AgentHubOverlay {
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match &self.view {
            HubView::Table => self.render_table(frame, area, theme),
            HubView::Transcript { agent_id } => {
                self.render_transcript(frame, area, theme, agent_id);
            }
        }
    }
    
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match &self.view {
            HubView::Table => self.handle_table_key(key),
            HubView::Transcript { .. } => self.handle_transcript_key(key),
        }
    }
}

fn render_table(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
    let agents = self.pool.list();
    let sorted = sort_agents(&agents);  // running > idle > parked > aborted
    
    // 헤더
    let header = Row::new(vec![
        Cell::from("Agent"),
        Cell::from("Status"),
        Cell::from("Task"),
        Cell::from("Activity"),
    ]).style(theme.bold());
    
    // 행
    let rows: Vec<Row> = sorted.iter().enumerate().map(|(i, h)| {
        let status = status_badge(h.hub_status(), theme);
        let task = h.current_task().unwrap_or_else(|| "—".into());
        let activity = format_age(h.last_activity());
        
        let mut row = Row::new(vec![
            Cell::from(h.display_name()),
            Cell::from(status),
            Cell::from(task),
            Cell::from(activity),
        ]);
        
        // 선택 행 하이라이트
        if i == self.selected {
            row = row.style(theme.selection());
        }
        row
    }).collect();
    
    let table = Table::new(rows, [Constraint::Length(20), Constraint::Length(12), Constraint::Min(1), Constraint::Length(12)])
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Agent Hub "));
    
    frame.render_widget(table, area);
    
    // 푸터 키 힌트
    let footer = " j/k: navigate  Enter: transcript  r: revive  x: abort  Esc: close";
    // ...
}

fn status_badge(status: AgentHubStatus, theme: &Theme) -> String {
    match status {
        AgentHubStatus::Running => format!("{} running", theme.icon("running")),
        AgentHubStatus::Idle => format!("{} idle", theme.icon("success")),
        AgentHubStatus::Parked => format!("{} parked", theme.icon("muted")),
        AgentHubStatus::Aborted => format!("{} aborted", theme.icon("error")),
    }.into()
}
```

### 3.4 트랜스크립트 뷰어

```rust
fn render_transcript(&self, frame: &mut Frame, area: Rect, theme: &Theme, agent_id: &str) {
    let agent = match self.pool.get(agent_id) {
        Some(a) => a,
        None => { /* 에이전트 사라짐 */ return; }
    };
    
    // 헤더: 에이전트 정보
    let header = format!(" {} — {} ", agent.display_name(), status_badge(agent.hub_status(), theme));
    
    // 트랜스크립트 로드
    let transcript = self.load_transcript(agent_id);
    
    // ScrollView 렌더
    let visible = self.visible_transcript_lines(&transcript, area.height);
    for (i, line) in visible.iter().enumerate() {
        // ... 렌더
    }
    
    // tail 따라가기
    if self.transcript_follow {
        self.transcript_scroll = transcript.len().saturating_sub(visible_count);
    }
}

/// 세션 파일에서 트랜스크립트 로드. mtime 변경 시에만 재로드.
fn load_transcript(&self, agent_id: &str) -> Vec<TranscriptLine> {
    let agent = self.pool.get(agent_id)?;
    let session_file = agent.session_file.read();
    let path = session_file.as_ref()?;
    
    // 캐시 확인 (mtime)
    if let Some(cached) = self.transcript_cache.get(agent_id) {
        if cached.mtime == file_mtime(path) {
            return cached.lines.clone();
        }
    }
    
    // 재로드
    let lines = parse_session_transcript(path);
    self.transcript_cache.insert(agent_id.into(), CachedTranscript {
        mtime: file_mtime(path),
        lines: lines.clone(),
    });
    lines
}
```

### 3.5 영속 에이전트 등록

세션 시작 시 디스크의 서브에이전트 세션 파일을 스캔하여 registry에 등록:

```rust
/// 세션 디렉토리에서 서브에이전트 .jsonl 파일을 스캔하여 pool에 등록.
pub fn register_persisted_subagents(pool: &Arc<AgentPool>, session_dir: &Path) {
    let entries = match std::fs::read_dir(session_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy();
        
        if !name.ends_with(".jsonl") { continue; }
        
        // 세션 파일에서 에이전트 메타데이터 추출
        let meta = extract_session_meta(&path);
        
        let handle = AgentHandle::for_persisted(
            id: name.trim_end_matches(".jsonl"),
            display_name: meta.display_name,
            kind: meta.kind,
            session_file: path,
        );
        
        pool.insert(handle.agent_id().to_string(), Arc::new(handle));
    }
}
```

### 3.6 IRC 통합 (선택, 후순위)

omp의 IRC 카운트는 `IrcBus`에서 가져온다. oxi에 IRC가 없으므로 (1차 계획에도 없음):

```rust
impl AgentHandle {
    /// IRC 미구현 시 항상 0. IRC 도입 시 IrcBus에서 카운트.
    pub fn unread_irc(&self) -> u32 {
        self.unread_irc.load(Ordering::Relaxed)
    }
}
```

> IRC는 1차 계획의 영구 제외 항목. Hub의 IRC 카운트는 0으로 고정, 향후 IRC 도입 시 연결.

---

## 4. subagent 도구와의 통합

### 4.1 서브에이전트 생성 시 registry 등록

`oxi-agent/src/tools/subagent.rs`에서 서브에이전트 spawn 시 pool에 등록:

```rust
async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
    // 서브에이전트 생성
    let sub_agent = Agent::new(provider, config.clone());
    
    // Agent Hub 등록 (⑥ 추가)
    if let Some(pool) = ctx.agent_pool.as_ref() {
        let handle = AgentHandle::new(
            sub_agent.clone(),
            config.clone(),
            Some(ctx.session_id.clone()),
            ctx.lifecycle_tx.clone(),
        ).with_display_name(config.name.clone())
         .with_kind(AgentKind::Task)
         .with_session_file(ctx.session_path.clone());
        
        pool.insert(handle.agent_id().to_string(), Arc::new(handle));
    }
    
    // 에이전트 실행
    let result = sub_agent.run(...).await;
    
    // 완료 시 상태 갱신 (parked로)
    if let Some(pool) = ctx.agent_pool.as_ref() {
        if let Some(handle) = pool.get(&agent_id) {
            handle.set_status(STATUS_STOPPED);
            handle.set_current_task(None);
        }
    }
    
    result
}
```

### 4.2 ToolContext 확장

```rust
pub struct ToolContext<'a> {
    // 기존 필드...
    pub agent_pool: Option<&'a Arc<AgentPool>>,      // ⑥ Agent Hub
    pub lifecycle_tx: Option<&'a broadcast::Sender<AgentLifecycleEvent>>,
}
```

### 4.3 트랜스크립트 실시간 갱신

서브에이전트가 세션에 엔트리를 append할 때마다 `last_activity` 갱신:

```rust
// agent_loop/mod.rs — 각 턴 종료 시
if let Some(pool) = &ctx.agent_pool {
    if let Some(handle) = pool.get(&self.agent_id) {
        handle.touch_activity();
        handle.set_current_task(Some(self.current_task_description()));
    }
}
```

---

## 5. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N2.1 | `AgentHandle` 확장 (display_name, kind, last_activity, current_task) | — |
| N2.2 | `AgentHubStatus` + `hub_status()` | N2.1 |
| N2.3 | `AgentPool::list_sorted()` (running > idle > parked > aborted) | N2.2 |
| N2.4 | `ToolContext.agent_pool` + `lifecycle_tx` 주입 | N2.1 |
| N2.5 | `subagent.rs` 등록 훅 (spawn → pool.insert, complete → status) | N2.4 |
| N2.6 | `AgentHubOverlay` 테이블 뷰 | N2.3 |
| N2.7 | `status_badge` + `format_age` 헬퍼 | N2.6 |
| N2.8 | `AgentHubOverlay` 트랜스크립트 뷰 | N2.6 |
| N2.9 | 세션 파일 트랜스크립트 로드 (mtime 캐시) | N2.8 |
| N2.10 | 키 바인딩 (j/k/Enter/r/x/Esc) | N2.8 |
| N2.11 | 영속 에이전트 등록 (세션 디렉토리 스캔) | N2.5 |
| N2.12 | 오버레이 팩토리 등록 (`/agents` 슬래시) | N2.10 |
| N2.13 | ⑤ sticky panel과 연동 (서브에이전트 설명 → todo 매칭) | N2.5, ⑤ |

> **독립성**: ⑥은 ⑤ todo와 병렬 가능. N2.13만 양쪽 완료 후.
> **oxi-sdk 기반**: AgentPool/AgentHandle이 이미 있으므로 TUI 레이어가 주 작업.

---

## 6. Agent Dashboard (후순위)

omp의 `agent-dashboard.ts` (1,206줄)는 에이전트 정의(definition)를 관리하는 별도 오버레이다:
- 등록된 에이전트 목록 (2-컬럼: 리스트 + 인스펙터)
- 모델 오버라이드
- 새 에이전트 생성
- 소스 탭 (project / user)

이는 oxi의 `agent_definition.rs` (567줄)과 겹친다. **후순위**: N2 완료 후, `/agents` 명령을 dashboard로 확장.

---

## 7. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| `parked` 에이전트 메모리 유지 비용 | 🟡 모니터 | 완료된 에이전트를 메모리에 유지. TTL 도입 (omp는 parked → 일정 시간 후 제거) |
| 트랜스크립트 폴링 주기 (250ms) | 🟢 결정됨 | omp와 동일. mtime 변경 시만 재로드 |
| IRC 카운트 (IRC 미구현) | 🟢 고정 | 항상 0. IRC 도입 시 연결 |
| 대량 서브에이전트 (32개 동시) | 🟡 최적화 | 테이블 스크롤 + 가상화 검토 |
| 에이전트 정의 관리 (dashboard) | 🔴 후순위 | N2 완료 후 별도 설계 |
| advisor 에이전트 (omp 전용) | 🔴 후순위 | oxi에 advisor 개념 없음. 향후 별도 검토 |
| collab 게스트 (원격 에이전트) | 🔴 영구 제외 | 네트워킹 계층 필요. oxios 제품 |

---

## 8. 테스트 계획

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_list_sorted_by_status() {
        let pool = AgentPool::new();
        // parked, running, idle, aborted 순으로 삽입
        pool.insert("parked".into(), parked_handle());
        pool.insert("running".into(), running_handle());
        pool.insert("idle".into(), idle_handle());
        pool.insert("aborted".into(), aborted_handle());
        
        let sorted = pool.list_sorted();
        // running이 먼저, aborted가 마지막
        assert_eq!(sorted[0].hub_status(), AgentHubStatus::Running);
        assert_eq!(sorted[3].hub_status(), AgentHubStatus::Aborted);
    }

    #[test]
    fn handle_touch_updates_activity() {
        let handle = test_handle();
        let before = handle.last_activity();
        std::thread::sleep(Duration::from_millis(10));
        handle.touch_activity();
        assert!(handle.last_activity() > before);
    }

    #[test]
    fn transcript_cache_hits_on_same_mtime() {
        // mtime 변경 없으면 캐시 반환
    }
}
```

---

## 9. 부록: omp → oxi 매핑

| omp 위치 | oxi 위치 |
|---|---|
| `modes/components/agent-hub.ts` (566) | `oxi-cli/src/tui/overlay/agent_hub.rs` |
| `modes/components/agent-transcript-viewer.ts` (461) | `oxi-cli/src/tui/overlay/agent_hub.rs` (통합) |
| `modes/components/agent-dashboard.ts` (1,206) | 후순위 (별도 설계) |
| `registry/agent-registry.ts` | `oxi-sdk/src/lifecycle/agent_pool.rs` (기존) |
| `registry/agent-lifecycle.ts` | `oxi-sdk/src/lifecycle/supervisor.rs` (기존) |
| `registry/agent-registry.ts` (AgentRef) | `AgentHandle` 확장 필드 |
| `irc/bus.ts` (unread count) | 미구현 (항상 0) |
| `advisor/` | 후순위 (별도 설계) |
