# 서브에이전트 시스템 통합 설계서

> 날짜: 2026-06-09
> 범위: 디스커버리 통합, 안전장치, Orchestrated 전략, Workflow DSL 실행기
> 이전 이터레이션: v3 설계서 (이미 구현 완료)

---

## 1. 배경 및 동기

현재 서브에이전트 시스템은 3개 레이어에 걸쳐 부분적으로 구현되어 있다:

| 레이어 | 모듈 | 상태 |
|--------|------|------|
| 실행 (Agent) | `oxicode-agent/src/tools/subagent.rs` | ✅ 완전 구현 (Single/Parallel/Chain) |
| 정의 (SDK) | `oxicode-sdk/src/agent_definition.rs` | ✅ 완전 구현 |
| 오케스트레이션 (SDK) | `oxicode-sdk/src/agent_group.rs` | ⚠️ Orchestrated 스텁 |
| 워크플로우 (SDK) | `oxicode-sdk/src/workflow_dsl.rs` | ❌ 파서만, 실행기 없음 |
| 보안 (SDK) | `oxicode-sdk/src/security/` | ✅ Capability 정의됨 |

해결해야 할 5가지 문제:

1. **디스커버리 이원화**: `subagent.rs`의 `AgentConfig`/`discover_agents()`와 SDK의 `AgentDefinition`/`AgentDiscovery`가 독립 구현
2. **깊이 제한 미집행**: `max_subagent_depth` 필드는 정의되었으나 실행 시 체크하지 않음
3. **Orchestrated 전략 스텁**: 리더만 실행하고 워커 위임이 없음
4. **Workflow DSL 실행기 부재**: 파서는 6가지 스텝을 정의하지만 실행기가 없음
5. **`ForEach`/`Vote` 모듈 미존재**: `CoordinatedGroup`, `Consensus`가 아직 없음

---

## 2. 설계 원칙

1. **SDK가 정규(Canonical) 소스**: 에이전트 정의, 발견, 검증은 모두 SDK를 통해 이루어진다. Agent 툴은 SDK를 호출한다.
2. **하이브리드 실행 모델 유지**: `SubagentTool`은 프로세스 격리(보안), `AgentGroup`/`WorkflowEngine`은 인프로세스(성능) 방식을 유지한다.
3. **점진적 구현**: Phase 1→2→3 순서로, 각 Phase가 독립적으로 배포 가능하다.
4. **기존 호환성**: `~/.oxicode/agents/` 디렉토리 구조, `.md` 파일 포맷, `subagent` 툴 스키마는 변경하지 않는다.

---

## 3. Phase 1 — 디스커버리 통합 + 깊이 제한

### 3.1. 통합 에이전트 디스커버리

**문제**: `subagent.rs`와 `agent_definition.rs`가 각각 독립적으로 에이전트를 발견한다. 디렉토리 구조도 다름:
- SDK: `~/.oxicode/agents/<name>/agent.md` (서브디렉토리)
- Agent: `~/.oxicode/agents/<name>.md` (플랫 파일)

**해결**: SDK의 `AgentDiscovery`를 정규 API로 삼고, 두 포맷을 모두 지원한다.

```
~/.oxicode/agents/
├── scout.md                    ← 플랫 파일 (기존 SubagentTool 포맷)
├── scout/agent.md              ← 서브디렉토리 (기존 SDK 포맷, 우선순위 높음)
├── reviewer/
│   └── agent.md
└── worker.md
```

**우선순위 규칙** (같은 이름 충돌 시):
1. 프로젝트 > 사용자 (기존과 동일)
2. 서브디렉토리 > 플랫 파일 (새로운 규칙)

#### 변경: `oxicode-sdk/src/agent_definition.rs`

`AgentDiscovery::discover_from_dir()` 확장:

