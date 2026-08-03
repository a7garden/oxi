# MCP 기능 고도화 설계서

> **참고:** [pi-mcp-adapter](https://github.com/nicobailon/pi-mcp-adapter) 아키텍처를 기반으로,
> oxicode의 기존 MCP 구현을 확장하고 TUI 대시보드를 추가하는 설계.
> **리뷰 피드백 반영 (v2) + SDK 레이어 추가 (v3).**

---

## 1. 현황 분석 (Gap Analysis)

### 1.1 oxicode MCP — 이미 구현된 것

| 영역 | 상태 | 비고 |
|------|------|------|
| stdio 전송 | ✅ | Content-Length 프레이밍 JSON-RPC |
| 초기화 핸드셰이크 | ✅ | `initialize` + `notifications/initialized` |
| `tools/list` · `tools/call` | ✅ | prefixed name 라우팅 |
| `resources/list` · `resources/read` | ✅ | |
| `prompts/list` · `prompts/get` | ✅ | |
| 4-파일 설정 병합 | ✅ | shared global → oxicode global → shared project → oxicode project |
| Lazy connect | ✅ | 첫 호출 시 연결 |
| Failure backoff | ✅ | 30초 기본 |
| 프록시 툴 (`mcp`) | ✅ | 단일 게이트웨이 tool |
| 툴 검색 (search/describe) | ✅ | fuzzy + regex |
| 우아한 종료 | ✅ | SIGTERM → 5s → SIGKILL |

### 1.2 oxicode MCP — 미구현 (pi-mcp-adapter 대비)

| 영역 | 중요도 | pi-mcp-adapter | 설명 |
|------|--------|----------------|------|
| **Lifecycle 실행** | 🔴 High | `lifecycle.ts` | `Lazy`/`Eager`/`KeepAlive` 정의만 있고 실행 안 됨 |
| **Idle timeout** | 🔴 High | `server-manager.ts` | 설정은 있지만 타이머 없음 → 서버 프로세스 무한 실행 |
| **Metadata cache** | 🔴 High | `metadata-cache.ts` | 디스크 캐시 없음 → 재시작마다 서버 재연결 필요 |
| **TUI 대시보드** | 🔴 High | `mcp-panel.ts` | `/mcp` 대시보드 패널 없음 |
| **Direct tools** | 🟡 Medium | `direct-tools.ts` | 개별 툴을 `AgentTool`로 등록 불가 |
| **HTTP/SSE 전송** | 🟡 Medium | `server-manager.ts` | `url` 필드 있지만 구현 안 됨 |
| **Health check / 재연결** | 🟡 Medium | `server-manager.ts` | KeepAlive 서버 자동 재연결 없음 |
| **Consent system** | 🟡 Medium | `consent-manager.ts` | MCP 툴 실행 전 사용자 승인 없음 |
| **OAuth 인증** | 🟢 Low | `mcp-auth.ts` | Bearer/OAuth 미지원 |
| **Sampling (server→host)** | 🟢 Low | `sampling-handler.ts` | 타입만 정의, 미연결 |
| **Elicitation** | 🟢 Low | `elicitation-handler.ts` | MCP 서버→사용자 입력 요청 |
| **Config hot-reload** | 🟢 Low | — | 설정 변경 시 자동 감지 없음 |

### 1.3 크레이트 경계 제약

```
oxicode-ai  ←  oxicode-agent  ←  oxicode-sdk  ←  oxicode-cli
oxicode-tui  (독립, oxicode-* 의존 없음)  ←  oxicode-cli
```

- **oxicode-agent**: MCP 클라이언트 핵심 로직 (연결, 캐시, lifecycle, direct tools)
- **oxicode-tui**: 제네릭 대시보드 위젯 (MCP 도메인 지식 없이 순수 위젯)
- **oxicode-cli**: MCP 전용 뷰 모델 + TUI 오버레이 + bootstrap 연결

### 1.4 기존 코드의 핵심 제약

> 리뷰에서 발견한 구현 시 반드시 고려해야 할 사항.

1. **`McpManager` 생성 위치:** `ToolRegistry::with_builtins_cwd()` 내부에서 `OnceCell`로 생성.
   oxicode-cli는 `McpManager`에 직접 접근할 수 없음 → getter 또는 생성 패턴 변경 필요.

2. **`McpManagerInner`는 `tokio::sync::Mutex` 보호:** idle timer 콜백이 다시 뮤텍스를 잡으면
   데드락 → lifecycle 이벤트는 mpsc 채널 + 백그라운드 태스크로 분리.

3. **Overlay `handle_key()`는 동기:** MCP 재연결(비동기)은 `TuiNextAction`으로 디스패치
   후 이벤트 루프에서 처리.

---

## 2. 아키텍처 설계

### 2.1 전체 구조도

```
┌─────────────────────────────────────────────────────────────────┐
│                           oxicode-cli                               │
│  ┌──────────────┐  ┌────────────────────┐  ┌────────────────┐  │
│  │  Bootstrap    │  │  TUI               │  │  Print/RPC     │  │
│  │              │  │  ┌──────────────┐  │  │  Mode          │  │
│  │  McpManager  │  │  │ McpDashboard │  │  │                │  │
│  │  생성/주입    │  │  │ Overlay      │  │  │  동일          │  │
│  │  (spawn)     │  │  │ (oxicode-cli)    │  │  │  McpManager    │  │
│  │  DirectTool  │  │  └──────┬───────┘  │  │  사용          │  │
│  │  등록        │  │  McpDashboardWidget │  │                │  │
│  │              │  │  (oxicode-tui, 제네릭) │  │                │  │
│  └──────┬───────┘  └────────┬───────────┘  └───────┬────────┘  │
└─────────┼──────────────────┼───────────────────────┼───────────┘
          │                  │                       │
          ▼                  ▼                       ▼
┌─────────────────────────────────────────────────────────────────┐
│                         oxicode-agent                               │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                      McpManager                            │ │
│  │  ┌─────────────┐  ┌──────────┐  ┌──────────────────────┐  │ │
│  │  │ McpClient   │  │ Metadata │  │ Lifecycle Task       │  │ │
│  │  │ (stdio/HTTP)│  │ Cache    │  │ (mpsc 채널 기반,     │  │ │
│  │  │             │  │          │  │  Mutex 밖에서 실행)  │  │ │
│  │  └─────────────┘  └──────────┘  └──────────────────────┘  │ │
│  │  ┌─────────────────────────────────────────────────────┐   │ │
│  │  │ ToolRegistry Bridge                                 │   │ │
│  │  │  - McpProxyTool (기존 mcp 프록시)                    │   │ │
│  │  │  - McpDirectTool (개별 등록, 새로 추가)              │   │ │
│  │  └─────────────────────────────────────────────────────┘   │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 파일 배치 계획

#### oxicode-agent (핵심 로직)

```
oxicode-agent/src/mcp/
├── mod.rs                  # McpManager (확장: spawn(), dashboard_data(), 채널 기반 lifecycle)
├── client.rs               # McpClient (확장: Transport 위임, ping)
├── config.rs               # 설정 로드 (확장: 검증 강화)
├── types.rs                # 타입 정의 (확장: DirectTools, Consent, CacheEntry, McpDashboardData)
├── tool.rs                 # McpProxyTool (기존, 수정 최소화)
├── content.rs              # 콘텐츠 변환 (기존 유지)
├── lifecycle.rs            # [NEW] mpsc 채널 기반 lifecycle 이벤트 루프
├── cache.rs                # [NEW] 디스크 metadata cache (원본 이름만 저장)
├── direct_tool.rs          # [NEW] 개별 MCP 툴 → AgentTool 브릿지
├── consent.rs              # [NEW] 툴 실행 승인 (Allow/Deny만, Ask는 Phase 4+)
└── transport/
    ├── mod.rs              # [NEW] Transport trait 정의
    └── stdio.rs            # [NEW] 기존 stdio 로직 추출 (Phase 1에서 래핑)
```

#### oxicode-tui (순수 위젯, MCP 도메인 지식 없음)

```
oxicode-tui/src/widgets/
├── dashboard.rs            # [NEW] 제네릭 섹션 기반 대시보드 위젯
```

#### oxicode-cli (MCP 전용 뷰 모델 + 오버레이 + Bootstrap)

```
oxicode-cli/src/tui/overlay/
├── mcp_dashboard.rs        # [NEW] OverlayComponent 구현체 + MCP 뷰 모델 변환

oxicode-cli/src/tui/slash.rs    # "/mcp" 명령 추가
oxicode-cli/src/tui/handlers.rs # OverlayAction::McpAction, TuiNextAction 확장
```

---

## 3. Phase별 구현 계획

> **리뷰 반영:** TUI 대시보드를 Phase 2로 앞당기고, Consent Ask 모드는 Phase 4+로 이연.

### Phase 1: Cache + Lifecycle + Transport 인터페이스 (핵심 인프라)

> **목표:** 디스크 캐시, 채널 기반 lifecycle, Transport trait 인터페이스.
> pi-mcp-adapter의 `lifecycle.ts` + `metadata-cache.ts`에 해당.

#### 3.1.1 `cache.rs` — Metadata Cache

```rust
/// MCP 툴 메타데이터 디스크 캐시
///
/// **중요:** 캐시에는 **원본 툴 이름만** 저장.
/// prefixed name은 런타임에 현재 설정의 `tool_prefix`로 계산.
/// 이렇게 하면 설정 변경 시 캐시 무효화가 필요 없음.
pub struct MetadataCache {
    /// 캐시 파일 경로 (dirs::config_dir()/oxicode/mcp-cache.json)
    cache_path: PathBuf,
    /// 인메모리 캐시 (서버 이름 → 툴 목록)
    cache: parking_lot::RwLock<CacheStore>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CacheStore {
    version: u32,
    servers: HashMap<String, ServerCacheEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerCacheEntry {
    updated_at: String,
    /// 원본 툴 정의 (prefixed name 없음)
    tools: Vec<CachedToolDef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedToolDef {
    name: String,                          // 원본 이름 (예: "take_screenshot")
    description: String,
    input_schema: Option<serde_json::Value>,
}

impl MetadataCache {
    pub fn new() -> Self;

    /// 디스크에서 캐시 로드
    pub fn load(&self) -> anyhow::Result<()>;

    /// 특정 서버의 캐시된 툴 목록 반환 (ToolMetadata로 변환)
    pub fn get_tools(&self, server_name: &str, prefix_mode: &ToolPrefix) -> Vec<ToolMetadata>;

    /// 서버 연결 후 툴 목록을 캐시에 저장 (원본 이름만)
    /// 저장 후 즉시 디스크 flush (temp + rename)
    pub fn update(&self, server_name: &str, tools: &[McpToolDef]) -> anyhow::Result<()>;

    /// 특정 서버 캐시 무효화
    pub fn invalidate(&self, server_name: &str) -> anyhow::Result<()>;
}
```

**캐시 파일 구조 (`{config_dir}/oxicode/mcp-cache.json`):**
```json
{
  "version": 1,
  "servers": {
    "chrome-devtools": {
      "updated_at": "2026-06-13T10:30:00Z",
      "tools": [
        {
          "name": "take_screenshot",
          "description": "Take a screenshot...",
          "input_schema": { "type": "object", "properties": { ... } }
        }
      ]
    }
  }
}
```

**설계 결정 — 원본 이름만 캐시:** 캐시 저장 시점의 `tool_prefix` 설정과
나중에 다를 수 있음. prefixed name은 항상 런타임에 계산.

#### 3.1.2 `lifecycle.rs` — 채널 기반 Lifecycle 관리

> **리뷰 반영:** `LifecycleManager`를 `McpManagerInner` 안에 두면
> idle timer 콜백이 다시 뮤텍스를 잡아야 해서 **데드락** 발생.
> → mpsc 채널 + 독립 백그라운드 태스크로 분리.

```rust
/// Lifecycle 이벤트 (McpManager → 백그라운드 태스크로 전송)
enum LifecycleEvent {
    /// idle 타이머 시작/갱신
    StartIdleTimer { server: String, timeout: Duration },
    /// idle 타이머 취소
    CancelIdleTimer { server: String },
    /// keep-alive 서버 health check 시작
    StartHealthCheck { server: String },
    /// 서버 종료 → 모든 타이머 정리
    ServerStopped { server: String },
    /// 전체 종료
    Shutdown,
}

/// Lifecycle 이벤트 루프 (McpManager::spawn()에서 tokio::spawn)
async fn lifecycle_event_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<LifecycleEvent>,
    manager: std::sync::Weak<McpManager>,
) {
    let mut idle_timers: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut health_handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            LifecycleEvent::StartIdleTimer { server, timeout } => {
                // 기존 타이머 취소
                if let Some(h) = idle_timers.remove(&server) { h.abort(); }
                let mgr = manager.clone();
                let srv = server.clone();
                idle_timers.insert(server, tokio::spawn(async move {
                    tokio::time::sleep(timeout).await;
                    if let Some(m) = mgr.upgrade() {
                        m.disconnect_server(&srv).await;
                    }
                }));
            }
            LifecycleEvent::StartHealthCheck { server } => { /* 주기적 ping */ }
            LifecycleEvent::CancelIdleTimer { server } => {
                if let Some(h) = idle_timers.remove(&server) { h.abort(); }
            }
            LifecycleEvent::ServerStopped { server } => {
                if let Some(h) = idle_timers.remove(&server) { h.abort(); }
                if let Some(h) = health_handles.remove(&server) { h.abort(); }
            }
            LifecycleEvent::Shutdown => break,
        }
    }
    // 정리
    for (_, h) in idle_timers { h.abort(); }
    for (_, h) in health_handles { h.abort(); }
}
```

**동작:**
- `Lazy`: 연결 후 첫 툴 호출에서 `StartIdleTimer` 전송.
- `Eager`: `spawn()` 시점에 백그라운드 연결. idle 타이머는 설정된 경우만.
- `KeepAlive`: `spawn()` 시점에 백그라운드 연결. idle 타이머 없음. `StartHealthCheck` 전송.

**데드락 회포 원리:**
```
[Agent 스레드]                    [Lifecycle 태스크]
     │                                  │
     │ lock(McpManagerInner)            │ sleep(idle_timeout)
     │ call_tool()                      │ ...
     │ unlock                           │ timeout 만료
     │                                  │ Weak::upgrade() → Arc<McpManager>
     │                                  │ disconnect_server() → lock(McpManagerInner)
     │                                  │ unlock
     │                                  │
     │ ← 경쟁 없음: 락을 동시에 잡지 않음
```

#### 3.1.3 Transport Trait 인터페이스 (Phase 1에서 정의만)

```rust
/// MCP 전송 계층 추상화.
/// Phase 1에서는 인터페이스만 정의하고 기존 stdio 코드를 StdioTransport으로 래핑.
/// Phase 5에서 HttpSseTransport 구현 시 이 trait만 구현하면 됨.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// JSON-RPC 메시지 전송
    async fn send(&mut self, json: &str) -> anyhow::Result<()>;
    /// JSON-RPC 메시지 수신
    async fn recv(&mut self) -> anyhow::Result<RawJsonRpcMessage>;
    /// 연결 종료
    async fn close(&mut self) -> anyhow::Result<()>;
    /// 연결 상태
    fn is_connected(&self) -> bool;
}
```

Phase 1에서는 `McpClient` 내부에 `StdioTransport`을 필드로 추가하되,
외부 API는 그대로 유지. `HttpSseTransport`는 Phase 5에서 구현.

#### 3.1.4 `McpManager` 확장 — spawn 패턴

```rust
pub struct McpManager {
    inner: tokio::sync::Mutex<McpManagerInner>,
    config: parking_lot::RwLock<McpConfig>,
    cache: MetadataCache,
    lifecycle_tx: tokio::sync::mpsc::UnboundedSender<LifecycleEvent>,
    _lifecycle_handle: tokio::task::JoinHandle<()>,
}

