# oxi SDK v2 — 완벽한 설계

**날짜:** 2026-05-16  
**목표:** oxi를 TUI 코딩 에이전트 + AI Agent OS SDK 양쪽 모두에서 훌륭하게 작동하게 만들기  
**참조:** pi SDK (`@earendil-works/pi-coding-agent/core/sdk.ts`)

---

## 0. 핵심 원칙

1. **인스턴스 격리 (Instance Isolation)** — 모든 상태는 인스턴스에 귀속. 글로벌 static 없음.
2. **명시적 주입 (Explicit Injection)** — workspace, auth, HTTP client 모두 생성자/빌더에서 주입.
3. **하위호환 (Backward Compatibility)** — 기존 글로벌 편의 함수는 내부적으로 글로벌 인스턴스에 위임하여 유지.
4. **Send + Sync 안전** — Agent, AgentLoop이 tokio 태스크로 직접 spawn 가능.
5. **최소 의존** — SDK 사용자는 oxi-sdk 하나만 의존. oxi-store, oxi-tui, oxi-cli는 가져오지 않음.

---

## 1. 크레이트 구조 (변경 후)

```
oxi/
├── oxi-ai/          ← 최하위, 순수 LLM API (의존 없음)
│   ├── types.rs          Model, Api, Cost, InputModality
│   ├── messages.rs       Message, ContentBlock, ToolCall, ToolResult
│   ├── context.rs        Context (대화 컨텍스트)
│   ├── compaction.rs     CompactionManager, CompactionStrategy
│   ├── providers/
│   │   ├── trait_def.rs     Provider 트레이트
│   │   ├── mod.rs           ProviderRegistry (인스턴스), 글로벌 편의 함수
│   │   └── register_builtins.rs  built-in Provider 생성
│   ├── model_registry.rs   ModelRegistry (인스턴스 + 글로벌 편의)
│   ├── provider_registry.rs   ProviderAuthRegistry (인증)
│   └── ...
│
├── oxi-agent/       ← Agent 런타임 (oxi-ai만 의존)
│   ├── agent.rs           Agent (공개 API)
│   ├── agent_loop/        AgentLoop (내부 실행 루프)
│   ├── config.rs          AgentConfig, AgentHooks
│   ├── tools/             빌트인 도구 (read, write, edit, bash, ...)
│   ├── state.rs           SharedState
│   ├── events.rs          AgentEvent
│   └── ...
│
├── oxi-sdk/         ← [SDK 진입점] (oxi-ai + oxi-agent 의존)
│   ├── lib.rs             re-exports, prelude
│   ├── engine.rs          Oxi 엔진 (레지스트리 컨테이너)
│   ├── builder.rs         OxiBuilder (엔진 빌더)
│   ├── agent_builder.rs   AgentBuilder (에이전트 빌더)
│   ├── tool_factory.rs    도구 팩토리 (coding_tools, readonly_tools)
│   └── prelude.rs         자주 쓰는 타입
│
├── oxi-store/       ← CLI 전용 (세션, 설정, Auth 파일)
├── oxi-tui/         ← TUI 전용 (터미널 UI)
└── oxi-cli/         ← CLI 바이너리 (oxi-store, oxi-tui, oxi-sdk 의존)
```

### 의존성 그래프

```
oxi-ai ────────────── (최하위)
  ↑ oxi-agent ──────── oxi-ai만
  ↑   ↑ oxi-sdk ────── oxi-ai + oxi-agent
  ↑     ↑ oxi-cli ──── oxi-ai + oxi-agent + oxi-store + oxi-tui + oxi-sdk

oxi-tui ────────────── (독립)
oxi-store ──────────── oxi-ai만
```

**oxios(Agent OS)는 oxi-sdk만 의존.** oxi-store, oxi-tui, oxi-cli는 가져오지 않음.

---

## 2. 현재 문제 — 진단

### 🔴 P0-Critical: workspace_dir이 도구에 도달하지 않음

**파이프라인:**
```
AgentConfig.workspace_dir    ✅ 값 있음
  → Agent::run_with_channel_inner()
    → AgentLoopConfig.workspace_dir   ✅ 복사됨
      → AgentLoop.config.workspace_dir  ✅ 읽기 가능
        → tool.execute(id, params, signal)  ❌ workspace_dir 전달 안 됨
```

**원인:** 도구들은 `self.root_dir: PathBuf`를 생성자에서 받음.
Agent::new()는 빈 ToolRegistry를 만들고(또는 AgentBuilder가 workspace에 맞는 도구를 만들지만),
Agent::run()은 self.tools를 그대로 AgentLoop에 전달.
workspace_dir은 AgentLoopConfig에만 있고 도구에는 전달되지 않음.

**영향:** oxios에서 WORKSPACE_MUTEX로 직렬화 중. 병렬 에이전트 실행 불가.

### 🔴 P0-Critical: ProviderRegistry가 인스턴스를 들고 있지 않음

```rust
// 현재 Oxi
pub struct Oxi {
    providers: Arc<ProviderRegistry>,  // custom provider만 저장
    models: Arc<ModelRegistry>,
}

// ProviderRegistry.get()이 글로벌 get_provider()에 폴백
pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
    self.custom.get(name)  // 1. 로컬 검색
    .or_else(|| get_provider(name).map(|b| Arc::from(b)))  // 2. 글로벌 검색
}
```

