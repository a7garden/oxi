# oxi SDK 설계안

**날짜:** 2026-05-16  
**참조:** pi (`@earendil-works/pi-ai`, `@earendil-works/pi-agent-core`, `@earendil-works/pi-coding-agent`)  
**목표:** oxi를 CLI 도구에서 범용 AI 에이전트 SDK로 전환

---

## 1. 현재 구조 vs 목표 구조

### 현재 (CLI 도구)

```
oxi-ai       ← 전역 static (CUSTOM_PROVIDERS, MODELS, DYNAMIC_MODELS)
oxi-agent    ← AgentLoop::new(provider, config, tools, state)
oxi-store    ← CLI 전용 (Settings::load, SessionManager, AuthStorage)
oxi-tui      ← TUI 전용
oxi-cli      ← 바이너리 + App 구조체
```

**문제:** 모든 레지스트리가 전역 static. 인스턴스화 불가. 테스트 격리 불가. 다중 에이전트 불가.

### 목표 (SDK)

```
oxi-ai       ← OxiAi 인스턴스 (ProviderRegistry + ModelRegistry + HttpClient)
oxi-agent    ← AgentLoop::new(provider, config, tools, state, workspace)
oxi-store    ← CLI 전용 (그대로)
oxi-sdk      ← [NEW] 퍼사드: OxiBuilder → OxiEngine
oxi-tui      ← TUI 전용 (그대로)
oxi-cli      ← 바이너리 (oxi-sdk 사용)
```

---

## 2. pi가 푼 문제와 oxi의 대응

### 2.1 전역 레지스트리 → 인스턴스화

**pi의 해결:**
```typescript
// pi-ai: 글로벌이지만 sourceId로 그룹화 → unregister 가능
registerApiProvider(provider, sourceId);
unregisterApiProviders(sourceId);

// pi-coding-agent: AuthStorage, ModelRegistry 모두 create() 패턴
const authStorage = AuthStorage.create(path);
const modelRegistry = ModelRegistry.create(authStorage, modelsPath);
```

**oxi의 설계:** 인스턴스 기반 레지스트리 + 글로벌 편의 함수 유지

```rust
// oxi-ai/src/provider_registry.rs (새 설계)

/// 인스턴스화 가능한 프로바이더 레지스트리
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self { ... }
    pub fn register(&self, name: &str, provider: impl Provider + 'static) { ... }
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> { ... }
    pub fn remove(&self, name: &str) { ... }
}

/// 모델 레지스트리
pub struct ModelRegistry {
    static_models: HashMap<String, Model>,  // 빌트인
    dynamic_models: RwLock<HashMap<String, Model>>,  // 런타임 등록
}

impl ModelRegistry {
    pub fn new() -> Self { ... }
    pub fn register(&self, model: Model) { ... }
    pub fn lookup(&self, provider: &str, model_id: &str) -> Option<Model> { ... }
}

/// 글로벌 편의 함수 (하위호환) — 내부적으로 GLOBAL_INSTANCE 사용
pub fn register_provider(name: &str, provider: impl Provider + 'static) {
    global_registry().register(name, provider)
}
```

### 2.2 CWD 전역 의존 → workspace_dir 주입

**pi의 해결:**
```typescript
// pi-coding-agent/core/tools/index.ts
export function createCodingTools(options: {
    cwd: string;           // ← 명시적 workspace
    bashOperations?: ...;
    fileMutationQueue?: ...;
}): ToolDefinition[]
```
모든 도구가 `cwd`를 파라미터로 받음. `process.cwd()`에 의존하지 않음.

**oxi의 설계:**

```rust
// oxi-agent/src/agent_loop/config.rs
pub struct AgentLoopConfig {
    pub workspace_dir: Option<PathBuf>,  // ← 추가
    // ... 기존 필드
}

// oxi-agent/src/tools/*.rs
// PathGuard가 config.workspace_dir을 우선 사용하도록 수정
```

또는 더 나은 방법 — 도구에 workspace를 주입:

```rust
/// 도구 실행 컨텍스트
pub struct ToolContext {
    pub workspace: PathBuf,
    pub session_id: Option<String>,
    pub state: SharedState,
}

// AgentTool 트레이트에 컨텍스트 전달
#[async_trait]
pub trait AgentTool: Send + Sync {
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        shutdown: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,  // ← 추가
    ) -> Result<AgentToolResult, ToolError>;
}
```

### 2.3 SDK 퍼사드 — `OxiBuilder`

**pi의 해결:**
```typescript
// pi-coding-agent/core/sdk.ts
const { session } = await createAgentSession({
    cwd: "/path/to/project",
    model: getModel('anthropic', 'claude-opus-4-5'),
    tools: ["read", "bash", "edit", "write"],
    customTools: [myTool],
    authStorage: AuthStorage.create(path),
    sessionManager: SessionManager.inMemory(),
});
```