pub struct McpManagerInner {
    clients: HashMap<String, McpClient>,
    /// "서버 이름 → 원본 툴 목록" (prefixed name은 조회 시 계산)
    raw_tool_metadata: HashMap<String, Vec<McpToolDef>>,
    failure_tracker: HashMap<String, Instant>,
    /// 연결 진행 중인 서버 (동시성 보호)
    connecting: HashSet<String>,
}

impl McpManager {
    /// 기존 API 호환 — 내부적으로 spawn() 호출.
    /// ToolRegistry::with_builtins_cwd()에서 사용.
    pub fn new() -> Arc<Self> {
        Self::spawn()
    }

    /// 생성 + 백그라운드 lifecycle 태스크 시작.
    /// Arc로 감싸서 반환 (Weak 참조를 lifecycle 태스크에 전달).
    pub fn spawn() -> Arc<Self> {
        let config = config::load_mcp_config();
        let cache = MetadataCache::new();
        cache.load().ok(); // 없어도 에러 아님

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let manager = Arc::new(Self {
            inner: Mutex::new(McpManagerInner {
                clients: HashMap::new(),
                raw_tool_metadata: cache.all_server_tools(),
                failure_tracker: HashMap::new(),
                connecting: HashSet::new(),
            }),
            config: RwLock::new(config),
            cache,
            lifecycle_tx: tx,
            _lifecycle_handle: {
                let weak = Arc::downgrade(&manager); // ← 순환 참조 불가
                tokio::spawn(lifecycle_event_loop(rx, weak))
            },
        });

        // Eager/KeepAlive 서버 백그라운드 연결
        let mgr = manager.clone();
        tokio::spawn(async move {
            mgr.start_eager_servers().await;
        });

        manager
    }

