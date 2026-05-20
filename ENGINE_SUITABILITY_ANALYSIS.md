# oxi 엔진 적합성 분석 (oxios 관점)

**날짜:** 2026-05-16  
**대상:** oxi v0.12.0 → oxios(Agent OS)에서 엔진으로 사용  
**비교:** pi → OpenClaw(유사 구조)

---

## 현재 oxios의 oxi 사용 방식

```
oxios (Agent OS)
├── oxios-kernel ← oxi-ai + oxi-agent (직접 의존)
├── oxios-ouroboros ← oxi-ai (직접 의존)
├── oxios-gateway ← oxios-kernel
├── oxios-cli/web/telegram ← oxios-gateway
└── oxi-store, oxi-tui, oxi-cli ← 사용 안 함
```

oxios는 **oxi-ai**와 **oxi-agent**만 직접 사용합니다. `oxi-store`, `oxi-tui`, `oxi-cli`는 사용하지 않습니다.

---

## ✅ 잘된 점

### 1. 크레이트 분리가 올바름
oxios가 필요한 레이어(oxi-ai, oxi-agent)만 선택적으로 의존할 수 있습니다. CLI/TUI는 가져오지 않습니다.

### 2. `EngineProvider` 트레이트 추상화
```rust
// oxios-kernel/src/engine.rs
pub trait EngineProvider: Send + Sync {
    fn create_provider(&self, provider_name: &str) -> Result<Arc<dyn Provider>>;
    fn resolve_model(&self, model_id: &str) -> Result<Model>;
    fn default_model_id(&self) -> &str;
}
```
oxi-ai의 `get_provider()`/`get_model()`을 래핑해서 교체 가능하게 만들었습니다. 테스트용 mock도 가능합니다.

### 3. `AgentTool` 트레이트가 확장 가능함
```rust
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(...) -> Result<AgentToolResult, ToolError>;
}
```
oxios가 커널 도구(exec, memory, persona, space, cron 등)를 이 트레이트로 구현해서 AgentLoop에 주입합니다. 완벽한 확장 패턴입니다.

### 4. `ToolRegistry`가 런타임 구성 가능
```rust
let registry = ToolRegistry::new();
register_tools_from_cspace(&registry, &kernel_handle, &cspace, ...);
```
oxios는 CSpace(역량 공간) 기반으로 도구를 동적으로 구성합니다. 이게 가능한 건 `ToolRegistry::new()`가 빈 레지스트리를 만들어주기 때문입니다.

---

## 🔴 Critical — 반드시 고쳐야 할 문제

### 1. CWD가 프로세스 전역 (`std::env::current_dir()`)

**현재:** oxi-agent의 모든 파일 도구(read, write, edit, grep, find, ls)가 `std::env::current_dir()`로 작업 디렉토리를 결정합니다.

```rust
// oxi-agent/src/tools/edit.rs
let guard = PathGuard::new(&std::env::current_dir().unwrap_or_else(|_| ...));
```

**문제:** oxios는 **다중 에이전트를 동시에 실행**합니다. 각 에이전트가 다른 workspace를 가져야 하지만, `set_current_dir()`은 프로세스 전역이므로 레이스 컨디션이 발생합니다.

**oxios의 현재 대응:** `WORKSPACE_MUTEX`로 전체 에이전트 실행을 직렬화 (!)
```rust
// oxios agent_runtime.rs
static WORKSPACE_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
let _workspace_guard = WORKSPACE_MUTEX.lock();
```
→ **에이전트를 한 번에 하나만 실행할 수밖에 없습니다.** Agent OS인데 병렬 실행이 안 됩니다.

**해결:** `AgentLoopConfig`에 `workspace_dir: Option<PathBuf>` 추가. 모든 파일 도구가 `config.workspace_dir`을 우선 사용:

```rust
// 제안
pub struct AgentLoopConfig {
    pub workspace_dir: Option<PathBuf>,  // 추가!
    ...
}

// 각 도구에서
let cwd = config.workspace_dir.as_deref()
    .unwrap_or_else(|| std::env::current_dir().unwrap());
```

### 2. `AgentLoop`이 `!Send` — `spawn_blocking` 강제

```rust
// oxios agent_runtime.rs
tokio::task::spawn_blocking(move || {
    run_agent_loop(ctx)  // 내부에서 Handle::block_on 사용
}).await
```

**원인:** `AgentLoop` 내부의 `RwLock<Vec<Message>>` (parking_lot), 훅 클로저 등이 `Send`를 만족하지 않아 tokio 태스크로 직접 spawn할 수 없습니다.

**문제:**
- `spawn_blocking`은 별도 스레드를 차지 → 스레드 풀 고갈 가능
- `Handle::block_on` inside `spawn_blocking` → 중첩 런타임 위험
- 동시 에이전트 수가 스레드 풀 크기(기본 512)로 제한

**해결:** `AgentLoop`을 `Send` 안전하게 만들거나, LocalSet 패턴을 지원:
```rust
// Option A: AgentLoop을 Send로
// parking_lot::RwLock은 Send이므로, 클로저만 Send로 만들면 됨
// 훅 타입을 Arc<dyn Fn(...) + Send + Sync>로 이미 선언됨 — 내부 구현 확인 필요

// Option B: LocalSet API 제공
pub fn run_local<'a>(self, prompt: String, emit: ...) -> LocalBoxFuture<'a, Result<...>>
```

### 3. 전역 상태가 하드코딩됨

```rust
// oxi-ai/src/provider_registry.rs — 글로벌 레지스트리
pub fn register_provider(name: &str, provider: impl Provider + 'static) { ... }
pub fn get_provider(name: &str) -> Option<Arc<dyn Provider>> { ... }
```

