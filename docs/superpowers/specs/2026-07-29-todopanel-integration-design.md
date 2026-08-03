# P0.1: TodoPanel Tape Render Integration — Design

> **Tier 1 — Architectural design.** 대상: tape_render.rs + TodoPanel 위젯.
> 선행: REMAINING.md §P0.1.

## Problem

`oxicode-tui/src/widgets/todo_panel.rs`에 `TodoPanel` StatefulWidget이 완전히 구현되어 있다 (line_count, render, collapsed/expanded, strikethrough 스타일). 하지만 `oxicode-cli/src/tui/tape_render.rs`는 이 위젯을 전혀 호출하지 않고, 대신 compact `"X todos"` badge 문자열만 sticky 영역에 추가한다.

즉, TodoPanelState는 AppState에서 매 프레임 sync되지만, 그 데이터를 시각화하는 코드가 tape_render.rs에 없다.

## Current Code Path

```
AppState.todo_panel (TodoPanelState)
  ← app.rs:1409 sync_todo_panel() from TodoStateProvider  [✅ works]
  → tape_render.rs:47-49: format!(" {} todos", ...)      [❌ badge only]
  → TodoPanel::render()                                    [❌ never called]
```

## Constraints

1. **tape_render.rs는 TapeEngine 위에서 동작** — sticky row가 `LiveRegion::Pinned`로 관리된다. TodoPanel은 ratatui `StatefulWidget`이므로 `Buffer` + `Rect`가 필요하다.
2. **기존 sticky rows 유지** — steering messages, status line, input line은 TodoPanel 아래에 그대로 있어야 한다.
3. **v2 pipeline과의 관계** — 현재 tape_render.rs는 `TapeRenderState.sync()`에서 plain `Vec<String>`을 빌드한다. TodoPanel은 이 string 기반 파이프라인이 아니라 Buffer 기반 렌더가 필요하다.
4. **`todo_panel_enabled` 설정** — false일 때는 기존 badge 동작으로 fallback.

## Design

### Approach: Dual-layer sticky render

tape_render.rs의 현재 sync() 메서드는 모든 sticky row를 `self.rows: Vec<String>`에 string으로 쌓고, 이를 TapeEngine의 pinned 영역에 한 번에 flush한다. TodoPanel은 Buffer 기반이므로 두 가지 접근이 가능하다:

**Option A (recommended):** TodoPanel을 TapeEngine 밖에서 Buffer에 렌더링한 후, 결과 문자열을 rows에 삽입한다.
- 장점: TapeEngine 변경 불필요, 기존 string 파이프라인과 공존
- 단점: ratatui Span → String 변환 오버헤드

**Option B:** TapeEngine에 Buffer-overlay 슬롯을 추가한다.
- 장점: 더 순수한 접근
- 단점: TapeEngine core 변경, 복잡도 증가, TodoPanel 하나를 위해 인프라 변경

**Decision: Option A.** TapeEngine을 건드리지 않고 tape_render.rs만 변경.

### Detailed Design

#### Step 1: TodoPanelState → String 변환 헬퍼

`oxicode-cli/src/tui/tape_render.rs`에 새 메서드 추가:

```rust
/// Render the todo panel into sticky lines, using the ratatui StatefulWidget.
/// Outputs styled text lines that fit `width`.
fn render_todo_lines(
    panel: &mut TodoPanelState,
    theme: &Theme,
    width: u16,
) -> Vec<String> {
    if panel.is_empty() {
        return Vec::new();
    }
    // Use ratatui Buffer + TodoPanel::render(), then extract text
    let area = Rect::new(0, 0, width, panel.line_count() as u16);
    let mut buf = Buffer::empty(area);
    TodoPanel::new(theme).render(area, &mut buf, panel);
    
    // Convert buf lines to plain strings
    (0..area.height)
        .map(|y| {
            let cells = buf.cells_at(0, y);
            // ...
        })
        .collect()
}
```

