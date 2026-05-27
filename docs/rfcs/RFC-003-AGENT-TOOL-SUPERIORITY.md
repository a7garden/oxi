# RFC-003: Agent/Tool 우위 확보 — 내장 툴, MCP, Subagent 고도화

**상태**: 초안  
**우선순도**: P1 — oxi의 핵심 경쟁 우위 영역  
**현재 완성도**: ~115% (pi 대비 우위, 일부 정교화 필요)  
**목표**: 격차 확대 + pi에 없는 기능 정교화  

---

## 1. 현재 상태 분석

oxi는 이미 **20개 내장 툴**(AgentTool 트레이트 구현체)로 pi의 7개를 크게 능가한다. 하지만 일부 영역(pi급 툴 렌더링, 스트리밍 진행률)에서 정교함이 부족하다.

### 1.1 실제 툴 인벤토리 (oxi-agent/src/tools/ + mcp/)

**AgentTool 트레이트 구현체 (20개):**

| # | 툴 | 파일 | essential |
|---|-----|------|-----------|
| 1 | ReadTool | `read.rs` | ✅ |
| 2 | WriteTool | `write.rs` | ✅ |
| 3 | EditTool | `edit.rs` | ✅ |
| 4 | BashTool | `bash.rs` | ✅ |
| 5 | GrepTool | `grep.rs` | ✅ |
| 6 | FindTool | `find.rs` | ✅ |
| 7 | LsTool | `ls.rs` | ✅ |
| 8 | WebSearchTool | `web_search.rs` | |
| 9 | GetSearchResultsTool | `search_cache.rs` | |
| 10 | GitHubTool | `github.rs` | |
| 11 | GitHubSearchTool | `github_search.rs` | |
| 12 | SubagentTool | `subagent.rs` | |
| 13 | Context7ResolveLibraryIdTool | `context7.rs` | |
| 14 | Context7QueryDocsTool | `context7.rs` | |
| 15 | McpTool | `mcp/tool.rs` | |
| 16 | QuestionnaireTool | `questionnaire.rs` | |
| 17 | ToolDefinitionWrapper | `tool_definition_wrapper.rs` | |
| 18 | BrowseTool | `browse/browse_tool.rs` | |
| 19 | BrowseExtractTool | `browse/browse_extract_tool.rs` | |
| 20 | BrowseSessionTool | `browse/browse_session_tool.rs` | |
| 21 | BrowseScriptTool | `browse/browse_script_tool.rs` | `#[cfg(native-browser)]` |

> **Note**: 총 22개 `.rs` 파일이 `oxi-agent/src/tools/`에 있으나, 그 중 7개는 유틸리티 모듈로 `AgentTool`을 구현하지 않음: `path_security.rs`, `path_utils.rs`, `render_utils.rs`, `search_cache.rs`(캐시 유틸 + GetSearchResultsTool), `tool_definition_wrapper.rs`, `truncate.rs`, `file_mutation_queue.rs`, `http_client.rs`.

### 1.2 기존 인프라 (이미 구현됨)

이 RFC에서 제안하는 일부 기능은 이미 부분적으로 구현되어 있다. 중복을 피하기 위해 기존 구현을 명확히 파악한다.

