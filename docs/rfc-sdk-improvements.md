# RFC: oxicode-sdk 개선 — oxios 컨슈머 관점의 5가지 약점 해소

> **상태**: 설계 완료, 구현 대기
> **대상 크레이트**: `oxicode-sdk`, `oxicode-agent`
> **영향 범위**: `oxios` (하위 호환 유지)

---

## 문제 요약

| # | 약점 | 심각도 | 근원 크레이트 |
|---|------|--------|--------------|
| 1 | `oxicode-ai` 직접 의존 잔존 | 중간 | `oxicode-sdk` (re-export 불충분) |
| 2 | `OxicodeBuilder`에 credential 주입 불가 | 중간 | `oxicode-sdk` |
| 3 | `AgentLoop::run()` 콜백이 `Fn` → Mutex 강제 | 낮음 | `oxicode-agent` |
| 4 | `ToolRegistry`에 내장 도구 팩토리 없음 | 낮음 | `oxicode-sdk` |
| 5 | `CompactionEvent` 콜백에서 비동기 작업 곤란 | 낮음 | `oxicode-agent` |

---

## 해결 1: `oxicode-ai` 직접 의존 완전 제거

### 현황

`oxios-ouroboros`가 `oxicode_ai::Context`, `oxicode_ai::Message` 등을 직접 import:

```rust
// ouroboros_engine.rs
use oxicode_ai::{Context, Message, Model, Provider, UserMessage};
```

oxicode-sdk는 이미 이 타입을 re-export하지만, **`Context` 생성 메서드**가 노출되지 않아 직접 의존이 필요함.

### 설계

**A. `oxicode-sdk/src/lib.rs`에 누락된 re-export 추가**

```rust
// ── Context builder re-exports ─────────────────────────────────────────
pub use oxicode_ai::Context;
pub use oxicode_ai::UserMessage;
```

현재 `lib.rs`를 확인:

```rust
pub use oxicode_ai::{
    Api, CompactionStrategy, ContentBlock, Context, Cost, InputModality, Message, Model,
    ModelRegistry, Provider, ProviderError, ProviderEvent, ProviderOptions, ProviderRegistry,
    StreamOptions, UserMessage,
};
```

`Context`와 `UserMessage`는 이미 re-export되어 있음. 문제는 **oxios의 Cargo.toml**에서 `oxicode-ai`를 직접 의존하는 것.

**B. 해결책: oxios 측 import 변경**

```diff
  // ouroboros_engine.rs
- use oxicode_ai::{Context, Message, Model, Provider, UserMessage};
+ use oxicode_sdk::{Context, Message, Model, Provider, UserMessage};
```

```diff
  // Cargo.toml (oxios-kernel)
- oxicode-ai = { workspace = true }
  # oxicode-sdk만 유지. oxicode-sdk가 oxicode-ai를 re-export하므로 불필요.
```

oxios-ouroboros의 `Cargo.toml`도 동일하게 `oxicode-ai` 의존 제거.

**C. 검증 체크리스트**

oxios에서 `oxicode_ai::`를 사용하는 유일한 곳:

```
crates/oxios-ouroboros/src/ouroboros_engine.rs   → oxicode_sdk::로 교체
crates/oxios-ouroboros/tests/scenario_test.rs     → oxicode_sdk::로 교체
```

두 파일만 수정하면 `oxicode-ai` workspace 의존을 완전히 제거 가능.

### 변경 파일

| 파일 | 변경 |
|------|------|
| `oxios/crates/oxios-ouroboros/Cargo.toml` | `oxicode-ai` 의존 제거, `oxicode-sdk`만 유지 |
| `oxios/crates/oxios-kernel/Cargo.toml` | `oxicode-ai` 의존 제거 |
| `oxios/Cargo.toml` | `[workspace.dependencies]`에서 `oxicode-ai` 제거 |
| `oxios/crates/oxios-ouroboros/src/ouroboros_engine.rs` | `oxicode_ai::` → `oxicode_sdk::` |
| `oxios/crates/oxios-ouroboros/tests/scenario_test.rs` | `oxicode_ai::` → `oxicode_sdk::` |

---

## 해결 2: `OxicodeBuilder`에 Credential 주입

### 현황

