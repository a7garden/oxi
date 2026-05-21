# oxi 0.13.0 → oxios를 위한 제안서

**날짜:** 2026-05-16  
**목표:** oxios Agent OS를 위한 완벽한 엔진으로 oxi 재설계  
**작성:** oxios 분석 기반

---

## 1. 현재 oxi 0.13.0 아키텍처 요약

### 크레이트 구조
```
oxi-ai       → LLM 프로바이더 통합 (12개 프로바이더, 스트리밍, 컴팩션)
oxi-agent    → 에이전트 런타임 (AgentLoop, ToolRegistry, 빌트인 도구 13개)
oxi-sdk      → SDK 진입점 (OxiBuilder → AgentBuilder → Agent 플루언트 API)
oxi-store    → CLI 전용 (세션, 설정, Auth 파일)
oxi-tui      → TUI 전용
oxi-cli      → CLI 바이너리
```

### 0.13.0에서 달성된 것
- ✅ `oxi-sdk` 크레이트 신설: `OxiBuilder` + `AgentBuilder` 플루언트 API
- ✅ 인스턴스 격리: 글로벌 static 없이 독립 `Oxi` 인스턴스 생성 가능
- ✅ `ProviderResolver` 트레이트: Agent가 글로벌 레지스트리 대신 주입된 리졸버 사용
- ✅ `ToolContext` 런타임 주입: workspace_dir이 실행 시점에 도구에 전달됨
- ✅ `ClosureTool`: 클로저로 커스텀 도구 생성 가능 (sync/async)
- ✅ `tool_factory`: `coding_tools()`, `readonly_tools()` 프리셋

---

## 2. 현재 oxios의 oxi 사용 방식과 문제점

### oxios → oxi 의존 현황
```
oxios-kernel
  ├── oxi-ai (path dep)     ← EngineProvider로 래핑해서 사용 중
  ├── oxi-agent (path dep)   ← AgentRuntime에서 AgentLoop 직접 사용
  └── oxi-sdk (미사용)       ← 아직 의존하지 않음!
```

### 🔴 문제 1: oxios가 oxi-sdk를 안 쓰고 있다
`AgentRuntime`이 `AgentLoop::new()`를 직접 호출하고, `OxiEngineProvider`가 `oxi_ai::get_provider()` 글로벌 함수를 직접 사용. SDK의 격리 기능을 전혀 활용 못 함.

### 🔴 문제 2: `OxiEngineProvider`가 글로벌 static에 의존
```rust
// oxios-kernel/src/engine.rs
fn create_provider(&self, provider_name: &str) -> Result<Arc<dyn Provider>> {
    oxi_ai::get_provider(provider_name)  // ← 글로벌!
}
```
oxios는 다중 에이전트 OS인데, 모든 에이전트가 하나의 글로벌 프로바이더 레지스트리를 공유.

### 🔴 문제 3: AgentRuntime이 `spawn_blocking`을 사용
```rust
// AgentRuntime::execute()
let result = tokio::task::spawn_blocking(move || {
    run_agent_loop(ctx)  // ← 내부에서 tokio::runtime::Handle::block_on()
}).await;
```
AgentLoop가 `!Send` 문제로 인해 blocking 스레드에서 실행. 진짜 원인은 AgentLoop 내부의 `Box<dyn Future>`가 Send 바운드가 없는 것.

### 🟡 문제 4: AgentRuntime이 매 실행마다 AgentLoop를 새로 만듦
```rust
// AgentRuntime::run_agent_loop()
let agent_loop = AgentLoop::new(provider, loop_config, tools, state);
```
에이전트 상태(state)를 AgentRuntime이 아닌 AgentLoop 생성 시마다 새로 만들면, 세션 컨티뉴에이션이 불가능.

### 🟡 문제 5: oxi-sdk의 AgentBuilder가 CSpace/커널 도구를 모름
SDK의 `coding_tools()`는 read/write/edit/ls만 제공. oxios의 exec, mcp, memory, browser, a2a 등 커널 도구는 SDK에 없고, AgentRuntime에서 직접 ToolRegistry에 수동 등록.

---

## 3. oxios를 위한 oxi 재설계 / 기능 제안

### 🏗️ 제안 A: oxi-sdk를 oxios의 공식 엔진 인터페이스로 승격

**현재:** oxios-kernel이 oxi-ai, oxi-agent를 직접 의존  
**제안:** oxios-kernel이 **oxi-sdk만** 의존