**문제:** `get_provider()`는 글로벌 `CUSTOM_PROVIDERS` + `register_builtins` 사용.
테스트에서 격리 불가. oxios 인스턴스 간 프로바이더 공유.

### 🔴 P0-Critical: Agent::switch_model()이 글로벌 get_provider() 사용

```rust
pub fn switch_model(&self, model_id: &str) -> Result<()> {
    let new_provider = oxi_ai::get_provider(&new_model.provider)  // ← 글로벌
```

SDK에서 생성한 Agent는 Oxi 인스턴스의 ProviderRegistry를 사용해야 함.

### 🟡 P1-High: AgentLoop이 !Send

AgentLoop 내부의 `RwLock<Vec<Message>>` + 훅 클로저가 Send를 만족하지 않아
`spawn_blocking`이 강제됨. tokio 태스크로 직접 spawn 불가.

### 🟡 P1-High: resolve_model_from_id이 글로벌 STATIC_MODELS 사용

```rust
pub fn resolve_model_from_id(model_id: &str) -> Option<Model> {
    get_model(parts[0], &parts[1..].join("/")).cloned()  // ← 글로벌 STATIC_MODELS
}
```

Oxi 인스턴스의 ModelRegistry를 사용해야 함.

### 🟡 P1-High: CompactionManager 생성이 글로벌 STATIC_MODELS 사용

```rust
// Agent::new() 안에서
let model = crate::model_id::resolve_model_from_id(&config.model_id);
// ↑ 글로벌 STATIC_MODELS 조회
// → Oxi 인스턴스의 ModelRegistry를 사용해야 함
```

AgentLoop::new()에서도 동일. ProviderResolver.resolve_model()로 대체 필요.

### 🟢 P2-Medium: oxi-cli가 oxi-sdk에 의존하지만 사용 안 함

불필요한 컴파일 의존성.

---

## 3. 해결 설계

### 3.1 ToolContext — 도구에 컨텍스트 전달

**핵심 결정:** `AgentTool::execute()`에 `ToolContext` 추가.

이것은 pi의 `createCodingTools({ cwd })` 방식과 다릅니다.
pi는 도구 생성 시점에 cwd를 바인딩하지만,
oxi는 실행 시점에 컨텍스트를 주입합니다.

**이유:**
- Agent::switch_model(), workspace 변경 등 런타임 변화에 대응 가능
- 도구를 재생성할 필요 없음
- 인스턴스 하나로 여러 workspace에서 재사용 가능

```rust
// oxi-agent/src/tools.rs

/// 도구 실행 컨텍스트 — 런타임에 AgentLoop에서 주입
#[derive(Clone)]
pub struct ToolContext {
    /// 파일 도구의 기준 디렉토리
    pub workspace: PathBuf,
    /// 세션 ID (로깅/트레이싱용)
    pub session_id: Option<String>,
}

impl ToolContext {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace, session_id: None }
    }
}

/// 수정된 AgentTool 트레이트
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;

    /// 도구 실행 (새 시그니처)
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,  // ← 추가
    ) -> Result<AgentToolResult, ToolError>;

    /// 하위호환: 이전 execute() 시그니처 호출 지원
    /// 기본 구현은 root_dir(생성자에서 설정)을 사용
    fn execute_legacy(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
    ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, ToolError>> + Send + '_>> {
        // 기본적으로 사용되지 않음 — 새 execute()만 사용
        unimplemented!("Use execute() with ToolContext")
    }

    fn on_progress(&self, _callback: ProgressCallback) {}
    fn essential(&self) -> bool { false }

    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: serde_json::from_value(self.parameters_schema()).unwrap_or_default(),
        }
    }
}
```

**빌트인 도구의 변경:**

```rust
// oxi-agent/src/tools/read.rs (변경 전)
pub struct ReadTool {
    root_dir: PathBuf,
}

impl ReadTool {
    pub fn new() -> Self {
        Self::with_cwd(std::env::current_dir().unwrap())
    }
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self { root_dir: cwd }
    }
}

// execute()에서 self.root_dir 사용
async fn execute(&self, id: &str, params: Value, signal: ...) {
    let guard = PathGuard::new(&self.root_dir);
    // ...
}
```

```rust
// oxi-agent/src/tools/read.rs (변경 후)
pub struct ReadTool {
    root_dir: Option<PathBuf>,  // None이면 ToolContext.workspace 사용
}

impl ReadTool {
    pub fn new() -> Self {
        Self { root_dir: None }
    }
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self { root_dir: Some(cwd) }
    }
}

#[async_trait]
impl AgentTool for ReadTool {
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // root_dir이 있으면 우선, 없으면 ToolContext.workspace 사용
        let base = self.root_dir.as_deref()
            .unwrap_or(&ctx.workspace);
        let guard = PathGuard::new(base);
        // ... 기존 로직 동일
    }
}
```

**이 방식의 장점:**
- `ReadTool::new()` + ToolContext만으로 작동 (SDK 사용)
- `ReadTool::with_cwd(path)`로 명시적 설정도 가능 (CLI 사용)
- 하위호환: root_dir이 Some이면 기존 동작 유지

### 3.2 AgentLoop → ToolContext 전달

