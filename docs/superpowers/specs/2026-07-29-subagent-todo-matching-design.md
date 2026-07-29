# P1.4: Subagent-Todo Auto Matching — Design

> **Tier 1 — Architectural design.** 대상: SubagentTool + TodoStateProvider.
> 선행: REMAINING.md §P1.4.

## Problem

현재 subagent spawn과 todo system이 disconnected:
1. LLM이 `task`(subagent)로 서브에이전트를 띄우면, todo phase에 자동 등록되지 않음
2. 서브에이전트 완료 시 해당 todo item이 자동으로 done 마킹되지 않음
3. TUI에서 strikethrough 애니메이션 없음

omp에서는 subagent spawn 시 자동으로 todo item이 생성되고, 완료 시 strikethrough+slide로 표시된다.

## Enabling Infrastructure (이미 존재)

- `ToolContext.todo: Option<Arc<dyn TodoStateProvider>>` — 모든 tool에서 접근 가능
- `TodoStateProvider` trait: `apply_ops(ops: &[TodoOp]) -> TodoUpdateResult`
- `TodoOp::Start { task }`, `TodoOp::Done { task }` — item 단위 op
- `TodoPanelStatus::Completed` → `Modifier::CROSSED_OUT` (TodoPanel 위젯 내장)

## Design

### Core Mechanism: SubagentTool.execute()에서 todo 연동

`oxi-agent/src/tools/subagent.rs`의 `SubagentTool.execute()`에서 spawn 전후에 todo state provider를 호출한다.

#### Spawn 전: todo item 생성

```
ctx.todo.apply_ops(&[TodoOp::Start {
    task: subagent_task_description,
    phase: Some("Subagents"),  // 자동 생성 phase
}])
```

`subagent_task_description`은 `task` 파라미터의 prompt에서 추출. 구체적으로는 `params["task"]`의 첫 80자.

#### 완료 후: todo item done 마킹

```
ctx.todo.apply_ops(&[TodoOp::Done {
    task: subagent_task_description,
}])
```

### Phase 관리

- 자동 생성되는 todo item은 `"Subagents"`라는 고정 phase에 등록
- Phase가 없으면 `apply_ops`가 자동 생성 (TodoStateProvider에 위임)
- 사용자는 `/todo` 명령으로 이 phase를 자유롭게 편집/이동 가능

### Striketheough Animation

이미 `TodoPanelStatus::Completed` → `Modifier::CROSSED_OUT` 처리가 `TodoPanel` 위젯에 구현되어 있다고 가정. 확인 결과 `todo_panel.rs`의 `render()`에서 `TodoPanelStatus::Completed`에 대해 `Modifier::CROSSED_OUT`을 적용한다. (line 200+ 영역)

### TUI Transition (선택사항)

omp의 strikethrough + slide animation은 ratatui의 단일 프레임 모델에서 구현하기 어렵다. 대신:
- Completed item: 즉시 `Modifier::CROSSED_OUT` + `Style::fg(dim)` 처리
- 일정 시간 후 (선택): fade out (dim 스타일만 유지)
- 애니메이션은 Phase 2 (todo panel UX 개선 시)로 defer

### TodoTool과의 협력

TodoTool은 이미 `TodoStateProvider`를 통해 상태를 읽고 쓴다. SubagentTool이 같은 provider를 공유하므로 충돌 없음. 두 도구가 동시에 write하는 race condition은 `apply_ops`의 atomicity에 의존 — `TodoUpdateResult`로 결과 확인.

### Feature Gate

`todo_panel_enabled: false`일 때는 SubagentTool이 todo 호출을 skip한다.

```rust
// check via settings — ToolContext에 settings 전달 필요
if ctx.todo_panel_enabled {
    let _ = ctx.todo.as_ref().map(|t| t.apply_ops(&ops));
}
```

`ctx.todo_panel_enabled`는 어디서? 현재 ToolContext에는 settings가 없다. 대안:
- Option A: ToolContext에 `todo_panel_enabled: bool` 추가 (단순, 약간의 coupling)
- Option B: SubagentTool이 settings를 몰라도 todo가 None이면 skip (이미 그렇게 동작)

**Decision: Option B.** `ctx.todo`가 `None`이면 자동 skip. `todo_panel_enabled: false`일 때 `oxi-cli`가 `with_todo(None)`으로 설정하면 SubagentTool은 아무것도 하지 않는다. 즉, gating은 oxi-cli 레벨에서 처리.

### Files to Modify

| File | Change |
|---|---|
| `oxi-agent/src/tools/subagent.rs` | `execute()` 내 spawn 전: `ctx.todo.apply_ops([Start])`. spawn 후: `apply_ops([Done])`. SubagentTaskResult 도착 시 done 마킹. |
| `oxi-tui/src/widgets/todo_panel.rs` | (확인) Completed 상태에 CROSSED_OUT 적용 확인. 없으면 추가. |
| `oxi-cli/src/bootstrap.rs` | (변경 없음) — `with_todo` 설정은 이미 존재 |

### SubagentTool.execute() 의사코드

```rust
// Before spawning
let desc = params["task"].as_str().unwrap_or("subagent task");
if let Some(todo) = &ctx.todo {
    let ops = [TodoOp::Start {
        task: desc.chars().take(80).collect(),
        phase: Some("Subagents".into()),
    }];
    let _ = todo.apply_ops(&ops);
}

// Spawn the subagent (existing logic)
let result = self.spawn_subagent(params, signal, ctx).await?;

// After completion
if let Some(todo) = &ctx.todo {
    let ops = [TodoOp::Done {
        task: desc.chars().take(80).collect(),
    }];
    let _ = todo.apply_ops(&ops);
}
```

### Error Handling

- `apply_ops` 실패 → trace warn, subagent 실행 계속 (todo 실패가 subagent를 막지 않음)
- todo provider가 `None` → skip (정상)
- description이 너무 길면 trim (80자 제한)

### Acceptance Criteria

1. Subagent spawn 시 "Subagents" phase에 todo item이 자동 생성됨
2. Subagent 완료 시 해당 item이 자동 done 마킹됨
3. TodoPanel 위젯에서 Completed item이 CROSSED_OUT 스타일로 표시됨
4. `ctx.todo == None`일 때 subagent는 정상 동작 (skip)
5. 기존 todo tool 테스트 모두 통과

### Test Strategy

- `subagent.rs`에 테스트: `MockTodoStateProvider` 만들어서 spawn 전후의 state 변화 확인
- 기존 `todo.rs` 테스트는 변경 없음
- 수동 확인: TUI에서 `task("do X")` 호출 시 "Subagents" phase 생성 + item 표시
