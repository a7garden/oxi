# MCP 기능 고도화 설계 리뷰

## 총평

설계서는 pi-mcp-adapter의 아이디어를 oxi 아키텍처에 잘 매핑했습니다.
Phase 분리가 합리적이고, 크레이트 경계 원칙(oxi-tui 독립성, 의존 흐름)을 준수합니다.
다만 **실제 코드 기반으로 검증하면 수정이 필요한 부분**이 몇 가지 있습니다.

---

## 1. 설계서와 실제 코드의 불일치 (정정 필요)

### 1.1 `McpManager` 생성 위치 — 설계서가 틀림

**설계서 내용:**
> `oxi-agent/src/tools.rs` → `ToolRegistry::with_builtins_cwd()`에서 `OnceCell`로 `McpManager` 생성

**실제 코드:**
```rust
// oxi-agent/src/tools.rs:469-472
let mcp_once: std::cell::OnceCell<Arc<crate::mcp::McpManager>> = std::cell::OnceCell::new();
let mcp_manager = mcp_once
    .get_or_init(|| Arc::new(crate::mcp::McpManager::new()))
    .clone();
```

**문제:** `McpManager`는 **oxi-agent 내부**에서 생성됩니다. oxi-cli는 `McpManager`에 직접 접근할 수 없습니다.
설계서 Phase 3에서 `McpDashboardOverlay`가 `Arc<McpManager>`를 들고 있다고 가정하지만,
현재 아키텍처에서는 `McpManager`가 `ToolRegistry` 내부에 캡슐화되어 있습니다.

**해결 방안:**
- `ToolRegistry`에 `fn mcp_manager(&self) -> Option<Arc<McpManager>>` getter 추가
- 또는 `McpManager`를 `ToolRegistry` 외부에서 생성 후 주입하는 패턴으로 변경

### 1.2 `McpManagerInner`에 `LifecycleManager`를 넣을 수 없음

**설계서:**
```rust
pub struct McpManagerInner {
    clients: HashMap<String, McpClient>,
    tool_metadata: HashMap<String, Vec<ToolMetadata>>,
    failure_tracker: HashMap<String, Instant>,
    lifecycle: LifecycleManager,       // ← 문제
}
```

**문제:** `LifecycleManager`는 `tokio::task::JoinHandle`을 들고 있어야 합니다.
`McpManagerInner`는 이미 `tokio::sync::Mutex`로 보호되어 있는데,
idle timer가 발동하면 다시 `McpManagerInner`의 락을 잡아야 합니다 → **데드락 위험**.

```rust
// idle timer 콜백 (spawn된 태스크)
async move {
    tokio::time::sleep(duration).await;
    // McpManagerInner 락을 다시 잡아야 함 → 데드락!
    let mut inner = manager.inner.lock().await;
    inner.clients.remove(server_name);
}
```

**해결 방안:**
- `LifecycleManager`를 `McpManager` (뮤텍스 밖)에 배치하고 별도 동기화
- 또는 idle timer 콜백에서 직접 disconnect하지 않고 채널(`tokio::sync::mpsc`)로 메시지를 보내,
  `McpManager`의 백그라운드 이벤트 루프가 처리하게 설계

```rust
pub struct McpManager {
    inner: tokio::sync::Mutex<McpManagerInner>,
    config: parking_lot::RwLock<McpConfig>,
    // NEW: lifecycle 이벤트 채널
    lifecycle_tx: tokio::sync::mpsc::UnboundedSender<LifecycleEvent>,
    lifecycle_handle: tokio::task::JoinHandle<()>,
}

enum LifecycleEvent {
    StartIdleTimer { server: String, timeout: Duration },
    ResetIdleTimer { server: String, timeout: Duration },
    CancelTimers { server: String },
    StartHealthCheck { server: String },
}
```

### 1.3 `McpManager::status()`가 `McpManager`의 공개 API가 아님

**설계서:**
> `McpManager::status_data()` → `McpDashboardData`