    /// TUI 대시보드용 구조적 상태 데이터.
    /// 기존 status() 메서드(문자열 반환)는 유지.
    pub async fn dashboard_data(&self) -> McpDashboardData;

    /// Direct tools 캐시에서 조회 (ToolRegistry 등록용)
    pub fn direct_tools_from_cache(&self) -> Vec<DirectToolDef>;

    /// lifecycle 태스크에 이벤트 전송
    fn send_lifecycle(&self, event: LifecycleEvent) {
        let _ = self.lifecycle_tx.send(event);
    }
}
```

**기존 API 호환성:**
- `new()` → `Arc<Self>` 반환으로 변경.
  `ToolRegistry`에서 `Arc::new(McpManager::new())` → `McpManager::new()` 로 수정.
  (기존 코드에서도 `Arc`로 한 번 더 감싸고 있었으므로, `Arc` 한 겹 제거로 동일)

- `McpTool` 생성: `McpTool::new(mcp_manager.clone())` → 동일.

**연결 동시성 보호 (`connecting` set):**
```rust
async fn ensure_connected(&self, server_name: &str) -> bool {
    {
        let inner = self.inner.lock().await;
        if inner.clients.contains_key(server_name) { return true; }
        if inner.connecting.contains(server_name) { return false; }
    }
    // connecting 마크
    self.inner.lock().await.connecting.insert(server_name.to_string());
    let result = self.connect(server_name).await;
    self.inner.lock().await.connecting.remove(server_name);
    result
}
```

#### 3.1.5 `ToolRegistry` 수정 — Direct Tools 등록

```rust
// tools.rs
pub fn with_builtins_cwd(cwd: PathBuf, disabled_tools: &[String]) -> Self {
    // ...
    let mcp_manager = crate::mcp::McpManager::new(); // Arc<McpManager> 반환

    // Direct tools: 캐시에서 툴 목록 읽어 개별 등록
    let direct_tools = mcp_manager.direct_tools_from_cache();
    for def in &direct_tools {
        all_tools.push(Box::new(
            crate::mcp::McpDirectTool::new(mcp_manager.clone(), def.clone())
        ));
    }

    // 프록시 툴 (설정에 따라 생략 가능)
    if !mcp_manager.should_disable_proxy() {
        all_tools.push(Box::new(crate::mcp::McpTool::new(mcp_manager.clone())));
    }

    // McpManager getter 저장 (TUI에서 접근용)
    // ...
}
```

#### 3.1.6 `ToolRegistry`에 McpManager getter 추가

```rust
impl ToolRegistry {
    /// TUI 대시보드에서 McpManager에 접근하기 위한 getter.
    /// mcp 툴이 등록되어 있지 않으면 None 반환.
    pub fn mcp_manager(&self) -> Option<Arc<McpManager>> {
        self.tools.iter()
            .find(|t| t.name() == "mcp")
            .and_then(|t| t.as_mcp_tool())
            .map(|t| t.manager())
    }
}
```

또는, `ToolRegistry` 생성 시 `Arc<McpManager>`를 별도 필드에 저장:

```rust
pub struct ToolRegistry {
    tools: Vec<Box<dyn AgentTool>>,
    // ...
    mcp_manager: Option<Arc<McpManager>>, // NEW
}
```

---

### Phase 2: TUI Dashboard (MCP 관리 패널)

> **리뷰 반영:** Phase 3에서 Phase 2로 앞당김.
> 캐시/lifecycle 없이도 기본 상태 표시 가능 → 사용자 가치 조기 제공.

#### 3.2.1 아키텍처

```
┌──────────────────────────────────────────────────────────────┐
│  /mcp 명령 입력                                               │
│         │                                                     │
│         ▼                                                     │
│  slash.rs                                                     │
│    session.tool_registry().mcp_manager()                      │
│    state.overlay_state =                                      │
│        Some(Box::new(McpDashboardOverlay::new(manager)))      │
│         │                                                     │
│         ▼                                                     │
│  ┌──────────────────────────────────────────────────────┐     │
│  │              McpDashboardOverlay (oxicode-cli)            │     │
│  │  handle_key() → OverlayAction::McpAction(McpAction)  │     │
│  │  render() → build_dashboard_data() → widget.update() │     │
│  │                                                      │     │
│  │  ┌──────────────────────────────────────────────┐    │     │
│  │  │  DashboardWidget (oxicode-tui, 제네릭)            │    │     │
│  │  │  MCP 도메인 지식 없이 순수 렌더링만           │    │     │
│  │  └──────────────────────────────────────────────┘    │     │
│  └──────────────────────────────────────────────────────┘     │
│         │ OverlayAction::McpAction                            │
│         ▼                                                     │
│  handlers.rs::handle_overlay_key()                            │
│    match OverlayAction::McpAction(action) {                   │
│      Reconnect(srv) => {                                      │
│        state.next_action = Some(TuiNextAction::McpReconnect); │
│        // 오버레이는 닫지 않음 → 비동기 처리 후 자동 새로고침  │
│      }                                                        │
│    }                                                          │
│         │                                                     │
│         ▼                                                     │
│  메인 이벤트 루프에서 비동기 실행                               │
│    manager.connect(srv).await                                 │
│    overlay.mark_refresh()                                     │
└──────────────────────────────────────────────────────────────┘
```

#### 3.2.2 oxicode-tui: 제네릭 `DashboardWidget`

> **리뷰 반영:** oxicode-tui에 MCP 전용 타입을 넣지 않음.
> 제네릭 섹션/아이템 기반 위젯으로 MCP 독립성 유지.

```rust
/// 제네릭 섹션 기반 대시보드 위젯.
/// MCP에 특화되지 않은 범용 구조이므로 oxicode-tui에 배치.
pub struct DashboardWidget {
    sections: Vec<DashboardSection>,
    state: DashboardState,
}

#[derive(Debug, Clone)]
pub struct DashboardSection {
    pub title: String,
    pub items: Vec<DashboardItem>,
    pub collapsed: bool,
}

