# oxi 0.14.0 — oxios Agent OS 엔진 설계서

**날짜:** 2026-05-16  
**기반:** OXIOS_INTEGRATION_PROPOSAL.md 분석 + 현재 코드베이스 분석  
**목표:** oxi를 oxios의 공식 엔진으로 만들기 위한 3-Phase 설계

---

## 현재 상태 진단 (코드 분석 기반)

### ✅ 이미 잘 된 것
- `OxiBuilder` → `Oxi` 엔진 인스턴스 격리 (글로벌 static 없음)
- `ProviderResolver` 트레이트 → `Agent::new_with_resolver()`로 주입 가능
- `AgentBuilder` 플루언트 API → `.workspace()`, `.coding_tools()`, `.custom_tool()`
- `ToolContext` 런타임 주입 → workspace_dir이 실행 시점에 전달됨
- `ClosureTool` → 클로저로 커스텀 도구 생성 (sync/async)
- `AgentEvent` 풍부한 이벤트 시스템 → lifecycle, tool, compaction, retry
- `SharedState` / `AgentState` → parking_lot::RwLock 기반 스레드 안전

### ❌ 해결해야 할 것
1. **`std::sync::mpsc` 채널** → `Agent::run()`이 `std::sync::mpsc::channel` 사용. tokio 생태계와 안 맞음
2. **AgentLoop 재생성** → `run_with_channel_inner()`가 매 호출마다 `AgentLoop::new_with_resolver()`로 새 인스턴스 생성
3. **`AgentState` 직렬화 불가** → `#[derive(Clone)]`은 있지만 `Serialize/Deserialize` 없음
4. **커널 도구 브릿지 없음** → oxios의 exec, mcp, memory 등을 SDK 레벨에서 주입할 방법 없음
5. **ProviderPool 없음** → 동시 API 호출 rate limiting 불가
6. **AgentGroup 없음** → 다중 에이전트 오케스트레이션 불가
7. **Structured Output 없음** → JSON Schema 검증 불가

---

## Phase 1: 기반 인프라 (P0 — oxios 연결을 위해 반드시 필요)

> 목표: oxios-kernel이 oxi-sdk만 의존해서 에이전트를 실행할 수 있게 만든다

### 1-1. tokio 채널 기반 EventStream

**문제:** 현재 `Agent::run()`이 `std::sync::mpsc::channel` 사용. tokio 비동기 런타임에서 recv()가 블로킹.

**해결:** `tokio::sync::mpsc` 채널 옵션 추가.

```rust
// oxi-agent/src/agent.rs — 새 메서드 추가

impl Agent {
    /// tokio 채널로 이벤트 스트리밍. WebSocket/SSE 게이트웨이 친화적.
    pub async fn run_tokio_stream(
        &self,
        prompt: String,
    ) -> Result<(
        tokio::sync::mpsc::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<Response>>,
    )> {
        let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
        
        // is_running 가드
        if self.is_running.compare_exchange(
            false, true,
            Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
            return Err(Error::msg("Agent is already running"));
        }

        let agent = self.clone_inner(); // Arc로 감싸서 클론 가능하게
        let handle = tokio::spawn(async move {
            let result = agent.run_with_tokio_channel(prompt, tx).await;
            agent.is_running.store(false, Ordering::SeqCst);
            result
        });

        Ok((rx, handle))
    }
}
```

**AgentLoop 수정점:** `run()`의 emit 콜백 시그니처를 `Box<dyn Fn(AgentEvent) + Send + Sync>` → 유지하되, Agent 레벨에서 `tokio::sync::mpsc::Sender::send()`를 감싸는 브릿지 제공:

```rust
// Agent::run_with_tokio_channel 내부
let tx_clone = tx.clone();
let emit_bridge = move |event: AgentEvent| {
    let _ = tx_clone.try_send(event); // non-blocking
};
```

> 기존 `run()`, `run_with_channel()`, `run_streaming()`은 그대로 유지 (호환성).
> 새 `run_tokio_stream()`만 추가.

### 1-2. AgentState 직렬화

**문제:** `AgentState`에 `Serialize/Deserialize` derive가 없어서 세션 저장/복원 불가.

