# Design: TUI Input Area & Overlay Fixes

> Date: 2025-05-25
> Status: Draft

## Problem Summary

3개의 독립적인 TUI 렌더링 버그가 있다.

### Bug 1: Input Area Status Line이 Separator에 의해 덮어씌워짐

**원인**: `render_input_area`에서 `separator_row`의 y좌표 계산이 잘못됨.

```
queue가 없을 때 (queue_lines = 0):
  status_row.y = area.y + 0        ← 상태 텍스트 렌더링
  separator_row.y = area.y + 1 + 0 - 1 = area.y + 0  ← 같은 위치! 덮어씌움
```

`render_status_line`이 `── ○ Idle ──────` 형태로 상태 텍스트를 구분선 안에 그리는데,
바로 뒤에 `separator_row`가 **같은 y좌표**에 순수 `──────` 구분선을 그려서 상태 텍스트를 지워버린다.

**결과**: 상태 표시(Working/Idle)가 사라지고 구분선만 보임.

**수정**:
- `separator_row`는 상태 줄 아래에 위치해야 함.
- `queue_lines = 0`일 때: `separator_row.y = area.y + 1` (status 아래 한 줄)
- `queue_lines > 0`일 때: `separator_row.y = area.y + 1 + queue_lines` (queue 아래)
- 즉, 공식은 `area.y + 1 + queue_lines` (끝에 `- 1` 제거)
- **또는**, separator_line 자체가 필요 없음. status line이 이미 구분선 형태이므로, queue 아래 입력창 위에만 구분선 추가.

### Bug 2: Provider 선택 — 카테고리 그룹화 UI + 스크롤 불가

**원인**: `render_provider_list`가 `Paragraph` 위젯을 사용.

`Paragraph`는 ratatui에서 스크롤을 지원하지 않는 정적 텍스트 위젯이다.
반면 `render_selectable_list`는 `List` 위젯 + 수동 스크롤 윈도우를 사용한다.
Provider 목록만 `Paragraph`를 써서 화면보다 항목이 많으면:
- 하단 항목이 화면 밖으로 잘림
- 키보드로 내려가도 스크롤 안 됨 (selected index는 바뀌지만 화면엔 반영 안 됨)
- 전체 항목 수 / 현재 위치 / 남은 항목 수를 알 수 없음

**수정**: `render_provider_list`를 `List` 위젯 기반으로 재작성.
- 카테고리 헤더 + 일반 항목을 모두 `ListItem`으로 구성
- 수동 스크롤 윈도우 (`window_start`) 계산 추가
- 하단 hint에 `(3/28 providers, 25 below)` 형태로 위치 정보 표시
- 카테고리별 구분선/배경색 개선 (카테고리 헤더에 연한 배경색)

### Bug 3: Model Select Overlay — 스크롤 지원 없음

**원인**: `render_model_select`는 `render_selectable_list`를 사용하므로 스크롤 자체는 되지만,
필터가 있을 때 selected 인덱스 매핑이 단순함. popup 크기가 고정(0.7 × 0.7)이라 작은 터미널에서 잘릴 수 있음.

**수정**:
- `render_selectable_list`에 `(selected+1/total)` 위치 표시 추가
- 모델 수가 많을 때 터미널 크기에 맞춰 popup 크기 동적 조정

---

## Detailed Design

### 1. Input Area Status Line Fix

**File**: `oxicode-cli/src/tui/render.rs` → `render_input_area()`

현재 레이아웃 (queue 없음, queue_lines=0):
```
y+0: status_row       ← render_status_line 그림
y+0: separator_row    ← 같은 위치! 덮어씌움  ← BUG
y+1: input_row
```

수정 후 레이아웃:
```
y+0: status_row       ← render_status_line (이미 ── ○ Idle ── 형태)
y+1: input_row        ← 바로 입력창 (추가 구분선 불필요)
```

queue 있음 (queue_lines=N):
```
y+0: status_row       ← 상태 구분선
y+1 ~ y+N: queue rows ← queue 항목들
y+N+1: input_row      ← 입력창 (queue 마지막 줄과 입력창 사이에 구분선은 필요 없음)
```

