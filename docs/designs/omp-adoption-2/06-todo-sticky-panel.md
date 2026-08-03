# 세부 설계 ⑤(b) — todo sticky panel (TUI 표시 계층)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §1 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`05-todo-tool.md`](./05-todo-tool.md) (데이터 원천)
> omp 분석: `modes/interactive-mode.ts` 1,375–1,567, `tools/todo.ts:818-938` (todoToolRenderer)
> 후속: N1 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

omp의 **sticky todo panel**은 입력창 바로 위에 상주하는 진행 표시기로, 에이전트가 `todo` 도구를 호출할 때마다 실시간 갱신된다. 핵심 디테일:

1. **위치**: 대화 스크롤과 입력창 사이. 항상 보임(sticky).
2. **활성 phase 윈도우**: 여러 phase 중 현재 진행 phase의 task만 표시, 나머지는 접힘.
3. **서브에이전트 매칭 하이라이트**: 실행 중인 서브에이전트와 매칭되는 todo는 accent 색으로 "불이 켜짐".
4. **스트라이크루 애니메이션**: task가 `completed`로 전환되면 14프레임에 걸쳐 취소선이 점진적으로 칠해짐.
5. **자동 제거 타이머**: 닫힌(completed/abandoned) todo는 설정된 지연 후 패널에서 사라짐.
6. **접기/펼치기**: 단일 키(`Tab` 제안)로 전체 phase 펼치기/활성 phase만 보기 토글.

본 설계는 oxicode-tui에 `TodoPanel` 위젯을 추가하고, `AgentEvent::TodoUpdate`를 소비해 렌더하는 파이프라인을 정의한다.

---

## 1. omp 메커니즘

### 1.1 패널 구조 (`modes/interactive-mode.ts:1,525-1,567`)

```
┌─────────────────────────────────────────────────────┐
│  (대화 스크롤 영역 — ChatView)                       │
│                                                      │
├─────────────────────────────────────────────────────┤
│                                                      │
│  📋 Todos                                            │  ← todoContainer
│    ☑ Scaffold crate                          ✓       │
│    ▶ Wire workspace [matched: BuildLoader]  ●       │  ← accent (매칭)
│    ☐ Write tests                                    │
│    + 2 more in "Testing" phase                      │  ← 접힘
│                                                      │
├─────────────────────────────────────────────────────┤
│  > _                                                │  ← 입력창
└─────────────────────────────────────────────────────┘
```

### 1.2 렌더 로직 (`#renderTodoList`, `interactive-mode.ts:1,525`)

```typescript
#renderTodoList(): void {
    this.todoContainer.clear();
    const phases = this.todoPhases.filter(phase => phase.tasks.length > 0);
    if (phases.length === 0) return;

    const indent = "";
    const lines = ["", indent + theme.bold(theme.fg("accent", "Todos"))];

    const activePhase = this.#getActivePhase(phases);
    // 실행 중 서브에이전트 설명 수집
    const activeDescs = this.#getActiveSubagentDescriptions();

    const isMatched = (todo) =>
        activeDescs.length > 0 &&
        todoMatchesAnyDescription(todo.content, activeDescs);

    if (!this.todoExpanded) {
        // 축소: 활성 phase의 처음 5개만
        const { visible, hiddenOpenCount } = selectStickyTodoWindow(activePhase.tasks, 5);
        visible.forEach((todo, index) => {
            const prefix = `${indent}  `;
            lines.push(this.#formatTodoLine(todo, prefix, isMatched(todo)));
        });
        if (hiddenOpenCount > 0) {
            lines.push(`${indent}  ${theme.fg("dim", `+ ${hiddenOpenCount} more`)}`);
        }
    } else {
        // 확장: 모든 phase 전체 표시
        phases.forEach(phase => {
            lines.push(`${indent}${theme.bold(theme.fg("accent", phase.name))}`);
            phase.tasks.forEach(todo => {
                lines.push(this.#formatTodoLine(todo, `${indent}  `, isMatched(todo)));
            });
        });
    }
    this.todoContainer.addChild(new Text(lines.join("\n"), 1, 0));
}
```