```rust
// oxi-agent/src/agent_loop/tool_exec.rs (변경)

// AgentLoop.run()에서 ToolContext 구성
fn build_tool_context(&self) -> ToolContext {
    let workspace = self.config.workspace_dir.clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    ToolContext {
        workspace,
        session_id: self.session_id.clone(),
    }
}

// execute_tool_calls()에서 ToolContext 전달
async fn execute_prepared_tool_call_static(
    tool_call: ToolCall,
    tool: Option<Arc<dyn AgentTool>>,
    args: Value,
    after_hook: Option<AfterToolCallHook>,
    emit: Arc<dyn Fn(AgentEvent) + Send + Sync>,
    ctx: ToolContext,  // ← 추가
) -> ExecutedToolCallOutcome {
    // ...
    if let Some(ref tool) = tool {
        match tool.execute(&tool_call_id, args, None, &ctx).await {
            Ok(r) => result = r,
            Err(e) => { result = AgentToolResult::error(e); is_error = true; }
        }
    }
    // ...
}
```

### 3.3 Oxi 엔진 — ProviderRegistry 내재화

```rust
// oxi-sdk/src/engine.rs (재설계)

use std::sync::Arc;
use oxi_ai::{Provider, ProviderRegistry as AiProviderRegistry, Model, ModelRegistry};
use oxi_agent::ToolRegistry;

/// oxi 엔진 인스턴스.
///
/// 완전히 격리된 Provider + Model 레지스트리를 보유.
/// 서로 다른 Oxi 인스턴스는 상태를 공유하지 않음.
pub struct Oxi {
    /// 커스텀 + 빌트인 프로바이더 레지스트리
    providers: Arc<ProviderStore>,
    /// 모델 레지스트리 (static + dynamic)
    models: Arc<ModelRegistry>,
}

/// 인스턴스 격리된 프로바이더 스토어.
///
/// 글로벌 static에 의존하지 않고, 자체적으로 Provider를 관리.
struct ProviderStore {
    /// 커스텀 프로바이더
    custom: parking_lot::RwLock<HashMap<String, Arc<dyn Provider>>>,
    /// 빌트인 프로바이더 활성화 여부
    include_builtins: bool,
}

impl ProviderStore {
    fn new(include_builtins: bool) -> Self {
        Self {
            custom: parking_lot::RwLock::new(HashMap::new()),
            include_builtins,
        }
    }

    fn register(&self, name: &str, provider: impl Provider + 'static) {
        self.custom.write().insert(name.to_string(), Arc::new(provider));
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        // 1. 커스텀 프로바이더 우선
        if let Some(p) = self.custom.read().get(name) {
            return Some(Arc::clone(p));
        }
        // 2. 빌트인 프로바이더 (include_builtins이면)
        if self.include_builtins {
            oxi_ai::get_provider(name).map(|b| Arc::from(b))
        } else {
            None
        }
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.custom.read().keys().cloned().collect();
        if self.include_builtins {
            // 빌트인 프로바이더 이름도 포함
            names.extend(oxi_ai::register_builtins::builtin_provider_names());
        }
        names.sort();
        names.dedup();
        names
    }
}
```

**문제:** `oxi_ai::get_provider()` 자체가 글로벌 `CUSTOM_PROVIDERS`를 조회함.

**참고:** `shared_client()` (글로벌 OnceLock<reqwest::Client>)은 HTTP 연결 풀이므로
격리할 필요 없음 — 클라이언트는 stateless이고 프로바이더 간 공유해도 안전.

**해결:** 빌트인 프로바이더 생성을 직접 호출:

```rust
// oxi-ai/src/providers/register_builtins.rs에 추가

/// 빌트인 프로바이더 이름 목록
pub fn builtin_provider_names() -> Vec<&'static str> {
    vec![
        "anthropic", "openai", "openai-responses", "google", "vertex",
        "deepseek", "mistral", "groq", "cerebras", "xai", "openrouter",
        "azure-openai", "bedrock",
    ]
}

/// 이름으로 빌트인 프로바이더 생성 (글로벌 상태 없이)
pub fn create_builtin_provider(name: &str) -> Option<Box<dyn Provider>> {
    match name {
        "anthropic" => Some(Box::new(AnthropicProvider::new())),
        "openai" => Some(Box::new(OpenAiProvider::new())),
        "openai-responses" => Some(Box::new(OpenAiResponsesProvider::new())),
        "google" => Some(Box::new(GoogleProvider::new())),
        "deepseek" => Some(Box::new(OpenAiProvider::with_base_url("https://api.deepseek.com"))),
        // ... 기타 프로바이더
        _ => None,
    }
}
```

**최종 ProviderStore.get():**

```rust
fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
    // 1. 커스텀 프로바이더
    if let Some(p) = self.custom.read().get(name) {
        return Some(Arc::clone(p));
    }
    // 2. 빌트인 프로바이더 (글로벌 상태 없이 직접 생성)
    if self.include_builtins {
        oxi_ai::register_builtins::create_builtin_provider(name)
            .map(|b| Arc::from(b))
    } else {
        None
    }
}
```

### 3.4 OxiBuilder — 엔진 빌더