**핵심 변경**:
- `separator_row` 렌더링 코드를 완전히 제거
- `render_status_line`이 이미 구분선 형태이므로 별도 구분선 불필요
- `input_row` y좌표: `area.y + 1 + queue_lines`

### 2. Provider List Rewrite

**File**: `oxicode-cli/src/tui/render.rs` → `render_provider_list()`

현재: `Paragraph::new(lines)` — 스크롤 불가

수정: `List` 위젯 + 스크롤 윈도우

```rust
fn render_provider_list(
    f: &mut Frame,
    area: Rect,
    providers: &[ProviderInfo],
    selected: usize,
    styles: &ThemeStyles,
    theme: &Theme,
) {
    // 1. 카테고리 순서대로 평면 리스트 구성
    //    각 항목은 ListItem (카테고리 헤더도 ListItem)
    //    카테고리 헤더는 non-selectable이므로 인덱스 매핑 필요

    // 2. 스크롤 윈도우 계산
    //    - 실제 provider 항목의 인덱스만 스크롤에 관여
    //    - 카테고리 헤더는 항상 visible window 안에 포함

    // 3. 하단에 위치 표시
    //    " ↑↓ select  |  Enter confirm  |  q quit  (12/28, 16 below)"
}
```

**구현 상세**:

a) **평면 인덱스 매핑**: providers 배열 인덱스 → 표시되는 줄 번호 매핑을 만든다.
   카테고리 헤더 줄도 포함하므로 selected provider 인덱스를 표시 줄 번호로 변환 필요.

b) **스크롤 윈도우**: `render_selectable_list`와 동일한 방식:
   ```rust
   let max_show = list_area.height as usize;
   let window_start = if selected_line >= max_show {
       selected_line - max_show + 1
   } else {
       0
   };
   ```

c) **카테고리 헤더 스타일 개선**:
   - 현재: `Color::Cyan` 볼드 텍스트만
   - 개선: 연한 배경색 + 왼쪽 패딩 + 카테고리 구분 선
   ```
    ── Primary Providers ──────────────
    ✓ OpenAI       — GPT-4, GPT-3.5     [key set]
    ○ Anthropic    — Claude 3.5
    ── Chinese AI ──────────────────────
    ○ DeepSeek     — DeepSeek V3
   ```

d) **위치 표시**: 하단 hint에 현재 위치와 남은 항목 수 표시

### 3. Scrollable List Position Indicator

**File**: `oxicode-cli/src/tui/render.rs` → `render_selectable_list()`

하단에 위치 표시 추가:
```rust
let position_hint = if items.len() > max_show {
    let below = items.len().saturating_sub(window_start + max_show);
    format!(" ({} below)", below)
} else {
    String::new()
};
```

이 hint는 `render_selectable_list`의 반환값으로 전달하거나,
호출부에서 추가로 렌더링한다.

---

## Files to Modify

| File | Change |
|------|--------|
| `oxicode-cli/src/tui/render.rs` | Bug 1: `render_input_area`에서 separator 제거, input_row y좌표 수정 |
| `oxicode-cli/src/tui/render.rs` | Bug 2: `render_provider_list`를 List 기반 스크롤 가능하게 재작성 |
| `oxicode-cli/src/tui/render.rs` | Bug 3: `render_selectable_list`에 위치 표시 추가 |
| `oxicode-cli/src/tui/render.rs` | Provider 카테고리 헤더 스타일 개선 |

---

## Implementation Order

1. **Bug 1** (Input area status line) — 가장 간단, 5분
2. **Bug 3** (Scrollable list position) — `render_selectable_list` 수정, 15분
3. **Bug 2** (Provider list rewrite) — 가장 복잡, 30분
4. 각 버그 수정 후 `cargo clippy` + `cargo test -p oxicode-cli` 확인

---

## Testing Plan

- Bug 1: oxicode 실행 → Idle 상태에서 `── ○ Idle ──` 표시 확인, Working 상태에서 스피너 표시 확인
- Bug 2: `/provider` 실행 → provider 목록에서 방향키로 끝까지 스크롤, 하단 위치 표시 확인
- Bug 3: `/model` 실행 → 모델 목록 스크롤 + 위치 표시 확인
- 작은 터미널 창(40행)에서도 스크롤 동작 확인