**해결:** `AgentState`와 관련 타입에 Serialize/Deserialize 추가.

```rust
// oxi-agent/src/state.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub messages: Vec<Message>,        // Message는 이미 Serialize 가능
    pub iteration: usize,
    pub stop_reason: Option<StopReason>,
    pub tool_results: Vec<ToolResult>,
    pub total_tokens: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
}
```

Agent에 export/import 추가:

```rust
// oxi-agent/src/agent.rs
impl Agent {
    /// 상태를 JSON으로 직렬화하여 반환
    pub fn export_state(&self) -> Result<serde_json::Value> {
        let state = self.state.get_state();
        serde_json::to_value(&state)
            .map_err(|e| Error::msg(format!("State export failed: {}", e)))
    }

    /// JSON에서 상태를 복원
    pub fn import_state(&self, value: serde_json::Value) -> Result<()> {
        let state: AgentState = serde_json::from_value(value)
            .map_err(|e| Error::msg(format!("State import failed: {}", e)))?;
        self.state.update(|s| *s = state);
        Ok(())
    }
}
```

### 1-3. Agent 재사용 (세션 컨티뉴에이션)

**문제:** `run_with_channel_inner()`가 매 호출마다 새 `AgentLoop`를 생성하고, 실행 후 상태를 다시 동기화. AgentLoop가 Agent에 소속되지 않고 일회성.

**해결:** AgentLoop를 Agent 내부에 캐싱하고, 상태를 직접 공유.

```rust
// oxi-agent/src/agent.rs 수정

pub struct Agent {
    inner: RwLock<AgentInner>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
    hooks: parking_lot::RwLock<AgentHooks>,
    is_running: AtomicBool,
    resolver: Arc<dyn ProviderResolver>,
    // ↓ 새 필드: 캐시된 AgentLoop
    cached_loop: RwLock<Option<Arc<AgentLoop>>>,
}

impl Agent {
    /// 기존 대화에 이어서 새 프롬프트 실행 (세션 유지)
    pub async fn continue_with(&self, prompt: String) -> Result<(Response, Vec<AgentEvent>)> {
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        let result = self.continue_with_channel(prompt, tx).await;
        let mut events = Vec::new();
        while let Ok(event) = rx.recv() {
            events.push(event);
        }
        result.map(|r| (r, events))
    }

    /// 내부: 기존 상태 기반으로 계속 실행
    async fn continue_with_channel(
        &self,
        prompt: String,
        tx: std::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Response> {
        // is_running 가드
        if self.is_running.compare_exchange(
            false, true,
            Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
            return Err(Error::msg("Agent is already running"));
        }

        // 캐시된 AgentLoop 재사용 또는 새로 생성
        let agent_loop = {
            let cached = self.cached_loop.read();
            if let Some(al) = cached.as_ref() {
                Arc::clone(al)
            } else {
                drop(cached);
                self.create_agent_loop()
            }
        };

        // ... 실행 로직 (run_with_channel_inner와 유사하지만 상태 유지)
        let result = self.execute_with_loop(&agent_loop, prompt, tx).await;
        self.is_running.store(false, Ordering::SeqCst);
        
        // 캐시에 저장
        let mut cached = self.cached_loop.write();
        *cached = Some(agent_loop);
        
        result
    }

    fn create_agent_loop(&self) -> Arc<AgentLoop> {
        let inner = self.inner.read();
        let loop_config = self.build_loop_config(&inner);
        let fresh_state = SharedState::new();
        let current = self.state.get_state();
        fresh_state.update(|s| *s = current);

        Arc::new(AgentLoop::new_with_resolver(
            Arc::clone(&inner.provider),
            loop_config,
            Arc::clone(&self.tools),
            fresh_state,
            Arc::clone(&self.resolver),
        ))
    }
}
```

### 1-4. KernelToolProvider 트레이트

**문제:** oxios의 커널 도구(exec, mcp, memory, browser, persona)를 SDK에 플러그인할 방법 없음.

**해결:** `oxi-sdk`에 `KernelToolProvider` 트레이트 추가.