#[derive(Debug, Clone)]
pub struct DashboardItem {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub status: ItemStatus,
    pub badges: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    Active,
    Inactive,
    Error(String),
}

pub struct DashboardState {
    selected_section: usize,
    selected_item: usize,
    filter: String,
    mode: DashboardMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DashboardMode {
    Overview,
    Detail,
    FilterInput,
}
```

**렌더 레이아웃:**

```
┌─────────────────────── MCP Servers ───────────────────────┐
│                                                           │
│  ● chrome-devtools    Connected   26 tools  [eager]       │  ← 서버 섹션
│  ○ github             Cached      18 tools  [lazy]        │
│  ○ supabase           Error: ...   0 tools  [lazy]        │
│                                                           │
│  ─── chrome-devtools Tools ──────────────────────────      │
│  [DIRECT] take_screenshot    Take a screenshot...          │  ← 툴 섹션
│  [PROXY]  navigate           Navigate to URL...            │
│                                                           │
│  ↑↓ Navigate  Enter:Detail  r:Reconnect  d:Direct  Esc:← │
└───────────────────────────────────────────────────────────┘
```

#### 3.2.3 oxicode-cli: `McpDashboardOverlay` + 뷰 모델 변환

```rust
/// MCP 전용 뷰 모델 변환 로직 (oxicode-cli에 배치, oxicode-tui 독립성 유지)
pub struct McpDashboardOverlay {
    /// 제네릭 대시보드 위젯
    widget: DashboardWidget,
    /// McpManager 참조
    manager: Arc<McpManager>,
    /// 비동기 처리 후 다음 render에서 데이터 새로고침 플래그
    needs_refresh: bool,
}

impl McpDashboardOverlay {
    /// McpManager.dashboard_data()를 DashboardSection/Item으로 변환
    fn build_dashboard_data(&self) -> Vec<DashboardSection> {
        // 데이터는 render() 호출 시마다 동기적으로 변환
        // (McpManager.dashboard_data()는 async → 별도 처리 필요)
        // 해결: dashboard_data()의 결과를 캐시하고 needs_refresh 시에만 갱신
    }
}

impl OverlayComponent for McpDashboardOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Char('r') => {
                let server = self.widget.selected_item_id();
                return OverlayAction::McpAction(McpAction::Reconnect(server));
            }
            KeyCode::Char('d') => {
                // direct/proxy 토글
                let (server, tool) = self.widget.selected_item_ids();
                return OverlayAction::McpAction(McpAction::ToggleDirect { server, tool });
            }
            KeyCode::Esc => return OverlayAction::Close,
            // ... 기타 키
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget.render(frame, area, theme);
    }

    fn hint(&self) -> &str {
        "↑↓ Navigate │ Enter: Detail │ r: Reconnect │ d: Direct │ /: Filter │ Esc: Close"
    }
}
```

#### 3.2.4 비동기 오버레이 액션 처리

> **리뷰 반영:** Overlay `handle_key()`는 동기이므로,
> 비동기 MCP 작업은 `TuiNextAction`으로 디스패치.

```rust
// handlers.rs
async fn handle_overlay_key(key: KeyEvent, state: &mut AppState, session: &Session) -> Option<Action> {
    if let Some(ref mut overlay) = state.overlay_state {
        let action = overlay.handle_key(key);
        match action {
            OverlayAction::Close => {
                state.overlay_state = None;
            }
            // NEW: MCP 비동기 액션
            OverlayAction::McpAction(mcp_action) => {
                match mcp_action {
                    McpAction::Reconnect(server) => {
                        let manager = session.tool_registry().mcp_manager()
                            .expect("MCP manager not available");
                        // 비동기 실행 (오버레이는 열어둠)
                        match manager.connect(&server).await {
                            Ok(_) => {
                                // 오버레이 새로고침 트리거
                                if let Some(ref mut o) = state.overlay_state {
                                    if let Some(mcp) = o.as_mcp_dashboard_mut() {
                                        mcp.mark_refresh();
                                    }
                                }
                            }
                            Err(e) => {
                                // 오버레이에 에러 표시
                                state.show_notification(&format!("MCP reconnect failed: {}", e));
                            }
                        }
                    }
                    McpAction::ToggleDirect { server, tool } => {
                        // 설정 업데이트 (동기 파일 쓰기)
                        // ...
                    }
                    _ => {}
                }
            }
            // ... 기존 variants
            _ => {}
        }
    }
    None
}
```

#### 3.2.5 OverlayAction 및 McpAction 확장

```rust
// overlay/mod.rs
pub enum OverlayAction {
    // 기존 variants...
    None,
    Close,
    SwitchSession(String),
    NewSession,
    ExecuteSlashCommand(String),
    SendPrompt(String),
    OpenRouterSetup { ... },
    ForkFromEntry { ... },
    NavigateToEntry { ... },
    ProviderKeySaved { ... },
    ModelSelected { ... },

    // NEW
    McpAction(McpAction),
}

#[derive(Debug)]
pub enum McpAction {
    /// 서버 (재)연결 (비동기)
    Reconnect(String),
    /// 서버 연결 해제 (비동기)
    Disconnect(String),
    /// 메타데이터 새로고침 (비동기)
    Refresh(Option<String>),
    /// Direct tool 토글 (동기 설정 저장)
    ToggleDirect { server: String, tool: String },
    /// 설정 저장
    SaveConfig,
}
```

#### 3.2.6 `/mcp` 슬래시 명령

```
/mcp              → 대시보드 오버레이 열기
/mcp status       → 상태 텍스트 출력 (오버레이 없이)
/mcp tools        → 전체 툴 목록 텍스트 출력
/mcp reconnect    → 전체 서버 재연결
/mcp reconnect X  → 특정 서버 재연결
/mcp refresh      → 캐시 새로고침
```

#### 3.2.7 `McpDashboardData` — oxicode-agent에 정의

```rust
// types.rs (oxicode-agent)
/// TUI 대시보드용 구조적 상태 데이터.
/// oxicode-tui의 제네릭 DashboardWidget에 맵핑하기 위해
/// oxicode-cli에서 DashboardSection/Item으로 변환.
#[derive(Debug, Clone)]
pub struct McpDashboardData {
    pub servers: Vec<McpServerInfo>,
    pub settings: McpSettingsView,
}

#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub status: McpConnectionStatus,
    pub lifecycle: String,
    pub tool_count: usize,
    pub tools: Vec<McpToolInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum McpConnectionStatus {
    Connected,
    Disconnected,
    Connecting,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,          // prefixed name
    pub original_name: String,
    pub description: String,
    pub is_direct: bool,
    pub consent: ConsentState,
}

#[derive(Debug, Clone)]
pub struct McpSettingsView {
    pub tool_prefix: String,
    pub idle_timeout: Option<u64>,
    pub total_servers: usize,
    pub connected_servers: usize,
    pub total_tools: usize,
}
```

---

### Phase 3: Direct Tools

> **목표:** 개별 MCP 툴을 AgentTool로 직접 등록.
> pi-mcp-adapter의 `direct-tools.ts`에 해당.
> Consent Allow/Deny 사전 승인 모델 포함.

#### 3.3.1 `direct_tool.rs` — 개별 툴 등록

```rust
/// MCP 툴을 개별 AgentTool으로 등록하는 브릿지
pub struct McpDirectTool {
    server_name: String,
    tool_name: String,       // 원본 이름
    description: String,
    schema: serde_json::Value,
    manager: Arc<McpManager>,
}

