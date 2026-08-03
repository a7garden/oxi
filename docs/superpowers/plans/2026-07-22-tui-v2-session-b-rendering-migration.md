# Session B: oxicode-cli 렌더링 마이그레이션 (전체)

> **독립 세션용 문서.** 이 문서만 읽고 작업할 수 있도록 작성됨.
> **수정 파일**: `oxicode-cli/src/tui/`, `oxicode-cli/Cargo.toml`, workspace `Cargo.toml`
> **금지 파일**: `oxicode-tui/src/` (Session A가 작업 중), `oxicode-tui/tests/`, `oxicode-tui/benches/`

## 전제 상태

브랜치 `oxicode-tui-v2-plan-a` (44+ commits).

### 현재 아키텍처

```
oxicode-cli main loop
    ↓
draw_frame_closure(term, cursor, focus, theme, caps, |ctx| {
    if OXICODE_V2_RENDER=1 {
        v2_render::draw_v2(ctx, state)   ← 새 ChatView 렌더링 (채팅 영역만)
    } else {
        ctx.with_frame(|frame| render::draw(frame, state, theme))  ← legacy 전체
    }
})
```

`v2_render::draw_v2`는 ChatView(채팅 영역)만 렌더링하고, 하단 4줄(footer+input)은 Clear만 함. overlay는 렌더링 안 함.

### v2 라이브러리에서 사용 가능한 위젯

```rust
oxicode_tui::widget::chat::ChatView         // 채팅 (이미 v2_render에서 사용 중)
oxicode_tui::widget::panel::Footer          // 상태 바 (model, tokens, cost, spinner)
oxicode_tui::widget::panel::Sticky          // 상단/하단 고정 패널
oxicode_tui::widget::panel::Overlay         // 모달 컨테이너
oxicode_tui::input::InputArea               // 텍스트 입력 (stock ratatui-textarea)
oxicode_tui::widget::primitive::{Border, List, Scrollbar, Text}
```

### AppState의 v2 필드 (이미 존재)

```rust
// oxicode-cli/src/tui/app.rs
pub v2_chat: oxicode_tui::content::ChatLog,              // dual-write 대상
pub v2_chat_view: oxicode_tui::widget::chat::ChatView,   // ChatView 위젯
pub cursor_state: oxicode_tui::pipeline::CursorState,    // 커서 dedup 상태
```

---

## 작업 1: Footer v2 마이그레이션 (~2시간)

### 목표

`v2_render::draw_v2`의 하단 영역에 legacy footer 대신 새 `Footer` 위젯 렌더링.

### 파일

- `oxicode-cli/src/tui/app.rs` — `v2_footer: oxicode_tui::widget::panel::Footer` 필드 추가
- `oxicode-cli/src/tui/v2_render.rs` — Footer 렌더링 추가
- `oxicode-cli/src/tui/handlers.rs` — agent 이벤트에서 Footer 데이터 동기화

### 구현

**app.rs**: 필드 추가
```rust
pub v2_footer: oxicode_tui::widget::panel::Footer,
// 초기화: v2_footer: Footer::new(),
```

**handlers.rs**: 이벤트에서 Footer 업데이트
```rust
// MessageStart 시:
state.v2_footer.set_model(&model_name);
// TokenUpdate 시:
state.v2_footer.set_tokens(tokens_in, tokens_out);
state.v2_footer.set_cost(cost);
state.v2_footer.advance_spinner();
```

**v2_render.rs**: draw_v2에 Footer 렌더링 추가
```rust
pub fn draw_v2(ctx: &mut RenderCtx, state: &mut AppState) {
    let area = ctx.area();
    let chat_height = area.height.saturating_sub(4);
    let chat_area = Rect { x: area.x, y: area.y, width: area.width, height: chat_height };
    let footer_area = Rect { x: area.x, y: area.y + chat_height, width: area.width, height: 1 };
    let input_area = Rect { x: area.x, y: area.y + chat_height + 1, width: area.width, height: 3 };

    // 1. ChatView (기존)
    sync_chat_view(&mut state.v2_chat_view, &state.v2_chat);
    state.v2_chat_view.render(chat_area, ctx);

    // 2. Footer (새로 추가)
    state.v2_footer.render(footer_area, ctx);

    // 3. Input (아직 legacy — with_frame로 위임)
    ctx.with_frame(|frame| {
        // legacy input rendering
        // 기존 render::draw의 input 부분만 호출하거나 직접 textarea.render
    });
}
```