### 1.3 todo 라인 포맷 (`#formatTodoLine`, `interactive-mode.ts:1,375`)

```typescript
#formatTodoLine(todo, prefix, matched): string {
    switch (todo.status) {
        case "completed":
            return theme.fg("success", `${prefix}${checkbox.checked} ${chalk.strikethrough(todo.content)}`);
        case "in_progress":
            if (matched) {
                return theme.fg("accent", `${prefix}${checkbox.unchecked} ${todo.content}`);  // ← 불 켜짐
            }
            return theme.fg("accent", `${prefix}${checkbox.unchecked} ${todo.content}`);
        case "abandoned":
            return theme.fg("error", `${prefix}${checkbox.unchecked} ${chalk.strikethrough(todo.content)}`);
        case "pending":
        default:
            if (matched) {
                return theme.fg("accent", `${prefix}${checkbox.unchecked} ${todo.content}`);
            }
            return theme.fg("dim", `${prefix}${checkbox.unchecked} ${todo.content}`);
    }
}
```

### 1.4 스트라이크루 애니메이션 (`tools/todo.ts:718-769`)

```typescript
const TODO_STRIKE_HOLD_FRAMES = 2;     // 완료 후 유지 프레임
const TODO_STRIKE_REVEAL_FRAMES = 12;  // 점진적 취소선 프레임
const TODO_STRIKE_TOTAL_FRAMES = 14;
const STRIKE_START = "\x1b[9m";
const STRIKE_END = "\x1b[29m";

function partialStrikethrough(text: string, visibleChars: number): string {
    // visibleChars까지만 취소선, 나머지는 보통
    return `${STRIKE_START}${text.slice(0, visibleChars)}${STRIKE_END}${text.slice(visibleChars)}`;
}

function strikeRevealCount(text: string, frame: number | undefined): number | undefined {
    if (frame === undefined) return undefined;
    if (frame < TODO_STRIKE_HOLD_FRAMES) return 0;        // 아직 취소선 없음
    const revealFrame = frame - TODO_STRIKE_HOLD_FRAMES;
    if (revealFrame >= TODO_STRIKE_REVEAL_FRAMES) return text.length;  // 완료
    // 점진적: 글자 수에 비례
    return Math.ceil((text.length * revealFrame) / TODO_STRIKE_REVEAL_FRAMES);
}
```

### 1.5 자동 제거 타이머 (`#syncTodoAutoClearTimer`, `interactive-mode.ts:1,464`)

```typescript
#syncTodoAutoClearTimer(): void {
    this.#cancelTodoAutoClearTimer();
    const delaySeconds = this.settings.get("tasks.todoClearDelay");  // 기본 30초
    if (!Number.isFinite(delaySeconds) || delaySeconds < 0) return;
    if (!this.#hasClosedTodos(this.todoPhases)) return;

    this.#todoAutoClearTimer = setTimeout(() => {
        this.todoPhases = this.#removeClosedTodos(this.todoPhases);
        this.#renderTodoList();
    }, delaySeconds * 1000);
    this.#todoAutoClearTimer.unref?.();
}
```

### 1.6 서브에이전트 매칭 (`#reconcileTodosWithSubagents`, `interactive-mode.ts:1,415`)

```typescript
#reconcileTodosWithSubagents(): void {
    const completedDescs = this.#getRecentlyCompletedSubagentDescriptions();
    if (completedDescs.length === 0) return;

    let changed = false;
    const next = this.todoPhases.map(phase => ({
        ...phase,
        tasks: phase.tasks.map(task => {
            if (task.status === "pending" || task.status === "in_progress") {
                if (todoMatchesAnyDescription(task.content, completedDescs)) {
                    changed = true;
                    return { ...task, status: "completed" };
                }
            }
            return task;
        }),
    }));

    if (changed) {
        this.todoPhases = next;
        this.session.setTodoPhases(next);
    }
}
```

---

## 2. oxicode-tui 설계: `TodoPanel` 위젯