```rust
// oxi-sdk/src/builder.rs

use std::path::PathBuf;
use oxi_ai::{Provider, Model, ModelRegistry};
use oxi_agent::ToolRegistry;
use crate::engine::Oxi;

/// oxi 엔진 빌더.
///
/// # 예시
///
/// ```rust
/// use oxi_sdk::{OxiBuilder, AgentConfig};
///
/// // 최소 설정
/// let oxi = OxiBuilder::new().with_builtins().build();
///
/// // 커스텀 프로바이더 + 모델
/// let oxi = OxiBuilder::new()
///     .with_builtins()
///     .provider("my-llm", MyProvider::new())
///     .model(Model::new("my-model", "My Model", Api::OpenAiCompletions, "my-llm", "https://..."))
///     .build();
///
/// // 완전 격리 (빌트인 없이)
/// let oxi = OxiBuilder::new()
///     .provider("mock", MockProvider::new())
///     .model(mock_model)
///     .build();
/// ```
pub struct OxiBuilder {
    providers: ProviderStore,
    models: ModelRegistry,
    include_builtins: bool,
}

impl OxiBuilder {
    /// 빈 빌더 생성.
    pub fn new() -> Self {
        Self {
            providers: ProviderStore::new(false),
            models: ModelRegistry::new(),
            include_builtins: false,
        }
    }

    /// 빌트인 프로바이더와 모델을 포함.
    ///
    /// - 10개 빌트인 프로바이더 (anthropic, openai, google, ...)
    /// - 50+ 빌트인 모델
    pub fn with_builtins(mut self) -> Self {
        self.include_builtins = true;
        self.models = ModelRegistry::from_static();
        self
    }

    /// 커스텀 프로바이더 등록.
    pub fn provider(mut self, name: &str, p: impl Provider + 'static) -> Self {
        self.providers.register(name, p);
        self
    }

    /// 커스텀 모델 등록.
    pub fn model(mut self, model: Model) -> Self {
        self.models.register(model);
        self
    }

    /// Oxi 엔진 인스턴스 빌드.
    pub fn build(self) -> Oxi {
        Oxi {
            providers: Arc::new(ProviderStore::new_with(
                self.providers,
                self.include_builtins,
            )),
            models: Arc::new(self.models),
        }
    }
}
```

### 3.5 Agent — Provider 레지스트리 내재화

**핵심 변경:** Agent가 Oxi의 ProviderStore를 참조하여 switch_model()에서도 격리 유지.

```rust
// oxi-agent/src/agent.rs (변경)

/// 프로바이더 리졸버 트레이트 — Agent가 프로바이더를 찾는 방법의 추상화
pub trait ProviderResolver: Send + Sync + 'static {
    fn resolve(&self, provider_name: &str) -> Option<Arc<dyn Provider>>;
    fn resolve_model(&self, model_id: &str) -> Option<Model>;
}

/// Agent 런타임.
pub struct Agent {
    inner: RwLock<AgentInner>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
    hooks: parking_lot::RwLock<AgentHooks>,
    is_running: AtomicBool,
    provider_resolver: Arc<dyn ProviderResolver>,  // ← 추가
}

// Agent::switch_model() 변경
impl Agent {
    pub fn switch_model(&self, model_id: &str) -> Result<()> {
        let new_model = self.provider_resolver.resolve_model(model_id)
            .ok_or_else(|| Error::msg(format!("Model '{}' not found", model_id)))?;

        let new_provider = self.provider_resolver.resolve(&new_model.provider)
            .ok_or_else(|| Error::msg(format!("Provider '{}' not found", new_model.provider)))?;

        // ... 기존 로직 (메시지 변환, 업데이트)
    }
}
```

**Oxi가 ProviderResolver를 구현:**

```rust
// oxi-sdk/src/engine.rs

impl ProviderResolver for Oxi {
    fn resolve(&self, provider_name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(provider_name)
    }

    fn resolve_model(&self, model_id: &str) -> Option<Model> {
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        let (provider, model) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("anthropic", parts[0])
        };
        self.models.lookup(provider, model)
    }
}
```

### 3.6 AgentBuilder — 완전한 fluent API

```rust
// oxi-sdk/src/agent_builder.rs

use std::path::PathBuf;
use std::sync::Arc;
use oxi_ai::{Provider, Model};
use oxi_agent::{Agent, AgentConfig, AgentHooks, AgentTool, ToolRegistry};
use crate::engine::Oxi;

pub struct AgentBuilder<'a> {
    oxi: &'a Oxi,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace_dir: Option<PathBuf>,
    system_prompt: Option<String>,
    hooks: Option<AgentHooks>,
}

impl<'a> AgentBuilder<'a> {
    pub fn new(oxi: &'a Oxi, config: AgentConfig) -> Self {
        Self {
            oxi,
            config,
            tools: ToolRegistry::new(),
            workspace_dir: None,
            system_prompt: None,
            hooks: None,
        }
    }

    /// 워크스페이스 설정. 파일 도구의 기준 경로.
    pub fn workspace(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = Some(dir.into());
        self
    }

    /// 시스템 프롬프트 설정.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// 빌트인 코딩 도구 등록 (read, write, edit, bash, grep, find, ls, ...).
    pub fn coding_tools(self) -> Self {
        self._register_builtin_tools(true)
    }

    /// 읽기 전용 도구 등록 (read, ls, grep, find).
    pub fn readonly_tools(self) -> Self {
        self._register_builtin_tools(false)
    }