#[async_trait]
impl AgentTool for McpDirectTool {
    fn name(&self) -> &str {
        // prefixed name 반환: 설정에 따라 server/short/none 모드 적용
        // 캐시에 원본 이름만 저장하므로 여기서 계산
    }
    fn label(&self) -> &str { &self.tool_name }
    fn description(&self) -> &str { &self.description }
    fn parameters_schema(&self) -> Value { self.schema.clone() }
    fn essential(&self) -> bool { false }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, String> {
        // 1. consent 체크 (Allow/Deny만)
        if self.manager.consent().check(&self.tool_name) == ConsentState::Deny {
            return Ok(AgentToolResult::error(
                format!("Tool '{}' is denied by consent policy", self.tool_name)
            ));
        }
        // 2. ensure_connected
        // 3. call_tool
        // 4. lifecycle idle timer 리셋
        // 5. 결과 반환
        self.manager.call_tool(&self.tool_name, params, Some(&self.server_name))
            .await
            .map(|r| {
                if r.is_error {
                    AgentToolResult::error(content::transform_mcp_content(&r.content))
                } else {
                    AgentToolResult::success(content::transform_mcp_content(&r.content))
                }
            })
            .map_err(|e| e.to_string())
    }
}
```

#### 3.3.2 `consent.rs` — 툴 실행 승인 (Allow/Deny만)

> **리뷰 반영:** `Ask` 모드는 Phase 4+로 이연.
> 초기에는 사전 승인(Allow)과 거부(Deny)만 지원.
> `/mcp` 대시보드에서만 관리.

```rust
/// MCP 툴 실행 승인 상태 (Ask 모드는 Phase 4+에서 추가)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentState {
    Allow,
    Deny,
}

impl Default for ConsentState {
    fn default() -> Self { ConsentState::Allow }
}

/// 툴 승인 관리자
pub struct ConsentManager {
    /// 툴 이름(또는 서버 이름) → 승인 상태
    decisions: parking_lot::RwLock<HashMap<String, ConsentState>>,
    persist_path: PathBuf,
}

impl ConsentManager {
    /// 툴 실행 전 승인 확인 (기본: Allow)
    pub fn check(&self, tool_name: &str) -> ConsentState;

    /// 승인 결정 저장 (메모리 + 디스크, temp + rename)
    pub fn decide(&self, tool_name: &str, state: ConsentState);
}
```

#### 3.3.3 설정 스키마 확장 (`types.rs`)

```rust
pub struct ServerEntry {
    // 기존 필드 (변경 없음)
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub lifecycle: Option<LifecycleMode>,
    pub idle_timeout: Option<u64>,
    pub debug: Option<bool>,

    // NEW
    pub direct_tools: Option<DirectToolsConfig>,
    pub exclude_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DirectToolsConfig {
    All(bool),
    Specific(Vec<String>),
}

pub struct McpSettings {
    // 기존 (변경 없음)
    pub tool_prefix: Option<ToolPrefix>,
    pub idle_timeout: Option<u64>,
    pub failure_backoff_secs: Option<u64>,