**실제:** `McpManager::status()`는 `pub async fn`이지만 `String`을 반환합니다.
TUI용 구조적 데이터를 반환하려면 **새 메서드**가 필요합니다.

**권장:** `status()`는 유지(프록시 툴용)하고, 새로 `dashboard_data()` 추가:

```rust
impl McpManager {
    /// TUI 대시보드용 구조적 상태 데이터
    pub async fn dashboard_data(&self) -> McpDashboardData {
        // 서버 상태, 툴 목록, lifecycle 정보 등을 구조적으로 반환
    }
}
```

---

## 2. 설계 결함 및 누락

### 2.1 🔴 Direct Tools 등록 시점 문제

**설계서:** "Bootstrap 시점 (프로세스 시작)에 캐시에서 툴 목록을 읽어 ToolRegistry에 등록"

**문제:** 현재 `ToolRegistry::with_builtins_cwd()`는 `McpManager`를 `OnceCell`로 내부에서 생성합니다.
Direct tools를 등록하려면:
1. `McpManager`를 먼저 생성
2. 캐시에서 툴 읽기
3. 각 툴을 `McpDirectTool`로 래핑해서 `ToolRegistry`에 추가

이 순서가 현재 `with_builtins_cwd()` 내부에서 처리되어야 합니다.
**설계서가 이 메서드의 수정을 명시하지 않음.**

**권장 수정:**
```rust
// tools.rs
pub fn with_builtins_cwd(cwd: PathBuf, disabled_tools: &[String]) -> Self {
    // ...
    let mcp_manager = Arc::new(McpManager::new_with_cache(&cwd));
    
    // Direct tools 등록
    let direct_tools = mcp_manager.direct_tools_from_cache();
    for tool_def in direct_tools {
        all_tools.push(Box::new(McpDirectTool::new(mcp_manager.clone(), tool_def)));
    }
    
    // 프록시 툴 (설정에 따라 생략 가능)
    if !mcp_manager.disable_proxy_tool() {
        all_tools.push(Box::new(McpTool::new(mcp_manager)));
    }
    // ...
}
```

### 2.2 🔴 `McpDashboardOverlay`가 비동기 작업을 어떻게 하는지 불명확

**설계서:** `McpDashboardOverlay`가 `Arc<McpManager>`를 들고 있고,
`r` 키 누르면 서버 재연결(`OverlayAction::McpAction(Reconnect(...))`)

**문제:** Overlay의 `handle_key()`는 동기 함수입니다.
MCP 서버 재연결은 비동기 작업입니다.
`OverlayAction::McpAction`을 반환한 후 `handlers.rs`에서 비동기 처리를 해야 하는데,
이 패턴이 기존 핸들러에 없습니다.

기존 핸들러에서 비동기 오버레이 액션을 처리하는 패턴:
```rust
// handlers.rs::handle_overlay_key()
OverlayAction::SwitchSession(path) => {
    state.next_action = Some(TuiNextAction::SwitchSession(path));
    state.overlay_state = None;
}
```

여기서 `state.next_action`은 즉각적인 액션이고, 실제 세션 전환은 이벤트 루프에서 처리됩니다.
MCP 재연결도 동일한 패턴을 사용해야 합니다:

```rust
OverlayAction::McpAction(McpAction::Reconnect(server)) => {
    state.next_action = Some(TuiNextAction::McpReconnect(server));
    // 오버레이는 닫지 않음 → 재연결 완료 후 대시보드가 새로고침됨
}
```

**설계서에 명시 필요:**
1. `TuiNextAction`에 MCP 관련 액션 추가
2. 오버레이가 비동기 결과를 어떻게 반영받는지 (폴링? 채널? `needs_refresh` 플래그?)

### 2.3 🟡 캐시 파일 경로가 config 로드와 불일치

**설계서:** `~/.oxi/mcp-cache.json`