oxios는 API 키를 `AgentLoopConfig.api_key`로 **실행 시점에** 우회 전달:

```rust
// agent_runtime.rs
AgentLoopConfig {
    api_key: config.api_key,  // CredentialStore에서 해석
    provider_options: config.provider_options,
    ..
}
```

이 방식은 동작하지만:
1. 매 실행마다 키를 전달해야 함
2. `OxicodeBuilder`에서 생성한 provider는 환경 변수만 참조
3. 커스텀 provider (예: ZAI)에 base_url + key를 주입할 방법이 factory뿐

### 설계

**A. `OxicodeBuilder`에 credential 메서드 추가**

```rust
// oxicode-sdk/src/builder.rs

impl OxicodeBuilder {
    /// Register an API key for a specific provider.
    ///
    /// When `create_provider(name)` is called, the key is injected into
    /// the provider's `StreamOptions::api_key` automatically.
    ///
    /// Keys registered here take precedence over environment variables.
    pub fn api_key(mut self, provider_name: &str, key: impl Into<String>) -> Self {
        self.api_keys.insert(provider_name.to_string(), key.into());
        self
    }

    /// Register a base URL override for a specific provider.
    ///
    /// Useful for OpenAI-compatible providers (ZAI, Groq, etc.)
    /// that use a different endpoint.
    pub fn base_url(mut self, provider_name: &str, url: impl Into<String>) -> Self {
        self.base_urls.insert(provider_name.to_string(), url.into());
        self
    }

    /// Register a full credential set for a provider.
    ///
    /// Convenience method combining `api_key` and `base_url`.
    pub fn credential(
        self,
        provider_name: &str,
        api_key: impl Into<String>,
        base_url: Option<&str>,
    ) -> Self {
        let mut builder = self.api_key(provider_name, api_key);
        if let Some(url) = base_url {
            builder = builder.base_url(provider_name, url);
        }
        builder
    }
}
```

**B. `Oxicode` 구조체에 credential 저장 및 provider 생성 시 주입**

```rust
// oxicode-sdk/src/builder.rs

#[derive(Clone)]
pub struct Oxicode {
    providers: Arc<ProviderRegistry>,
    models: Arc<ModelRegistry>,
    tools: Arc<ToolRegistry>,
    include_builtins: bool,
    /// Per-provider API keys (takes precedence over env vars).
    api_keys: Arc<HashMap<String, String>>,
    /// Per-provider base URL overrides.
    base_urls: Arc<HashMap<String, String>>,
}

impl Oxicode {
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        // 1. Custom providers
        if let Some(p) = self.providers.get_custom(name) {
            return Ok(p);
        }

        // 2. Factory providers (already handle their own credential)
        if let Some(p) = self.providers.get_factory(name) {
            return Ok(p);
        }

        // 3. Built-in providers with credential injection
        if self.include_builtins {
            let api_key = self.api_keys.get(name).cloned();
            let base_url = self.base_urls.get(name).cloned();
            if let Some(p) = oxicode_ai::create_builtin_provider_with_options(
                name,
                api_key.as_deref(),
                base_url.as_deref(),
            ) {
                return Ok(Arc::from(p));
            }
        }

        Err(anyhow::anyhow!("Provider '{}' not found", name))
    }
}
```

**C. `oxicode-ai`에 `create_builtin_provider_with_options` 추가**

```rust
// oxicode-ai/src/providers/register_builtins.rs

/// Create a built-in provider with optional credential overrides.
///
/// When `api_key` is `Some`, the provider is constructed with this key
/// instead of reading from the environment. When `base_url` is `Some`,
/// the provider's endpoint is overridden (useful for OpenAI-compatible APIs).
pub fn create_builtin_provider_with_options(
    name: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Option<Box<dyn Provider>> {
    match name {
        "anthropic" => {
            let key = api_key
                .map(String::from)
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())?;
            Some(Box::new(AnthropicProvider::new(key)))
        }
        "openai" => {
            let key = api_key
                .map(String::from)
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())?;
            let provider = if let Some(url) = base_url {
                OpenAiProvider::with_base_url_and_key(url, key)
            } else {
                OpenAiProvider::new(key)
            };
            Some(Box::new(provider))
        }
        // ... 각 프로바이더별 동일 패턴
        _ => None,
    }
}
```