### 2.1 위젯 구조

`oxicode-tui/src/widgets/todo_panel.rs` (신규):

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{StatefulWidget, Widget};

/// todo sticky panel. ChatView와 Input 사이에 렌더.
pub struct TodoPanel<'a> {
    theme: &'a Theme,
}

/// 패널 상태 (AppState가 소유)
pub struct TodoPanelState {
    pub phases: Vec<TodoPhase>,
    pub expanded: bool,
    /// 현재 표시 중인 스트라이크루 애니메이션 프레임.
    /// completed_tasks가 비어 있으면 None.
    pub strike_frame: Option<u32>,
    /// 자동 제거 타이머 활성화 여부.
    pub auto_clear_scheduled: bool,
    /// 실행 중 서브에이전트 설명 (매칭 하이라이트용).
    pub active_subagent_descs: Vec<String>,
}

impl TodoPanelState {
    pub fn new() -> Self {
        Self {
            phases: Vec::new(),
            expanded: false,
            strike_frame: None,
            auto_clear_scheduled: false,
            active_subagent_descs: Vec::new(),
        }
    }

    /// 활성 phase (in_progress task를 포함하거나 첫 번째 미완료 phase).
    pub fn active_phase(&self) -> Option<&TodoPhase> {
        self.phases.iter()
            .find(|p| p.tasks.iter().any(|t| t.status == TodoStatus::InProgress))
            .or_else(|| self.phases.iter()
                .find(|p| p.tasks.iter().any(|t| {
                    matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress)
                })))
    }

    /// 닫힌(completed/abandoned) task가 있는지.
    pub fn has_closed_todos(&self) -> bool {
        self.phases.iter()
            .any(|p| p.tasks.iter()
                .any(|t| matches!(t.status, TodoStatus::Completed | TodoStatus::Abandoned)))
    }

    /// 표시할 라인 수 (레이아웃 계산용).
    pub fn line_count(&self) -> usize {
        if self.phases.is_empty() { return 0; }
        let header = 2;  // 빈 줄 + "Todos" 헤더
        if self.expanded {
            let body: usize = self.phases.iter()
                .map(|p| 1 + p.tasks.len())  // phase 헤더 + tasks
                .sum();
            header + body
        } else {
            let active = self.active_phase();
            match active {
                Some(phase) => {
                    let visible = phase.tasks.len().min(5);
                    let hidden = phase.tasks.len().saturating_sub(5);
                    let hidden_line = if hidden > 0 { 1 } else { 0 };
                    header + visible + hidden_line
                }
                None => 0,
            }
        }
    }
}
```

### 2.2 렌더 구현

```rust
impl StatefulWidget for TodoPanel<'_> {
    type State = TodoPanelState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if state.phases.is_empty() { return; }

        let phases: Vec<&TodoPhase> = state.phases.iter()
            .filter(|p| !p.tasks.is_empty())
            .collect();
        if phases.is_empty() { return; }

        let mut y = area.y;
        let x = area.x + 1;  // 좌측 1칸 들여쓰기

        // 헤더: "📋 Todos"
        let header = format!(" {} Todos", checkbox_style(&state));
        buf.set_string(x, y, &header, self.theme.accent_bold());
        y += 2;  // 헤더 + 빈 줄

        if state.expanded {
            // 확장: 모든 phase
            for (i, phase) in phases.iter().enumerate() {
                if phases.len() > 1 {
                    let phase_header = format_phase_display(&phase.name, i + 1);
                    buf.set_string(x, y, &phase_header, self.theme.accent_bold());
                    y += 1;
                }
                for task in &phase.tasks {
                    render_todo_line(buf, x + 1, y, task, state, self.theme);
                    y += 1;
                }
            }
        } else {
            // 축소: 활성 phase만
            if let Some(phase) = state.active_phase() {
                let visible_count = phase.tasks.len().min(5);
                for task in phase.tasks.iter().take(visible_count) {
                    render_todo_line(buf, x + 1, y, task, state, self.theme);
                    y += 1;
                }
                let hidden = phase.tasks.len().saturating_sub(5);
                if hidden > 0 {
                    let hidden_text = format!("+ {} more", hidden);
                    buf.set_string(x + 1, y, &hidden_text, self.theme.dim());
                }
            }
        }
    }
}
```

### 2.3 todo 라인 렌더 (스트라이크루 포함)

```rust
fn render_todo_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    task: &TodoItem,
    state: &TodoPanelState,
    theme: &Theme,
) {
    let matched = !state.active_subagent_descs.is_empty()
        && todo_matches_any_description(&task.content, &state.active_subagent_descs);

    let (icon, base_style) = match task.status {
        TodoStatus::Completed => ("☑", theme.success()),
        TodoStatus::InProgress => ("▶", theme.accent()),
        TodoStatus::Abandoned => ("✗", theme.error()),
        TodoStatus::Pending => {
            if matched { ("☐", theme.accent()) }
            else { ("☐", theme.dim()) }
        }
    };

    // 아이콘 렌더
    buf.set_string(x, y, icon, base_style);

    // 내용 렌더 (스트라이크루 애니메이션)
    let content = &task.content;
    match (&task.status, state.strike_frame) {
        (TodoStatus::Completed, Some(frame)) => {
            // 점진적 스트라이크루
            let reveal = strike_reveal_count(content.chars().count(), frame);
            if let Some(n) = reveal {
                render_partial_strikethrough(buf, x + 2, y, content, n, theme);
            } else {
                // 애니메이션 종료: 전체 취소선
                buf.set_string(x + 2, y, content, theme.success().add_modifier(Modifier::CROSSED_OUT));
            }
        }
        (TodoStatus::Completed, None) => {
            // 애니메이션 없음: 바로 전체 취소선
            buf.set_string(x + 2, y, content, theme.success().add_modifier(Modifier::CROSSED_OUT));
        }
        _ => {
            buf.set_string(x + 2, y, content, base_style);
        }
    }
}
```

> **ratatui `Modifier::CROSSED_OUT`**: ratatui는 `Modifier::CROSSED_OUT`를 지원 (ANSI SGR 9). omp가 직접 ANSI 이스케이프를 쓰는 것과 동일 효과.

### 2.4 스트라이크루 애니메이션 헬퍼

```rust
const TODO_STRIKE_HOLD_FRAMES: u32 = 2;
const TODO_STRIKE_REVEAL_FRAMES: u32 = 12;
const TODO_STRIKE_TOTAL_FRAMES: u32 = TODO_STRIKE_HOLD_FRAMES + TODO_STRIKE_REVEAL_FRAMES;