**실제:** `McpManager::new()`는 `config::load_mcp_config()`를 호출하는데,
이 함수는 `dirs::config_dir()` 기반으로 설정을 읽습니다.
macOS에서는 `~/Library/Application Support/` 일 수 있습니다.

캐시 파일 경로도 `dirs::config_dir()` 기반이어야 일관성이 있습니다:
```rust
let cache_path = dirs::config_dir()
    .unwrap_or_else(|| PathBuf::from("."))
    .join("oxi")
    .join("mcp-cache.json");
```

### 2.4 🟡 캐시 flush 타이밍 명확화 필요

**설계서:** "서버 연결 후 툴 목록을 캐시에 저장"

**문제:** 언제 디스크에 쓰는지 불명확.
- 연결 성공 직후마다? → 잦은 디스크 쓰기
- 세션 종료 시? → 크래시 시 캐시 손실
- debounced? → 복잡도 증가

**권장:** 연결 성공 직후 즉시 저장 (temp + rename으로 원자적 쓰기).
MCP 서버 연결은 빈번하지 않으므로 성능 문제 없음.

### 2.5 🟡 ConsentManager — TUI 인터랙션 설계 누락

**설계서:** "Ask 상태인 툴 호출 시, agent loop에서 사용자에게 인라인 프롬프트 표시"

**문제:** "인라인 프롬프트"가 구체적으로 무엇인지 정의 안 됨.
기존 oxi에 사용자 확인용 인라인 UI가 없습니다.

**옵션:**
1. **에이전트가 텍스트로 질문** → LLM이 사용자에게 "이 툴을 실행할까요?"라고 물어보고 응답 대기
2. **별도 미니 오버레이** → 툴 실행 중에 작은 확인 팝업
3. **미리 승인** → `/mcp` 대시보드에서만 관리, 실행 시에는 Allow/Deny만

**권장:** Phase 2에서는 옵션 3 (사전 승인만)으로 단순화.
TUI 대시보드에서 consent 관리, 실행 시에는 consent 상태만 체크.
Ask 모드는 아예 빼거나 Phase 3+로 미룸.

---

## 3. 아키텍처 개선 제안

### 3.1 LifecycleManager는 채널 기반으로

위 1.2에서 지적한 데드락 문제의 구체적 해결:

```rust
pub struct McpManager {
    inner: tokio::sync::Mutex<McpManagerInner>,
    config: parking_lot::RwLock<McpConfig>,
    cache: MetadataCache,
    lifecycle_tx: mpsc::UnboundedSender<LifecycleEvent>,
}

impl McpManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let manager = Self {
            inner: Mutex::new(McpManagerInner::default()),
            config: RwLock::new(config::load_mcp_config()),
            cache: MetadataCache::new(&cache_dir),
            lifecycle_tx: tx,
        };

        // 백그라운드 lifecycle 이벤트 루프
        let weak = Arc::downgrade(&manager); // ← 불가능... Self가 Arc 안에 있음
        // → 다른 패턴 필요

        manager
    }
}
```

**실제 권장 패턴:** `McpManager`를 `Arc`로 감싸고 `new()` 대신 `async fn start()` 제공:

```rust
impl McpManager {
    /// 생성 + 백그라운드 태스크 시작
    pub fn spawn() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let manager = Arc::new(Self {
            inner: Mutex::new(McpManagerInner::default()),
            config: RwLock::new(config::load_mcp_config()),
            cache: MetadataCache::new(&cache_dir),
            lifecycle_tx: tx,
        });

        let mgr = Arc::downgrade(&manager);
        tokio::spawn(async move {
            // lifecycle 이벤트 루프
            while let Some(event) = rx.recv().await {
                if let Some(m) = mgr.upgrade() {
                    m.handle_lifecycle_event(event).await;
                }
            }
        });

        manager
    }
}
```

이 패턴은 `McpManager::new()` → `McpManager::spawn()`으로 변경됨.
`ToolRegistry`에서 `Arc::new(McpManager::new())` 대신 `McpManager::spawn()` 사용.