```rust
fn discover_from_dir(dir: &Path, agents: &mut HashMap<String, AgentDefinition>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // 서브디렉토리: <name>/agent.md (기존 SDK 포맷)
            let agent_file = path.join("agent.md");
            if agent_file.exists() {
                let dir_name = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match AgentDefinition::from_markdown(&agent_file) {
                    Ok(def) => { agents.insert(dir_name.to_lowercase(), def); }
                    Err(e) => { tracing::warn!("..."); }
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            // 플랫 파일: <name>.md (기존 SubagentTool 포맷)
            let name = path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() { continue; }
            match AgentDefinition::from_markdown(&path) {
                Ok(def) => {
                    // 서브디렉토리가 이미 등록했으면 스킵 (우선순위)
                    agents.entry(name.to_lowercase()).or_insert(def);
                }
                Err(e) => { tracing::warn!("..."); }
            }
        }
    }
    Ok(())
}
```

`AgentDefinition::from_markdown()`은 이미 프론트매터가 없으면 파일명을 이름으로 사용하므로, 플랫 `.md` 파일도 자동 처리된다.

#### 변경: `oxicode-agent/src/tools/subagent.rs`

`discover_agents()`를 SDK의 `AgentDiscovery`에 위임:

```rust
use oxicode_sdk::agent_definition::{AgentDefinition, AgentDiscovery};

/// 에이전트 발견 — SDK의 AgentDiscovery에 위임.
pub fn discover_agents(cwd: &Path, scope: AgentScope) -> Vec<ResolvedAgent> {
    let mut agents = Vec::new();

    // SDK discovery는 항상 user + project를 모두 탐색
    // scope 필터링은 여기서 수행
    let discovered = AgentDiscovery::discover(cwd).unwrap_or_default();

    for (name, def) in discovered {
        let source = match scope {
            AgentScope::Project => {
                // 프로젝트 스코프: .oxicode/agents/에 있는 것만
                if def.source == "project" { Some("project") } else { None }
            }
            AgentScope::User => {
                // 사용자 스코프: 글로벌 ~/.oxicode/agents/만
                if def.source == "user" { Some("user") } else { None }
            }
            AgentScope::Both => Some("both"), // 모두 포함
        };

        if let Some(source) = source {
            agents.push(ResolvedAgent {
                name: def.name.clone(),
                definition: def,
                source: source.to_string(),
            });
        }
    }

    agents
}
```

`AgentConfig`는 `AgentDefinition`으로 대체하고, `SubagentTool` 실행 시 `AgentDefinition`을 직접 사용:

```rust
/// SDK AgentDefinition의 래퍼 — source 정보 추가.
pub struct ResolvedAgent {
    pub name: String,
    pub definition: AgentDefinition,
    pub source: String,
}
```

기존 `AgentConfig`는 제거. `parse_frontmatter()`, `parse_agent_file()`, `load_agents_from_dir()` 등 중복 코드도 모두 제거.

#### `AgentDiscovery::discover()` 시그니처 조정

현재 `discover()`는 user + project를 모두 탐색하므로, `AgentScope` 파라미터를 추가하거나 반환값에 source 정보를 포함해야 함:

```rust
pub struct AgentDefinition {
    // 기존 필드...
    /// 발견 소스: "user" 또는 "project"
    #[serde(default)]
    pub source: String,
}

impl AgentDiscovery {
    pub fn discover(cwd: &Path) -> Result<Vec<(String, AgentDefinition)>> {
        // 기존 로직에서 source 필드 채우기
        // 글로벌 에이전트: source = "user"
        // 프로젝트 에이전트: source = "project"
    }
}
```

### 3.2. 깊이 제한 집행

**문제**: `AgentDefinition::max_subagent_depth`가 정의되어 있지만, `SubagentTool::execute()`에서 체크하지 않아 무한 중첩이 가능하다.

**해결**: 환경 변수 `OXICODE_SUBAGENT_DEPTH`를 통해 깊이를 추적하고, 제한을 초과하면 즉시 에러를 반환한다.

#### 메커니즘

```
부모 프로세스 (depth=0)
  └─ spawn 자식 프로세스 (OXICODE_SUBAGENT_DEPTH=1)
       └─ spawn 손자 프로세스 (OXICODE_SUBAGENT_DEPTH=2)
            └─ depth >= max_subagent_depth → 에러 반환
```

#### 변경: `oxicode-agent/src/tools/subagent.rs`