/// 프레임 번호에 따라 취소선을 칠할 글자 수 반환.
/// None = 애니메이션 종료 (전체 취소선).
fn strike_reveal_count(char_count: usize, frame: u32) -> Option<usize> {
    if frame >= TODO_STRIKE_TOTAL_FRAMES { return None; }
    if frame < TODO_STRIKE_HOLD_FRAMES { return Some(0); }
    let reveal_frame = frame - TODO_STRIKE_HOLD_FRAMES;
    Some(char_count * (reveal_frame as usize) / TODO_STRIKE_REVEAL_FRAMES as usize)
}

/// 부분 취소선 렌더: 처음 n글자만 취소선, 나머지는 보통.
fn render_partial_strikethrough(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    text: &str,
    n: usize,
    theme: &Theme,
) {
    let chars: Vec<char> = text.chars().collect();
    let (struck, rest): (String, String) = chars.iter().enumerate()
        .partition(|(i, _)| *i < n);
    let struck: String = struck.iter().map(|(_, c)| *c).collect();
    let rest: String = rest.iter().map(|(_, c)| *c).collect();

    let struck_style = theme.success().add_modifier(Modifier::CROSSED_OUT);
    let rest_style = theme.success();

    let mut cx = x;
    if !struck.is_empty() {
        buf.set_string(cx, y, &struck, struck_style);
        cx += unicode_width::UnicodeWidthStr::width(struck.as_str()) as u16;  // v2: CJK 2칸 처리
    }
    if !rest.is_empty() {
        buf.set_string(cx, y, &rest, rest_style);
    }
}
```

---

## 3. AppState 통합

### 3.1 상태 필드

`oxicode-cli/src/tui/app.rs`의 `AppState`에 추가:

```rust
pub struct AppState {
    // 기존 필드...
    pub todo_panel: TodoPanelState,
    /// 자동 제거 타이머 핸들.
    todo_auto_clear: Option<tokio::time::JoinHandle<()>>,
}
```

### 3.2 이벤트 소비

`AgentEvent::TodoUpdate`를 받으면 `AppState` 갱신:

```rust
// handlers.rs 또는 app.rs의 이벤트 루프
async fn handle_agent_event(event: AgentEvent, state: &mut AppState) {
    match event {
        AgentEvent::TodoUpdate { phases, completed_tasks } => {
            state.todo_panel.phases = phases;

            // 완료된 task가 있으면 스트라이크루 애니메이션 시작
            if !completed_tasks.is_empty() {
                state.todo_panel.strike_frame = Some(0);
                // TODO: 애니메이션 타이머 시작 (tick마다 strike_frame 증가)
            }

            // 자동 제거 타이머 재스케줄
            if state.todo_panel.has_closed_todos() {
                schedule_todo_auto_clear(state);
            }
        }
        _ => {}
    }
}
```

### 3.3 자동 제거 타이머

```rust
fn schedule_todo_auto_clear(state: &mut AppState) {
    // 기존 타이머 취소
    if let Some(handle) = state.todo_auto_clear.take() {
        handle.abort();
    }

    let delay = state.settings.todo_clear_delay;  // 기본 30초
    if delay == 0 { return; }  // 0 = 자동 제거 비활성화

    // tokio::time::sleep으로 지연 후 phase에서 닫힌 task 제거
    // (실제 구현에서는 mpsc 채널로 지연 메시지 전송)
    state.todo_auto_clear = Some(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(delay)).await;
        // UiEvent::TodoAutoClear 전송
    }));
}
```

### 3.4 서브에이전트 설명 수집 (⑥ Agent Hub 의존)

```rust
/// 현재 실행 중인 서브에이전트의 설명 목록.
/// ⑥ Agent Hub 완료 전에는 빈 vec (매칭 비활성화).
fn active_subagent_descs(state: &AppState) -> Vec<String> {
    if let Some(registry) = &state.agent_registry {
        registry.list_running()
            .iter()
            .map(|a| a.description.clone())
            .collect()
    } else {
        Vec::new()
    }
}
```

---

## 4. 레이아웃 통합

### 4.1 ChatView와 Input 사이에 패널 삽입

`oxicode-tui/src/widgets/chat/mod.rs`의 `StatefulWidget::render` 또는 `oxicode-cli/src/tui/render.rs`에서:

```rust
// 레이아웃 분할
let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(1),                           // ChatView
        Constraint::Length(todo_panel_height),         // TodoPanel (동적)
        Constraint::Length(footer_height),             // Footer/Status
        Constraint::Length(input_height),              // Input
    ])
    .split(area);