### 3.2 oxi-tui의 뷰 타입 — 크레이트 경계 재고

**설계서:**
> `McpServerView`, `McpToolView`, `McpSettingsSummary` 등을 oxi-tui에 정의

**문제:** 이 타입들은 MCP 도메인 지식(서버, 툴, lifecycle, consent)을 포함합니다.
oxi-tui는 "oxi-* 의존 없음" 원칙을 지켜야 합니다.
이 뷰 타입을 oxi-tui에 넣으면 oxi-tui가 MCP 개념에 결합됩니다.

**권장:** 뷰 타입은 **oxi-agent에** 정의하고, oxi-tui에는 **제네릭 대시보드 위젯**만 제공:

```rust
// oxi-tui: 제네릭 섹션/아이템 기반 대시보드 위젯
pub struct SectionedDashboard {
    pub sections: Vec<DashboardSection>,
    pub selected_section: usize,
    pub selected_item: usize,
    pub filter: String,
}

pub struct DashboardSection {
    pub title: String,
    pub items: Vec<DashboardItem>,
}

pub struct DashboardItem {
    pub label: String,
    pub detail: String,
    pub status: ItemStatus,       // Connected, Disconnected, Error
    pub badge: Option<String>,    // "eager", "lazy", "DIRECT", "PROXY"
}
```

그리고 oxi-cli에서 MCP 데이터를 이 제네릭 구조로 변환.
이렇게 하면 oxi-tui는 MCP에 대해 아무것도 모르면서도 대시보드를 렌더링할 수 있습니다.

**트레이드오프:** 제네릭 위젯은 MCP 특화 UI(consent 토글, direct/proxy 전환)를 
표현력 있게 렌더링하기 어려울 수 있음. 실용적으로는 MCP 전용 위젯을 oxi-tui에 
넣되, oxi-agent 타입을 의존하지 않고 독립적인 뷰 모델을 사용하는 것이 타협점.

### 3.3 Transport trait 추출은 Phase 1에서 미리 준비

**설계서:** Phase 4에서 Transport trait 추출

**권장:** Phase 1에서 인터페이스만 정의. 구현은 기존 stdio 코드를 그대로 래핑.

```rust
// Phase 1: trait만 정의
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn send(&mut self, json: &str) -> anyhow::Result<()>;
    async fn recv(&mut self) -> anyhow::Result<RawJsonRpcMessage>;
    async fn close(&mut self) -> anyhow::Result<()>;
    fn is_connected(&self) -> bool;
}

// 기존 McpClient 내부 로직을 StdioTransport으로 래핑 (기능 변화 없음)
struct StdioTransport { /* 기존 McpClient의 전송 부분 */ }
```

이렇게 하면 Phase 4에서 HTTP/SSE 추가 시 `McpClient` 수정 없이 새 Transport만 구현하면 됨.

---

## 4. Phase 우선순위 재검토

### 현재 설계서의 Phase 순서
```
Phase 1: Lifecycle + Cache       (P0)
Phase 2: Direct Tools + Consent  (P1)
Phase 3: TUI Dashboard           (P0)
Phase 4: HTTP/SSE                (P2)
```

### 리뷰어 제안
```
Phase 1: Cache + Lifecycle + Transport trait 인터페이스  (P0, ~5일)
  - cache.rs (디스크 캐시)
  - lifecycle.rs (채널 기반, mpsc + spawn 패턴)
  - Transport trait 정의 (구현은 기존 stdio 래핑)
  - McpManager::spawn() 패턴 도입
  - McpManager 대시보드 데이터 API

Phase 2: TUI Dashboard                                    (P0, ~4일)
  - oxi-tui: MCP 대시보드 위젯 (독립 뷰 모델)
  - oxi-cli: McpDashboardOverlay + OverlayAction
  - /mcp 슬래시 명령
  - ※ Phase 1보다 먼저 해도 됨 — 캐시/lifecycle이 없어도 기본 상태 표시 가능

Phase 3: Direct Tools                                      (P1, ~2일)
  - direct_tool.rs (AgentTool 구현)
  - ToolRegistry 통합
  - 설정 스키마 확장 (directTools, excludeTools)

Phase 4: Consent + 설정 관리                                (P1, ~1.5일)
  - 사전 승인 모델만 (Ask 모드는 제외)
  - /mcp 대시보드에서 consent 관리
  - 설정 저장 (direct/proxy 토글)

Phase 5: HTTP/SSE Transport                                (P2, ~2일)
  - HttpSseTransport 구현
  - Phase 1에서 정의한 Transport trait 구현체
```