```rust
/// 현재 서브에이전트 깊이를 가져온다.
/// 기본값 0 (최상위 프로세스).
fn current_depth() -> u8 {
    std::env::var("OXICODE_SUBAGENT_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// 깊이 제한을 가져온다.
/// 우선순위: OXICODE_MAX_SUBAGENT_DEPTH 환경변수 > 기본값 3.
/// 에이전트 정의의 max_subagent_depth는 Command::env()로 전달되어
/// 자식 프로세스에서 이 값을 읽게 된다.
fn max_depth() -> u8 {
    std::env::var("OXICODE_MAX_SUBAGENT_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}
```

`build_agent_args()`에서 자식 프로세스에 깊이 전달:

> **참고**: `oxicode` CLI에는 `--env` 인자가 없다. 대신 `Command::env()`를 사용해
> 자식 프로세스의 환경 변수를 직접 설정한다.

```rust
// build_agent_args()가 아니라 run_single_agent()의 Command 생성 시점에:
let mut cmd = tokio::process::Command::new(binary_path);
cmd.args(&args)
    .current_dir(&working_dir)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .stdin(std::process::Stdio::null())
    // 깊이 전달
    .env("OXICODE_SUBAGENT_DEPTH", (current_depth() + 1).to_string())
    .env("OXICODE_MAX_SUBAGENT_DEPTH", agent.max_subagent_depth.to_string());
```

`execute()` 진입점에서 체크:

```rust
async fn execute(&self, ...) -> Result<AgentToolResult, ToolError> {
    let depth = current_depth();
    let max = max_depth();

    if depth >= max {
        return Ok(AgentToolResult::error(format!(
            "Subagent depth limit reached ({}/{}). \
             Increase max_subagent_depth in your agent definition.",
            depth, max
        )));
    }

    // 기존 실행 로직...
}
```

> **대안 고려**: `ToolContext`에 depth를 넣을 수도 있지만, 프로세스 격리 모델에서는 환경 변수가 더 자연스럽다. 인프로세스 모델(AgentGroup)은 `ToolContext` 기반이지만, 여기서는 이미 `Agent::run()`이 자체 컨텍스트를 관리하므로 별도 체크가 필요 없다.

### 3.3. 파일 변경 요약 (Phase 1)

| 파일 | 변경 내용 |
|------|-----------|
| `oxicode-sdk/src/agent_definition.rs` | `discover_from_dir()`에 플랫 `.md` 지원 추가, `source` 필드 추가 |
| `oxicode-agent/src/tools/subagent.rs` | `AgentConfig`/`discover_agents()` 제거 → SDK `AgentDiscovery` 위임, `current_depth()`/`max_depth()` 추가 |
| `oxicode-agent/src/tools/subagent.rs` | `parse_frontmatter()`, `parse_agent_file()`, `load_agents_from_dir()` 제거 |
| `oxicode-agent/src/tools/subagent.rs` | `run_single_agent()` 시그니처: `&[AgentConfig]` → `&[ResolvedAgent]` |

---

## 4. Phase 2 — Orchestrated 전략 + Workflow 실행기

### 4.1. Orchestrated 전략 구현

**현재**: `AgentGroup::run_orchestrated()`는 리더 에이전트만 실행.

**목표**: 리더가 태스크를 분석 → 서브태스크로 분해 → 워커에게 배분 → 결과 수집.

#### 설계: Leader-Worker 패턴

```
┌─────────────────────────────────────────┐
│           Orchestrated Execution         │
│                                         │
│  1. Leader: "Analyze this codebase"     │
│     → 응답: JSON 형태의 태스크 분해      │
│                                         │
│  2. 파서: JSON → Vec<WorkerTask>        │
│                                         │
│  3. Workers: 병렬 실행                  │
│     [worker-0] [worker-1] [worker-2]    │
│                                         │
│  4. Aggregator: 결과 수집               │
│                                         │
│  5. Leader: "Merge these results"       │
│     → 최종 출력                          │
└─────────────────────────────────────────┘
```

#### 프롬프트 템플릿

리더에게 분해를 요청하는 시스템 프롬프트:

```rust
const DECOMPOSITION_PROMPT: &str = r#"
You are a task decomposition engine. Given the following task, break it into
subtasks that can be executed in parallel by specialized workers.

Respond with a JSON array of objects:
[
  {"id": 0, "task": "...", "agent_hint": "optional-agent-name"},
  {"id": 1, "task": "...", "agent_hint": "optional-agent-name"}
]

Rules:
- Each subtask must be self-contained and independently executable
- Include all necessary context in each subtask
- If the task cannot be decomposed, return a single-element array
- Output ONLY the JSON array, no other text
"#;

const MERGE_PROMPT: &str = r#"
You are a result synthesis engine. Given the original task and worker results
below, produce a single coherent response that addresses the original task.

Original task: {original_task}

Worker results:
{worker_results}
"#;
```

#### 변경: `oxicode-sdk/src/agent_group.rs`

```rust
/// 분해된 서브태스크.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkerTask {
    id: usize,
    task: String,
    #[serde(default)]
    agent_hint: Option<String>,
}

impl AgentGroup {
    async fn run_orchestrated(
        &self,
        prompt: String,
        leader_idx: usize,
    ) -> Result<Vec<AgentGroupOutput>> {
        let leader = &self.agents[leader_idx];
        let workers: Vec<_> = self.agents.iter()
            .enumerate()
            .filter(|(i, _)| *i != leader_idx)
            .collect();

        // 1단계: 리더에게 태스크 분해 요청
        let decompose_prompt = format!("{}\n\nTask: {}", DECOMPOSITION_PROMPT, prompt);
        let (response, _) = leader.run(decompose_prompt).await?;

        // 2단계: JSON 파싱
        let tasks: Vec<WorkerTask> = parse_worker_tasks(&response.content)?;

        // 3단계: 워커에게 병렬 배분
        let worker_outputs = if workers.is_empty() || tasks.is_empty() {
            // 워커가 없거나 태스크가 없으면 리더 결과를 그대로 반환
            vec![]
        } else {
            self.dispatch_to_workers(&workers, &tasks).await?
        };

        // 4단계: 리더에게 결과 병합 요청
        if worker_outputs.is_empty() {
            return Ok(vec![AgentGroupOutput {
                name: leader.model_id(),
                content: response.content,
                success: true,
                error: None,
            }]);
        }

        let results_text = worker_outputs.iter()
            .enumerate()
            .map(|(i, o)| format!("### Worker {} ({})\n{}", i, o.name, o.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let merge_prompt = MERGE_PROMPT
            .replace("{original_task}", &prompt)
            .replace("{worker_results}", &results_text);

        let (final_response, _) = leader.run(merge_prompt).await?;

        // 5단계: 결과 조합
        let mut results = vec![AgentGroupOutput {
            name: leader.model_id(),
            content: final_response.content,
            success: true,
            error: None,
        }];
        results.extend(worker_outputs);
        Ok(results)
    }

    /// 워커에게 태스크를 병렬 배분.
    async fn dispatch_to_workers(
        &self,
        workers: &[(usize, Arc<Agent>)],
        tasks: &[WorkerTask],
    ) -> Result<Vec<AgentGroupOutput>> {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
        let mut handles = Vec::new();

        for (i, task) in tasks.iter().enumerate() {
            // 라운드로빈으로 워커 선택
            let (_, worker) = workers[i % workers.len()];
            let worker = Arc::clone(worker);
            let task_text = task.task.clone();
            let sem = Arc::clone(&semaphore);

            handles.push(tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async move {
                    let _permit = sem.acquire().await.expect("semaphore");
                    match worker.run(task_text).await {
                        Ok((resp, _)) => AgentGroupOutput {
                            name: worker.model_id(),
                            content: resp.content,
                            success: true,
                            error: None,
                        },
                        Err(e) => AgentGroupOutput {
                            name: worker.model_id(),
                            content: String::new(),
                            success: false,
                            error: Some(e.to_string()),
                        },
                    }
                })
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(output) => results.push(output),
                Err(e) => results.push(AgentGroupOutput {
                    name: String::new(),
                    content: String::new(),
                    success: false,
                    error: Some(format!("Join error: {}", e)),
                }),
            }
        }
        Ok(results)
    }
}

/// 리더 응답에서 WorkerTask 배열을 파싱.
fn parse_worker_tasks(content: &str) -> Result<Vec<WorkerTask>> {
    // JSON 코드 블록 추출 (```json ... ```)
    let json_str = if let Some(start) = content.find("```json") {
        let start = start + 7;
        if let Some(end) = content[start..].find("```") {
            content[start..start + end].trim()
        } else {
            content.trim()
        }
    } else if let Some(start) = content.find('[') {
        if let Some(end) = content.rfind(']') {
            &content[start..=end]
        } else {
            content.trim()
        }
    } else {
        content.trim()
    };

    let tasks: Vec<WorkerTask> = serde_json::from_str(json_str)
        .with_context(|| "Leader did not return valid task decomposition")?;

    Ok(tasks)
}
```

### 4.2. Workflow DSL 실행기

**현재**: `WorkflowDefinition` 파서만 있고 실행기가 없음.

**설계**: `WorkflowEngine`이 `WorkflowDefinition`을 받아 `AgentGroup` + `MessageBus` + `SharedMemory`를 조합해 실행.

#### 새 파일: `oxicode-sdk/src/workflow_engine.rs`

```rust
//! Workflow execution engine.
//!
//! Takes a parsed `WorkflowDefinition` and executes it using the
//! SDK's coordination primitives (AgentGroup, MessageBus).