    fn _register_builtin_tools(mut self, writable: bool) -> Self {
        let cwd = self.workspace_dir.clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        if writable {
            // 모든 빌트인 도구 (root_dir = None, ToolContext에서 주입)
            let registry = ToolRegistry::with_builtins_cwd(cwd, &[]);
            for name in registry.names() {
                if let Some(tool) = registry.get(&name) {
                    self.tools.register_arc(tool);
                }
            }
        } else {
            // 읽기 전용만
            let registry = ToolRegistry::with_selected_tools(cwd, &["read", "ls", "grep", "find"]);
            for name in registry.names() {
                if let Some(tool) = registry.get(&name) {
                    self.tools.register_arc(tool);
                }
            }
        }
        self
    }

    /// 커스텀 도구 등록.
    pub fn tool(mut self, tool: impl AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// 여러 커스텀 도구 등록.
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl AgentTool + 'static>) -> Self {
        for tool in tools {
            self.tools.register(tool);
        }
        self
    }

    /// 클로저 기반 커스텀 도구 (간편 등록).
    pub fn custom_tool(
        mut self,
        name: &str,
        description: &str,
        schema: serde_json::Value,
        handler: impl Fn(serde_json::Value, &ToolContext) -> Result<AgentToolResult, String>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.tools.register(ClosureTool::new(name, description, schema, handler));
        self
    }

    /// 훅 설정.
    pub fn hooks(mut self, hooks: AgentHooks) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// 에이전트 빌드.
    pub fn build(self) -> anyhow::Result<Agent> {
        // 1. 모델 리졸브
        let model = self.oxi.resolve_model(&self.config.model_id)?;

        // 2. 프로바이더 생성
        let provider = self.oxi.create_provider(&model.provider)?;

        // 3. 설정 머지
        let mut config = self.config.clone();
        config.workspace_dir = self.workspace_dir.or(config.workspace_dir);
        if let Some(ref prompt) = self.system_prompt {
            config.system_prompt = Some(prompt.clone());
        }

        // 4. Agent 생성 (ProviderResolver로 Oxi 전달)
        let agent = Agent::new_with_resolver(
            provider,
            config,
            Arc::new(self.tools),
            Arc::new(self.oxi.clone()),  // Oxi가 ProviderResolver를 구현
        );

        // 5. 훅 설정
        if let Some(hooks) = self.hooks {
            agent.set_hooks(hooks);
        }

        Ok(agent)
    }
}

/// 클로저 기반 간편 도구
struct ClosureTool {
    name: String,
    description: String,
    schema: serde_json::Value,
    handler: Box<dyn Fn(serde_json::Value, &ToolContext) -> Result<AgentToolResult, String>
        + Send
        + Sync>,
}

impl ClosureTool {
    fn new(
        name: &str,
        description: &str,
        schema: serde_json::Value,
        handler: impl Fn(serde_json::Value, &ToolContext) -> Result<AgentToolResult, String>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            schema,
            handler: Box::new(handler),
        }
    }
}

#[async_trait]
impl AgentTool for ClosureTool {
    fn name(&self) -> &str { &self.name }
    fn label(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn parameters_schema(&self) -> serde_json::Value { self.schema.clone() }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, String> {
        (self.handler)(params, ctx)
    }
}
```

### 3.7 Oxi 공개 API

```rust
// oxi-sdk/src/engine.rs

impl Oxi {
    /// 에이전트 빌더 생성.
    pub fn agent(&self, config: AgentConfig) -> AgentBuilder<'_> {
        AgentBuilder::new(self, config)
    }

    /// 모델 리졸브 ("provider/model" 또는 "model").
    pub fn resolve_model(&self, model_id: &str) -> Result<Model> {
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        let (provider, model) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("anthropic", parts[0])
        };
        self.models
            .lookup(provider, model)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_id))
    }

    /// 프로바이더 생성.
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        self.providers
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", name))
    }

    /// 모델 레지스트리 접근.
    pub fn models(&self) -> &ModelRegistry { &self.models }

    /// 프로바이더 스토어 접근.
    pub fn providers(&self) -> &ProviderStore { &self.providers }
}
```

---

## 4. oxi-ai 변경 사항

### 4.1 register_builtins.rs — 상태 없는 프로바이더 생성

```rust
// oxi-ai/src/providers/register_builtins.rs (추가)

/// 빌트인 프로바이더 이름 목록
pub fn builtin_provider_names() -> Vec<&'static str> {
    vec![
        "anthropic", "openai", "openai-responses", "google", "vertex",
        "deepseek", "mistral", "groq", "cerebras", "xai", "openrouter",
        "azure-openai", "bedrock",
    ]
}