**변경 이유:**
- TUI 대시보드는 사용자 가치가 높고, 기존 기능(상태 표시, 수동 연결/해제)만으로도 유용
- Direct tools은 설정 변경이 필요하므로 consent/설정 관리와 묶는 게 자연스러움
- Consent는 단순화(Ask 제외)하여 복잡도 감소

---

## 5. 누락된 세부 사항

### 5.1 McpManager 재연결 중 동시성

여러 에이전트 턴이 동시에 같은 서버의 툴을 호출하면?
현재 `tokio::sync::Mutex`로 직렬화되지만, lazy_connect → connect 사이에
경쟁이 발생할 수 있음.

**해결:** `connect()` 시작 시 먼저 client 슬롯에 "Connecting" 상태를 표시하는 패턴 필요.

### 5.2 캐시와 설정의 tool_prefix 불일치

캐시된 툴 이름은 캐시 저장 시점의 `tool_prefix` 설정으로 만들어집니다.
설정을 변경하면 캐시된 툴 이름이 실제 설정과 불일치.

**해결:** 캐시에는 **원본 이름만** 저장하고, prefixed name은 런타임에 계산.

### 5.3 세션 재개 시 MCP 상태

oxi의 세션 시스템(Append-only)과 MCP 연결 상태는 독립적입니다.
세션을 재개(resume)하면 MCP 서버는 disconnected 상태에서 시작.
캐시가 있으면 검색/조회는 되지만, 툴 호출 시 재연결 필요.

**설계서에서 언급 필요:** 세션 재개 시 eager/keep-alive 서버 자동 연결 여부.

### 5.4 로깅

현재 `tracing::warn!`으로 MCP 관련 로그가 남습니다.
`debug: true`인 서버는 stderr를 상속하지만, idle disconnect 등의 이벤트는
사용자가 볼 수 없습니다.

**제안:** TUI 대시보드에 이벤트 로그 탭 추가 (Phase 2+).

---

## 6. 종합 평가

| 항목 | 평가 |
|------|------|
| 아키텍처 방향 | ✅ 올바름 — pi-mcp-adapter의 핵심 패턴을 잘 이해함 |
| 크레이트 경계 | ⚠️ oxi-tui에 MCP 뷰 타입을 넣는 것은 재고 필요 |
| Phase 분리 | ✅ 합리적이나, TUI를 Phase 2로 올리는 걸 권장 |
| 데드락 분석 | ❌ LifecycleManager + Mutex 조합의 데드락 미감지 |
| McpManager 접근성 | ❌ oxi-cli에서 McpManager에 접근하는 경로 미설계 |
| 비동기/동기 경계 | ⚠️ Overlay handle_key (동기) vs MCP 연결 (비동기) 패턴 불명확 |
| 호환성 | ✅ 기존 API 100% 유지하는 방향은 좋음 |
| 테스트 | ✅ 테스트 계획이 구체적 |
| 누락 | ⚠️ 캐시/설정 prefix 불일치, 세션 재개, 동시성 |

**결론:** 설계의 방향은 옳지만, **McpManager의 비동기 lifecycle 관리(데드락 회피)와
oxi-cli → McpManager 접근 경로** 두 가지를 먼저 해결해야 구현에 들어갈 수 있습니다.
위 리뷰의 수정사항을 반영한 후 Phase 1 구현을 시작하는 것을 권장합니다.