    // NEW
    pub direct_tools: Option<DirectToolsConfig>,
    pub disable_proxy_tool: Option<bool>,
}
```

---

### Phase 4: Consent Ask 모드 (선택)

> `Ask` 모드 추가 — 툴 실행 전 사용자 인라인 확인.
> 이 Phase는 TUI 인라인 프롬프트 UI가 필요하므로 별도 Phase로 분리.

**구현 내용:**
- `ConsentState::Ask` variant 추가
- Agent loop에서 `Ask` 상태 감지 시 사용자에게 y/n 프롬프트
- "Always allow" / "Always deny" 선택 시 consent 자동 저장

---

### Phase 5: HTTP/SSE Transport (선택)

> pi-mcp-adapter의 HTTP/SSE 전송 지원에 해당.
> Phase 1에서 정의한 `McpTransport` trait의 구현체 추가.

#### 3.5.1 `transport/http_sse.rs`

```rust
/// HTTP/SSE 기반 MCP 전송 계층
pub struct HttpSseTransport {
    url: String,
    headers: HashMap<String, String>,
    // SSE 연결 상태, 메시지 큐 등
}

#[async_trait]
impl McpTransport for HttpSseTransport {
    async fn send(&mut self, json: &str) -> anyhow::Result<()> { /* POST 요청 */ }
    async fn recv(&mut self) -> anyhow::Result<RawJsonRpcMessage> { /* SSE 스트림 읽기 */ }
    async fn close(&mut self) -> anyhow::Result<()> { /* 연결 종료 */ }
    fn is_connected(&self) -> bool { /* ... */ }
}
```

---

## 4. 의존성 및 크레이트 영향

### 4.1 Cargo.toml 변경

#### oxicode-agent
```toml
[dependencies]
# 기존 유지 — 새 의존 불필요 (tokio, serde, parking_lot으로 충분)
```

#### oxicode-tui
```toml
[dependencies]
# 기존 유지 — 새 의존 불필요
```

#### oxicode-cli
```toml
[dependencies]
oxicode-agent = { path = "../oxicode-agent" }
oxicode-tui = { path = "../oxicode-tui" }
# 기존 유지 — 새 의존 불필요
```

### 4.2 공개 API 변경

| 크레이트 | 변경 | 영향 |
|---------|------|------|
| oxicode-agent | `McpManager::new()` → `Arc<Self>` 반환 | `ToolRegistry`에서 `Arc` 감싸기 제거 |
| oxicode-agent | `McpManager::spawn()` (신규) | `ToolRegistry`에서 사용 |
| oxicode-agent | `McpManager::dashboard_data()` (신규) | TUI에서 상태 조회 |
| oxicode-agent | `McpManager::direct_tools_from_cache()` (신규) | Direct tool 등록용 |
| oxicode-agent | `McpDirectTool` pub | `ToolRegistry`에서 등록 |
| oxicode-agent | `McpDashboardData` 등 뷰 타입들 pub | CLI에서 TUI로 변환 |
| oxicode-agent | `ToolRegistry::mcp_manager()` (신규 getter) | TUI에서 McpManager 접근 |
| oxicode-sdk | `McpManager`, `McpTool`, `McpDirectTool` re-export | SDK 컨슈머가 MCP 직접 사용 |
| oxicode-sdk | `OxicodeBuilder::with_mcp_config()` (신규) | 커스텀 MCP 설정 주입 |
| oxicode-sdk | `mcp_tools()` 팩토리 (신규) | `coding_tools()`와 동일 패턴 |
| oxicode-tui | `DashboardWidget`, `DashboardSection/Item` pub | CLI에서 MCP 데이터 주입 |
| oxicode-cli | `McpDashboardOverlay`, `McpAction` | 내부 구현 |

---

## 5. 테스트 계획

### 5.1 단위 테스트

| 모듈 | 테스트 | 방식 |
|------|--------|------|
| `cache.rs` | 로드/저장/무효화/빈캐시/원본이름만저장 | 임시 디렉토리 |
| `lifecycle.rs` | idle 타이머/취소/eager시작/health check | `tokio::time::pause()` |
| `consent.rs` | 승인/거부/기본값/디스크저장 | 임시 디렉토리 |
| `direct_tool.rs` | 등록/실행/에러/consent-deny | Mock McpManager |
| `DashboardWidget` | 렌더링/선택/필터/섹션접기 | ratatui TestBackend |

### 5.2 통합 테스트

| 시나리오 | 파일 | 방식 |
|---------|------|------|
| 전체 lifecycle (lazy → connect → idle → disconnect → reconnect) | `tests/mcp_lifecycle.rs` | Mock MCP server |
| 캐시 → offline 검색 → 서버 없이 search 동작 | `tests/mcp_cache_offline.rs` | 캐시 파일 생성 후 검색 |
| Direct tool 등록 → 툴 호출 → 결과 | `tests/mcp_direct_tool.rs` | Mock server |
| 동시 연결 경쟁 (두 태스크가 동시에 lazy_connect) | `tests/mcp_concurrency.rs` | Mock server |

### 5.3 Mock MCP Server

```rust
/// 테스트용 간단 MCP 서버 (stdio)
/// `tools/list` → 고정 툴 목록 반환
/// `tools/call` → 고정 결과 반환
struct MockMcpServer;
impl MockMcpServer {
    /// mock 서버를 spawn하고 연결 가능한 command/args 반환
    fn spawn() -> (String, Vec<String>) { /* ... */ }
}
```

---

## 6. 마이그레이션 및 호환성

### 6.1 기존 설정 파일 호환

- 기존 `mcp.json` 형식 100% 호환 유지
- 새 필드 (`direct_tools`, `exclude_tools`)는 `Option<T>` → 생략 시 기본값
- 캐시 파일은 없어도 정상 동작 (기존 동작과 동일)

### 6.2 기존 McpTool 동작 유지

- `mcp` 프록시 툴은 계속 작동
- `settings.disable_proxy_tool: true` 설정 시에만 숨김
- Direct tools는 프록시와 병행 존재

### 6.3 세션 재개 시 MCP 상태

> **리뷰 반영:** 세션 재개 시 eager/keep-alive 서버 자동 재연결.

- 세션 재개(resume) 시 `McpManager::spawn()`이 다시 호출됨
- `spawn()`에서 eager/keep-alive 서버를 백그라운드로 자동 연결
- Lazy 서버는 첫 툴 호출 시 연결 (기존 동작)
- 캐시가 있으면 offline 검색 가능 → 재시작 후 즉시 툴 검색 가능

### 6.4 단계적 도입

```
Phase 1 (cache + lifecycle + transport 인터페이스)
  → 기존 동작에 투명하게 개선 (서버 관리 최적화)
  → 사용자에게 보이는 변화: 서버가 더 이상 무한 실행되지 않음
  → 재시작 후 캐시로 오프라인 툴 검색 가능

Phase 2 (TUI dashboard)
  → /mcp 명령으로 관리 UI 제공
  → 없어도 기존 CLI/RPC 모드 정상 동작
  → 캐시/lifecycle이 없어도 기본 상태 표시 가능

Phase 3 (direct tools + consent allow/deny)
  → 설정에 따라 Opt-in
  → directTools 설정한 서버만 개별 등록

Phase 4 (consent ask 모드) — 선택
  → 툴 실행 전 사용자 확인 프롬프트

Phase 5 (HTTP/SSE) — 선택
  → url 필드 설정한 서버만 해당
  → stdio 서버에 영향 없음
```

---

## 7. 구현 우선순위 및 추정 공수

> **리뷰 반영:** Phase 재배열, 공수 재추정.

| Phase | 작업 | 예상 LOC | 우선순위 | 공수 |
|-------|------|----------|---------|------|
| 1 | `cache.rs` | ~200 | 🔴 P0 | 1일 |
| 1 | `lifecycle.rs` (채널 기반) | ~250 | 🔴 P0 | 1.5일 |
| 1 | `transport/mod.rs` (trait 정의) | ~40 | 🔴 P0 | 0.5일 |
| 1 | `McpManager::spawn()` 리팩터링 | ~200 수정 | 🔴 P0 | 1.5일 |
| 1 | 단위 테스트 | ~300 | 🔴 P0 | 1일 |
| 2 | oxicode-tui `DashboardWidget` | ~350 | 🔴 P0 | 1.5일 |
| 2 | oxicode-agent `McpDashboardData` | ~100 | 🔴 P0 | 0.5일 |
| 2 | oxicode-cli `McpDashboardOverlay` | ~250 | 🔴 P0 | 1.5일 |
| 2 | OverlayAction + 핸들러 | ~120 | 🔴 P0 | 0.5일 |
| 2 | 슬래시 명령 | ~60 | 🔴 P0 | 0.5일 |
| 3 | `direct_tool.rs` | ~200 | 🟡 P1 | 1일 |
| 3 | `consent.rs` (Allow/Deny) | ~120 | 🟡 P1 | 0.5일 |
| 3 | `types.rs` 확장 + Bootstrap | ~100 | 🟡 P1 | 1일 |
| 4 | `ConsentState::Ask` + 인라인 프롬프트 | ~200 | 🟢 P2 | 1.5일 |
| 5 | `transport/stdio.rs` 추출 | ~100 | 🟢 P2 | 0.5일 |
| 5 | `transport/http_sse.rs` | ~300 | 🟢 P2 | 1.5일 |
| — | **SDK 레이어 (병렬 가능)** | | | |
| SDK | `lib.rs` re-export | ~20 | 🔴 P0 | 0.25일 |
| SDK | `builder.rs` API | ~80 | 🔴 P0 | 0.5일 |
| SDK | `tool_factory.rs` mcp_tools() | ~50 | 🟡 P1 | 0.25일 |
| SDK | `agent_builder.rs` 통합 | ~30 | 🟡 P1 | 0.25일 |
| SDK | 단위 테스트 | ~100 | 🔴 P0 | 0.5일 |
| | **총계** | **~3,270** | | **~16.75일** |

---

## 8. 핵심 설계 결정 (Design Decisions)

### D0: MCP를 SDK에 어떻게 노출할 것인가 (v3 추가)
- **결정:** Port가 아닌 **OxicodeBuilder API + Factory + Re-export** 3계층 노출.
- **이유:** MCP는 인프라가 아닌 에이전트 기능. `coding_tools()`와 동일한 패턴으로
  일관성 유지. 커스텀 백엔드 필요 시에만 Port로 승격.
- **참고:** §9에서 상세 설명.

### D1: 캐시 저장 위치
- **결정:** `dirs::config_dir()/oxicode/mcp-cache.json`
- **이유:** `config::config_paths()`와 동일한 `dirs::config_dir()` 기반으로 일관성 유지.
  macOS에서는 `~/Library/Application Support/oxicode/mcp-cache.json`.

### D2: 캐시는 원본 이름만 저장
- **결정:** 캐시 파일에는 unprefixed 원본 툴 이름만 저장. prefixed name은 런타임에 `ToolPrefix` 설정으로 계산.
- **이유:** `tool_prefix` 설정을 변경해도 캐시 무효화가 필요 없음.
- **pi-mcp-adapter와의 차이:** pi-mcp-adapter는 prefixed name을 캐시에 저장하지만,
  oxicode는 원본만 저장하여 설정 변경에 강건함.

### D3: Direct tools 등록 시점
- **결정:** `ToolRegistry::with_builtins_cwd()` 내부에서 캐시 기반 등록.
- **이유:** `McpManager::spawn()`이 `Arc<Self>`를 반환하므로
  `with_builtins_cwd()`에서 바로 사용 가능.
- **첫 실행 예외:** 캐시가 없으면 proxy-only로 시작.
  백그라운드에서 첫 연결 시 캐시 생성. 재시작 후 direct tools 활성화.

### D4: Lifecycle은 채널 기반 (데드락 회피)
- **결정:** `mpsc::UnboundedReceiver<LifecycleEvent>` + `tokio::spawn` 백그라운드 태스크.
  `McpManager`는 `Arc`로 감싸고 `Weak` 참조를 태스크에 전달.
- **이유:** `McpManagerInner` 안에 `JoinHandle`을 두면 idle timer 콜백이
  다시 `tokio::sync::Mutex`를 잡아야 해서 데드락 발생.
- **trade-off:** 약간의 복잡도 증가 vs 데드락 방어. 채널 패턴이 더 안전.

### D5: oxicode-tui에는 MCP 전용 타입 없음
- **결정:** oxicode-tui에는 제네릭 `DashboardWidget`만 배치.
  MCP 뷰 모델(`McpServerInfo` 등)은 oxicode-agent에 정의,
  oxicode-cli에서 제네릭 위젯의 `DashboardSection/Item`으로 변환.
- **이유:** oxicode-tui의 "oxicode-* 의존 없음" 원칙 준수.
  향후 다른 대시보드(extensions, skills 등)에도 재사용 가능.

### D6: McpManager 접근 경로
- **결정:** `ToolRegistry::mcp_manager() -> Option<Arc<McpManager>>` getter 제공.
- **이유:** TUI 오버레이가 `Arc<McpManager>`에 접근해야 함.
  `McpManager`는 `ToolRegistry` 생성 시 `Arc`로 저장되므로 clone만 하면 됨.

### D7: Consent Ask 모드는 Phase 4+로 이연
- **결정:** 초기에는 Allow/Deny만. `/mcp` 대시보드에서 사전 승인 관리.
- **이유:** Ask 모드는 TUI 인라인 프롬프트 UI가 필요하여 복잡도가 높음.
  사전 승인만으로도 대부분의 사용 사례를 커버.

### D8: 비동기 오버레이 액션은 TuiNextAction 패턴
- **결정:** `OverlayAction::McpAction(McpAction)` 반환 →
  `handle_overlay_key()`에서 직접 `await` → 오버레이에 `mark_refresh()`.
- **이유:** 기존 `OverlayAction::SwitchSession` 등도 `handle_overlay_key()`에서
  비동기 처리하므로 동일한 패턴 유지.
- **trade-off:** `handle_overlay_key()`가 비동기이므로 await 가능.
  별도 `TuiNextAction` 큐 없이 직접 처리하는 게 더 단순.

### D9: Transport trait은 Phase 1에서 인터페이스만
- **결정:** Phase 1에서 `McpTransport` trait 정의.
  기존 stdio 코드를 `StdioTransport`으로 래핑 (기능 변화 없음).
- **이유:** Phase 5에서 HTTP/SSE 추가 시 `McpClient` 수정 없이
  새 Transport만 구현하면 됨. 인터페이스 선정의 비용은 낮음.

### D10: 캐시 flush 타이밍
- **결정:** 서버 연결 성공 후 즉시 디스크 저장 (temp + rename).
- **이유:** MCP 서버 연결은 빈번하지 않아 성능 문제 없음.
  크래시 시에도 캐시 손실을 방지.

### D11: MCP Port 승격 보류 (v3 추가)
- **결정:** MCP를 port trait이 아닌 OxicodeBuilder API로 제공.
- **이유:** MCP는 툴(기능)이지 인프라(state, auth)가 아님.
  `coding_tools()` 패턴과 일관성 유지.
  커스텀 백엔드 필요 시에만 Port 12로 승격.

---

## 9. SDK 레이어 — oxicode-sdk를 통한 MCP 제공

> **문제:** 현재 설계는 MCP가 `oxicode-agent` 내부에만 존재하고, oxicode-sdk 컨슈머(oxios 등)가
> MCP를 사용하거나 커스터마이즈할 방법이 없음.
> oxicode-sdk의 "single dependency" 원칙(oxios → oxicode-sdk, oxicode-agent 직접 의존 없음)에 위배.

### 9.1 현황 — SDK에서 MCP가 보이지 않음

```
현재:
  oxicode-agent/src/mcp/*     ← MCP 전체 구현 (여기에만 있음)
  oxicode-sdk/src/lib.rs       ← McpManager re-export 없음
  oxicode-sdk/src/tool_factory ← mcp_tools() 없음
  oxicode-sdk/src/builder.rs   ← OxicodeBuilder에 MCP 설정 API 없음
  oxicode-sdk/src/ports/mod.rs  ← MCP 관련 port trait 없음
```

SDK 컨슈머가 겪는 문제:
1. `OxicodeBuilder::new().with_builtins().build()` → MCP 툴 포함 여부를 제어할 수 없음
2. `coding_tools()` 팩토리에는 MCP가 없음 → MCP를 추가하려면 `ToolRegistry` 직접 조작
3. MCP 설정(서버 목록, lifecycle, direct tools)을 프로그래밍적으로 구성할 수 없음
4. `McpManager`에 접근할 수 없어 상태 조회, 연결 관리 불가

### 9.2 해결 방안 — 3계층 노출

```
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 1: Re-export (oxicode-sdk/src/lib.rs)                           │
│  MCP 핵심 타입과 McpManager를 SDK 표면에 노출                      │
│  → SDK 컨슈머가 MCP를 직접 사용 가능                              │
└─────────────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 2: OxicodeBuilder API (oxicode-sdk/src/builder.rs)                  │
│  MCP 설정을 프로그래밍적으로 주입                                   │
│  → 설정 파일 없이도 MCP 서버 구성 가능                              │
└─────────────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 3: Tool Factory (oxicode-sdk/src/tool_factory.rs)               │
│  `mcp_tools()` 팩토리 함수                                         │
│  → coding_tools(), browsing_tools()와 동일한 패턴                   │
└─────────────────────────────────────────────────────────────────────┘
```

#### Layer 1: Re-export

```rust
// oxicode-sdk/src/lib.rs에 추가

// ── MCP (Model Context Protocol) ──────────────────────────────────────
//
// SDK consumers can use MCP servers alongside built-in tools.
// McpManager is created via `McpManager::spawn()` and injected into
// the tool registry. Use `OxicodeBuilder::with_mcp_config()` for
// programmatic configuration, or let `mcp_tools()` auto-discover
// from standard config files.

pub use oxicode_agent::mcp::{
    McpManager, McpTool, McpDirectTool,
    McpConfig, McpSettings, ServerEntry, LifecycleMode, ToolPrefix,
    McpDashboardData, McpServerInfo, McpConnectionStatus, McpToolInfo,
    ConsentState, DirectToolsConfig,
};
```

#### Layer 2: OxicodeBuilder API

```rust
// oxicode-sdk/src/builder.rs — OxicodeBuilder에 추가

impl OxicodeBuilder {
    /// MCP 서버 설정을 프로그래밍적으로 주입.
    /// 설정 파일 무시하고 이 설정만 사용.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use oxicode_sdk::{OxicodeBuilder, McpConfig, ServerEntry, LifecycleMode};
    ///
    /// let mut mcp = McpConfig::default();
    /// mcp.mcp_servers.insert("my-server".into(), ServerEntry {
    ///     command: Some("npx".into()),
    ///     args: Some(vec!["-y".into(), "@my/mcp-server".into()]),
    ///     lifecycle: Some(LifecycleMode::Lazy),
    ///     ..Default::default()
    /// });
    ///
    /// let oxicode = OxicodeBuilder::new()
    ///     .with_builtins()
    ///     .with_mcp_config(mcp)
    ///     .build();
    /// ```
    pub fn with_mcp_config(mut self, config: McpConfig) -> Self {
        self.mcp_config = Some(config);
        self
    }

    /// MCP 툴을 기본 툴 세트에 포함.
    /// false로 설정하면 mcp 툴이 등록되지 않음.
    /// 기본값: true (with_builtins() 호출 시).
    pub fn with_mcp(mut self, enabled: bool) -> Self {
        self.mcp_enabled = enabled;
        self
    }
}
```

**OxicodeBuilder 필드 추가:**
```rust
pub struct OxicodeBuilder {
    // 기존 필드...

    // NEW
    mcp_config: Option<McpConfig>,
    mcp_enabled: bool,
}
```

**`Oxicode::build()`에서 MCP 연동:**
```rust
impl OxicodeBuilder {
    pub fn build(self) -> Oxicode {
        // ...

        // MCP: 설정이 제공되면 해당 설정으로 McpManager 생성
        // 설정이 없으면 McpManager::spawn()이 표준 파일에서 로드
        let mcp_manager = if self.mcp_enabled {
            if let Some(config) = self.mcp_config {
                Some(McpManager::spawn_with_config(config))
            } else {
                Some(McpManager::spawn())
            }
        } else {
            None
        };

        Oxicode {
            // 기존 필드...
            mcp_manager, // NEW
        }
    }
}
```

**`Oxicode`에 MCP 접근자 추가:**
```rust
impl Oxicode {
    /// McpManager에 접근. MCP가 비활성화되면 None.
    pub fn mcp(&self) -> Option<Arc<McpManager>> {
        self.mcp_manager.clone()
    }
}
```

#### Layer 3: Tool Factory

```rust
// oxicode-sdk/src/tool_factory.rs에 추가

use oxicode_agent::mcp::{McpManager, McpTool, McpDirectTool};

/// MCP 프록시 툴 + Direct tools를 등록한 ToolRegistry 생성.
///
/// 표준 설정 파일에서 MCP 설정을 자동 발견하거나,
/// `config` 파라미터로 프로그래밍 설정을 주입.
///
/// # Example
///
/// ```ignore
/// use oxicode_sdk::mcp_tools;
///
/// // 자동 발견 (표준 설정 파일 사용)
/// let tools = mcp_tools(Path::new("/workspace"), None);
///
/// // 프로그래밍 설정
/// let config = McpConfig { /* ... */ };
/// let tools = mcp_tools(Path::new("/workspace"), Some(config));
/// ```
pub fn mcp_tools(cwd: &Path, config: Option<McpConfig>) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();

    let manager = match config {
        Some(cfg) => McpManager::spawn_with_config(cfg),
        None => McpManager::spawn_from_cwd(cwd),
    };

    // Direct tools 등록 (캐시에서)
    let direct_tools = manager.direct_tools_from_cache();
    for def in &direct_tools {
        registry.register(McpDirectTool::new(manager.clone(), def.clone()));
    }

    // 프록시 툴 등록 (설정에 따라 생략 가능)
    if !manager.should_disable_proxy() {
        registry.register(McpTool::new(manager.clone()));
    }

    // McpManager 저장 (나중에 접근 가능)
    registry.set_mcp_manager(manager);

    Arc::new(registry)
}
```

### 9.3 AgentBuilder 통합

```rust
// oxicode-sdk/src/agent_builder.rs — MCP 지원 추가