### 주의

- Footer 데이터(model, tokens, cost)는 legacy AppState 필드에서 읽어와야 함
- Footer::render는 `&mut self` → state.v2_footer.render() 호출 가능
- with_frame 내부에서 state를 빌리면 안 됨 (borrow 충돌) → input 데이터를 미리 복사

---

## 작업 2: Input v2 마이그레이션 (~3시간) ★ 커서 브로커 포함

### 목표

입력 영역을 새 `InputArea` 위젯으로 렌더링. **★ 핵심**: `ctx.set_cursor(pos)`를 호출하여 커서가 화면에 보이도록 해야 함.

### 파일

- `oxicode-cli/src/tui/app.rs` — `v2_input: oxicode_tui::input::InputArea` 필드 추가
- `oxicode-cli/src/tui/v2_render.rs` — InputArea 렌더링 + 커서 브로커

### 구현

**app.rs**:
```rust
pub v2_input: oxicode_tui::input::InputArea,
// 초기화: v2_input: InputArea::new(),
```

**v2_render.rs**:
```rust
// 작업 1의 input_area 부분을 교체:
// 3. Input (v2)
state.v2_input.set_text(&state.input.text());  // legacy input에서 텍스트 동기화
state.v2_input.render(input_area, ctx);
// ★ InputArea::render가 ctx.set_cursor(pos)를 호출해야 함
// 커서 위치 = input_area 시작 + textarea 내부 커서 오프셋
```

### ★ 커서 브로커 (가장 중요)

현재 `draw_frame_closure`는 `ctx.take_cursor_slot()`로 커서를 읽음. 하지만 legacy render는 `frame.set_cursor_position()`을 호출하지 않으므로 커서가 안 보임.

InputArea::render가 `ctx.set_cursor(Position { x, y })`를 호출하면:
1. `ctx.cursor = CursorSlot::Show(pos)` 설정
2. `draw_frame_closure`가 `ctx.take_cursor_slot().resolve(None)` → `Some(pos)` 반환
3. `CursorState::reconcile(Some(pos), term)` → `show_cursor + set_cursor_position` emit
4. ★ 커서가 화면에 보임 + 깜빡임 보존 (같은 위치면 0 bytes)

InputArea의 render 구현에서 (oxicode-tui/src/input/textarea.rs):
```rust
fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
    // ... textarea를 buffer에 그림 ...
    
    // ★ 커서 위치 계산: area 시작점 + textarea 내부 커서 (row, col)
    let (row, col) = self.textarea.cursor();
    let cursor_x = area.x + col as u16;
    let cursor_y = area.y + row as u16;
    ctx.set_cursor(Position { x: cursor_x, y: cursor_y });
}
```

이것이 `OXICODE_V2_RENDER=1`일 때 커서가 보이게 만드는 핵심 수정.

---

## 작업 3: Overlay LegacyOverlayAdapter 마이그레이션 (~각 30분, 18개)

### 목표

각 overlay(settings, mcp_config, model_select 등)를 `LegacyOverlayAdapter`로 감싸서 v2_render 경로에서도 overlay가 표시되도록 함.

### 파일

- `oxicode-cli/src/tui/v2_render.rs` — overlay 렌더링 추가

### 현재 상태

`LegacyOverlayAdapter`는 이미 `oxicode-cli/src/tui/v2_overlay_adapter.rs`에 구현되어 있음. `Renderable`을 구현하며, 내부적으로 `ctx.with_frame(|frame| overlay.render(frame, area, theme))`을 호출.

### 구현

**v2_render.rs**: draw_v2 끝에 overlay 렌더링 추가
```rust
pub fn draw_v2(ctx: &mut RenderCtx, state: &mut AppState) {
    // ... ChatView + Footer + Input 렌더링 ...

    // 4. Overlay (활성 overlay가 있으면)
    if let Some(overlay) = state.active_overlay.take() {
        let mut adapter = LegacyOverlayAdapter::new(overlay);
        adapter.render(ctx.area(), ctx);
        state.active_overlay = Some(adapter.into_overlay());
    }
}
```

