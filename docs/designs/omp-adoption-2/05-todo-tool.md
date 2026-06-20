# 세부 설계 ⑤ — todo 도구 (phased 작업 관리)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §1·§2 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `tools/todo.ts` (938줄), `modes/components/todo-reminder.ts`
> 후속: N1 구현 → CHANGELOG.md
> 짝: [`06-todo-sticky-panel.md`](./06-todo-sticky-panel.md) (TUI 표시 계층)
>
> **⚠️ v2**: §4 ToolContext 확장이 능력 특성 패턴으로 수정됨. ToolError 변형 제거. [`00-design-revisions.md`](./00-design-revisions.md)가 코드 스니펫의 권위적 출처.

---

## 0. 핵심 (TL;DR)

omp의 **todo 도구**는 에이전트가 다단계 작업을 phase로 구조화하고, 진행 상태를 세션에 영속하며, TUI sticky panel에 실시간 반영하는 계층이다. **phase → task → status** 3단계 모델로, 7개 오퍼레이션(`init`/`start`/`done`/`drop`/`rm`/`append`/`view`)을 통해 변경한다.

oxi는 현재 todo 도구가 **완전히 부재**다. AGENTS.md가 omp와 동일한 스펙을 서술하지만 구현이 없다. 본 설계는 oxi-agent에 todo 도구를 추가하고, 세션 영속 + TUI 이벤트 브리지를 정의한다. **렌더링(sticky panel)은 `06`이 담당**한다.

### omp가 검증한 가치
- **진행 가시성** — 에이전트가 "지금 5단계 중 3단계"임을 사용자에게 보임.
- **자기 규율** — 에이전트가 체계적 분해를 강제받음 (단계 누락 방지).
- **세션 영속** — 재개 시 이전 todo 복원.
- **서브에이전트 연동** — `task` 서브에이전트 완료 시 매칭되는 todo 자동 체크.

---

## 1. omp 메커니즘

### 1.1 데이터 모델 (`tools/todo.ts:20-41`)

```typescript
type TodoStatus = "pending" | "in_progress" | "completed" | "abandoned";

interface TodoItem {
    content: string;          // 5-10단어, "what" not "how"
    status: TodoStatus;
    notes?: string[];         // HUD 노트
}

interface TodoPhase {
    name: string;             // 짧은 명사구 ("Foundation", "Auth")
    tasks: TodoItem[];
}
```

### 1.2 7개 오퍼레이션 (`tools/todo.ts:47`)

| op | 필드 | 효과 |
|---|---|---|
| `init` | `list: [{phase, items[]}]` 또는 `items[]` | 전체 리스트 교체 (기존 삭제) |
| `start` | `task` 또는 `phase` | task를 `in_progress`로 (동시에 다른 task는 `pending`으로 정규화) |
| `done` | `task` 또는 `phase` | task/phase 전체를 `completed`로 |
| `drop` | `task` 또는 `phase` | task/phase를 `abandoned`로 |
| `rm` | `task` 또는 `phase` (선택) | task/phase 제거. 둘 다 생략 시 전체 삭제 |
| `append` | `phase`, `items[]` | phase에 task 추가 (phase 없으면 생성) |
| `view` | — | 읽기 전용, 변경 없음 |

### 1.3 핵심 알고리즘

- **`normalizeInProgressTask`** (`todo.ts:136`): 한 phase에 `in_progress` task가 2개 이상이면 첫 번째만 유지, 나머지 `pending`으로.
- **`getCompletionTransitions`** (`todo.ts:98`): 이전/이후 phase 배열을 비교해 새로 `completed`가 된 task 목록 추출 → TUI 스트라이크루 애니메이션 트리거.
- **`todoTransitionKey`** (`todo.ts:94`): `${phase}\u0000${content}` 키로 중복 식별.
- **`selectStickyTodoWindow`** (`todo.ts:173`): 활성 phase의 처음 N개(기본 5)만 표시, 나머지는 `+M more`로 접음.
- **`todoMatchesAnyDescription`** (`todo.ts:212`): todo 내용과 서브에이전트 설명을 6자 이상 중복 정규화로 매칭.