```toml
# oxios-kernel/Cargo.toml (변경 후)
[dependencies]
oxi-sdk = { path = "../../oxi/oxi-sdk" }
# oxi-ai, oxi-agent는 oxi-sdk를 통해 간접 의존
```

**이유:**
- `Oxi` 엔진이 이미 `ProviderResolver`를 구현 → `EngineProvider` 트레이트 제거 가능
- SDK의 `OxiBuilder::new().with_builtins().build()` → oxios의 엔진 초기화 한 줄로 해결
- 버전 관리 단순화: oxi-sdk 버전만 맞추면 됨

### 🏗️ 제안 B: `oxi-sdk`에 KernelToolBridge 트레이트 추가

oxios의 커널 도구(exec, mcp, memory, browser, persona 등)를 SDK 레벨에서 플러그 가능하게:

```rust
// oxi-sdk에 제안할 트레이트
pub trait KernelToolProvider: Send + Sync {
    /// 커널 컨텍스트에 맞는 도구들을 ToolRegistry에 등록
    fn register_kernel_tools(
        &self,
        registry: &ToolRegistry,
        context: &KernelToolContext,
    );
    
    /// 이 프로바이더가 제공하는 도구 이름 목록
    fn tool_names(&self) -> Vec<&str>;
}

pub struct KernelToolContext {
    pub workspace_dir: PathBuf,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub permissions: Vec<String>,  // CSpace 기반
}
```

oxios는 이 트레이트를 구현해서 `AgentBuilder`에 주입:
```rust
let agent = oxi.agent(config)
    .workspace("/workspace")
    .kernel_tools(oxios_kernel_bridge)  // ← 새 메서드
    .build()?;
```

### 🏗️ 제안 C: AgentLoop의 `Send` 문제 해결

AgentLoop 내부의 `Box<dyn Future>` → `Box<dyn Future + Send>` 변경. 이것만으로 oxios에서 `spawn_blocking` 없이 직접 `tokio::spawn` 가능.

```rust
// oxi-agent/src/agent_loop/tool_exec.rs (변경 제안)
// Before:
type ToolFuture = Box<dyn Future<Output = ...>>;
// After:
type ToolFuture = Box<dyn Future<Output = ...> + Send>;
```

영향: `AgentTool::execute()`는 이미 `async fn`이므로 `Send` future를 반환. 문제는 내부 래핑에서 `Send` 바운드가 누락된 곳.

### 🏗️ 제안 D: Agent 재사용 (Stateful Agent)

현재 AgentRuntime은 매 seed마다 AgentLoop를 새로 만듦. 대신:

```rust
// oxi-sdk 제안: Agent에 세션 컨티뉴에이션 추가
impl Agent {
    /// 기존 state로 새 프롬프트 실행 (세션 유지)
    pub async fn continue_with(&self, prompt: String) -> Result<(Response, Vec<AgentEvent>)>;
    
    /// 상태를 직렬화하여 저장/복원
    pub fn export_state(&self) -> AgentState;
    pub fn import_state(&self, state: AgentState);
}
```

oxios의 Supervisor는 Agent 인스턴스를 풀로 관리:
```rust
// oxios-kernel/supervisor (개념)
struct BasicSupervisor {
    agents: HashMap<AgentId, Arc<Agent>>,  // ← 재사용
}
```

### 🏗️ 제안 E: oxi-sdk에 EventStream 타입 추가

oxios의 Gateway는 Agent 이벤트를 WebSocket/SSE로 스트리밍해야 함. 현재 `std::sync::mpsc::channel` 기반은 tokio 생태계와 안 맞음.

```rust
// oxi-sdk 제안: tokio 채널 기반 런타임
impl Agent {
    /// tokio 채널로 이벤트 스트리밍 (WebSocket/SSE 친화적)
    pub async fn run_stream(
        &self, 
        prompt: String,
    ) -> Result<(tokio::sync::mpsc::Receiver<AgentEvent>, JoinHandle<Result<Response>>)>;
}
```

### 🏗️ 제안 F: oxi-ai에 `ProviderPool` 추가

oxios는 다중 에이전트가 동시에 같은 프로바이더를 사용. Rate limiting, API 풀링이 필요:

```rust
// oxi-ai 제안
pub struct ProviderPool {
    provider: Arc<dyn Provider>,
    semaphore: Arc<Semaphore>,  // concurrent request 제한
    rate_limiter: Arc<RateLimiter>,
}

impl Provider for ProviderPool {
    async fn stream(...) -> ... {
        let _permit = self.semaphore.acquire().await?;
        self.rate_limiter.wait_for_quota().await?;
        self.provider.stream(model, context, options).await
    }
}
```

SDK에서:
```rust
let oxi = OxiBuilder::new()
    .with_builtins()
    .with_provider_pool("anthropic", 5, RateLimit::rpm(60))  // ← 새 API
    .build();
```

### 🏗️ 제안 G: oxi-agent에 Structured Output / Schema Validation 추가

oxios의 Ouroboros 프로토콜은 Seed의 결과를 구조화된 데이터로 받아야 함. 현재는 텍스트만:

```rust
// oxi-agent 제안
pub struct AgentConfig {
    // ... 기존 필드
    /// 출력 스키마 (JSON Schema). 설정 시, 마지막 assistant 메시지를 스키마에 맞게 파싱
    pub output_schema: Option<serde_json::Value>,
    /// 구조화된 출력 추출 모드
    pub output_mode: OutputMode,
}

pub enum OutputMode {
    /// 텍스트 그대로
    Text,
    /// 마지막 메시지에서 JSON 추출
    Json,
    /// JSON Schema로 검증
    ValidatedJson,
}
```

### 🏗️ 제안 H: 다중 에이전트 오케스트레이션 프리미티브

oxios의 핵심은 다중 에이전트인데, oxi에는 에이전트 간 협업 기본 블록이 없음:

```rust
// oxi-sdk 제안: AgentGroup
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
}

pub enum GroupStrategy {
    /// 순차 실행, 이전 결과를 다음 에이전트에 전달
    Pipeline,
    /// 병렬 실행, 결과를 취합
    Parallel,
    /// 리더가 작업 분배
    Orchestrated { leader: usize },
}

impl AgentGroup {
    pub async fn run(&self, prompt: String) -> Result<GroupResult>;
}
```

---

## 4. oxios 당장 해야 할 업데이트 (우선순위)

| 우선순위 | 작업 | 설명 |
|----------|------|------|
| **P0** | oxios-kernel → oxi-sdk 의존 전환 | `OxiEngineProvider` 제거, `Oxi` 엔진 직접 사용 |
| **P0** | AgentRuntime → SDK AgentBuilder 사용 | `AgentLoop::new()` 대신 `oxi.agent(config).build()` |
| **P0** | `spawn_blocking` 제거 | AgentLoop `Send` 문제 해결 후 직접 `tokio::spawn` |
| **P1** | 커널 도구 SDK 플러그인 | exec, mcp, memory 등을 `KernelToolProvider` 트레이트로 |
| **P1** | 이벤트 스트림 tokio 채널 | Gateway WebSocket을 위한 `run_stream()` |
| **P2** | ProviderPool | 다중 에이전트 동시 API 호출 관리 |
| **P2** | AgentState 직렬화/복원 | 세션 컨티뉴에이션 |

---

## 5. 요약: oxi가 Agent OS 엔진으로 완벽해지려면

| 측면 | 현재 상태 | 목표 |
|------|----------|------|
| **격리** | ✅ 인스턴스 격리 달성 | oxios에서 실제로 사용 |
| **도구 주입** | ✅ ToolContext 런타임 | 커널 도구 브릿지 트레이트 |
| **동시성** | ❌ `!Send` → `spawn_blocking` | `Send` future → `tokio::spawn` |
| **이벤트** | 🟡 `std::sync::mpsc` | `tokio::sync` 채널 옵션 |
| **다중 에이전트** | ❌ 없음 | AgentGroup, ProviderPool |
| **구조화 출력** | ❌ 없음 | JSON Schema validation |
| **상태 관리** | 🟡 AgentState 존재 | export/import, 세션 컨티뉴에이션 |

**핵심 메시지:** oxi 0.13.0의 SDK는 훌륭한 첫걸음이고, 설계 방향(인스턴스 격리, 빌더 패턴, ProviderResolver)은 oxios와 완벽하게 맞음. 이제 이 SDK를 **oxios가 실제로 사용**하도록 연결하고, **다중 에이전트 OS에 필요한 프리미티브** (Send 안전, ProviderPool, EventStream, AgentGroup)를 추가하면, oxi는 단순한 코딩 에이전트 런타임을 넘어 **범용 Agent OS 엔진**이 됨.