/// 이름으로 빌트인 프로바이더 생성 (글로벌 상태 없이)
pub fn create_builtin_provider(name: &str) -> Option<Box<dyn Provider>> {
    match name {
        "anthropic" => Some(Box::new(AnthropicProvider::new())),
        "openai" => Some(Box::new(OpenAiProvider::new())),
        "openai-responses" => Some(Box::new(OpenAiResponsesProvider::new())),
        "google" => Some(Box::new(GoogleProvider::new())),
        "vertex" => Some(Box::new(VertexProvider::new())),
        "deepseek" => Some(Box::new(
            OpenAiProvider::with_base_url("https://api.deepseek.com")
        )),
        "mistral" => Some(Box::new(MistralProvider::new())),
        "groq" => Some(Box::new(
            OpenAiProvider::with_base_url("https://api.groq.com/openai/v1")
        )),
        "cerebras" => Some(Box::new(
            OpenAiProvider::with_base_url("https://api.cerebras.ai/v1")
        )),
        "xai" => Some(Box::new(
            OpenAiProvider::with_base_url("https://api.x.ai/v1")
        )),
        "openrouter" => Some(Box::new(
            OpenAiProvider::with_base_url("https://openrouter.ai/api/v1")
        )),
        "azure-openai" => Some(Box::new(AzureProvider::new())),
        "bedrock" => Some(Box::new(BedrockProvider::new())),
        _ => None,
    }
}
```

### 4.2 기존 글로벌 함수 유지 (하위호환)

```rust
// oxi-ai/src/providers/mod.rs — 기존 코드 유지

/// 기존 글로벌 get_provider()는 그대로 작동.
/// 내부적으로 CUSTOM_PROVIDERS + create_builtin_provider() 사용.
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    // 1. 커스텀 프로바이더 (글로벌)
    { /* 기존 코드 */ }

    // 2. 빌트인 프로바이더
    register_builtins::create_builtin_provider(name)
}
```

---

## 5. oxi-agent 변경 사항

### 5.1 AgentTool 트레이트 변경 (Breaking Change)

```rust
// execute() 시그니처에 ToolContext 추가
async fn execute(
    &self,
    tool_call_id: &str,
    params: Value,
    signal: Option<oneshot::Receiver<()>>,
    ctx: &ToolContext,  // ← 추가
) -> Result<AgentToolResult, ToolError>;
```

**마이그레이션 가이드:**
- 기존 도구 구현에 `ctx: &ToolContext` 파라미터 추가
- `self.root_dir` 대신 `ctx.workspace` 사용
- CLI는 `ToolRegistry::with_builtins_cwd(cwd)`로 계속 작동 (root_dir이 Some이므로)

### 5.2 Agent::new() 시그니처 변경

```rust
// 기존
pub fn new(provider: Arc<dyn Provider>, config: AgentConfig, tools: Arc<ToolRegistry>) -> Self

// 추가
pub fn new_with_resolver(
    provider: Arc<dyn Provider>,
    config: AgentConfig,
    tools: Arc<ToolRegistry>,
    resolver: Arc<dyn ProviderResolver>,
) -> Self
```

**기존 `Agent::new()`는 유지** (글로벌 resolver 사용):
```rust
pub fn new(provider: Arc<dyn Provider>, config: AgentConfig, tools: Arc<ToolRegistry>) -> Self {
    Self::new_with_resolver(provider, config, tools, Arc::new(GlobalProviderResolver))
}

/// 글로벌 resolver — oxi_ai::get_provider() 사용 (기존 동작)
struct GlobalProviderResolver;

impl ProviderResolver for GlobalProviderResolver {
    fn resolve(&self, name: &str) -> Option<Arc<dyn Provider>> {
        oxi_ai::get_provider(name).map(|b| Arc::from(b))
    }
    fn resolve_model(&self, model_id: &str) -> Option<Model> {
        crate::model_id::resolve_model_from_id(model_id)
    }
}
```

### 5.3 빌트인 도구 변경 요약

| 도구 | 변경 | 영향 |
|------|------|------|
| ReadTool | `root_dir: PathBuf` → `root_dir: Option<PathBuf>`, execute에서 ctx.workspace 폴백 | 낮음 |
| WriteTool | 동일 | 낮음 |
| EditTool | 동일 | 낮음 |
| BashTool | 동일 (BashTool은 path가 아닌 cwd 개념) | 낮음 |
| GrepTool | 동일 | 낮음 |
| FindTool | 동일 | 낮음 |
| LsTool | 동일 | 낮음 |
| WebSearchTool | ToolContext 무시 (파일 경로 없음) | 없음 |
| GitHubTool | ToolContext 무시 | 없음 |
| SubagentTool | cwd 대신 ctx.workspace 사용 | 중간 |
| McpTool | ToolContext 무시 | 없음 |
| QuestionnaireTool | ToolContext 무시 | 없음 |

---

## 6. 전체 사용 예시

### 6.1 최소 SDK 사용 (oxios)

```rust
use oxi_sdk::{OxiBuilder, AgentConfig, AgentEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let oxi = OxiBuilder::new().with_builtins().build();

    let agent = oxi.agent(AgentConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".into(),
        max_iterations: 20,
        ..Default::default()
    })
    .workspace("/workspace/agent-1")
    .system_prompt("You are an autonomous coding agent.")
    .coding_tools()
    .build()?;

    let (response, events) = agent.run("Build a REST API".into()).await?;
    println!("Result: {}", response.content);
    Ok(())
}
```

### 6.2 병렬 다중 에이전트 (oxios)

```rust
let oxi = OxiBuilder::new().with_builtins().build();

let agent_a = oxi.agent(AgentConfig::new("anthropic/claude-sonnet-4-20250514"))
    .workspace("/workspace/frontend")
    .coding_tools()
    .build()?;

let agent_b = oxi.agent(AgentConfig::new("deepseek/deepseek-chat"))
    .workspace("/workspace/backend")
    .coding_tools()
    .build()?;