use crate::agent_definition::AgentDiscovery;
use crate::agent_group::{AgentGroup, AgentGroupOutput, GroupStrategy, GroupResult};
use crate::workflow_dsl::{WorkflowDefinition, WorkflowStepDef};
use anyhow::{Context, Result};
use oxicode_agent::Agent;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Workflow shared state — key-value store accessible by all steps.
type SharedState = parking_lot::RwLock<HashMap<String, serde_json::Value>>;

/// Workflow execution engine.
pub struct WorkflowEngine {
    /// Pre-built agents indexed by name.
    agents: HashMap<String, Arc<Agent>>,
    /// Shared state for SetState/ForEach steps.
    state: Arc<SharedState>,
}

/// Result of a workflow execution.
#[derive(Debug)]
pub struct WorkflowResult {
    /// Per-step outputs.
    pub step_outputs: Vec<StepOutput>,
    /// Total execution time in milliseconds.
    pub total_duration_ms: u64,
}

/// Output from a single workflow step.
#[derive(Debug)]
pub struct StepOutput {
    /// Step index in the workflow.
    pub step_index: usize,
    /// Step type name.
    pub step_type: String,
    /// Whether the step succeeded.
    pub success: bool,
    /// Output content (combined from all agents in the step).
    pub content: String,
    /// Error message if the step failed.
    pub error: Option<String>,
}

impl WorkflowEngine {
    /// Create a new engine with the given agents.
    pub fn new(agents: HashMap<String, Arc<Agent>>) -> Self {
        Self {
            agents,
            state: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        }
    }

    /// Create an engine by discovering agents from the filesystem.
    pub fn from_discovery(cwd: &Path, providers: ...) -> Result<Self> {
        // AgentDiscovery::discover() → AgentDefinition → Agent 빌드
        // 에이전트별 모델/공급자 해석
    }