| 기능 | 기존 구현 | 위치 | 상태 |
|------|----------|------|------|
| AgentTool 트레이트 | `name()`, `label()`, `description()`, `parameters_schema()`, `essential()`, `execute()`, `on_progress()`, `to_definition()` | `oxi-agent/src/tools.rs` | ✅ 완료 |
| 툴 렌더링 | `tool_renderer.rs` (705 LOC) | `oxi-tui/src/widgets/tool_renderer.rs` | ✅ 완료 (확장 필요) |
| 렌더링 유틸 | `render_utils.rs` (경로 단축, 바이너리 정제, 출력 미리보기) | `oxi-agent/src/tools/render_utils.rs` | ✅ 완료 |
| 동시 편집 직렬화 | `file_mutation_queue.rs` (파일별 Mutex, 자동 정리, 1024엔트리 캡) | `oxi-agent/src/tools/file_mutation_queue.rs` | ✅ 완료 |
| MCP Client | `tools/list`, `tools/call`, `resources/list`, `resources/read` | `oxi-agent/src/mcp/client.rs` | 🔶 부분 |
| Subagent | 단일, 병렬(max 4 동시), 체인 모드; 에이전트 발견(유저/프로젝트) | `oxi-agent/src/tools/subagent.rs` | ✅ 완료 |
| SDK 조정 | `WorkQueue`, `SharedMemory`, `Consensus`, `CoordinatedGroup` (fan-out, vote, map-reduce) | `oxi-sdk/src/coordination/` | ✅ 완료 |
| 진행 콜백 | `on_progress(ProgressCallback)` + `ProgressCallback = Arc<dyn Fn(String)>` | `oxi-agent/src/tools.rs` | 🔶 기본 |

### 1.3 oxi 우위 영역 (유지/확대)

| 기능 | pi | oxi | 전략 |
|------|----|-----|------|
| 내장 툴 수 | 7 | 20+ | 우위 유지 |
| MCP Client | ❌ | ✅ (tools + resources) | 정교화 |
| Subagent | ❌ | ✅ (single/parallel/chain) | 정교화 |
| Web Search | ❌ | ✅ (a3s-search + DDG 폴백) | 유지 |
| GitHub | ❌ | ✅ (gh CLI + REST API) | 유지 |
| Browse | ❌ | ✅ (4개 툴 + 세션 관리) | 안정화 |
| Context7 | ❌ | ✅ (resolve + query) | 유지 |
| SDK 조정 | ❌ | ✅ (WorkQueue, SharedMemory, Consensus) | 유지 |

### 1.4 보완 필요 영역 (pi에서 배울 점)

| 기능 | pi | oxi | 필요 작업 |
|------|----|-----|---------|
| 툴 커스텀 렌더링 | 툴별 render 함수 | `tool_renderer.rs` (범용) | 툴별 커스텀 렌더 지원 |
| 에디트 diff 품질 | 통합 diff + 배치 | `edit.rs` (기본) | diff 품질 향상 |
| 툴 실행 훅 | beforeToolCall/afterToolCall | BeforeToolCallHook | 동등 |
| 툴 결과 스트리밍 | onUpdate 콜백 (구조화) | `on_progress(String)` | 구조화된 진행 타입 |
| 에디트 충돌 감지 | concurrent edit 감지 | `file_mutation_queue` (직렬화만) | 내용 기반 충돌 감지 추가 |
| MCP 프로토콜 | — | tools + resources만 | prompts, sampling, logging 추가 |

---

## 2. 설계 원칙

1. **기존 구현 위에 구축**: `tool_renderer.rs`, `render_utils.rs`, `file_mutation_queue.rs`, `subagent.rs`를 대체하지 않고 확장.
2. **MCP 프로토콜 고급 기능**: 이미 구현된 `resources/list`, `resources/read` 위에 `prompts/*`, `sampling/*`, `logging/*` 추가.
3. **Subagent + SDK 통합**: 새 WorkflowEngine을 만들지 않고 기존 `SubagentTool`(chain/parallel)과 `oxi-sdk/coordination`(WorkQueue, CoordinatedGroup)을 활용.
4. **툴 출력 스트리밍**: 기존 `on_progress(String)`을 구조화된 `ToolProgress` enum으로 업그레이드.

---

## 3. 아키텍처

### 3.1 AgentTool 트레이트 확장

기존 트레이트(`tools.rs`)에 선택적 메서드를 추가한다. 기존 7개 메서드는 유지.