### 주의

- overlay의 theme 타입이 legacy `oxicode_tui_legacy::Theme` → v2 `oxicode_tui::theme::Theme` 변환 필요
- `LegacyOverlayAdapter::content_hash`가 매 프레임 다른 값을 반환하므로 항상 재렌더링됨 (overlay는 volatile하므로 OK)
- 18개 overlay 각각에 대해 렌더링이 정상 동작하는지 `OXICODE_V2_RENDER=1` 상태에서 시각 확인 필요

---

## 작업 4: v2_render 기본 활성화 (~30분)

### 전제 조건: 작업 1-3 완료 + 시각 테스트 통과

### 목표

`OXICODE_V2_RENDER` 환경변수 체크를 제거하고 v2를 기본 렌더링 경로로 설정.

### 파일

- `oxicode-cli/src/tui/app.rs` — 환경변수 체크 제거

### 구현

```rust
// BEFORE:
let use_v2_render = std::env::var("OXICODE_V2_RENDER").as_deref() == Ok("1");
|ctx| {
    if use_v2_render { v2_render::draw_v2(ctx, state) }
    else { ctx.with_frame(|f| render::draw(f, state, &theme)) }
}

// AFTER:
|ctx| {
    v2_render::draw_v2(ctx, state)
}
```

### 주의

- 작업 1-3이 모두 완료되지 않으면 footer/input/overlay가 안 보임
- `cargo run --bin oxicode`로 직접 실행하여 시각 확인 필수
- 문제 발생 시 `OXICODE_V2_RENDER=0` 환경변수로 legacy 폴백 유지 (안전망)

---

## 작업 5: oxicode-tui-legacy 제거 (Plan D)

### 전제 조건: 작업 4 완료 (v2 기본 활성화)

### 목표

workspace에서 `oxicode-tui-legacy` 크레이트 제거.

### 파일

- workspace `Cargo.toml` — members에서 `oxicode-tui-legacy` 제거
- `oxicode-cli/Cargo.toml` — `oxicode-tui-legacy` 의존성 제거
- `oxicode-cli/src/` — 모든 `oxicode_tui_legacy::` 참조 제거 또는 `oxicode_tui::`로 교체
- `oxicode-tui-legacy/` 디렉토리 — 삭제

### 순서

1. `grep -rn 'oxicode_tui_legacy' --include='*.rs' oxicode-cli/src/ | wc -l` — 남은 참조 카운트
2. 각 참조를 `oxicode_tui::` 또는 로컬 타입으로 교체
3. `oxicode-cli/Cargo.toml`에서 legacy 의존성 제거
4. workspace `Cargo.toml`에서 members 제거
5. `rm -rf oxicode-tui-legacy/`
6. `cargo check --workspace` 통과 확인
7. `cargo nextest run --workspace` 통과 확인

### 주의

- 이 작업은 가장 파괴적 — 신중하게 진행
- 각 파일을 하나씩 마이그레이션하면서 `cargo check -p oxicode-cli`로 확인
- legacy 타입(`oxicode_tui_legacy::Theme`, `oxicode_tui_legacy::widgets::*`)을 v2 타입으로 교체해야 함
- 일부 legacy 타입에 대응하는 v2 타입이 없을 수 있음 → 필요시 v2에 추가하거나 oxicode-cli 내부에 로컬 타입 정의

---

## 진행 순서 (권장)

```
작업 1 (Footer) → 작업 2 (Input + 커서) → 작업 3 (Overlay) → 작업 4 (기본 활성화) → 작업 5 (legacy 제거)
```

각 작업 후 `cargo run --bin oxicode`로 시각 확인. 문제 시 `OXICODE_V2_RENDER=0`으로 폴백.

## 체크리스트

- [ ] 작업 1: Footer v2 마이그레이션
- [ ] 작업 2: Input v2 마이그레이션 + ★ 커서 브로커
- [ ] 작업 3: Overlay LegacyOverlayAdapter (18개)
- [ ] 작업 4: v2_render 기본 활성화
- [ ] 작업 5: oxicode-tui-legacy 제거 (Plan D)
- [ ] 최종: `cargo nextest run --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --all -- --check`
