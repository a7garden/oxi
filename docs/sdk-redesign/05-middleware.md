# 05. Middleware & Plugin 시스템

모듈 경로: `oxicode-sdk/src/middleware/`

---

## 5.1 설계 원칙

| 원칙 | 의미 |
|------|------|
| **Ordered chain** | 미들웨어는 등록 순서대로 실행. 첫 `Block`/`Terminate`에서 체인 중단 |
| **Phase-aware** | 각 미들웨어는 관심 있는 phase만 선언. 관심 없는 phase는 자동 스킵 |
| **Bridge pattern** | `MiddlewarePipeline`을 `AgentHooks`로 변환하는 adapter가 핵심 |
| **Composable** | SecurityMiddleware, RateLimitMiddleware 등이 독립적으로 작동하면서 하나의 파이프라인으로 조합 |

---

## 5.2 Middleware Trait

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewarePhase {
    BeforeLlm,
    AfterLlm,
    BeforeTool,
    AfterTool,
    BeforeRun,
    AfterRun,
}

#[derive(Debug, Clone)]
pub enum MiddlewareData {
    BeforeLlm { messages: Vec<Message>, model_id: String },
    AfterLlm { response_text: String, tokens_used: Option<TokenUsage> },
    BeforeTool { tool_name: String, params: Value },
    AfterTool { tool_name: String, params: Value, result: AgentToolResult },
    BeforeRun { prompt: String },
    AfterRun { response: String, success: bool, duration_ms: u64 },
}