// ChatView 렌더
ChatView::new(theme).render(chunks[0], buf, &mut state.chat);

// TodoPanel 렌더 (phase가 있을 때만)
if !state.todo_panel.phases.is_empty() {
    TodoPanel::new(theme).render(chunks[1], buf, &mut state.todo_panel);
}

// Footer, Input 렌더...
```

> **동적 높이**: `todo_panel_height = state.todo_panel.line_count() as u16`. phase가 없으면 0.

### 4.2 패널 토글 키

`oxicode-cli/src/tui/handlers.rs`에 추가:

```rust
// Tab 키 (또는 설정 가능한 키)로 패널 확장/축소 토글
KeyCode::Tab => {
    if !state.todo_panel.phases.is_empty() {
        state.todo_panel.expanded = !state.todo_panel.expanded;
    }
}
```

> **키 충돌 주의**: `Tab`이 입력 필드 자동완성에 이미 사용 중이면 별도 키 필요. omp는 `Ctrl+T` 사용 제안. `oxicode-tui/src/keybindings/`에서 설정 가능하도록.

---

## 5. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N1.15 | `TodoPanelState` + `line_count()` | ⑤ N1.1 |
| N1.16 | `TodoPanel` 위젯 (기본 렌더: 헤더 + phase + task) | N1.15 |
| N1.17 | todo 라인 렌더 (상태별 아이콘 + 색상) | N1.16 |
| N1.18 | `render_partial_strikethrough` + `strike_reveal_count` | N1.17 |
| N1.19 | 스트라이크루 애니메이션 타이머 (tick → frame 증가) | N1.18 |
| N1.20 | `AgentEvent::TodoUpdate` → `TodoPanelState` 브리지 | N1.16, ⑤ N1.9 |
| N1.21 | 레이아웃 통합 (ChatView/Input 사이 패널 슬롯) | N1.20 |
| N1.22 | 접기/펼치기 토글 키 | N1.21 |
| N1.23 | 자동 제거 타이머 | N1.20 |
| N1.24 | 서브에이전트 매칭 하이라이트 (⑥ 완료 후) | N1.17, ⑥ |
| N1.25 | 도구 호출 결과에도 인라인 todo 렌더 (tool_renderer 확장) | N1.17 |

> **⑤ N1.7 이후 착수**: TodoTool이 `AgentEvent::TodoUpdate`를 발생시켜야 패널이 갱신됨.

---

## 6. 도구 호출 결과 인라인 렌더

`todo` 도구가 반환한 결과를 TUI 대화 내에도 표시 (`tools/todo.ts:849-936`의 `todoToolRenderer` 대응).

`oxicode-tui/src/widgets/tool_renderer.rs`에 todo 분기 추가:

```rust
fn render_todo_result(result: &str, args: &Value, theme: &Theme) -> Vec<Line<'_>> {
    // 결과 텍스트를 그대로 표시하되, phase 헤더와 체크박스를 색상화
    // omp의 framedBlock 스타일: 테두리 + "Todo N tasks" 헤더
    let mut lines = Vec::new();

    // 헤더
    lines.push(Line::from(vec![
        Span::styled("📋 ", theme.accent()),
        Span::styled("Todo", theme.bold()),
    ]));

    // 본문 (결과 텍스트 파싱 → 색상화)
    for line in result.lines() {
        let styled = style_todo_line(line, theme);
        lines.push(styled);
    }

    lines
}
```

> 이 인라인 렌더는 sticky panel과 **독립** — 대화 스크롤 내에 도구 호출 카드로 표시. sticky panel은 입력창 위에 별도.

---

## 7. 설정

```rust
pub struct Settings {
    pub todo_panel_enabled: bool,       // 기본 true
    pub todo_clear_delay: u64,          // 기본 30 (초). 0 = 비활성화
    pub todo_strikethrough_animation: bool,  // 기본 true
    pub todo_max_visible: usize,        // 기본 5 (축소 모드 표시 개수)
}
```

---

## 8. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| 렌더 성능 (매 프레임 재구축) | 🟡 최적화 | omp는 render cache. oxicode는 `line_count()`로 높이만 계산, 라인은 캐시 |
| 스트라이크루 애니메이션 프레임 속도 | 🟢 결정됨 | 60fps tick (16ms). 14프레임 = ~230ms |
| 토글 키 충돌 (Tab vs 자동완성) | 🟡 미결정 | `Ctrl+T` 제안. keybindings에서 설정 가능 |
| 패널이 없을 때 레이아웃 (빈 줄) | 🟢 결정됨 | `line_count() == 0`이면 높이 0, 레이아웃에서 제외 |
| 다중 phase 표시 (축소 시 다음 phase 미리보기) | 🟢 결정됨 | 활성 phase만. 확장 시 전체 |
| ratatui `Modifier::CROSSED_OUT` 호환성 | 🟢 검증됨 | ratatui 0.28+ 지원. 구버전 fallback: 색상만 |
| 자동 제거 타이머와 세션 종료 | 🟢 결정됨 | 세션 종료 시 타이머 abort. phase는 세션에 영속 |

---

## 9. 테스트 계획

### 9.1 위젯 단위 테스트

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    #[test]
    fn empty_state_renders_nothing() {
        let mut state = TodoPanelState::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        TodoPanel::new(&test_theme()).render(
            Rect::new(0, 0, 40, 10),
            &mut buf,
            &mut state,
        );
        // phase가 없으면 아무것도 렌더하지 않음
        assert!(buf.content().iter().all(|c| c.symbol() == " "));
    }

    #[test]
    fn collapsed_shows_active_phase_only() {
        let mut state = TodoPanelState::new();
        state.phases = vec![
            TodoPhase { name: "A".into(), tasks: vec![
                TodoItem { content: "a1".into(), status: TodoStatus::Completed, notes: None },
                TodoItem { content: "a2".into(), status: TodoStatus::InProgress, notes: None },
            ]},
            TodoPhase { name: "B".into(), tasks: vec![
                TodoItem { content: "b1".into(), status: TodoStatus::Pending, notes: None },
            ]},
        ];
        // 축소 모드: phase A만 (in_progress task 포함)
        assert_eq!(state.active_phase().unwrap().name, "A");
    }

    #[test]
    fn strike_reveal_progression() {
        // frame 0: 0글자 취소선
        assert_eq!(strike_reveal_count(10, 0), Some(0));
        // frame 2 (HOLD 끝): 0글자
        assert_eq!(strike_reveal_count(10, 2), Some(0));
        // frame 8 (중간): 10 * 6/12 = 5글자
        assert_eq!(strike_reveal_count(10, 8), Some(5));
        // frame 14 (종료): None (전체)
        assert_eq!(strike_reveal_count(10, 14), None);
    }

    #[test]
    fn line_count_collapsed_capped_at_5() {
        let mut state = TodoPanelState::new();
        state.phases = vec![TodoPhase {
            name: "Big".into(),
            tasks: (0..10).map(|i| TodoItem {
                content: format!("task {}", i),
                status: TodoStatus::Pending,
                notes: None,
            }).collect(),
        }];
        // 헤더(2) + 5 visible + 1 hidden line = 8
        assert_eq!(state.line_count(), 8);
    }
}
```