Buffer→String 변환은 `buf.cells_at(x, y)`로 각 셀의 symbol을 읽어 한 줄로 조합한다. ratatui의 `Buffer`는 `Vec<Cell>`로, 각 `Cell`은 `symbol: String`을 가진다.

#### Step 2: tape_render.rs sync() 수정

현재 코드:
```rust
if !app.todo_panel.phases.is_empty() {
    self.rows.push(format!(" {} todos", app.todo_panel.phases.len()));
}
```

수정 후:
```rust
if app.todo_panel_enabled && !app.todo_panel.is_empty() {
    let todo_lines = render_todo_lines(
        &mut app.todo_panel.clone(),
        theme,
        content_width,
    );
    self.rows.extend(todo_lines);
} else if !app.todo_panel.is_empty() {
    // Fallback: compact badge (existing behavior)
    self.rows.push(format!(" {} todos", app.todo_panel.phases.len()));
}
```

`todo_panel.clone()`이 필요한 이유: `render_todo_lines`가 `&mut TodoPanelState`를 요구하지만, `app`은 immutable borrow (`&AppState`)다. TodoPanelState는 `Clone`이므로 복사 후 렌더한다.

#### Step 3: Sticky layout 조정

현재 tape_render.rs의 sticky 영역 순서:
1. steering messages
2. TODO badge (compact)
3. status (working/ready)
4. input

수정 후:
1. steering messages
2. TodoPanel (expanded or collapsed — 여러 줄일 수 있음)
3. status (working/ready)
4. input

`LiveRegion::Pinned { start }`의 `start` 값은 sticky 영역 시작 전 transcript 끝 위치다. TodoPanel 줄 수가 동적으로 변하므로, `start` 계산은 `self.rows.len()` 기준으로 유지 (이미 동적임).

### AppState에 todo_panel_enabled 전달

`AppState`에 `todo_panel_enabled: bool` 필드 추가. `sync_todo_panel()` 호출 시점에 `settings.todo_panel_enabled` 값을 반영.

```rust
// oxicode-cli/src/tui/app.rs
pub struct AppState {
    // ... existing fields
    pub todo_panel_enabled: bool,  // synced from settings
}
```

### Files to Modify

| File | Change |
|---|---|
| `oxicode-cli/src/tui/tape_render.rs` | Add `render_todo_lines()` helper. Replace badge line with full panel render. Gate with `app.todo_panel_enabled`. |
| `oxicode-cli/src/tui/app.rs` | Add `todo_panel_enabled: bool` field. Sync from settings at state init. |
| `oxicode-tui/src/widgets/todo_panel.rs` | No changes needed (widget API is sufficient). |

### Striketheough Animation Note

현재 `TodoPanel` 위젯에는 strikethrough 스타일이 `TodoPanelStatus::Completed`에 대해 `Modifier::CROSSED_OUT`로 구현되어 있다 (코드 확인 필요 — 라인 200+ 영역). 이 설계에서는 strikethrough가 이미 위젯에 내장되어 있다고 가정한다. 만약 없다면 Completed 상태에 `Modifier::CROSSED_OUT`을 추가하는 작은 변경이 별도로 필요하다.

### Acceptance Criteria

1. TodoPanel 위젯이 화면 상단 sticky 영역에 표시됨 (phase + task 목록)
2. Collapsed 모드: active phase만 표시, "N more..." 줄
3. Expanded 모드: 모든 phase 표시
4. `todo_panel_enabled: false` → compact badge fallback
5. Steering messages, status, input 라인이 TodoPanel 아래에 그대로 유지
6. `cargo test` 통과, `cargo build` 통과

### Test Strategy

- `tape_render.rs`에 `render_todo_lines()` 단위 테스트 추가 (빈 패널, 1 phase, multi-phase, collapsed/expanded)
- 기존 `todo_panel.rs` 테스트는 변경 없음
- 수동 확인: TUI 실행 후 `/todo`로 phase 생성 시 화면 상단 패널 표시