```rust
/// oxi-agent/src/tools.rs — 기존 트레이트에 선택적 확장 추가

#[async_trait]
pub trait AgentTool: Send + Sync {
    // ── 기존 (변경 없음) ──
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn essential(&self) -> bool { false }
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError>;
    
    // 기존: 기본 진행 콜백 (String)
    fn on_progress(&self, _callback: ProgressCallback) {}
    
    // 기존: ToolDefinition 변환
    fn to_definition(&self) -> ToolDefinition { /* ... */ }
    
    // ── 새로 추가 (선택적 오버라이드) ──
    
    /// 툴 호출 시각화 (TUI용).
    /// None이면 `tool_renderer.rs`의 기본 포매터 사용.
    /// 기존 `render_utils.rs` 유틸리티를 활용하여 구현.
    fn render_call(&self, params: &Value) -> Option<RenderOutput> {
        None  // 기본: tool_renderer.rs가 처리
    }
    
    /// 툴 결과 시각화 (TUI용).
    /// None이면 `tool_renderer.rs`의 기본 포매터 사용.
    fn render_result(&self, result: &AgentToolResult) -> Option<RenderOutput> {
        None  // 기본: tool_renderer.rs가 처리
    }
    
    /// 실행 모드 (병렬 안전성).
    /// 기존 `file_mutation_queue.rs`와 연동하여 파일 뮤테이션 직렬화.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::ParallelSafe
    }
}

/// 실행 모드 — file_mutation_queue와 연동
#[derive(Debug, Clone)]
pub enum ToolExecutionMode {
    /// 어떤 툴과도 병렬 실행 가능
    ParallelSafe,
    /// 이 툴만 순차 실행
    SequentialOnly,
    /// 특정 파일 뮤테이션 — file_mutation_queue로 동일 파일 직렬화
    MutatesFile(Arc<Path>),
    /// 읽기 전용 — 항상 병렬 가능
    ReadOnly,
}

/// 렌더 출력 — render_utils.rs 기존 유틸리티와 호환
#[derive(Debug)]
pub struct RenderOutput {
    /// 렌더링된 텍스트 줄 (마크다운 또는 플레인 텍스트)
    pub content: String,
    /// 기본적으로 접힘 여부
    pub collapsed: bool,
    /// 요약 텍스트 (TUI 푸터용)
    pub summary: Option<String>,
}
```

**기존 시스템과의 관계:**
- `oxi-tui/src/widgets/tool_renderer.rs` (705 LOC): 모든 툴의 기본 렌더링을 처리. `render_call`/`render_result`가 `None`을 반환하면 이 모듈이 자동으로 사용됨.
- `oxi-agent/src/tools/render_utils.rs`: `shorten_path()`, `sanitize_binary_output()`, `truncate_output_preview()` 등의 유틸리티. 새 `RenderOutput`은 이 유틸들을 호출하여 일관된 포맷팅 보장.

### 3.2 MCP 고도화

현재 oxi의 MCP는 `tools/list`, `tools/call`, `resources/list`, `resources/read`를 이미 지원한다 (`client.rs` 218-250라인). 누락된 고급 기능을 추가한다.