**oxi의 설계:**

```rust
// oxi-sdk/src/lib.rs (새 크레이트)

/// oxi SDK 진입점
pub struct Oxi {
    providers: Arc<ProviderRegistry>,
    models: Arc<ModelRegistry>,
    http: Arc<reqwest::Client>,
}

/// 빌더 패턴으로 Oxi 인스턴스 생성
pub struct OxiBuilder {
    providers: ProviderRegistry,
    models: ModelRegistry,
    http: Option<reqwest::Client>,
}

impl OxiBuilder {
    pub fn new() -> Self { ... }

    /// 기본 빌트인 프로바이더 등록
    pub fn with_builtins(self) -> Self { ... }

    /// 커스텀 프로바이더 추가
    pub fn provider(self, name: &str, provider: impl Provider) -> Self { ... }

    /// 커스텀 모델 추가
    pub fn model(self, model: Model) -> Self { ... }

    /// HTTP 클라이언트 커스터마이징
    pub fn http_client(self, client: reqwest::Client) -> Self { ... }

    /// 인스턴스 빌드
    pub fn build(self) -> Oxi { ... }
}

impl Oxi {
    // ── 에이전트 생성 ──

    /// 단일 에이전트 생성
    pub fn agent(&self, config: AgentConfig) -> AgentBuilder<'_> {
        AgentBuilder::new(self, config)
    }

    /// 빈 도구 레지스트리 생성
    pub fn tool_registry(&self) -> ToolRegistry { ... }

    /// 빌트인 코딩 도구 세트 생성 (cwd 지정)
    pub fn coding_tools(&self, cwd: &Path) -> Arc<ToolRegistry> { ... }

    /// 읽기 전용 도구 세트 생성
    pub fn readonly_tools(&self, cwd: &Path) -> Arc<ToolRegistry> { ... }

    // ── 모델/프로바이더 접근 ──

    pub fn resolve_model(&self, model_id: &str) -> Result<Model> { ... }
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> { ... }
    pub fn providers(&self) -> &ProviderRegistry { ... }
    pub fn models(&self) -> &ModelRegistry { ... }
}
```

### 2.4 AgentBuilder — 에이전트 플루언트 API

```rust
pub struct AgentBuilder<'a> {
    oxi: &'a Oxi,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace: Option<PathBuf>,
    system_prompt: Option<String>,
    hooks: Option<AgentHooks>,
}

impl<'a> AgentBuilder<'a> {
    /// 워크스페이스 디렉토리 (파일 도구의 기준 경로)
    pub fn workspace(mut self, dir: impl Into<PathBuf>) -> Self { ... }

    /// 시스템 프롬프트
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self { ... }

    /// 도구 등록
    pub fn tool(mut self, tool: impl AgentTool + 'static) -> Self { ... }

    /// 빌트인 코딩 도구 일괄 등록
    pub fn coding_tools(mut self) -> Self { ... }

    /// 읽기 전용 도구 일괄 등록
    pub fn readonly_tools(mut self) -> Self { ... }

    /// 커스텀 도구 등록 (클로저)
    pub fn custom_tool(
        mut self,
        name: &str,
        description: &str,
        schema: Value,
        handler: impl Fn(Value) -> Result<AgentToolResult> + Send + Sync + 'static,
    ) -> Self { ... }

    /// 훅 등록
    pub fn before_tool_call(mut self, hook: BeforeToolCallHook) -> Self { ... }
    pub fn after_tool_call(mut self, hook: AfterToolCallHook) -> Self { ... }
    pub fn on_event(mut self, handler: impl Fn(AgentEvent) + Send + Sync + 'static) -> Self { ... }

    /// 에이전트 빌드
    pub fn build(self) -> Result<Agent> { ... }
}
```

---

## 3. 사용 예시

### 3.1 최소 사용 (oxios 기준)

```rust
use oxi_sdk::{OxiBuilder, AgentConfig, AgentEvent};

#[tokio::main]
async fn main() -> Result<()> {
    // 1. 엔진 생성
    let oxi = OxiBuilder::new()
        .with_builtins()
        .build();

    // 2. 에이전트 생성
    let agent = oxi.agent(AgentConfig {
        model_id: "zai/glm-5.1".into(),
        max_iterations: 20,
        ..Default::default()
    })
    .workspace("/workspace/agent-1")
    .system_prompt("You are an autonomous agent.")
    .coding_tools()
    .build()?;

    // 3. 실행
    let result = agent.run("Build a REST API server".into(), |event| {
        match event {
            AgentEvent::ToolExecutionEnd { tool_name, .. } => {
                println!("Tool: {}", tool_name);
            }
            AgentEvent::AgentEnd { .. } => {}
            _ => {}
        }
    }).await?;

    println!("Result: {}", result);
    Ok(())
}
```