    /// Execute a workflow definition.
    pub async fn run(&self, workflow: &WorkflowDefinition) -> Result<WorkflowResult> {
        let start = std::time::Instant::now();
        let mut step_outputs = Vec::new();

        for (i, step) in workflow.steps.iter().enumerate() {
            let output = self.execute_step(i, step).await
                .with_context(|| format!("Step {} failed", i))?;

            // Chain의 경우 실패 시 즉시 중단
            if !output.success {
                step_outputs.push(output);
                return Ok(WorkflowResult {
                    step_outputs,
                    total_duration_ms: start.elapsed().as_millis() as u64,
                });
            }

            step_outputs.push(output);
        }

        Ok(WorkflowResult {
            step_outputs,
            total_duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn execute_step(&self, index: usize, step: &WorkflowStepDef) -> Result<StepOutput> {
        match step {
            WorkflowStepDef::Run { agent, task, .. } => {
                self.execute_run(index, agent, task).await
            }
            WorkflowStepDef::Parallel { agents, task, concurrency, .. } => {
                self.execute_parallel(index, agents, task, *concurrency).await
            }
            WorkflowStepDef::Chain { steps } => {
                self.execute_chain(index, steps).await
            }
            WorkflowStepDef::ForEach { items_key, agent, task_template, concurrency, .. } => {
                self.execute_foreach(index, items_key, agent, task_template, *concurrency).await
            }
            WorkflowStepDef::Vote { agents, question, threshold } => {
                self.execute_vote(index, agents, question, *threshold).await
            }
            WorkflowStepDef::SetState { key, value, .. } => {
                self.execute_set_state(index, key, value.clone())
            }
        }
    }
}
```

#### 각 스텝 타입 실행

**Run**: 단일 에이전트 실행

```rust
async fn execute_run(&self, index: usize, agent_name: &str, task: &str) -> Result<StepOutput> {
    let agent = self.get_agent(agent_name)?;
    match agent.run(task.to_string()).await {
        Ok((response, _)) => Ok(StepOutput {
            step_index: index,
            step_type: "run".into(),
            success: true,
            content: response.content,
            error: None,
        }),
        Err(e) => Ok(StepOutput {
            step_index: index,
            step_type: "run".into(),
            success: false,
            content: String::new(),
            error: Some(e.to_string()),
        }),
    }
}
```

**Parallel**: `AgentGroup::new(GroupStrategy::Parallel)`에 위임

```rust
async fn execute_parallel(
    &self, index: usize, agent_names: &[String], task: &str, concurrency: Option<usize>,
) -> Result<StepOutput> {
    let agents = self.get_agents(agent_names)?;
    let group = AgentGroup::new(GroupStrategy::Parallel {
        max_concurrency: concurrency.unwrap_or(4),
    });
    let group = agent_names.iter()
        .fold(group, |g, name| {
            if let Some(agent) = self.agents.get(name) {
                g.agent(Arc::clone(agent))
            } else { g }
        });

    let result = group.run(task.to_string()).await
        .map_err(|e| anyhow::anyhow!("Parallel execution failed: {}", e))?;

    Ok(StepOutput {
        step_index: index,
        step_type: "parallel".into(),
        success: result.all_succeeded(),
        content: result.combined_content(),
        error: if result.has_failures() {
            Some(format!("{}/{} failed", result.results.len() - result.success_count(), result.results.len()))
        } else { None },
    })
}
```

**Chain**: `AgentGroup::new(GroupStrategy::Pipeline)`에 위임

```rust
async fn execute_chain(&self, index: usize, steps: &[WorkflowStepDef]) -> Result<StepOutput> {
    let mut combined = String::new();
    for (sub_i, step) in steps.iter().enumerate() {
        let output = self.execute_step(index * 100 + sub_i, step).await?;
        if !output.success {
            return Ok(StepOutput {
                step_index: index,
                step_type: "chain".into(),
                success: false,
                content: combined,
                error: output.error,
            });
        }
        combined = output.content;
    }
    Ok(StepOutput {
        step_index: index,
        step_type: "chain".into(),
        success: true,
        content: combined,
        error: None,
    })
}
```

**ForEach**: 항목마다 에이전트 실행 (병렬)

```rust
async fn execute_foreach(
    &self, index: usize, items_key: &str, agent_name: &str,
    task_template: &str, concurrency: Option<usize>,
) -> Result<StepOutput> {
    // SharedState에서 아이템 목록 가져오기
    let items = {
        let state = self.state.read();
        state.get(items_key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };

    if items.is_empty() {
        return Ok(StepOutput {
            step_index: index,
            step_type: "for_each".into(),
            success: true,
            content: "(no items to process)".into(),
            error: None,
        });
    }

    let max_concurrency = concurrency.unwrap_or(4);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    let mut handles = Vec::new();

    for item in &items {
        let agent = self.get_agent(agent_name)?;
        let task = task_template.replace("{item}", &item.to_string());
        let sem = Arc::clone(&semaphore);

        handles.push(tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("runtime");
            rt.block_on(async move {
                let _permit = sem.acquire().await.expect("sem");
                agent.run(task).await
                    .map(|(r, _)| r.content)
                    .unwrap_or_else(|e| format!("Error: {}", e))
            })
        }));
    }

    let results: Vec<String> = handles.into_iter()
        .map(|h| h.unwrap_or_else(|e| format!("Join error: {}", e)))
        .collect();

    Ok(StepOutput {
        step_index: index,
        step_type: "for_each".into(),
        success: true,
        content: results.join("\n\n---\n\n"),
        error: None,
    })
}
```

**Vote**: 다수결 집계

```rust
async fn execute_vote(
    &self, index: usize, agent_names: &[String], question: &str,
    threshold: Option<f32>,
) -> Result<StepOutput> {
    let threshold = threshold.unwrap_or(0.5);
    let agents = self.get_agents(agent_names)?;

    // 병렬로 모든 에이전트에게 질문
    let group = AgentGroup::new(GroupStrategy::Parallel { max_concurrency: 4 });
    let group = agent_names.iter()
        .fold(group, |g, name| {
            if let Some(agent) = self.agents.get(name) {
                g.agent(Arc::clone(agent))
            } else { g }
        });

    let result = group.run(question.to_string()).await
        .map_err(|e| anyhow::anyhow!("Vote execution failed: {}", e))?;

    // 응답 그룹핑 (동일 응답 카운트)
    let mut vote_counts: HashMap<String, usize> = HashMap::new();
    for r in &result.results {
        *vote_counts.entry(r.content.trim().to_lowercase()).or_insert(0) += 1;
    }

    let total_votes = result.results.len() as f32;
    let winner = vote_counts.iter()
        .max_by_key(|(_, count)| *count)
        .map(|(answer, count)| (answer.clone(), *count));

    let consensus = winner
        .map(|(_, count)| (count as f32 / total_votes) >= threshold)
        .unwrap_or(false);

    let content = if consensus {
        let (answer, count) = winner.unwrap();
        format!(
            "Consensus reached ({}/{} = {:.0}% ≥ {:.0}%): {}",
            count, result.results.len(),
            (count as f32 / total_votes) * 100.0,
            threshold * 100.0,
            answer
        )
    } else {
        let votes: Vec<String> = vote_counts.iter()
            .map(|(a, c)| format!("{} ({} votes)", a, c))
            .collect();
        format!(
            "No consensus (threshold: {:.0}%).\nVotes:\n{}",
            threshold * 100.0,
            votes.join("\n")
        )
    };

    Ok(StepOutput {
        step_index: index,
        step_type: "vote".into(),
        success: consensus,
        content,
        error: if consensus { None } else {
            Some("No consensus reached".into())
        },
    })
}
```

**SetState**: 공유 상태에 값 저장

```rust
fn execute_set_state(&self, index: usize, key: &str, value: serde_json::Value) -> Result<StepOutput> {
    self.state.write().insert(key.to_string(), value);
    Ok(StepOutput {
        step_index: index,
        step_type: "set_state".into(),
        success: true,
        content: format!("Set {} = {:?}", key, self.state.read().get(key)),
        error: None,
    })
}
```

### 4.3. 파일 변경 요약 (Phase 2)

| 파일 | 변경 내용 |
|------|-----------|
| `oxicode-sdk/src/agent_group.rs` | `run_orchestrated()` 전체 구현, `dispatch_to_workers()`, `parse_worker_tasks()` 추가 |
| `oxicode-sdk/src/workflow_engine.rs` | **새 파일** — `WorkflowEngine`, `WorkflowResult`, `StepOutput` + 6개 스텝 실행 메서드 |
| `oxicode-sdk/src/lib.rs` | `pub mod workflow_engine;` + public re-export |
| `oxicode-sdk/src/prelude.rs` | `WorkflowEngine`, `WorkflowResult` re-export |

---

## 5. Phase 3 — 에이전트 빌더 통합

### 5.1. `WorkflowEngine::from_discovery()` 구현

에이전트 정의에서 `Agent` 인스턴스를 빌드하는 브릿지가 필요하다.

```rust
impl WorkflowEngine {
    /// 에이전트 정의 파일에서 에이전트를 발견하고 빌드한다.
    ///
    /// `provider_resolver`는 에이전트 정의의 `model` 필드를
    /// 실제 `Provider` 인스턴스로 해석하는 함수다.
    pub fn from_discovery(
        cwd: &Path,
        provider_resolver: &dyn Fn(&str) -> Result<Arc<dyn oxicode_ai::Provider>>,
    ) -> Result<Self> {
        let definitions = AgentDiscovery::discover(cwd)?;
        let mut agents = HashMap::new();

        for (name, def) in definitions {
            let model_id = def.model.clone().unwrap_or_default();
            let provider = provider_resolver(&model_id)
                .with_context(|| format!("No provider for agent '{}' model '{}'", name, model_id))?;

            let config = oxicode_agent::AgentConfig {
                model: model_id,
                system_prompt: def.system_prompt.clone(),
                ..Default::default()
            };

            let agent = Agent::new(provider, config);
            agents.insert(name, Arc::new(agent));
        }

        Ok(Self::new(agents))
    }
}
```

### 5.2. CLI 통합 — 워크플로우 명령

`oxicode workflow run <file>` CLI 서브커맨드 (선택적, Phase 3에서 논의).

---

## 6. 구현 순서

```
Phase 1: 디스커버리 통합 + 깊이 제한 (약 2-3일)
  ├── 1a. AgentDefinition에 source 필드 + 플랫 .md 지원
  ├── 1b. SubagentTool → SDK AgentDiscovery 위임
  └── 1c. OXICODE_SUBAGENT_DEPTH 환경 변수 기반 깊이 제한

Phase 2: Orchestrated + Workflow 실행기 (약 4-5일)
  ├── 2a. AgentGroup::run_orchestrated() 구현
  ├── 2b. WorkflowEngine 신규 파일 (Run/Parallel/Chain)
  ├── 2c. WorkflowEngine (ForEach/SetState)
  └── 2d. WorkflowEngine (Vote)

Phase 3: 통합 + CLI (약 2일)
  ├── 3a. WorkflowEngine::from_discovery()
  └── 3b. oxicode workflow 서브커맨드 (선택)
```

---

## 7. 리스크 및 완화

| 리스크 | 완화 |
|--------|------|
| LLM이 JSON 분해를 반환하지 않을 수 있음 | `parse_worker_tasks()`에 폴백: 전체 텍스트를 단일 태스크로 처리 |
| 프로세스 격리 + 환경 변수가 CI/CD에서 문제될 수 있음 | `OXICODE_SUBAGENT_DEPTH`가 설정되지 않으면 depth=0, max=3으로 기본 동작 |
| Vote가 동일 응답을 보장하지 않음 | 소문자 변환 + 트리밍으로 정규화, 임계값 조정 가능 |
| WorkflowEngine이 `Agent::run()`의 `!Send` 문제 상속 | `spawn_blocking` + current_thread 런타임 패턴 재사용 |
| `WorkflowEngine::from_discovery()`가 Provider 해석 방법을 모름 | `provider_resolver` 콜백으로 외부 주입 — SDK는 Provider 생성 책임 없음 |

---

## 8. 테스트 계획

### Phase 1 테스트

| 테스트 | 설명 |
|--------|------|
| `test_discover_flat_md` | 플랫 `.md` 파일 발견 |
| `test_discover_subdir_takes_priority` | `agent.md` 서브디렉토리가 플랫보다 우선 |
| `test_scope_project_filters` | Project 스코프가 글로벌 에이전트 제외 |
| `test_depth_limit_blocks` | depth >= max일 때 에러 반환 |
| `test_depth_env_inheritance` | 자식 프로세스에 depth 전달 |

### Phase 2 테스트

| 테스트 | 설명 |
|--------|------|
| `test_orchestrated_decomposition` | 리더가 JSON 배열 반환 시 파싱 |
| `test_orchestrated_fallback_single` | 리더가 일반 텍스트 반환 시 단일 태스크로 처리 |
| `test_workflow_run_single` | Run 스텝 실행 |
| `test_workflow_parallel` | Parallel 스텝 + 동시성 제한 |
| `test_workflow_chain_pipeline` | Chain 스텝 순차 실행 |
| `test_workflow_foreach` | ForEach + SharedState |
| `test_workflow_vote_consensus` | Vote + 임계값 달성 |
| `test_workflow_vote_no_consensus` | Vote + 임계값 미달 |
| `test_workflow_set_state` | SetState + ForEach 간 상태 공유 |