#[derive(Debug, Clone)]
pub struct MiddlewareContext {
    pub phase: MiddlewarePhase,
    pub agent_id: String,
    pub trace_id: Option<TraceId>,
    pub data: MiddlewareData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiddlewareAction {
    Continue,
    Block,
    Terminate,
}

#[derive(Debug, Clone)]
pub struct MiddlewareResult {
    pub action: MiddlewareAction,
    pub modified_data: Option<MiddlewareData>,
    pub reason: Option<String>,
}

impl MiddlewareResult {
    pub fn pass() -> Self { Self { action: Continue, modified_data: None, reason: None } }
    pub fn modify(data: MiddlewareData) -> Self { Self { action: Continue, modified_data: Some(data), reason: None } }
    pub fn block(reason: impl Into<String>) -> Self { Self { action: Block, modified_data: None, reason: Some(reason.into()) } }
    pub fn terminate(reason: impl Into<String>) -> Self { Self { action: Terminate, modified_data: None, reason: Some(reason.into()) } }
}

#[async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;
    fn phases(&self) -> Vec<MiddlewarePhase>;
    async fn handle(&self, ctx: &MiddlewareContext) -> MiddlewareResult;
}
```

---

## 5.3 MiddlewarePipeline

```rust
pub struct MiddlewarePipeline {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewarePipeline {
    pub fn new() -> Self;
    pub fn add(self, mw: impl Middleware + 'static) -> Self;
    pub fn add_arc(self, mw: Arc<dyn Middleware>) -> Self;

    /// 체인 실행. 첫 non-Continue 결과에서 중단.
    pub async fn execute(&self, ctx: &MiddlewareContext) -> MiddlewareResult;
    pub fn names(&self) -> Vec<&str>;
}
```

---

## 5.4 Built-in Middleware

### RateLimitMiddleware

```rust
pub struct RateLimitMiddleware {
    max_calls_per_minute: usize,
    counters: Arc<RwLock<HashMap<String, (usize, u64)>>>,
}

// BeforeTool: 분당 호출 수 추적, 초과 시 Block
```

### TokenBudgetMiddleware

```rust
pub struct TokenBudgetMiddleware {
    max_tokens: usize,
    usage: Arc<AtomicU64>,
    cost_tracker: Option<Arc<CostTracker>>,
}

// AfterLlm: 누적 토큰 추적, 초과 시 Terminate
// CostTracker가 연결되면 비용 기반 종료도 지원
```

### LoggingMiddleware

```rust
pub struct LoggingMiddleware { level: tracing::Level }

// BeforeTool, AfterTool, AfterRun: tracing 로그
```

### ContentFilterMiddleware

```rust
pub struct ContentFilterMiddleware { blocked_patterns: Vec<String> }

// AfterLlm, BeforeTool: 패턴 매칭으로 차단
```

---

## 5.5 Pipeline → AgentHooks Bridge

> **이전 설계에서 누락되었던 핵심 메커니즘.**
>
> `oxicode-agent`의 `AgentHooks`는 `before_tool_call: Option<Box<dyn Fn>>`으로 **하나의 훅만** 지원.
> 여러 middleware를 하나의 `AgentHooks`로 컴파일하려면 adapter가 필요함.

```rust
// src/middleware/bridge.rs

/// MiddlewarePipeline을 AgentHooks로 변환하는 adapter.
///
/// 여러 middleware를 하나의 before_tool_call / after_tool_call 클로저로 컴파일.
/// 각 클로저 내부에서 pipeline.execute()를 호출하여 체인을 실행.
pub struct MiddlewareBridge;

impl MiddlewareBridge {
    /// Pipeline을 AgentHooks로 변환.
    ///
    /// 생성된 hooks는:
    /// - before_tool_call: BeforeTool phase의 middleware 체인 실행
    /// - after_tool_call: AfterTool phase의 middleware 체인 실행
    /// - should_stop_after_turn: Terminate 감지 시 true 반환
    pub fn into_hooks(
        pipeline: Arc<MiddlewarePipeline>,
        agent_id: String,
        terminate_flag: Arc<AtomicBool>,
    ) -> AgentHooks {
        AgentHooks {
            before_tool_call: Some(Box::new({
                let pipeline = Arc::clone(&pipeline);
                let agent_id = agent_id.clone();
                let terminate_flag = Arc::clone(&terminate_flag);

                move |ctx: &BeforeToolCallContext| -> BeforeToolCallResult {
                    let mw_ctx = MiddlewareContext {
                        phase: MiddlewarePhase::BeforeTool,
                        agent_id: agent_id.clone(),
                        trace_id: None,  // TODO: trace context propagation
                        data: MiddlewareData::BeforeTool {
                            tool_name: ctx.tool_name.clone(),
                            params: ctx.args.clone(),
                        },
                    };

                    // 동기 실행 (middleware가 async이지만
                    // AgentHooks가 동기 콜백이므로 tokio::task::block_in_place 사용)
                    let rt = tokio::runtime::Handle::current();
                    let result = rt.block_on(pipeline.execute(&mw_ctx));

                    match result.action {
                        MiddlewareAction::Block => BeforeToolCallResult {
                            block: true,
                            reason: result.reason,
                        },
                        MiddlewareAction::Terminate => {
                            terminate_flag.store(true, Ordering::SeqCst);
                            BeforeToolCallResult {
                                block: true,
                                reason: result.reason,
                            }
                        }
                        MiddlewareAction::Continue => BeforeToolCallResult::default(),
                    }
                }
            })),

            after_tool_call: Some(Box::new({
                let pipeline = Arc::clone(&pipeline);
                let agent_id = agent_id.clone();

                move |ctx: &AfterToolCallContext| -> AfterToolCallResult {
                    let mw_ctx = MiddlewareContext {
                        phase: MiddlewarePhase::AfterTool,
                        agent_id: agent_id.clone(),
                        trace_id: None,
                        data: MiddlewareData::AfterTool {
                            tool_name: ctx.tool_name.clone(),
                            params: Value::Null,
                            result: AgentToolResult::success(ctx.result.clone()),
                        },
                    };

                    let rt = tokio::runtime::Handle::current();
                    let result = rt.block_on(pipeline.execute(&mw_ctx));

                    if result.action == MiddlewareAction::Terminate {
                        terminate_flag.store(true, Ordering::SeqCst);
                    }

                    AfterToolCallResult::default()
                }
            })),

            should_stop_after_turn: Some(Arc::new({
                let terminate_flag = Arc::clone(&terminate_flag);
                move |_ctx: &ShouldStopAfterTurnContext| -> bool {
                    terminate_flag.load(Ordering::SeqCst)
                }
            })),

            ..Default::default()
        }
    }
}
```

**작동 원리:**

```
AgentLoop                     MiddlewareBridge               MiddlewarePipeline
    │                               │                              │
    │── before_tool_call ──────────▶│                              │
    │                               │── execute(BeforeTool) ──────▶│
    │                               │                              │── [Security] ──▶ check cap
    │                               │                              │── [RateLimit] ─▶ check rate
    │                               │                              │── [Logging] ───▶ log
    │                               │◀── MiddlewareResult ─────────│
    │                               │                              │
    │◀── BeforeToolCallResult ──────│                              │
    │                               │                              │
    │  (block=true면 툴 실행 스킵)   │                              │
```

---

## 5.6 Plugin Loader

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub phases: Vec<String>,
    pub entry_point: String,
    pub permissions: Vec<String>,
}

pub struct PluginLoader {
    plugins_dir: PathBuf,
    loaded: Arc<RwLock<HashMap<String, Arc<dyn Middleware>>>>,
}

impl PluginLoader {
    pub fn new(plugins_dir: impl Into<PathBuf>) -> Self;
    pub async fn load(&self, manifest_path: &Path) -> anyhow::Result<String>;
    pub fn middleware(&self) -> Vec<Arc<dyn Middleware>>;
    pub fn unload(&self, name: &str) -> bool;
}
```

> WASM 플러그인은 extism/wasmtime 연동으로 Phase 7에서 다룸.

---

## 5.7 사용 예시

```rust
use oxicode_sdk::middleware::*;

// 1. Pipeline 구성
let pipeline = Arc::new(MiddlewarePipeline::new()
    .add(SecurityMiddleware::new(authorizer.clone()))
    .add(RateLimitMiddleware::new(60))
    .add(LoggingMiddleware::new(tracing::Level::INFO))
    .add(TokenBudgetMiddleware::new(100_000))
    .add(ContentFilterMiddleware::new(vec!["rm -rf".into()]))
);

// 2. AgentHooks로 변환 (bridge)
let terminate_flag = Arc::new(AtomicBool::new(false));
let hooks = MiddlewareBridge::into_hooks(
    pipeline, "agent-001".into(), terminate_flag.clone(),
);

// 3. Agent에 적용
let agent = oxicode.agent(config)
    .workspace("/project")
    .coding_tools()
    .build()?;
agent.set_hooks(hooks);

// 또는 AgentBuilder에서 직접:
let agent = oxicode.agent(config)
    .workspace("/project")
    .coding_tools()
    .middleware(SecurityMiddleware::new(authorizer))
    .with_rate_limit(60)
    .with_token_budget(100_000)
    .build()?;  // build() 내부에서 bridge 자동 실행
```