```rust
// oxi-sdk/src/kernel_bridge.rs (새 파일)

use std::path::PathBuf;
use oxi_agent::{AgentTool, ToolRegistry, ToolContext};

/// 커널 컨텍스트 정보. 커널 도구가 등록 시 필요한 메타데이터.
#[derive(Debug, Clone)]
pub struct KernelToolContext {
    /// 에이전트의 워크스페이스 디렉토리
    pub workspace_dir: PathBuf,
    /// oxios 에이전트 ID
    pub agent_id: String,
    /// 세션 ID (있는 경우)
    pub session_id: Option<String>,
    /// CSpace 기반 권한 목록
    pub permissions: Vec<String>,
}

/// 커널 도구 프로바이더 트레이트.
/// oxios-kernel이 구현해서 SDK에 주입.
pub trait KernelToolProvider: Send + Sync {
    /// 제공할 도구 이름 목록 반환
    fn tool_names(&self) -> Vec<&str>;

    /// 도구들을 ToolRegistry에 등록
    fn register_tools(
        &self,
        registry: &ToolRegistry,
        context: &KernelToolContext,
    );
}
```

AgentBuilder에 `kernel_tools()` 메서드 추가:

```rust
// oxi-sdk/src/agent_builder.rs
impl<'a> AgentBuilder<'a> {
    /// 커널 도구 프로바이더로부터 도구 등록
    pub fn kernel_tools(
        mut self,
        provider: &dyn KernelToolProvider,
        context: &KernelToolContext,
    ) -> Self {
        provider.register_tools(&self.tools, context);
        self
    }
}
```

oxios-kernel 구현 예시:

```rust
// oxios-kernel/src/bridge.rs
pub struct OxiosKernelBridge {
    kernel: Arc<Kernel>,
}

impl KernelToolProvider for OxiosKernelBridge {
    fn tool_names(&self) -> Vec<&str> {
        vec!["exec", "memory", "browser", "persona"]
    }

    fn register_tools(&self, registry: &ToolRegistry, context: &KernelToolContext) {
        registry.register(ExecTool::new(self.kernel.clone(), context.agent_id.clone()));
        registry.register(MemoryTool::new(self.kernel.clone(), context.agent_id.clone()));
        registry.register(BrowserTool::new(self.kernel.clone()));
        registry.register(PersonaTool::new(self.kernel.clone()));
    }
}
```

oxios-kernel 사용 예시:

```rust
// oxios-kernel에서 엔진 초기화
let oxi = OxiBuilder::new().with_builtins().build();
let bridge = OxiosKernelBridge::new(kernel);

let agent = oxi.agent(config)
    .workspace("/workspace")
    .coding_tools()
    .kernel_tools(&bridge, &KernelToolContext {
        workspace_dir: PathBuf::from("/workspace"),
        agent_id: "agent-001".into(),
        session_id: None,
        permissions: vec!["read".into(), "write".into(), "exec".into()],
    })
    .build()?;
```

---

## Phase 2: 다중 에이전트 인프라 (P1)

> 목표: oxios가 다중 에이전트를 효율적으로 운영할 수 있는 프리미티브 제공

### 2-1. ProviderPool (Rate Limiting + 동시성 제어)