### 1.4 Markdown 라운드트립 (`todo.ts:424-497`)

- `phasesToMarkdown`: phase를 `- [x]`/`- [ ]` 체크리스트로 직렬화.
- `markdownToPhases`: 사용자 편집된 체크리스트를 phase로 역직렬화.
- `/todo edit` 슬래시 명령이 $EDITOR 열어 사용자가 직접 편집.

### 1.5 세션 영속

todo phase는 세션 엔트리로 저장 (`getLatestTodoPhasesFromEntries`, `todo.ts:159`). 세션 재개 시 마지막 phase 상태 복원.

---

## 2. oxi화 설계

### 2.1 도구 위치: `oxi-agent/src/tools/todo.rs`

```rust
pub struct TodoTool {
    state: Arc<TodoState>,    // 세션 스코프, AgentSession이 소유
}

/// 세션 단위 todo 상태. AgentSession이 Arc로 보유.
pub struct TodoState {
    phases: parking_lot::RwLock<Vec<TodoPhase>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoPhase {
    pub name: String,
    pub tasks: Vec<TodoItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}
```

> **`parking_lot::RwLock` 선택**: todo 상태는 읽기가 압도적으로 많고(TUI 매 프레임 읽음), 쓰기는 도구 호출 시만. 가드를 `.await` 전에 drop하면 `!Send` 문제 없음 (AGENTS.md pitfall).

### 2.2 7개 오퍼레이션 디스패치

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TodoOp {
    Init {
        #[serde(default)]
        list: Option<Vec<InitListEntry>>,
        #[serde(default)]
        items: Option<Vec<String>>,
    },
    Start {
        #[serde(default)]
        task: Option<String>,
        #[serde(default)]
        phase: Option<String>,
    },
    Done {
        #[serde(default)]
        task: Option<String>,
        #[serde(default)]
        phase: Option<String>,
    },
    Drop {
        #[serde(default)]
        task: Option<String>,
        #[serde(default)]
        phase: Option<String>,
    },
    Rm {
        #[serde(default)]
        task: Option<String>,
        #[serde(default)]
        phase: Option<String>,
    },
    Append {
        phase: String,
        items: Vec<String>,
    },
    View,
}