// 병렬 실행 — CWD 충돌 없음!
let (r_a, r_b) = tokio::join!(
    agent_a.run("Build React app".into()),
    agent_b.run("Build Rust API".into()),
);
```

### 6.3 커스텀 도구만 (oxios 커널)

```rust
let oxi = OxiBuilder::new().with_builtins().build();

let agent = oxi.agent(AgentConfig::new("anthropic/claude-sonnet-4-20250514"))
    .workspace("/workspace")
    .custom_tool(
        "memory_recall",
        "Search long-term memory",
        json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        |params, ctx| {
            let query = params["query"].as_str().unwrap();
            Ok(AgentToolResult::success(format!("Recalled: {}", query)))
        },
    )
    .custom_tool(
        "exec",
        "Execute shell command",
        json!({"type": "object", "properties": {"command": {"type": "string"}}}),
        |params, ctx| {
            // ctx.workspace에 안전하게 접근 가능
            Ok(AgentToolResult::success("Executed"))
        },
    )
    .build()?;
```

### 6.4 완전 격리 테스트

```rust
#[test]
fn test_with_mock_provider() {
    let oxi = OxiBuilder::new()  // ← 빌트인 없음!
        .provider("mock", MockProvider::new())
        .model(Model::new("test-model", "Test", Api::OpenAiCompletions, "mock", "https://mock"))
        .build();

    let agent = oxi.agent(AgentConfig {
        model_id: "mock/test-model".into(),
        max_iterations: 1,
        ..Default::default()
    })
    .workspace(tempdir.path())
    .build()
    .unwrap();

    // 글로벌 상태 오염 없이 테스트
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(agent.run("test prompt".into()));
    assert!(result.is_ok());
}

#[test]
fn test_isolation_between_instances() {
    let oxi1 = OxiBuilder::new()
        .provider("p1", MockProvider::new())
        .model(Model::new("m1", "M1", Api::AnthropicMessages, "p1", "https://p1"))
        .build();

    let oxi2 = OxiBuilder::new()
        .provider("p2", MockProvider::new())
        .model(Model::new("m2", "M2", Api::OpenAiCompletions, "p2", "https://p2"))
        .build();

    // oxi1에 p2가 없어야 함
    assert!(oxi1.create_provider("p2").is_err());
    assert!(oxi2.create_provider("p1").is_err());
}
```

### 6.5 CLI 사용 (하위호환 — 변화 없음)

```rust
// oxi-cli/src/main.rs — 변경 없음
// 글로벌 편의 함수 계속 작동
// Agent::new() 사용 (GlobalProviderResolver 자동 적용)
// ToolRegistry::with_builtins_cwd(cwd) 사용
```

---

## 7. oxi-sdk re-export 구조

```rust
// oxi-sdk/src/lib.rs

// ── SDK 자체 ──
mod engine;
mod builder;
mod agent_builder;
mod tool_factory;
pub mod prelude;

pub use engine::Oxi;
pub use builder::OxiBuilder;
pub use agent_builder::AgentBuilder;

// ── oxi-ai에서 재수출 ──
pub use oxi_ai::{
    // 핵심 트레이트/타입
    Provider, Model, Api, Cost, InputModality,
    Context, Message, ContentBlock,
    ProviderEvent, StreamOptions,
    // 레지스트리
    ProviderRegistry, ModelRegistry, ProviderAuthRegistry,
    // 압축
    CompactionStrategy, CompactionManager,
    // 오류
    ProviderError,
    // 도구
    Tool, ToolValidationError, validate_args,
};