**D. oxios 사용 예 (개선 후)**

```rust
// oxios engine.rs — 개선 전
let builder = OxicodeBuilder::new().with_builtins();
let oxicode = builder.build();
// → 매 실행 시 AgentLoopConfig.api_key로 우회 전달

// oxios engine.rs — 개선 후
let oxicode = OxicodeBuilder::new()
    .with_builtins()
    .api_key("anthropic", credential_store.resolve("anthropic"))
    .api_key("openai", credential_store.resolve("openai"))
    .build();
// → provider 생성 시 자동으로 키 주입, AgentLoopConfig.api_key 불필요
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `oxicode-sdk/src/builder.rs` | `api_keys`, `base_urls` 필드 + `api_key()`, `base_url()`, `credential()` 메서드 |
| `oxicode-ai/src/providers/register_builtins.rs` | `create_builtin_provider_with_options()` 추가 |
| `oxicode-ai/src/providers/mod.rs` | 신규 함수 re-export |
| `oxios/crates/oxios-kernel/src/engine.rs` | `OxicodeBuilder`에 credential 주입으로 단순화 |

---

## 해결 3: `AgentLoop::run()` 콜백을 `FnMut` 지원

### 현황

```rust
// oxicode-agent/agent_loop/mod.rs
type EmitFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

pub async fn run(
    &self,
    prompt: String,
    emit: impl Fn(AgentEvent) + Send + Sync + 'static,  // ← Fn (불변)
) -> Result<Vec<AgentEvent>> {
```

oxios는 상태를 공유하기 위해 `Arc<Mutex<ExecuteState>>`로 우회:

```rust
let exec_state = Arc::new(Mutex::new(ExecuteState::default()));
let exec_state_clone = Arc::clone(&exec_state);
agent_loop.run(prompt, move |event| {
    let mut s = exec_state_clone.lock();  // 매 이벤트마다 락 획득
    // ...
})
```

### 설계 — 듀얼 API (하위 호환)

`Fn` 시그니처를 그대로 두고, `FnMut` 버전을 새 메서드로 추가:

```rust
// oxicode-agent/src/agent_loop/mod.rs

impl AgentLoop {
    /// Run with an `Fn` callback (existing, unchanged).
    pub async fn run(
        &self,
        prompt: String,
        emit: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) -> Result<Vec<AgentEvent>> {
        let message = Message::User(UserMessage::new(prompt));
        let emit = Arc::new(emit);
        self.run_messages(vec![message], emit).await
    }

    /// Run with an `FnMut` callback — allows mutable state capture.
    ///
    /// Use this when your callback needs to accumulate state without
    /// `Arc<Mutex<>>`. The callback receives `&mut` access to your
    /// state on each event.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut state = MyState::default();
    /// agent_loop.run_mut(prompt, |event, state| {
    ///     state.steps += 1;  // 직접 가변 접근, 락 없음
    /// }).await?;
    /// ```
    pub async fn run_mut<S: Send + 'static>(
        &self,
        prompt: String,
        state: S,
        mut emit: impl FnMut(AgentEvent, &mut S) + Send + 'static,
    ) -> Result<(Vec<AgentEvent>, S)> {
        let message = Message::User(UserMessage::new(prompt));

        // FnMut를 Arc<Mutex<>>로 래핑하여 Fn으로 변환
        let emit_fnmut = Arc::new(Mutex::new(emit));
        let state = Arc::new(Mutex::new(state));

        let emit_fn: EmitFn = Arc::new(move |event: AgentEvent| {
            let mut cb = emit_fnmut.lock();
            let mut s = state.lock();
            cb(event, &mut s);
        });

        let events = self.run_messages(vec![message], emit_fn).await?;

        // 상태 반환
        let state = Arc::try_unwrap(state)
            .unwrap_or_else(|arc| arc.into_inner())
            .into_inner();
        Ok((events, state))
    }
}
```

**B. oxios 사용 예 (개선 후)**

```rust
// agent_runtime.rs — 개선 전
let exec_state = Arc::new(Mutex::new(ExecuteState::default()));
let exec_state_clone = Arc::clone(&exec_state);
agent_loop.run(prompt, move |event| {
    let mut s = exec_state_clone.lock();
    match event { ... }
}).await;
let s = exec_state.lock();
Ok((s.final_content.clone(), s.steps_completed, s.success))