#[derive(Debug, Deserialize)]
pub struct InitListEntry {
    pub phase: String,
    pub items: Vec<String>,
}
```

### 2.3 매개변수 스키마

```rust
impl AgentTool for TodoTool {
    fn name(&self) -> &str { "todo" }
    fn label(&self) -> &str { "Todo" }
    fn essential(&self) -> bool { false }
    fn description(&self) -> &str {
        "Phased todo list manager. Use init to create, start/done/drop to \
         transition, append to add, rm to remove, view to read. Tasks are \
         5-10 words describing WHAT not HOW."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ops": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {
                                "type": "string",
                                "enum": ["init", "start", "done", "drop", "rm", "append", "view"]
                            },
                            "task": {"type": "string", "description": "Task content (verbatim)"},
                            "phase": {"type": "string", "description": "Phase name"},
                            "items": {"type": "array", "items": {"type": "string"}},
                            "list": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "phase": {"type": "string"},
                                        "items": {"type": "array", "items": {"type": "string"}}
                                    }
                                }
                            }
                        },
                        "required": ["op"]
                    }
                }
            },
            "required": ["ops"]
        })
    }
}
```

### 2.4 실행 로직

```rust
async fn execute(
    &self,
    _tool_call_id: &str,
    params: Value,
    _signal: Option<oneshot::Receiver<()>>,
    ctx: &ToolContext,
) -> Result<AgentToolResult, ToolError> {
    let ops: Vec<TodoOp> = serde_json::from_value(
        params.get("ops").cloned().unwrap_or(Value::Array(vec![]))
    ).map_err(|e| ToolError::InvalidParams(e.to_string()))?;

    let mut errors = Vec::new();
    let (new_phases, completed_tasks) = {
        let mut phases = self.state.phases.write();
        let old_phases = phases.clone();
        for op in &ops {
            apply_entry(&mut phases, op, &mut errors);
        }
        normalize_in_progress(&mut phases);
        let completed = get_completion_transitions(&old_phases, &phases);
        phases.clone()
    };
    // 가드 drop (RwLockWriteGuard 해제) — 이후 .await 안전

    // 세션 영속
    if let Some(session) = ctx.session_writer.as_ref() {
        session.append_todo(&new_phases).await.ok();
    }

    // TUI 이벤트 (sticky panel 갱신)
    if let Some(event_tx) = ctx.event_tx.as_ref() {
        let _ = event_tx.send(AgentEvent::TodoUpdate {
            phases: new_phases.clone(),
            completed_tasks,
        });
    }

    let summary = format_summary(&new_phases, &errors, matches!(ops.last(), Some(TodoOp::View)));
    Ok(AgentToolResult::success(summary))
}
```

### 2.5 핵심 알고리즘 (omp 계약 이식)

#### `apply_entry` — 단일 op 적용

```rust
fn apply_entry(phases: &mut Vec<TodoPhase>, op: &TodoOp, errors: &mut Vec<String>) {
    match op {
        TodoOp::Init { list, items } => {
            *phases = init_phases(list.as_deref(), items.as_deref(), errors);
        }
        TodoOp::Start { task, phase } => {
            // task 또는 phase로 대상 식별 → InProgress로
            // 동시에 다른 InProgress task는 Pending으로 정규화
            let targets = resolve_targets(phases, task.as_deref(), phase.as_deref(), errors);
            for (phase_idx, task_idx) in targets {
                phases[phase_idx].tasks[task_idx].status = TodoStatus::InProgress;
            }
        }
        TodoOp::Done { task, phase } => transition_status(phases, task, phase, TodoStatus::Completed, errors),
        TodoOp::Drop { task, phase } => transition_status(phases, task, phase, TodoStatus::Abandoned, errors),
        TodoOp::Rm { task, phase } => remove_tasks(phases, task.as_deref(), phase.as_deref(), errors),
        TodoOp::Append { phase, items } => append_items(phases, phase, items),
        TodoOp::View => { /* 읽기 전용, 변경 없음 */ }
    }
}
```

#### `normalize_in_progress` — 동시 진행 task 정규화

```rust
fn normalize_in_progress(phases: &mut Vec<TodoPhase>) {
    let mut found_in_progress = false;
    // 역순 순회: 가장 최근 phase의 task 우선 유지
    for phase in phases.iter_mut().rev() {
        for task in &mut phase.tasks {
            if task.status == TodoStatus::InProgress {
                if found_in_progress {
                    task.status = TodoStatus::Pending;
                } else {
                    found_in_progress = true;
                }
            }
        }
    }
}
```

#### `get_completion_transitions` — 완료 전환 추출

```rust
/// 이전 상태에서 새 상태로 바뀌며 새로 Completed가 된 task 목록.
/// TUI 스트라이크루 애니메이션 트리거용.
fn get_completion_transitions(
    previous: &[TodoPhase],
    updated: &[TodoPhase],
) -> Vec<TodoCompletionTransition> {
    let mut transitions = Vec::new();
    for new_phase in updated {
        let old_phase = previous.iter().find(|p| p.name == new_phase.name);
        for new_task in &new_phase.tasks {
            if new_task.status != TodoStatus::Completed { continue; }
            let was_completed = old_phase
                .and_then(|p| p.tasks.iter().find(|t| t.content == new_task.content))
                .map_or(false, |t| t.status == TodoStatus::Completed);
            if !was_completed {
                transitions.push(TodoCompletionTransition {
                    phase: new_phase.name.clone(),
                    content: new_task.content.clone(),
                });
            }
        }
    }
    transitions
}
```

#### `todo_matches_any_description` — 서브에이전트 매칭

```rust
/// todo 내용과 서브에이전트 설명이 같은 작업을 가리키는지.
/// 6자 이상 중복 정규화 매칭 (omp TODO_DESCRIPTION_MIN_OVERLAP = 6).
pub fn todo_matches_any_description(content: &str, descriptions: &[String]) -> bool {
    let normalized = normalize_for_match(content);
    if normalized.len() < 6 { return false; }
    descriptions.iter().any(|d| {
        let d_norm = normalize_for_match(d);
        d_norm.contains(&normalized) || normalized.contains(&d_norm)
    })
}