```rust
// oxi-ai/src/provider_pool.rs (새 파일)

use std::sync::Arc;
use tokio::sync::Semaphore;
use std::time::Instant;

/// Rate limiting 정책
#[derive(Debug, Clone)]
pub struct RateLimitPolicy {
    /// 분당 최대 요청 수
    pub rpm: u32,
    /// 동시 요청 최대 수
    pub max_concurrent: usize,
}

impl RateLimitPolicy {
    pub fn rpm(rpm: u32) -> Self {
        Self { rpm, max_concurrent: rpm as usize / 6 } // 대략 10초 분배
    }

    pub fn per_second(rps: u32) -> Self {
        Self { rpm: rps * 60, max_concurrent: rps as usize }
    }
}

/// Provider를 래핑해서 rate limiting과 동시성 제어
pub struct ProviderPool {
    provider: Arc<dyn Provider>,
    semaphore: Arc<Semaphore>,
    rate_limiter: Arc<tokio::sync::Mutex<RateLimiterState>>,
    name: String,
}

struct RateLimiterState {
    rpm: u32,
    timestamps: Vec<Instant>,
}

impl ProviderPool {
    pub fn new(
        provider: Arc<dyn Provider>,
        policy: RateLimitPolicy,
        name: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            semaphore: Arc::new(Semaphore::new(policy.max_concurrent)),
            rate_limiter: Arc::new(tokio::sync::Mutex::new(RateLimiterState {
                rpm: policy.rpm,
                timestamps: Vec::new(),
            })),
            name: name.into(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for ProviderPool {
    async fn stream(
        &self,
        model: oxi_ai::Model,
        context: oxi_ai::Context,
        options: oxi_ai::StreamOptions,
    ) -> Result<oxi_ai::ProviderStream, oxi_ai::ProviderError> {
        // 1. 동시성 제한 획득
        let _permit = self.semaphore.acquire().await
            .map_err(|_| oxi_ai::ProviderError::RateLimited("Pool exhausted".into()))?;

        // 2. Rate limiting 대기
        {
            let mut limiter = self.rate_limiter.lock().await;
            limiter.wait_for_quota().await;
        }

        // 3. 실제 provider에 위임
        self.provider.stream(model, context, options).await
    }
}
```

SDK 통합:

```rust
// oxi-sdk/src/builder.rs
impl OxiBuilder {
    /// 프로바이더에 rate limiting 풀 적용
    pub fn provider_pool(
        mut self,
        name: &str,
        policy: RateLimitPolicy,
    ) -> Self {
        // build 시점에 풀 생성
        self.provider_pools.insert(name.to_string(), policy);
        self
    }
}
```

### 2-2. AgentGroup (다중 에이전트 오케스트레이션)

```rust
// oxi-sdk/src/agent_group.rs (새 파일)

use std::sync::Arc;
use oxi_agent::Agent;
use tokio::task::JoinSet;

/// 다중 에이전트 실행 전략
#[derive(Debug, Clone)]
pub enum GroupStrategy {
    /// 순차 실행. 이전 에이전트의 결과가 다음 에이전트에 전달.
    Pipeline,

    /// 병렬 실행. 모든 에이전트가 동시에 실행되고 결과를 취합.
    Parallel {
        /// 최대 동시 실행 수
        max_concurrency: usize,
    },

    /// 리더 에이전트가 작업을 분배
    Orchestrated {
        /// 리더 에이전트의 인덱스
        leader: usize,
    },
}

/// 에이전트 그룹 실행 결과
#[derive(Debug)]
pub struct GroupResult {
    /// 각 에이전트의 결과 (인덱스 순)
    pub results: Vec<AgentGroupOutput>,
    /// 총 실행 시간 (ms)
    pub total_duration_ms: u64,
}

#[derive(Debug)]
pub struct AgentGroupOutput {
    /// 에이전트 이름
    pub name: String,
    /// 최종 응답 텍스트
    pub content: String,
    /// 성공 여부
    pub success: bool,
    /// 에러 메시지 (실패 시)
    pub error: Option<String>,
}

/// 다중 에이전트 그룹
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
}

impl AgentGroup {
    pub fn new(strategy: GroupStrategy) -> Self {
        Self { agents: Vec::new(), strategy }
    }

    /// 에이전트 추가
    pub fn agent(mut self, agent: Arc<Agent>) -> Self {
        self.agents.push(agent);
        self
    }

    /// 그룹 실행
    pub async fn run(&self, prompt: String) -> anyhow::Result<GroupResult> {
        let start = std::time::Instant::now();

        match &self.strategy {
            GroupStrategy::Pipeline => self.run_pipeline(prompt).await,
            GroupStrategy::Parallel { max_concurrency } => {
                self.run_parallel(prompt, *max_concurrency).await
            }
            GroupStrategy::Orchestrated { leader } => {
                self.run_orchestrated(prompt, *leader).await
            }
        }.map(|results| GroupResult {
            total_duration_ms: start.elapsed().as_millis() as u64,
            results,
        })
    }

    async fn run_pipeline(&self, prompt: String) -> anyhow::Result<Vec<AgentGroupOutput>> {
        let mut results = Vec::new();
        let mut current_input = prompt;

        for agent in &self.agents {
            let (response, _events) = agent.run(current_input.clone()).await?;
            results.push(AgentGroupOutput {
                name: agent.model_id(),
                content: response.content.clone(),
                success: true,
                error: None,
            });
            current_input = response.content;
        }

        Ok(results)
    }

    async fn run_parallel(
        &self,
        prompt: String,
        max_concurrency: usize,
    ) -> anyhow::Result<Vec<AgentGroupOutput>> {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
        let mut handles = Vec::new();

        for agent in &self.agents {
            let agent = Arc::clone(agent);
            let prompt = prompt.clone();
            let sem = Arc::clone(&semaphore);

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                match agent.run(prompt).await {
                    Ok((response, _)) => AgentGroupOutput {
                        name: agent.model_id(),
                        content: response.content,
                        success: true,
                        error: None,
                    },
                    Err(e) => AgentGroupOutput {
                        name: agent.model_id(),
                        content: String::new(),
                        success: false,
                        error: Some(e.to_string()),
                    },
                }
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await?);
        }
        Ok(results)
    }

    async fn run_orchestrated(
        &self,
        prompt: String,
        leader_idx: usize,
    ) -> anyhow::Result<Vec<AgentGroupOutput>> {
        // 리더가 작업 분배 프롬프트를 생성 → 워커들이 실행 → 리더가 취합
        // (실제 구현은 oxios의 Ouroboros 프로토콜에 맞춰 커스터마이징)
        let leader = &self.agents[leader_idx];
        let (response, _) = leader.run(prompt).await?;
        Ok(vec![AgentGroupOutput {
            name: leader.model_id(),
            content: response.content,
            success: true,
            error: None,
        }])
    }
}
```