**문제:** `register_provider`가 글로벌 HashMap에 씁니다. 테스트 격리가 안 되고, oxios 인스턴스 간에 상태가 공유됩니다.

**해결:** `ProviderRegistry`를 인스턴스화 가능하게 만들어 `EngineProvider`에 주입:
```rust
// 제안
pub struct ProviderRegistry { ... }  // 이미 내부에 있음
impl ProviderRegistry {
    pub fn new() -> Self { ... }
    pub fn register(&mut self, name: &str, provider: ...) { ... }
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> { ... }
}
```

---

## 🟡 High — 개선하면 좋은 문제

### 4. `oxi-store`의 세션 관리를 oxios가 재구현 중

oxios는 자체적으로 `StateStore`, `PersonaStore`, `MemoryManager`를 갖고 있습니다. `oxi-store`의 `SessionManager`, `Settings`는 CLI 전용이라 oxios가 사용하지 않습니다.

→ 이건 분리가 잘 된 겁니다. 다만 `oxi-store`의 `ModelRegistry`는 oxi-ai와 결합되어 있어 oxios가 별도 모델 관리를 해야 합니다.

### 5. `AgentLoopConfig`에 workspace_dir이 없음

위 Critical #1과 연관. 현재 `AgentLoopConfig`에는 workspace 정보가 없습니다. oxios는 `AgentRuntimeConfig`에 별도로 `project_paths`와 `workspace_dir`을 정의했지만, 이걸 oxi-agent의 도구들에게 전달할 방법이 없습니다.

### 6. 에이전트 간 통신(A2A) 미지원

oxios는 A2A(Agent-to-Agent) 통신을 커널 수준에서 지원하려 하지만, oxi-agent의 `AgentLoop`은 단일 에이전트 실행만 지원합니다. 메시지 패싱, 스티어링, 인터럽트 등의 프리미티브가 없습니다.

**부분적 해결:** `steering_queue`가 있긴 함:
```rust
pub struct AgentLoop {
    steering_queue: RwLock<Vec<Message>>,
    ...
}
```
하지만 공개 API가 아닙니다.

### 7. `oxi-cli`이 엔진 래퍼로 부적합

현재 `oxi-cli`의 `lib.rs`는 `App` 구조체를 노출하지만:
- `App::new()`가 `Settings::load()`를 호출 (파일 시스템 결합)
- `App`이 `SkillManager`, `WasmExtensionManager` 등 CLI 전용 컴포넌트를 포함
- oxios는 `oxi-cli`를 아예 사용하지 않음 (올바른 선택)

→ **결론:** `oxi-cli`은 엔진 래퍼가 아니라 순수 CLI 앱입니다. 엔진 역할은 `oxi-ai` + `oxi-agent`가 담당합니다.

---

## 🟢 Low — 있으면 좋은 것

### 8. 라이프사이클 훅(Hooks) API

```rust
// oxi-agent 이미 지원 중
pub struct AgentHooks {
    pub before_tool_call: Option<BeforeToolCallHook>,
    pub after_tool_call: Option<AfterToolCallHook>,
    pub should_stop_after_turn: Option<ShouldStopAfterTurnHook>,
}
```
이건 잘 되어 있습니다. oxios가 이 훅들을 사용할 수 있습니다.

### 9. Compaction 커스터마이징

`CompactionStrategy`를 `oxi-ai`에서 정의하고 `AgentLoopConfig`에서 설정 가능합니다. oxios는 자체 메모리 시스템과 연동하려면 compaction 이벤트를 후킹할 수 있어야 합니다.

현재 `AgentEvent::Compaction`이 있어서 가능합니다.

---

## 📋 개선 제안 우선순위

| 순위 | 항목 | 영향 | 난이도 |
|------|------|------|--------|
| **P0** | `workspace_dir`를 AgentLoopConfig에 추가 | 병렬 에이전트 가능 | 중간 |
| **P0** | 글로벌 ProviderRegistry → 인스턴스화 | 테스트 격리, 멀티테넌시 | 중간 |
| **P1** | AgentLoop을 Send 안전하게 | spawn_blocking 제거 | 높음 |
| **P1** | steering_queue 공개 API화 | 에이전트 간 스티어링 | 낮음 |
| **P2** | oxi-agent에 "engine" 퍼사드 추가 | 진입 장벽 감소 | 낮음 |

---

## pi vs oxi 비교 (엔진 적합성)

| 관점 | pi → OpenClaw | oxi → oxios |
|------|--------------|-------------|
| **엔진 크레이트** | 명확히 분리됨 | oxi-ai + oxi-agent이 엔진 역할 (명시적 분리 없음) |
| **전역 상태** | 인스턴스화된 컨테이너 | 글로벌 함수 (register_provider 등) |
| **워크스페이스** | 컨텍스트 객체로 전달 | CWD에 의존 (프로세스 전역) |
| **멀티 에이전트** | 지원 | 단일 에이전트만 (WORKSPACE_MUTEX) |
| **도구 확장** | 트레이트 기반 (良好) | 트레이트 기반 (良好) |
| **설정** | 인스턴스 주입 | 파일 시스템 결합 (Settings::load) |

**핵심 차이:** pi는 처음부터 SDK/엔진으로 설계되었습니다. oxi는 CLI 도구로 시작했고 엔진 API는 사후에 노출되었습니다.

---

*이 분석은 oxi v0.12.0 소스코드와 oxios v0.2.0-alpha 소스코드를 기반으로 작성되었습니다.*