```rust
/// oxi-agent/src/mcp/client.rs 확장

/// MCP 서버 능력 (초기화 시 협상)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum McpCapability {
    Tools,           // tools/list, tools/call (✅ 이미 구현)
    Resources,       // resources/list, resources/read (✅ 이미 구현)
    Prompts,         // prompts/list, prompts/get (❌ 미구현)
    Sampling,        // sampling/createMessage (❌ 미구현)
    Logging,         // logging/setLevel + notifications (❌ 미구현)
    Completion,      // completion/complete (❌ 미구현)
}

impl McpClient {
    // ── 기존 (변경 없음) ──
    // async fn connect(...) → 초기화 + 능력 협상
    // async fn list_tools(...) → tools/list
    // async fn call_tool(...) → tools/call
    // async fn list_resources(...) → resources/list  (라인 218)
    // async fn read_resource(...) → resources/read   (라인 232)
    
    // ── 새로 추가 ──
    
    /// 프롬프트 템플릿 목록
    pub async fn list_prompts(&mut self) -> Result<Vec<McpPrompt>> {
        let result = self.send_request("prompts/list", None).await?;
        // ... 파싱
    }
    
    /// 프롬프트 가져오기 (인자 치환)
    pub async fn get_prompt(
        &mut self,
        name: &str,
        args: HashMap<String, String>,
    ) -> Result<Vec<oxi_ai::Message>> {
        let params = serde_json::json!({ "name": name, "arguments": args });
        let result = self.send_request("prompts/get", Some(params)).await?;
        // ... 파싱
    }
    
    /// 샘플링 요청 (MCP 서버가 LLM 호출을 요청)
    /// 에이전트 루프에 위임하여 처리
    pub async fn create_sample(
        &mut self,
        request: McpSamplingRequest,
    ) -> Result<oxi_ai::Message> {
        let params = serde_json::to_value(&request)?;
        let result = self.send_request("sampling/createMessage", Some(params)).await?;
        // ... 파싱 → AssistantMessage
    }
    
    /// 로그 레벨 설정
    pub async fn set_log_level(&mut self, level: McpLogLevel) -> Result<()> {
        let params = serde_json::json!({ "level": level.as_str() });
        self.send_request("logging/setLevel", Some(params)).await?;
        Ok(())
    }
}

/// 프롬프트 템플릿
#[derive(Debug, Clone)]
pub struct McpPrompt {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<McpPromptArgument>,
}

/// 샘플링 요청 (MCP 서버 → oxi LLM 호출 위임)
#[derive(Debug, Serialize)]
pub struct McpSamplingRequest {
    pub messages: Vec<serde_json::Value>,
    pub system_prompt: Option<String>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}
```

### 3.3 Subagent 워크플로우 — 기존 시스템과 통합

**새로운 WorkflowEngine을 만들지 않는다.** 기존 시스템이 이미 역할을 수행한다:

- **`subagent.rs`**: single, parallel (max 4 동시성), chain 모드 구현. 에이전트 발견(user/project 스코프). YAML 프론트매터가 포함된 마크다운 에이전트 정의 파일.
- **`oxi-sdk/coordination/`**: `WorkQueue` (우선순위 작업 큐), `SharedMemory` (버전 관리 KV 스토어), `Consensus` (투표), `CoordinatedGroup` (fan-out, vote, map-reduce).

대신, 기존 subagent에 **조건부 분기**와 **SDK 조정 모듈 연동**을 추가한다.

```rust
/// oxi-agent/src/tools/subagent.rs 확장

/// 기존 3모드(single, parallel, chain)에 conditional 추가
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum SubagentParams {
    /// 단일 에이전트 (기존)
    Single { agent: String, task: String },
    
    /// 병렬 실행 (기존, max 4 동시성)
    Parallel { tasks: Vec<SubagentTask> },
    
    /// 순차 체인 (기존, {previous} 변수 치환)
    Chain { chain: Vec<SubagentTask> },
    
    /// 조건부 분기 (새로 추가)
    Conditional {
        /// LLM에게 조건 평가를 요청하는 프롬프트
        condition_prompt: String,
        then_step: Box<SubagentParams>,
        else_step: Option<Box<SubagentParams>>,
    },
}

/// SDK WorkQueue 연동: 대규모 작업 분산
/// subagent.rs는 이미 독립 프로세스를 스폰하므로
/// SDK의 WorkQueue를 사용하여 작업을 분배하고 결과를 수집
impl SubagentTool {
    /// SDK WorkQueue를 통한 대규모 fan-out (기존 parallel의 8태스크 한계 초과 시)
    async fn execute_workqueue(
        &self,
        tasks: Vec<SubagentTask>,
        config: WorkQueueConfig,
    ) -> Result<Vec<WorkResult>> {
        let queue = WorkQueue::new(config);
        for task in tasks {
            queue.submit(WorkItem {
                id: task.agent.clone(),
                payload: serde_json::to_value(&task)?,
                priority: 0,
            }).await?;
        }
        // ... collect results
    }
}
```