### 2-3. Structured Output

```rust
// oxi-agent/src/structured_output.rs (새 파일)

use serde_json::Value;

/// 출력 모드
#[derive(Debug, Clone, Default)]
pub enum OutputMode {
    /// 텍스트 그대로 반환
    #[default]
    Text,
    /// 마지막 메시지에서 JSON 추출
    Json,
    /// JSON Schema로 검증 후 반환
    ValidatedJson {
        /// JSON Schema
        schema: Value,
    },
}

/// 구조화된 출력 추출기
pub struct StructuredOutput;

impl StructuredOutput {
    /// 에이전트의 마지막 응답에서 구조화된 출력 추출
    pub fn extract(
        content: &str,
        mode: &OutputMode,
    ) -> Result<Value, String> {
        match mode {
            OutputMode::Text => Ok(Value::String(content.to_string())),
            OutputMode::Json => Self::extract_json(content),
            OutputMode::ValidatedJson { schema } => {
                let json = Self::extract_json(content)?;
                Self::validate(&json, schema)?;
                Ok(json)
            }
        }
    }

    fn extract_json(content: &str) -> Result<Value, String> {
        // 1. 전체가 JSON인지 확인
        if let Ok(v) = serde_json::from_str::<Value>(content) {
            return Ok(v);
        }

        // 2. ```json ... ``` 블록에서 추출
        if let Some(start) = content.find("```json") {
            let json_start = start + 7;
            if let Some(end) = content[json_start..].find("```") {
                let json_str = &content[json_start..json_start + end];
                return serde_json::from_str(json_str)
                    .map_err(|e| format!("JSON parse error: {}", e));
            }
        }

        // 3. { ... } 또는 [ ... ] 블록에서 추출
        for (open, close) in [('{', '}'), ('[', ']')] {
            if let Some(start) = content.find(open) {
                let substr = &content[start..];
                if let Some(end) = Self::find_matching_bracket(substr, open, close) {
                    let json_str = &substr[..=end];
                    if let Ok(v) = serde_json::from_str(json_str) {
                        return Ok(v);
                    }
                }
            }
        }

        Err("No JSON found in response".into())
    }

    fn validate(json: &Value, schema: &Value) -> Result<(), String> {
        // jsonschema 크레이트 사용 (선택적 의존성)
        // 또는 간단한 타입 체크만 구현
        Ok(())
    }

    fn find_matching_bracket(s: &str, open: char, close: char) -> Option<usize> {
        let mut depth = 0;
        for (i, c) in s.char_indices() {
            match c {
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }
}
```

AgentConfig에 output_mode 추가:

```rust
// oxi-agent/src/config.rs
pub struct AgentConfig {
    // ... 기존 필드
    /// 출력 모드 (기본: Text)
    #[serde(default)]
    pub output_mode: Option<OutputMode>,
}
```

---

## Phase 3: 고급 기능 (P2)

> 목표: Agent OS로서의 완성도 높이기

### 3-1. 에이전트 간 통신 (Inter-Agent Message Bus)

```rust
// oxi-sdk/src/message_bus.rs (새 파일)

use tokio::sync::broadcast;
use std::sync::Arc;

/// 에이전트 간 메시지
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterAgentMessage {
    pub from: String,
    pub to: Option<String>,  // None = broadcast
    pub message_type: String,
    pub payload: serde_json::Value,
    pub timestamp: u64,
}

/// 에이전트 간 통신 버스
pub struct MessageBus {
    sender: broadcast::Sender<InterAgentMessage>,
}

impl MessageBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { sender: tx }
    }

    /// 메시지 발행
    pub fn publish(&self, msg: InterAgentMessage) {
        let _ = self.sender.send(msg);
    }

    /// 구독자 생성
    pub fn subscribe(&self) -> broadcast::Receiver<InterAgentMessage> {
        self.sender.subscribe()
    }

    /// 특정 에이전트 대상 필터링 구독
    pub fn subscribe_for(
        &self,
        agent_id: String,
    ) -> impl tokio_stream::Stream<Item = InterAgentMessage> {
        let rx = self.sender.subscribe();
        tokio_stream::wrappers::BroadcastStream::new(rx)
            .filter_map(move |result| async move {
                match result {
                    Ok(msg) if msg.to.as_deref() == Some(&agent_id) => Some(msg),
                    Ok(msg) if msg.to.is_none() => Some(msg),
                    _ => None,
                }
            })
    }
}
```

### 3-2. 에이전트 Health Check & Metrics

```rust
// oxi-sdk/src/metrics.rs (새 파일)