// agent_runtime.rs — 개선 후
#[derive(Default)]
struct ExecuteState {
    final_content: String,
    steps_completed: usize,
    success: bool,
}

let (_, state) = agent_loop.run_mut(prompt, ExecuteState::default(), |event, s| {
    match event {
        AgentEvent::ToolExecutionEnd { is_error: false, .. } => {
            s.steps_completed += 1;  // 락 없이 직접 접근
        }
        AgentEvent::AgentEnd { messages, stop_reason, .. } => {
            if let Some(Message::Assistant(a)) = messages.last() {
                s.final_content = a.text_content();
            }
            s.success = stop_reason.as_deref() == Some("Stop");
        }
        AgentEvent::Error { message, .. } => {
            s.final_content = message.clone();
            s.success = false;
        }
        _ => {}
    }
}).await?;

Ok((state.final_content, state.steps_completed, state.success))
```

### 고려사항

`run_mut`은 내부적으로 여전히 `Arc<Mutex<>>`를 사용하지만, **컨슈머 코드에서 락이 사라짐**. 콜백 내부에서 락이 이미 잡힌 상태로 호출되므로 dead-lock 위험이 없음.

향후 Rust의 `FnMut` in `dyn` 지원이 안정화되면 내부 래핑도 제거 가능.

### 변경 파일

| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/agent_loop/mod.rs` | `run_mut()` 메서드 추가 |
| `oxicode-sdk/src/lib.rs` | `run_mut` 관련 타입 re-export (변경 없을 수도 있음) |
| `oxios/crates/oxios-kernel/src/agent_runtime.rs` | `run_mut()`으로 전환 |

---

## 해결 4: `ToolRegistry` 내장 도구 팩토리

### 현황

`tool_factory.rs`가 `oxicode-sdk`에 존재하며, `AgentBuilder.coding_tools()`를 통해 사용됨. 하지만 `AgentLoop`를 직접 사용하는 oxios는 `ToolRegistry` 수준의 팩토리가 필요:

```rust
// oxios는 항상 이렇게 해야 함
let registry = ToolRegistry::new();
register_tools_from_cspace(&registry, ...);  // oxios 전용 등록 함수
```

### 설계 — `ToolRegistry`에 `extend` 메서드 + 팩토리 트레이트

**A. `ToolRegistry::extend_from()` 메서드 추가**

```rust
// oxicode-agent/src/tools.rs

impl ToolRegistry {
    /// Extend this registry with all tools from another registry.
    ///
    /// Useful for composing tool sets from multiple sources
    /// (e.g., coding tools + kernel tools + browser tools).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let base = ToolRegistry::coding_tools("/workspace");
    /// base.extend_from(&ToolRegistry::browser_tools(engine));
    /// ```
    pub fn extend_from(&self, other: &ToolRegistry) {
        for name in other.names() {
            if let Some(tool) = other.get(&name) {
                self.register_arc(tool);
            }
        }
    }

    /// Create a registry with the standard coding tools.
    ///
    /// Includes: read, write, edit, ls, grep, find, bash.
    pub fn coding_tools(cwd: &Path) -> Arc<ToolRegistry> {
        crate::tool_factory_default(cwd)
    }

    /// Create a registry with read-only tools.
    ///
    /// Includes: read, ls, grep, find.
    pub fn readonly_tools(cwd: &Path) -> Arc<ToolRegistry> {
        crate::tool_factory_readonly(cwd)
    }
}
```

**B. `tool_factory.rs`를 공개 팩토리로 격상**

현재 `tool_factory.rs`는 이미 `pub` 함수이지만, `ToolRegistry`의 연관 메서드처럼 보이도록 재배치:

```rust
// oxicode-sdk/src/tool_factory.rs — 기존 유지, 추가로 연관 메서드 래핑