### 3.4 툴 출력 스트리밍 강화

기존 `on_progress(ProgressCallback)` (여기서 `ProgressCallback = Arc<dyn Fn(String) + Send + Sync>`)을 구조화된 진행 타입으로 업그레이드한다.

```rust
/// oxi-agent/src/tools.rs 확장

/// 기존: ProgressCallback = Arc<dyn Fn(String) + Send + Sync>
/// 새로: 구조화된 진행 타입 지원

pub enum ToolProgress {
    /// 상태 메시지 (진행 중)
    Status { message: String },
    
    /// 부분 출력 (bash stdout 등)
    PartialOutput { 
        output: String, 
        is_error: bool,
    },
    
    /// 진행률 (0.0 - 1.0)
    Percentage { 
        current: f64, 
        total: Option<f64>,
        message: Option<String>,
    },
    
    /// 파일 작업 진행
    FileOperation {
        operation: FileOp,
        path: PathBuf,
        bytes_processed: Option<u64>,
        total_bytes: Option<u64>,
    },
}

pub enum FileOp {
    Reading, Writing, Searching, Editing,
}

/// 구조화된 진행 콜백 (기존 String 콜백과 병행)
pub type StructuredProgressCallback = Arc<dyn Fn(ToolProgress) + Send + Sync>;

#[async_trait]
pub trait AgentTool: Send + Sync {
    // ... 기존 메서드 ...
    
    /// 기존 String 콜백 (하위 호환)
    fn on_progress(&self, _callback: ProgressCallback) {}
    
    /// 새로운 구조화된 진행 콜백
    fn on_structured_progress(&self, _callback: StructuredProgressCallback) {}
}
```

### 3.5 에디트 툴 강화

기존 `file_mutation_queue.rs`는 파일별 Mutex로 동시 쓰기를 직렬화한다. 여기에 **내용 기반 충돌 감지**를 추가한다.

```rust
/// oxi-agent/src/tools/edit.rs 확장
/// file_mutation_queue.rs는 직렬화만 담당 —
/// 내용 기반 충돌 감지는 edit.rs에서 구현

impl AgentTool for EditTool {
    async fn execute(&self, ...) -> Result<AgentToolResult, ToolError> {
        // 기존: file_mutation_queue가 동일 파일 직렬화
        
        // ── 새로 추가: 내용 기반 충돌 감지 ──
        // params에 expected_hash가 있으면 마지막 읽기 이후 변경 여부 확인
        if let Some(expected_hash) = params.get("expected_hash").and_then(|v| v.as_str()) {
            let current_content = std::fs::read_to_string(&file_path)?;
            let current_hash = sha256(&current_content);
            if current_hash != expected_hash {
                return Ok(AgentToolResult::error(
                    "File has been modified since last read. Re-read the file and retry."
                ));
            }
        }
        
        // ── 새로 추가: dry-run 모드 ──
        if params.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false) {
            let diff = compute_diff(&old_content, &new_content);
            return Ok(AgentToolResult::success(diff));
        }
        
        // 기존 편집 로직...
    }
}
```

**기존 시스템과의 관계:**
- `file_mutation_queue.rs`: 파일 경로 기반 Mutex로 동시 write 직렬화. 동일 파일에 대한 동시 편집을 순차적으로 처리.
- 새 `expected_hash`: 직렬화 외에 내용 기반 충돌 감지 추가. LLM이 마지막으로 읽은 내용과 현재 파일 내용이 다르면 편집 거부.

---

## 4. 구현 계획

### Phase 1: AgentTool 트레이트 확장 (1주)

| 작업 | 산출물 | 기존 코드 영향 |
|------|--------|---------------|
| `render_call` / `render_result` 추가 | 트레이트 확장 + 기본 구현 (None) | `tools.rs` 수정 |
| `execution_mode` 추가 | `ToolExecutionMode` enum | `tools.rs` 수정 |
| `RenderOutput` 타입 정의 | `render_utils.rs` 확장 | 기존 유틸 유지 |
| `tool_renderer.rs` 연동 | render이 None이면 기존 렌더러 사용 | `oxi-tui` 수정 없음 |