### 3.2 다중 에이전트 (oxios)

```rust
let oxi = OxiBuilder::new().with_builtins().build();

// 에이전트 A — 웹 개발
let agent_a = oxi.agent(config_a.clone())
    .workspace("/workspace/frontend")
    .coding_tools()
    .build()?;

// 에이전트 B — 백엔드 (동시 실행 가능!)
let agent_b = oxi.agent(config_b.clone())
    .workspace("/workspace/backend")
    .coding_tools()
    .build()?;

// 병렬 실행 — CWD 충돌 없음
let (result_a, result_b) = tokio::join!(
    agent_a.run("Build React app".into(), |e| {}),
    agent_b.run("Build Rust API".into(), |e| {}),
);
```

### 3.3 커스텀 도구만 사용 (oxios 커널)

```rust
let oxi = OxiBuilder::new().with_builtins().build();

let agent = oxi.agent(AgentConfig {
    model_id: "anthropic/claude-sonnet-4-20250514".into(),
    ..Default::default()
})
.workspace("/workspace")
.custom_tool(
    "memory_recall",
    "Search long-term memory",
    json!({"type": "object", "properties": {"query": {"type": "string"}}}),
    |params| {
        let query = params["query"].as_str().unwrap();
        Ok(AgentToolResult::success(format!("Recalled: {}", query)))
    },
)
.custom_tool("exec", "Execute commands", schema, exec_handler)
.build()?;
```

### 3.4 CLI에서 사용 (하위호환)

```rust
// oxi-cli/src/main.rs

// 글로벌 편의 함수는 그대로 작동
oxi_ai::register_provider("my-provider", MyProvider::new());

// 또는 SDK 사용
let oxi = OxiBuilder::new()
    .with_builtins()
    .provider("custom", custom_provider)
    .build();
```

### 3.5 테스트 (격리)

```rust
#[test]
fn test_custom_provider() {
    // 완전 격리된 인스턴스 — 전역 상태 오염 없음
    let oxi = OxiBuilder::new()
        .provider("mock", MockProvider::new())
        .model(Model { id: "mock/test".into(), provider: "mock".into(), .. })
        .build();

    let agent = oxi.agent(AgentConfig {
        model_id: "mock/test".into(),
        max_iterations: 1,
        ..Default::default()
    })
    .workspace(tempdir.path())
    .build()
    .unwrap();

    let result = rt.block_on(agent.run("test".into(), |_| {})).unwrap();
    assert!(result.contains("mock response"));
}
```

---

## 4. 새 크레이트: `oxi-sdk`

```
oxi-sdk/
├── Cargo.toml
├── src/
│   ├── lib.rs           # pub mod + re-exports
│   ├── builder.rs       # OxiBuilder, Oxi
│   ├── agent_builder.rs # AgentBuilder
│   ├── tool_factory.rs  # coding_tools(), readonly_tools()
│   └── prelude.rs       # 자주 쓰는 타입 모음
```

```toml
# oxi-sdk/Cargo.toml
[package]
name = "oxi-sdk"
version = "0.12.0"
edition = "2021"
description = "oxi AI agent SDK — programmatic API for building AI agents"

[dependencies]
oxi-ai = { path = "../oxi-ai" }
oxi-agent = { path = "../oxi-agent" }
anyhow = "1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

### re-export 구조

```rust
// oxi-sdk/src/lib.rs

// oxi-ai 핵심 타입
pub use oxi_ai::{
    Provider, Model, Context, Message, ContentBlock,
    ProviderEvent, StreamOptions, CompactionStrategy,
    ProviderError, Api, Cost, InputModality,
};

// oxi-agent 핵심 타입
pub use oxi_agent::{
    Agent, AgentLoop, AgentLoopConfig, AgentConfig,
    AgentEvent, AgentState, SharedState,
    ToolRegistry, AgentTool, AgentToolResult, ToolError,
    AgentHooks, BeforeToolCallHook, AfterToolCallHook,
    ToolExecutionMode, AgentError,
};

// SDK 자체
mod builder;
mod agent_builder;
mod tool_factory;