impl<'a> AgentBuilder<'a> {
    /// 코딩 툴 + MCP 툴을 모두 등록.
    /// coding_tools() + mcp_tools() 조합.
    pub fn coding_tools_with_mcp(mut self) -> Self {
        self = self.coding_tools();
        // MCP 툴 추가
        if let Some(mcp) = self.oxicode.mcp() {
            let direct_tools = mcp.direct_tools_from_cache();
            for def in &direct_tools {
                self.registry.register(
                    McpDirectTool::new(mcp.clone(), def.clone())
                );
            }
            if !mcp.should_disable_proxy() {
                self.registry.register(McpTool::new(mcp));
            }
        }
        self
    }
}
```

### 9.4 파일 배치

```
ox-sdk/src/
├── lib.rs             # MCP re-export 추가
├── builder.rs         # OxicodeBuilder에 with_mcp_config(), with_mcp() 추가
│                      # Oxicode에 mcp_manager 필드, mcp() 접근자 추가
├── tool_factory.rs    # mcp_tools() 팩토리 추가
└── agent_builder.rs   # coding_tools_with_mcp() 추가
```

### 9.5 사용 예시 — SDK 컨슈머 관점

#### 예시 1: 표준 설정 파일 사용 (oxicode-cli 패턴)

```rust
use oxicode_sdk::OxicodeBuilder;