### Phase 2: MCP 고도화 (2주)

| 작업 | 산출물 | 기존 코드 영향 |
|------|--------|---------------|
| 능력 협랑 | `McpCapability` enum, 초기화 시 저장 | `client.rs` 확장 |
| Prompts | `list_prompts()`, `get_prompt()` | `client.rs`에 메서드 추가 |
| Sampling | `create_sample()` | `client.rs` + 에이전트 루프 위임 |
| Logging | `set_log_level()` | `client.rs`에 메서드 추가 |

**참고**: `resources/list`와 `resources/read`는 이미 구현됨 (client.rs 218-250라인). Phase 2는 누락된 3개 영역만 추가.

### Phase 3: Subagent 조건부 분기 + SDK 연동 (1.5주)

| 작업 | 산출물 | 기존 코드 영향 |
|------|--------|---------------|
| `Conditional` 모드 추가 | subagent.rs 확장 | 기존 3모드 유지 |
| SDK WorkQueue 연동 | 대규모 fan-out 시 WorkQueue 사용 | `oxi-sdk` 의존 |
| 템플릿 변수 확장 | `{task}`, `{previous}`, `{result}` 지원 | 기존 chain 로직 확장 |

**참고**: 새 WorkflowEngine을 만들지 않음. 기존 `subagent.rs`와 `oxi-sdk/coordination/`을 활용.

### Phase 4: 툴 스트리밍 (1주)

| 작업 | 산출물 | 기존 코드 영향 |
|------|--------|---------------|
| `ToolProgress` enum | 구조화된 진행 타입 | `tools.rs` 확장 |
| `on_structured_progress` | 새 콜백 메서드 | 기존 `on_progress(String)` 유지 |
| Bash 스트리밍 | 라인 단위 진행 보고 | `bash.rs` 수정 |
| AgentEvent 확장 | `ToolExecutionUpdate`에 진행 데이터 | `agent_loop/` 수정 |

### Phase 5: 에디트 강화 (1주)

| 작업 | 산출물 | 기존 코드 영향 |
|------|--------|---------------|
| 충돌 감지 | `expected_hash` 파라미터 | `edit.rs` 확장 |
| dry-run 모드 | diff만 반환 | `edit.rs` 확장 |
| file_mutation_queue 유지 | 변경 없음 | 직렬화 담당 유지 |

---

## 5. 성공 기준

- [ ] AgentTool: `render_call`/`render_result`로 툴별 커스텀 시각화 (기본은 `tool_renderer.rs` 사용)
- [ ] MCP: Prompts, Sampling, Logging 지원 (Resources는 이미 구현됨)
- [ ] Subagent: 조건부 분기 추가 + SDK WorkQueue 연동 (새 엔진 없이 기존 시스템 확장)
- [ ] 툴 스트리밍: `ToolProgress` enum으로 구조화된 진행 보고
- [ ] 에디트: 내용 기반 충돌 감지 (`expected_hash`) + dry-run
- [ ] 기존 20개 툴 회귀 테스트 100% 통과
- [ ] `cargo clippy --workspace -- -D warnings` 통과
- [ ] `cargo test --workspace` 통과

---

## 6. 위험 요소

| 위험 | 완화 방안 |
|------|----------|
| AgentTool 트레이트 변경이 기존 툴에 영향 | 모든 새 메서드에 기본 구현 제공 (None/default) |
| MCP 서버마다 프로토콜 구현 차이 | 능력 협상으로 지원 기능만 사용 |
| Subagent 프로세스 스폰 오버헤드 | 기존 MAX_PARALLEL_TASKS=8, MAX_CONCURRENCY=4 유지 |
| tool_renderer.rs와 render_call 중복 | render_call이 None이면 tool_renderer가 처리하는 폴백 체인 |