use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use std::sync::Arc;

/// 에이전트 실행 메트릭
#[derive(Debug, Default)]
pub struct AgentMetrics {
    /// 총 실행 횟수
    pub total_runs: AtomicU64,
    /// 성공한 실행 횟수
    pub successful_runs: AtomicU64,
    /// 실패한 실행 횟수
    pub failed_runs: AtomicU64,
    /// 총 토큰 사용량
    pub total_tokens: AtomicU64,
    /// 총 도구 호출 횟수
    pub tool_calls: AtomicU64,
    /// 평균 실행 시간 (ms)
    pub avg_duration_ms: AtomicU64,
}

impl AgentMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_runs: self.total_runs.load(Ordering::Relaxed),
            successful_runs: self.successful_runs.load(Ordering::Relaxed),
            failed_runs: self.failed_runs.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
            avg_duration_ms: self.avg_duration_ms.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct MetricsSnapshot {
    pub total_runs: u64,
    pub successful_runs: u64,
    pub failed_runs: u64,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub avg_duration_ms: u64,
}
```

---

## 파일 변경 요약

### 새 파일
| 파일 | Phase | 설명 |
|------|-------|------|
| `oxi-sdk/src/kernel_bridge.rs` | P1 | `KernelToolProvider` 트레이트 + `KernelToolContext` |
| `oxi-sdk/src/agent_group.rs` | P2 | `AgentGroup` 다중 에이전트 오케스트레이션 |
| `oxi-sdk/src/message_bus.rs` | P3 | 에이전트 간 통신 버스 |
| `oxi-sdk/src/metrics.rs` | P3 | 에이전트 실행 메트릭 |
| `oxi-ai/src/provider_pool.rs` | P2 | `ProviderPool` rate limiting |
| `oxi-agent/src/structured_output.rs` | P2 | `StructuredOutput` + `OutputMode` |

### 수정 파일
| 파일 | Phase | 변경 내용 |
|------|-------|----------|
| `oxi-agent/src/state.rs` | P1 | `AgentState`에 `Serialize/Deserialize` 추가 |
| `oxi-agent/src/agent.rs` | P1 | `export_state()`, `import_state()`, `continue_with()`, `run_tokio_stream()`, `cached_loop` 필드 추가 |
| `oxi-agent/src/config.rs` | P2 | `output_mode` 필드 추가 |
| `oxi-sdk/src/builder.rs` | P2 | `provider_pool()` 메서드 추가 |
| `oxi-sdk/src/agent_builder.rs` | P1 | `kernel_tools()` 메서드 추가 |
| `oxi-sdk/src/lib.rs` | P1-P3 | 새 모듈 re-export |

---

## 의존성 추가

```toml
# oxi-sdk/Cargo.toml
[dependencies]
tokio = { version = "1", features = ["sync", "time"] }
tokio-stream = "0.1"