let oxicode = OxicodeBuilder::new()
    .with_builtins()
    .build(); // MCP 자동 활성화, 표준 설정 파일에서 로드

let agent = oxicode.agent(config)
    .workspace("/project")
    .coding_tools_with_mcp()  // MCP 포함
    .build()?;

// MCP 상태 조회
if let Some(mcp) = oxicode.mcp() {
    let status = mcp.status().await;
    println!("{}", status);
}
```

#### 예시 2: 프로그래밍 설정 (oxios 패턴)

```rust
use oxicode_sdk::{OxicodeBuilder, McpConfig, ServerEntry, LifecycleMode};

let mut mcp_config = McpConfig::default();
mcp_config.mcp_servers.insert("database".into(), ServerEntry {
    command: Some("npx".into()),
    args: Some(vec!["-y".into(), "@my/db-mcp-server".into()]),
    lifecycle: Some(LifecycleMode::KeepAlive),
    ..Default::default()
});

let oxicode = OxicodeBuilder::new()
    .with_builtins()
    .with_mcp_config(mcp_config)  // 커스텀 설정 주입
    .build();

// 커스텀 설정으로 생성된 McpManager에 접근
let mcp = oxicode.mcp().expect("MCP should be enabled");
```

#### 예시 3: MCP 비활성화

```rust
let oxicode = OxicodeBuilder::new()
    .with_builtins()
    .with_mcp(false)  // MCP 툴 등록 안 함
    .build();
```

#### 예시 4: 독립 MCP 툴 팩토리

```rust
use oxicode_sdk::mcp_tools;

// 다른 툴 세트와 조합
let coding = coding_tools(Path::new("/workspace"));
let mcp = mcp_tools(Path::new("/workspace"), None);

let combined = ToolRegistry::new();
combined.extend_from(&coding);
combined.extend_from(&mcp);
```

### 9.6 MCP를 Port로 만들지 않은 이유

**검토:** MCP 설정을 Port 12(`McpConfigProvider`)로 정의할지 고려함.

**결정:** Port가 아닌 **OxicodeBuilder API + Factory**로 제공.

이유:
1. MCP는 인프라(state, auth, config)가 아니라 **에이전트 기능**(툴).
   이미 `AgentTool` 트레이트로 충분히 추상화됨.
2. Port는 "제품이 구현해야 하는 인터페이스"인데, MCP 설정은
   단순히 데이터(`McpConfig`)를 주입하는 것으로 충분.
   커스텀 백엔드(예: 원격 MCP 매니저)가 필요하면 그때 Port로 승격.
3. `tool_factory` 패턴(`coding_tools()`, `browsing_tools()`)과
   일관성 유지. 이 툴들도 Port가 아닌 팩토리로 제공됨.
4. `McpManager` 자체가 이미 충분한 추상화. 내부에서 설정 파일,
  캐시, lifecycle을 관리하므로 별도 port가 필요 없음.

**추후 Port 승격 조건:**
- oxios에서 MCP 서버 프로세스를 커스텀 백엔드(Kubernetes, WASM sandbox 등)에서 실행해야 할 때
- MCP 설정을 중앙 데이터베이스나 원격 API에서 로드해야 할 때
- MCP 연결 상태를 분산 시스템에서 공유해야 할 때

### 9.7 공수 영향

| 작업 | LOC | 공수 |
|------|-----|------|
| `lib.rs` re-export | ~20 | 0.25일 |
| `builder.rs` API | ~80 | 0.5일 |
| `tool_factory.rs` mcp_tools() | ~50 | 0.25일 |
| `agent_builder.rs` 통합 | ~30 | 0.25일 |
| 단위 테스트 | ~100 | 0.5일 |
| **총계** | **~280** | **~1.75일** |

Phase 1 구현 후 Phase 2와 병렬로 진행 가능.

---

## 10. 알려진 리스크 및 추가 고려사항

### 10.1 캐시/설정 tool_prefix 불일치
→ D2에서 해결: 원본 이름만 캐시, prefix는 런타임 계산.

### 10.2 세션 재개 시 MCP 상태
→ `McpManager::spawn()`에서 eager/keep-alive 서버 자동 연결.

### 10.3 동시 연결 경쟁
→ `McpManagerInner::connecting` set으로 중복 연결 방지.

### 10.4 로깅 가시성
→ Phase 2+에서 TUI 대시보드에 이벤트 로그 탭 추가 고려.
  현재는 `tracing::warn!`으로 충분.

### 10.5 McpManager::new() 반환 타입 변경
→ `Self` → `Arc<Self>` 변경은 breaking change.
  `ToolRegistry::with_builtins_cwd()` 내부에서만 사용하므로 영향 범위 제한적.
  `#[deprecated]` 어노테이션으로 마이그레이션 안내.

### 10.6 SDK 컨슈머의 MCP 의존성 (v3 추가)
→ oxicode-sdk가 oxicode-agent의 MCP 모듈을 re-export하면,
  oxicode-agent 의존이 있는 SDK 컨슈머는 MCP 없이도 oxicode-agent를 직접 의존할 수 있음.
  → re-export는 oxicode-sdk의 lib.rs에서 opt-in 아님. `use oxicode_sdk::McpManager`로
  필요할 때만 사용. 불필요한 의존 트리 증가 없음 (oxicode-sdk는 이미 oxicode-agent에 의존).