pub use builder::{Oxi, OxiBuilder};
pub use agent_builder::AgentBuilder;
pub use tool_factory::ToolFactory;
```

---

## 5. 기존 코드 변경 계획

### Phase 1: oxi-ai 인스턴스화 (Breaking change 없음)

| 파일 | 변경 |
|------|------|
| `oxi-ai/src/provider_registry.rs` | `ProviderRegistry` 구조체 추가. 글로벌 함수는 `global_registry()`에 위임 |
| `oxi-ai/src/model_registry.rs` | `ModelRegistry` 구조체 추가. 글로벌 함수는 `global_registry()`에 위임 |
| `oxi-ai/src/providers/mod.rs` | `shared_client()` 대신 `OxiAi::http()` 사용 가능하게 |

**하위호환:** 기존 `register_provider()`, `get_provider()`, `register_model()` 글로벌 함수는 그대로 작동. 내부적으로 글로벌 `ProviderRegistry` 인스턴스 사용.

### Phase 2: workspace_dir 주입

| 파일 | 변경 |
|------|------|
| `oxi-agent/src/agent_loop/config.rs` | `workspace_dir: Option<PathBuf>` 필드 추가 |
| `oxi-agent/src/tools/*.rs` | `std::env::current_dir()` → `workspace_dir` 우선 |
| `oxi-agent/src/tools/path_security.rs` | `PathGuard::with_root()` 추가 |

**하위호환:** `workspace_dir: None`이면 기존 동작 (`current_dir()`) 유지.

### Phase 3: oxi-sdk 크레이트 생성

| 파일 | 설명 |
|------|------|
| `oxi-sdk/` | 새 크레이트 |
| `oxi-sdk/src/builder.rs` | `OxiBuilder`, `Oxi` |
| `oxi-sdk/src/agent_builder.rs` | `AgentBuilder` |
| `oxi-sdk/src/tool_factory.rs` | 도구 팩토리 |

### Phase 4: oxi-cli 마이그레이션

| 파일 | 변경 |
|------|------|
| `oxi-cli/src/main.rs` | `OxiBuilder` 사용 |
| `oxi-cli/src/lib.rs` | `App` 내부를 `Oxi` 인스턴스로 교체 |

---

## 6. pi와의 비교 — 설계 결정

| 관점 | pi | oxi (제안) | 이유 |
|------|-----|-----------|------|
| **레지스트리** | 글로벌 + sourceId 그룹화 | 인스턴스 + 글로벌 편의 함수 | Rust의 소유권 모델에 더 적합 |
| **도구 팩토리** | `createCodingTools({cwd})` 함수 | `ToolFactory::coding(cwd)` | Rust 빌더 패턴 |
| **세션** | `SessionManager` (인스턴스) | `oxi-store` (CLI 전용), SDK는 세션 없음 | 분리가 더 깔끔함 |
| **설정** | `SettingsManager.create()` | `OxiBuilder`에서 직접 설정 | 설정 파일 강제하지 않음 |
| **확장** | `ExtensionFactory` + `ExtensionRunner` | `AgentTool` 트레이트만으로 충분 | Rust의 트레이트가 TS 인터페이스보다 강력 |

---

## 7. 마이그레이션 영향도

### oxios에 미치는 영향

```rust
// Before (현재 oxios)
let provider = oxi_ai::get_provider("anthropic").unwrap();  // 글로벌
let registry = ToolRegistry::new();  // CWD 의존
let agent = AgentLoop::new(Arc::from(provider), config, tools, state);
tokio::task::spawn_blocking(move || rt.block_on(agent.run(...)));

// After (SDK)
let oxi = OxiBuilder::new().with_builtins().build();
let agent = oxi.agent(config)
    .workspace("/workspace/agent-1")
    .coding_tools()
    .build()?;
agent.run(prompt, on_event).await?;  // spawn_blocking 필요 없음!
```

**개선:**
- `WORKSPACE_MUTEX` 제거 → 병렬 에이전트 실행 가능
- `spawn_blocking` 제거 → 더 효율적인 리소스 사용
- `EngineProvider` 트레이트 불필요 → `Oxi`가 그 역할
- 글로벌 상태 오염 없음 → 테스트 격리

---

## 8. 구현 우선순위

| 단계 | 작업 | 예상 공수 | 의존성 |
|------|------|-----------|--------|
| **P0** | `AgentLoopConfig.workspace_dir` 추가 + 도구 수정 | 2-3시간 | 없음 |
| **P0** | `ProviderRegistry`, `ModelRegistry` 인스턴스화 | 3-4시간 | 없음 |
| **P1** | `oxi-sdk` 크레이트 생성 | 4-6시간 | P0 완료 |
| **P1** | `AgentBuilder` 플루언트 API | 3-4시간 | P0 완료 |
| **P2** | `oxi-cli` 마이그레이션 | 2-3시간 | P1 완료 |
| **P2** | `ToolContext` 트레이트 변경 (선택) | 4-6시간 | P1 완료 |
| **P3** | oxios 통합 테스트 | 2-3시간 | P2 완료 |

**총 예상 공수:** 20-30시간

---

*이 설계안은 pi v0.74.0의 SDK 아키텍처를 참고하여 작성되었습니다.*