// ── oxi-agent에서 재수출 ──
pub use oxi_agent::{
    // 에이전트
    Agent, AgentConfig, AgentHooks,
    // 이벤트
    AgentEvent,
    // 상태
    AgentState, SharedState,
    // 도구
    AgentTool, AgentToolResult, ToolError, ToolRegistry, ToolContext,
    // 훅 관련
    ToolExecutionMode,
    BeforeToolCallContext, BeforeToolCallResult,
    AfterToolCallContext, AfterToolCallResult,
    ShouldStopAfterTurnContext,
    // 오류
    AgentError,
    // 복구
    CircuitBreaker, CircuitBreakerConfig, FallbackChain,
};
```

---

## 8. 구현 계획

### Phase 0: 기반 작업 (P0)

| 작업 | 파일 | 예상 공수 | 세부 내용 |
|------|------|-----------|-----------|
| `register_builtins::create_builtin_provider()` | `oxi-ai/src/providers/register_builtins.rs` | 1h | 각 프로바이더의 new() 생성자 확인 후 매치 암 구현 |
| `register_builtins::builtin_provider_names()` | 동일 | 10m | 이름 목록 반환 |
| `ToolContext` 구조체 | `oxi-agent/src/tools.rs` | 30m | workspace, session_id 필드 |
| `AgentTool::execute()` 시그니처 변경 | `oxi-agent/src/tools.rs` | 30m | ctx 파라미터 추가 |
| 빌트인 도구 7개 수정 | `oxi-agent/src/tools/{read,write,edit,bash,grep,find,ls}.rs` | 2h | root_dir을 Option으로, ctx.workspace 폴백 |
| SubagentTool 수정 | `oxi-agent/src/tools/subagent.rs` | 30m | ToolContext.workspace를 cwd로 전달 |
| 기타 도구 수정 | `web_search, github, mcp, questionnaire, context7` | 1h | ctx 파라미터 추가 (무시) |
| `ProviderResolver` 트레이트 | `oxi-agent/src/agent.rs` | 30m | resolve() + resolve_model() |
| `Agent::new_with_resolver()` | `oxi-agent/src/agent.rs` | 1.5h | ProviderResolver 저장, switch_model() + compaction에서 사용 |
| `AgentLoop`에 resolver 전달 | `oxi-agent/src/agent_loop/mod.rs` | 30m | compaction 시 resolver로 모델 리졸브 |
| `GlobalProviderResolver` | `oxi-agent/src/agent.rs` | 15m | 기존 Agent::new() 하위호환 |
| AgentLoop → ToolContext 전달 | `oxi-agent/src/agent_loop/tool_exec.rs` | 1h | build_tool_context(), execute에 ctx 전달 |

### Phase 1: SDK 재설계 (P0)

| 작업 | 파일 | 예상 공수 |
|------|------|-----------|
| `ProviderStore` 구현 | `oxi-sdk/src/engine.rs` | 1h |
| `Oxi` 엔진 재구현 | `oxi-sdk/src/engine.rs` | 1h |
| `OxiBuilder` 재구현 | `oxi-sdk/src/builder.rs` | 1h |
| `AgentBuilder` 재구현 | `oxi-sdk/src/agent_builder.rs` | 1h |
| `ClosureTool` 구현 | `oxi-sdk/src/agent_builder.rs` | 30m |
| `tool_factory` 업데이트 | `oxi-sdk/src/tool_factory.rs` | 30m |
| `lib.rs` re-export 정리 | `oxi-sdk/src/lib.rs` | 30m |

### Phase 2: CLI 마이그레이션 (P1)

| 작업 | 파일 | 예상 공수 |
|------|------|-----------|
| oxi-cli에서 oxi-sdk 의존 유지 | `oxi-cli/Cargo.toml` | 0 (이미 의존 중) |
| CLI가 OxiBuilder 사용하도록 | `oxi-cli/src/lib.rs` | 2h |
| 기존 Agent::new() → AgentBuilder로 마이그레이션 | `oxi-cli/src/` | 2h |

### Phase 3: 테스트 + 검증 (P1)

| 작업 | 예상 공수 |
|------|-----------|
| oxi-sdk 단위 테스트 (격리, 병렬, 커스텀 도구) | 2h |
| oxi-agent 통합 테스트 (ToolContext 전달) | 1h |
| 전체 워크스페이스 테스트 | 1h |
| 컴파일 에러/워닝 0 확인 | 30m |

**총 예상 공수:** ~24시간

---

## 9. 위험 분석

### Breaking Changes

| 변경 | 영향 범위 | 완화 방안 |
|------|----------|-----------|
| `AgentTool::execute()` 시그니처 | 모든 커스텀 도구 | `ctx` 파라미터 추가만으로 해결. 기존 도구는 `ctx` 무시 가능 |
| `ReadTool.root_dir` 타입 변경 | 직접 생성하는 코드 | `with_cwd()`는 그대로 작동. `new()`는 root_dir=None으로 변경 |
| `AgentLoop::new()` 내부 변경 | 직접 AgentLoop 생성하는 코드 | AgentLoop은 내부 API이므로 영향 최소 |
| Compaction 모델 리졸브 경로 | Agent, AgentLoop 생성 시 | ProviderResolver를 통해 리졸브하도록 변경 |
| `Agent::new_with_resolver()` 추가 | 없음 (추가만) | 기존 `Agent::new()`는 유지 |

### 하위호환 보장

- `oxi-cli`는 **변경 없이** 작동 (글로벌 편의 함수 유지)
- `ToolRegistry::with_builtins_cwd(cwd)`는 그대로 작동
- `oxi_ai::get_provider()`, `register_provider()` 등 글로벌 함수 유지
- `Agent::new()` 시그니처 유지

---

## 10. pi vs oxi 최종 비교

| 관점 | pi | oxi v2 |
|------|-----|--------|
| **엔진 구조** | `createAgentSession()` 함수 | `OxiBuilder.build().agent(config).build()` |
| **인스턴스 격리** | 함수 파라미터로 주입 | `Oxi` 인스턴스에 캡슐화 |
| **워크스페이스** | `cwd` 파라미터 | `ToolContext.workspace` (런타임 주입) |
| **프로바이더** | `AuthStorage.create()` | `OxiBuilder.provider()` / `with_builtins()` |
| **도구 생성** | `createCodingTools({cwd})` | `AgentBuilder.coding_tools()` |
| **커스텀 도구** | `ToolDefinition` 인터페이스 | `AgentTool` 트레이트 + `ClosureTool` |
| **세션 관리** | `SessionManager` (SDK 내장) | SDK에 세션 없음 (oxi-store가 담당) |
| **훅** | AgentSession의 이벤트 | `AgentHooks` (before/after tool call, steering) |
| **설정** | `SettingsManager.create()` | `AgentConfig`에 직접 설정 |
| **테스트 격리** | `SessionManager.inMemory()` | `OxiBuilder::new()` (빌트인 없이) |

---

*이 설계는 oxi v0.12.0 소스코드, pi v0.74.0 SDK, 그리고 oxios v0.2.0-alpha의 실제 사용 패턴을 기반으로 작성되었습니다.*