# oxi-ai/Cargo.toml (선택적)
[dependencies]
tokio = { version = "1", features = ["sync", "time"] }
```

---

## oxios-kernel 통합 예시 (Phase 1 완료 후)

```rust
// oxios-kernel/src/engine.rs (전체 교체)

use oxi_sdk::{
    OxiBuilder, AgentConfig, KernelToolProvider, KernelToolContext,
};

pub struct OxiosEngine {
    oxi: oxi_sdk::Oxi,
    kernel_bridge: Arc<dyn KernelToolProvider>,
}

impl OxiosEngine {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        let oxi = OxiBuilder::new()
            .with_builtins()
            .build();

        Self {
            oxi,
            kernel_bridge: Arc::new(OxiosKernelBridge::new(kernel)),
        }
    }

    /// 에이전트 생성 (oxios-kernel 관점)
    pub fn create_agent(
        &self,
        agent_id: &str,
        model_id: &str,
        workspace: &str,
    ) -> anyhow::Result<Arc<oxi_agent::Agent>> {
        let config = AgentConfig {
            name: agent_id.to_string(),
            model_id: model_id.to_string(),
            max_iterations: 20,
            ..Default::default()
        };

        let agent = self.oxi.agent(config)
            .workspace(workspace)
            .coding_tools()
            .kernel_tools(self.kernel_bridge.as_ref(), &KernelToolContext {
                workspace_dir: workspace.into(),
                agent_id: agent_id.to_string(),
                session_id: None,
                permissions: vec!["read".into(), "write".into(), "exec".into()],
            })
            .build()?;

        Ok(Arc::new(agent))
    }

    /// 에이전트 실행 (tokio 스트리밍)
    pub async fn run_agent(
        &self,
        agent: &Arc<oxi_agent::Agent>,
        prompt: String,
    ) -> anyhow::Result<(
        tokio::sync::mpsc::Receiver<oxi_agent::AgentEvent>,
        tokio::task::JoinHandle<anyhow::Result<oxi_agent::Response>>,
    )> {
        let (rx, handle) = agent.run_tokio_stream(prompt).await?;
        Ok((rx, handle))
    }

    /// 세션 저장
    pub fn save_session(&self, agent: &oxi_agent::Agent) -> anyhow::Result<serde_json::Value> {
        agent.export_state()
    }

    /// 세션 복원
    pub fn restore_session(
        &self,
        agent: &oxi_agent::Agent,
        state: serde_json::Value,
    ) -> anyhow::Result<()> {
        agent.import_state(state)
    }
}
```

**이제 `OxiEngineProvider` 트레이트, `spawn_blocking`, 글로벌 static 참조가 모두 사라짐.**

---

## 마일스톤

| Phase | 기간 | 산출물 |
|-------|------|--------|
| **Phase 1** | 1-2일 | tokio stream + state 직렬화 + 커널 브릿지 + Agent 재사용 |
| **Phase 2** | 2-3일 | ProviderPool + AgentGroup + Structured Output |
| **Phase 3** | 2-3일 | Message Bus + Metrics + 통합 테스트 |

Phase 1만 완료되면 oxios-kernel이 oxi-sdk만 의존해서 완전히 동작 가능. Phase 2-3은 oxios 기능 확장에 맞춰 점진적 도입.