// 기존 함수 그대로 유지 (하위 호환)
pub fn coding_tools(cwd: &Path) -> Arc<ToolRegistry> { ... }
pub fn readonly_tools(cwd: &Path) -> Arc<ToolRegistry> { ... }
pub fn browsing_tools(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> { ... }
```

**C. oxios 사용 예 (개선 후)**

```rust
// agent_runtime.rs — 개선 전
let registry = ToolRegistry::new();
let search_cache = Arc::new(SearchCache::new());
register_tools_from_cspace(&registry, &kernel_handle, &cspace, search_cache, agent_id);

// agent_runtime.rs — 개선 후
let registry = ToolRegistry::new();

// 파일 도구는 팩토리로 한 번에 추가
if cspace.has_domain("filesystem") {
    registry.extend_from(&ToolRegistry::coding_tools(&workspace));
}

// 커널 도구는 기존대로 CSpace 기반 등록
register_kernel_tools_from_cspace(&registry, &kernel_handle, &cspace, search_cache, agent_id);

// 브라우저 도구도 팩토리로 추가
if cspace.has_domain("browser") {
    registry.extend_from(browsing_tools(browser_engine));
}
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/tools.rs` | `extend_from()` 메서드 추가 |
| `oxicode-sdk/src/lib.rs` | `extend_from`이 자동 re-export됨 (ToolRegistry through) |
| `oxios/crates/oxios-kernel/src/tools/registration.rs` | 팩토리 활용으로 단순화 |

---

## 해결 5: `CompactionEvent` 비동기 콜백 지원

### 현황

```rust
// agent_runtime.rs
AgentEvent::Compaction { event } => {
    if let CompactionEvent::Completed { result, .. } = event {
        let mm = mm.clone();
        tokio::spawn(async move {  // fire-and-forget, 에러 조용히 무시
            mm.remember(entry).await
        });
    }
}
```

문제:
1. `tokio::spawn`으로 에러가 조용히 사라짐
2. compaction 결과가 메모리에 실제로 저장되었는지 보장 불가
3. 런타임 핸들 구하기가 콜백 내에서 어색함

### 설계 — `AgentLoopConfig`에 compaction 훅 추가

**A. 비동기 compaction 훅 타입 정의**

```rust
// oxicode-agent/src/agent_loop/config.rs

/// Async hook invoked after compaction completes.
///
/// Unlike the `Compaction` event in the `Fn` callback, this hook
/// is async and its result is awaited. Errors are logged but don't
/// fail the agent loop.
///
/// # Example
///
/// ```ignore
/// let config = AgentLoopConfig {
///     on_compaction: Some(Arc::new(|result: CompactedContext| {
///         let entry = MemoryEntry::from_compaction(&result);
///         Box::pin(async move {
///             memory_manager.remember(entry).await
///         })
///     })),
///     ..Default::default()
/// };
/// ```
pub type CompactionHook = Arc<
    dyn Fn(CompactedContext) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;
```

**B. `AgentLoopConfig`에 필드 추가**

```rust
#[derive(Clone)]
pub struct AgentLoopConfig {
    // ... 기존 필드 ...

    /// Async hook invoked after context compaction completes.
    ///
    /// When `Some`, this is called with the compaction result after
    /// the compaction is applied. The future is awaited within the
    /// agent loop, so async operations (memory storage, logging, etc.)
    /// are safe here.
    ///
    /// Errors are logged at WARN level but don't fail the loop.
    pub on_compaction: Option<CompactionHook>,
}
```

**C. `AgentLoop` 내부에서 훅 호출**

```rust
// oxicode-agent/src/agent_loop/mod.rs (run_loop 내부)

// 기존 compaction 처리 로직 이후에:
if let Some(ref hook) = self.config.on_compaction {
    if let CompactedContext { summary, .. } = &compaction_result {
        let ctx = compaction_result.clone();
        match hook(ctx).await {
            Ok(()) => {
                tracing::debug!("Compaction hook completed successfully");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Compaction hook failed");
            }
        }
    }
}

// 기존 emit도 유지 (Fn 콜백 쪽에는 여전히 Compaction 이벤트 발생)
emit(AgentEvent::Compaction {
    event: CompactionEvent::Completed { result: compaction_result },
});
```

**D. oxios 사용 예 (개선 후)**

```rust
// agent_runtime.rs — 개선 전
AgentEvent::Compaction { event } => {
    let mm = mm.clone();
    tokio::spawn(async move {
        if let Err(e) = mm.remember(entry).await {
            tracing::warn!(error = %e, "Failed to save compaction summary");
        }
    });
}

// agent_runtime.rs — 개선 후
let loop_config = AgentLoopConfig {
    on_compaction: Some(Arc::new(|ctx: CompactedContext| {
        let entry = MemoryEntry::from_compaction(&ctx);
        let mm = memory_manager.clone();
        Box::pin(async move {
            mm.remember(entry).await
        })
    })),
    ..base_config
};
```

콜백 내 `tokio::spawn`이 사라지고, compaction 결과가 **보장적으로(guaranteed)** 메모리에 저장됨. 에러도 적절히 로깅됨.

### 변경 파일

| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/agent_loop/config.rs` | `CompactionHook` 타입 + `on_compaction` 필드 |
| `oxicode-agent/src/agent_loop/mod.rs` | compaction 완료 후 훅 호출 로직 |
| `oxicode-sdk/src/lib.rs` | `CompactionHook`, `CompactedContext` re-export |
| `oxios/crates/oxios-kernel/src/agent_runtime.rs` | `on_compaction` 훅으로 전환, `Compaction` 이벤트 매칭 제거 |

---

## 구현 우선순위

| 순서 | 해결 | 난이도 | 영향 | 근거 |
|------|------|--------|------|------|
| **1** | #1 `oxicode-ai` 의존 제거 | ⭐ | 의존성 감소 | import 변경만으로 완료, 즉각적 효과 |
| **2** | #4 `ToolRegistry` 팩토리 | ⭐⭐ | API 발견성 | `extend_from()`만 추가, 하위 호환 |
| **3** | #3 `FnMut` 콜백 | ⭐⭐ | 컨슈머 DX | 신규 메서드, 기존 API 불변 |
| **4** | #2 Credential 주입 | ⭐⭐⭐ | 아키텍처 | `oxicode-ai`에 신규 함수, `Oxicode` 구조 변경 |
| **5** | #5 Compaction 훅 | ⭐⭐ | 안정성 | `AgentLoopConfig` 확장, 내부 로직 수정 |

---

## 하위 호환성

모든 변경은 **가산적(additive)**입니다:

- **해결 1**: oxios 측 import 변경만. oxicode-sdk 변경 없음.
- **해결 2**: `OxicodeBuilder`에 신규 메서드. 기존 `build()` 동작 불변.
- **해결 3**: 신규 `run_mut()` 메서드. 기존 `run()` 시그니처 불변.
- **해결 4**: `ToolRegistry`에 신규 메서드. 기존 API 불변.
- **해결 5**: `AgentLoopConfig`에 `Option` 필드 추가 (`Default::default()` 불변).

**SemVer**: 모두 **PATCH** 또는 **MINOR** 수준. breaking change 없음.

---

## 다이어그램: 개선 전후 비교

### Before

```
oxios-kernel
├── Cargo.toml: oxicode-sdk + oxicode-ai  ← 2개 의존
├── engine.rs: OxicodeBuilder::new().with_builtins().build()  ← credential 없음
├── agent_runtime.rs:
│   ├── Arc<Mutex<ExecuteState>>  ← Fn 콜백 우회
│   ├── AgentLoopConfig { api_key: Some(...) }  ← 수동 키 주입
│   └── tokio::spawn(mm.remember(...))  ← fire-and-forget compaction
└── tools/registration.rs: 수동 도구 등록
```

### After

```
oxios-kernel
├── Cargo.toml: oxicode-sdk  ← 1개 의존!
├── engine.rs:
│   OxicodeBuilder::new()
│     .with_builtins()
│     .api_key("anthropic", store.resolve("anthropic"))  ← 빌드 시 credential
│     .build()
├── agent_runtime.rs:
│   ├── ExecuteState (stack)  ← FnMut 콜백, Mutex 없음
│   ├── AgentLoopConfig { on_compaction: Some(...) }  ← 보장적 비동기 훅
│   └── (api_key 불필요)  ← OxicodeBuilder에서 처리
└── tools/registration.rs:
    registry.extend_from(&ToolRegistry::coding_tools(&workspace))  ← 팩토리
```