### 9.2 통합 테스트

- MockProvider → todo 도구 호출 → `AgentEvent::TodoUpdate` → `TodoPanelState` 갱신 확인.
- 토글 키 → `expanded` 전환 → `line_count` 변화 확인.

---

## 10. 부록: omp → oxicode 매핑

| omp 위치 | oxicode 위치 |
|---|---|
| `interactive-mode.ts:583` (`todoContainer`) | `oxicode-tui/src/widgets/todo_panel.rs` (`TodoPanel`) |
| `interactive-mode.ts:1,525` (`#renderTodoList`) | `TodoPanel::render` |
| `interactive-mode.ts:1,375` (`#formatTodoLine`) | `render_todo_line()` |
| `interactive-mode.ts:1,415` (`#reconcileTodosWithSubagents`) | `handlers.rs` (⑥ Agent Hub 이벤트 소비) |
| `interactive-mode.ts:1,464` (`#syncTodoAutoClearTimer`) | `schedule_todo_auto_clear()` |
| `interactive-mode.ts:3,818` (`toggleTodoExpansion`) | `handlers.rs` (토글 키) |
| `tools/todo.ts:818` (`todoToolRenderer`) | `oxicode-tui/src/widgets/tool_renderer.rs` (todo 분기) |
| `tools/todo.ts:718` (`TODO_STRIKE_*`) | `todo_panel.rs` (`TODO_STRIKE_*` 상수) |
| `tools/todo.ts:745` (`formatTodoLine` with strike) | `render_partial_strikethrough()` |
| `tools/todo.ts:173` (`selectStickyTodoWindow`) | `TodoPanelState::active_phase` + line_count |