fn normalize_for_match(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
```

> **6자 임계값 근거** (`todo.ts:190`): `review`, `Sonnet` 같은 단일 식별자는 허용하되 `test`, `fix` 같은 짧은 공통 부분문자열 충돌은 배제.

### 2.6 Markdown 라운드트립

```rust
pub fn phases_to_markdown(phases: &[TodoPhase]) -> String {
    let mut out = String::new();
    for (i, phase) in phases.iter().enumerate() {
        if phases.len() > 1 {
            writeln!(out, "{}. {}", roman_numeral(i + 1), phase.name).ok();
        }
        for task in &phase.tasks {
            let marker = match task.status {
                TodoStatus::Completed => "- [x]",
                TodoStatus::Abandoned => "- [-]",
                _ => "- [ ]",
            };
            writeln!(out, "  {} {}", marker, task.content).ok();
        }
    }
    out
}

pub fn markdown_to_phases(md: &str) -> Result<Vec<TodoPhase>, String> {
    // omp markdownToPhases 계약 이식:
    // - `## Phase Name` 또는 `N. Phase Name` 헤더
    // - `- [x]`/`- [ ]`/`- [-]` 체크박스
    // - 헤더 없으면 단일 "Tasks" phase
    ...
}
```

### 2.7 요약 포맷

```rust
fn format_summary(phases: &[TodoPhase], errors: &[String], read_only: bool) -> String {
    let total: usize = phases.iter().map(|p| p.tasks.len()).sum();
    let done: usize = phases.iter()
        .map(|p| p.tasks.iter().filter(|t| t.status == TodoStatus::Completed).count())
        .sum();

    let mut out = if read_only {
        format!("📋 Todo list (read-only) — {}/{} done\n\n", done, total)
    } else if errors.is_empty() {
        format!("✓ Todo updated — {}/{} done\n\n", done, total)
    } else {
        format!("⚠ Todo updated with {} error(s) — {}/{} done\n\n", errors.len(), done, total)
    };

    for (i, phase) in phases.iter().enumerate() {
        if phases.len() > 1 {
            writeln!(out, "{}. {}", roman_numeral(i + 1), phase.name).ok();
        }
        for task in &phase.tasks {
            let icon = match task.status {
                TodoStatus::Completed => "☑",
                TodoStatus::InProgress => "▶",
                TodoStatus::Abandoned => "✗",
                TodoStatus::Pending => "☐",
            };
            writeln!(out, "  {} {}", icon, task.content).ok();
        }
    }

    for err in errors {
        writeln!(out, "  ⚠ {}", err).ok();
    }
    out
}
```

---

## 3. 세션 영속

### 3.1 세션 엔트리 확장

`oxi-cli/src/store/session.rs`의 `SessionEntry`에 todo 타입 추가:

```rust
pub enum SessionEntry {
    // 기존 변형...
    Todo {
        phases: Vec<TodoPhase>,
        timestamp: DateTime<Utc>,
    },
}
```

또는 별도 파일로 분리 (omp는 세션 JSONL 내에 todo 엔트리를 append):

```jsonl
{"type":"todo","phases":[{"name":"Foundation","tasks":[{"content":"Scaffold crate","status":"completed"}]}],"ts":"2026-06-19T..."}
```

> **결정**: 세션 JSONL 내 append (omp 방식). 별도 파일은 세션 무결성 복잡도 증가.

### 3.2 세션 로드 시 복원

```rust
// session.rs
pub fn latest_todo_phases(entries: &[SessionEntry]) -> Vec<TodoPhase> {
    entries.iter().rev()
        .find_map(|e| match e {
            SessionEntry::Todo { phases, .. } => Some(phases.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
```

---

## 4. ToolContext 확장 — 능력 특성 주입 (v2)

> **v2 수정**: v1은 `ToolContext`에 `todo_state`, `session_writer`, `event_tx`를 직접 추가했으나, 실제 코드는 능력 특성 주입 패턴을 사용. [`00-design-revisions.md`](./00-design-revisions.md) §1 참조.

### 4.1 TodoStateProvider 능력 특성

todo 도구와 sticky panel이 공유하는 인터페이스. `ToolContext`의 기존 패턴(`MemoryBackend`, `UrlResolver`)을 따른다:

```rust
// oxi-agent/src/tools.rs — 능력 특성 추가

/// Todo 상태 접근 능력. todo 도구가 호출하고, TUI sticky panel이 소비.
pub trait TodoStateProvider: Send + Sync {
    /// 현재 phase 목록 읽기 (TUI 매 프레임 호출).
    fn get_phases(&self) -> Vec<TodoPhase>;
    /// todo ops 적용 후 새 phase 목록 반환.
    fn apply_ops<'a>(
        &'a self,
        ops: Vec<TodoOp>,
    ) -> Pin<Box<dyn Future<Output = Result<TodoUpdateResult, String>> + Send + 'a>>;
}

/// todo 적용 결과 — 완료 전환 + 갱신된 phase.
pub struct TodoUpdateResult {
    pub phases: Vec<TodoPhase>,
    pub completed_tasks: Vec<TodoCompletionTransition>,
    pub errors: Vec<String>,
}
```

`ToolContext`에 필드 추가 (기존 `with_memory` / `with_url_resolver` 패턴 준수):

```rust
pub struct ToolContext {
    // ...기존 필드 (변경 없음)...
    /// Todo 상태 (todo 도구 활성화 시).
    pub todo: Option<Arc<dyn TodoStateProvider>>,
}

impl ToolContext {
    pub fn with_todo(mut self, todo: Arc<dyn TodoStateProvider>) -> Self {
        self.todo = Some(todo);
        self
    }
}
```

### 4.2 TodoTool — 능력 특성 사용

```rust
pub struct TodoTool;

#[async_trait]
impl AgentTool for TodoTool {
    // ...name/label/description/schema 동일...

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let provider = ctx.todo.as_ref()
            .ok_or("Todo not configured")?;  // ToolError = String

        let ops: Vec<TodoOp> = serde_json::from_value(
            params.get("ops").cloned().unwrap_or(Value::Array(vec![]))
        ).map_err(|e| e.to_string())?;

        let result = provider.apply_ops(ops).await?;

        let summary = format_summary(&result.phases, &result.errors,
            matches!(ops.last(), Some(TodoOp::View)));
        Ok(AgentToolResult::success(summary))
    }
}
```

> **TUI 갱신**: `TodoStateProvider`의 구현체(oxi-cli)가 내부적으로 `Arc<TodoState>`를 보유하고, `apply_ops` 후 채널로 TUI에 알림. `AgentEvent::TodoUpdate`는 사용하지 않음 — 능력 특성 구현체가 직접 TUI 채널에 전송.

### 4.3 서브에이전트 자동 매칭 (⑥ Agent Hub와 연동)
서브에이전트 완료 시, `AgentPoolProvider`(⑥) 능력이 todo 자동 체크 훅을 호출:

```rust
// TodoStateProvider 구현체 (oxi-cli) 내부
pub fn reconcile_with_subagents(&self, pool: &dyn AgentPoolProvider) {
    let completed_descs: Vec<String> = pool.list_agents()
        .into_iter()
        .filter(|a| a.status == AgentHubStatus::Idle)  // 완료된 서브에이전트
        .map(|a| a.current_task.unwrap_or_default())
        .collect();

    let mut phases = self.phases.write();
    for phase in phases.iter_mut() {
        for task in &mut phase.tasks {
            if matches!(task.status, TodoStatus::Pending | TodoStatus::InProgress)
                && todo_matches_any_description(&task.content, &completed_descs)
            {
                task.status = TodoStatus::Completed;
            }
        }
    }
}
```

> **⑥ 의존**: `AgentPoolProvider` 능력이 주입되어야 자동 매칭 동작. N1.11은 ⑥ 완료 후.
## 5. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N1.1 | `TodoState` + `TodoPhase`/`TodoItem`/`TodoStatus` 타입 | — |
| N1.2 | 7개 오퍼레이션 디스패치 (`apply_entry`) | N1.1 |
| N1.3 | `normalize_in_progress` + `get_completion_transitions` | N1.2 |
| N1.4 | `todo_matches_any_description` (매칭 헬퍼) | N1.1 |
| N1.5 | Markdown 라운드트립 (`phases_to_markdown` / `markdown_to_phases`) | N1.1 |
| N1.6 | `format_summary` 출력 | N1.2 |
| N1.7 | `TodoTool` AgentTool 구현 + 스키마 | N1.2, N1.6 |
| N1.8 | `ToolRegistry::with_builtins_cwd` 등록 | N1.7 |
| N1.9 | `TodoStateProvider` 능력 특성 + `ToolContext.with_todo` | N1.7 |
| N1.10 | 세션 영속 (`SessionEntry::Todo` append) | N1.9 |
| N1.11 | 서브에이전트 자동 매칭 훅 (⑥ 완료 후) | N1.4, ⑥ |
| N1.12 | 시스템 프롬프트에 todo 도구 사용 가이드 추가 | N1.8 |
| N1.13 | 단위 테스트 (omp 계약 이식) | N1.7 |
| N1.14 | 통합 테스트 (MockProvider + todo 시퀀스) | N1.13 |

> **독립성**: ⑤는 1차 배치(①②③④)와 독립. ⑫ Mermaid와 병렬 가능.
> **⑥ 의존**: N1.11(자동 매칭)만 ⑥ Agent Hub를 필요로 함. N1.1–N1.10은 독립 동작.

---

## 6. 시스템 프롬프트 가이드

todo 도구 추가 시, 시스템 프롬프트에 사용 가이드 주입 (`oxi-cli/src/prompt/system_prompt.rs`):

```markdown
## Task Management

When working on multi-step tasks (3+ distinct steps), use the `todo` tool to:
1. `init` with a phased plan BEFORE starting work
2. `start` the current task when beginning it
3. `done` when a task is complete
4. `drop` if a task becomes unnecessary

Tasks should be 5-10 words describing WHAT to do, not HOW.
Phases group related tasks (e.g., "Foundation", "Implementation", "Testing").

Do NOT create todos for trivial single-step requests.
```

> **주의**: todo 사용은 **권장**이지 **강제**가 아님. AGENTS.md의 "user hands you a multi-step plan" 규칙만이 todo 사용을 의무화.

---

## 7. 슬래시 명령

### `/todo` 명령 그룹

`oxi-cli/src/tui/slash/builtin/todo.rs` (신규):

| 서브명령 | 동작 |
|---|---|
| `/todo` | 현재 phase 표시 (=`view`) |
| `/todo edit` | $EDITOR로 Markdown 체크리스트 열어 사용자 편집 |
| `/todo clear` | 모든 phase 제거 |
| `/todo expand` | sticky panel 펼치기/접기 토글 |

> **`/todo edit` 구현**: `phases_to_markdown` → 임시 파일 → `$EDITOR` → `markdown_to_phases` → `TodoState` 갱신.

---

## 8. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| `TodoState` 소유권 (AgentSession vs ToolRegistry) | 🟢 결정됨 | `AgentSession`이 `Arc<TodoState>` 소유, `TodoTool`은 참조. 세션 스코프 보장 |
| 동시성 (여러 도구가 동시에 todo 변경) | 🟢 해결됨 | `parking_lot::RwLock`으로 직렬화. 가드는 `.await` 전 drop |
| 세션 영속 포맷 (JSONL 내 vs 별도 파일) | 🟢 결정됨 | JSONL 내 append (omp 방식) |
| `abandoned` 상태 표시 (취소선 vs 별도 아이콘) | 🟡 미결정 | omp는 취소선. oxi는 `✗` 아이콘 제안 |
| todo 사용 강제 여부 | 🟢 결정됨 | 권장만. AGENTS.md "multi-step plan" 규칙만 의무화 |
| `notes` 필드 (HUD 노트) | 🔴 후순위 | omp의 todo notes 기능. N1 이후 별도 검토 |
| `user_todo_edit` 커스텀 이벤트 | 🟢 결정됨 | `/todo edit` 시 `AgentEvent::TodoUpdate`로 동일 이벤트 사용 |

---

## 9. 테스트 계획

### 9.1 단위 테스트 (omp 계약 이식)

omp의 `tools/__tests__/todo.test.ts` 케이스를 Rust로 이식 — 동일 입력/동일 출력:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_with_phased_list() {
        let mut phases = vec![];
        let mut errors = vec![];
        apply_entry(&mut phases, &TodoOp::Init {
            list: Some(vec![
                InitListEntry { phase: "A".into(), items: vec!["a1".into(), "a2".into()] },
                InitListEntry { phase: "B".into(), items: vec!["b1".into()] },
            ]),
            items: None,
        }, &mut errors);
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].name, "A");
        assert_eq!(phases[0].tasks.len(), 2);
        assert!(errors.is_empty());
    }

    #[test]
    fn start_normalizes_other_in_progress() {
        // task A를 in_progress로 한 뒤 task B를 in_progress로 하면
        // A는 pending으로 정규화되어야 함
    }

    #[test]
    fn completion_transition_detects_newly_completed() {
        // pending → completed 전환만 감지, 이미 completed는 제외
    }

    #[test]
    fn todo_matches_subagent_description() {
        assert!(todo_matches_any_description(
            "Implement auth module",
            &["Auth implementation".into()]
        ));
        assert!(!todo_matches_any_description(
            "fix",
            &["fix the bug".into()]  // 6자 미만 정규화 → 매칭 안 됨
        ));
    }

    #[test]
    fn markdown_roundtrip_preserves_state() {
        let phases = vec![TodoPhase {
            name: "Test".into(),
            tasks: vec![TodoItem {
                content: "Run tests".into(),
                status: TodoStatus::Completed,
                notes: None,
            }],
        }];
        let md = phases_to_markdown(&phases);
        let parsed = markdown_to_phases(&md).unwrap();
        assert_eq!(parsed[0].tasks[0].status, TodoStatus::Completed);
    }
}
```

### 9.2 통합 테스트

- MockProvider가 todo 도구를 호출하는 시퀀스 → `AgentEvent::TodoUpdate` 발생 확인.
- 세션 저장 후 재로드 → phase 복원 확인.
- `/todo edit` 슬래시 → Markdown 라운드트립 확인.

---

## 10. 부록: omp → oxi 매핑

| omp 파일 | oxi 위치 |
|---|---|
| `tools/todo.ts` (938줄) | `oxi-agent/src/tools/todo.rs` |
| `tools/todo.ts` (TodoItem/TodoPhase 타입) | `oxi-agent/src/tools/todo.rs` (동일 타입) |
| `tools/todo.ts` (applyEntry/normalizeInProgress) | `oxi-agent/src/tools/todo.rs` (apply_entry/normalize_in_progress) |
| `tools/todo.ts` (phasesToMarkdown/markdownToPhases) | `oxi-agent/src/tools/todo.rs` (phases_to_markdown/markdown_to_phases) |
| `tools/todo.ts` (todoToolRenderer) | `oxi-tui/src/widgets/todo_panel.rs` (⑥ 참조) |
| `tools/todo.ts` (selectStickyTodoWindow) | `oxi-tui/src/widgets/todo_panel.rs` (⑥ 참조) |
| `modes/components/todo-reminder.ts` | `oxi-cli/src/tui/overlay/todo_reminder.rs` (후순위) |
| `modes/controllers/todo-command-controller.ts` | `oxi-cli/src/tui/slash/builtin/todo.rs` |
| `slash-commands/helpers/todo.ts` | `oxi-cli/src/tui/slash/builtin/todo.rs` (통합) |
| `session/agent-session.ts` (getTodoPhases/setTodoPhases) | `oxi-cli/src/app/agent_session.rs` |
