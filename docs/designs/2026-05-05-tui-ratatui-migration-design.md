# TUI 아키텍처 리팩토링: oxi-tui → ratatui 컴포넌트 라이브러리

**저자:** won  
**날짜:** 2026-05-05  
**상태:** 초안

---

## 1. 배경

`oxi-tui`는 순수 Rust로 작성된 자체 TUI 프레임워크로, Surface/Cell/Renderer/Component 트레이트를 포함한다. 이 프레임워크는 학술적으로 견고하지만:

- **ratatui**가 이미 동일한 문제를 더 범용적으로 해결하고 있음
- `oxi-cli/tui_interactive.rs`가 ratatui를 직접 사용하면서 oxi-tui 컴포넌트를 무시하고 재구현했음
- Surface, Renderer, layout 모듈이 ratatui와 기능 중복
- 2개의 병렬 구현으로 유지보수 비용 2배

현재 oxi-tui 컴포넌트(`ChatView`, `Editor`, `Input`, `Footer`, `Markdown` 등)가 시각적으로 더 풍부한 반면 실제 TUI는 더 단순한 인라인 렌더링을 사용하고 있음.

---

## 2. 목표

```
oxi-tui의 도메인 컴포넌트를 ratatui Widget/StatefulWidget으로 재구현하여
단일 일관된 TUI 구현체를 만드는 것.
```

**원칙:**
- ratatui를 최대한 활용 (native integration)
- Widget 트레이트 구조는 ratatui 표준 따르기
- StatefulWidget 패턴으로 상태 관리
- ThemeManager를 ratatui Style 변환 레이어로 활용

---

## 3. 삭제 대상 (ratatui 대체)

| 모듈 | 이유 |
|---|---|
| `surface.rs` | `ratatui::buffer::Buffer`가 동일 역할 |
| `cell.rs` | `ratatui::buffer::Cell` 대체 |
| `renderer.rs` | `ratatui::backend::Backend` + `Terminal::draw` 대체 |
| `terminal.rs` | `ratatui::Terminal<CrosstermBackend>` 대체 |
| `layout.rs` | `ratatui::layout::Layout` 대체 |

---

## 4. 유지/재구현 대상

### 4.1 도메인 컴포넌트 (ratatui Widget으로 재구현)

```
oxi-tui/src/components/
├── chat_view.rs      → StatefulWidget: 메시지 목록, 스트리밍, 스크롤
├── editor.rs         → StatefulWidget: 멀티라인 에디터, undo/redo
├── input.rs         → StatefulWidget: 텍스트 입력 필드, 자동완성
├── footer.rs        → Widget: 상태바 (토큰/비용/브랜치/모델)
├── markdown.rs      → Widget: 마크다운 렌더링 (Lists/테이블 미지원 제외)
├── command_palette.rs → StatefulWidget: fuzzy 필터링 명령 팔레트
├── settings_overlay.rs → StatefulWidget: 설정 모달
├── model_selector_overlay.rs → StatefulWidget: 모델 선택
└── ...
```

### 4.2 지원 시스템

```
oxi-tui/src/
├── theme.rs              → 유지 + ratatui Style 변환 메서드 추가
├── event.rs              → 유지 (KeyCode, KeyModifiers 등)
├── keys.rs               → 유지
├── keybindings.rs        → 유지
├── autocomplete.rs       → 유지
├── fuzzy.rs              → 유지
├── undo_stack.rs         → 유지 (Editor에서 사용)
├── kill_ring.rs          → 유지
└── stdin_buffer.rs       → 유지
```

### 4.3 overlay.rs 재구현

기존 `OverlayHandle` / `OverlayContent` 트레이트를 ratatui 위에 재구현:
- 오버레이 스택 관리 (z-order)
- backdrop 렌더링
- Escape/click-outside 닫기

---

## 5. 아키텍처 설계

### 5.1 새 모듈 구조

```
oxi-tui/src/
├── lib.rs                    # 공개 API
├── widgets/                  # ratatui Widget/StatefulWidget
│   ├── mod.rs
│   ├── chat.rs              # ChatView widget
│   ├── editor.rs            # Editor widget
│   ├── input.rs             # Input widget
│   ├── footer.rs            # Footer widget
│   ├── markdown.rs          # Markdown widget
│   ├── command_palette.rs   # CommandPalette widget
│   ├── settings_overlay.rs  # SettingsOverlay widget
│   └── model_selector.rs    # ModelSelector widget
├── theme.rs                 # Theme + to_style() 변환
├── ratatui_integration.rs   # Theme → ratatui Style 변환 레이어
├── event.rs                 # (유지)
├── widgets/overlay.rs        # Overlay manager for ratatui
└── ...
```

### 5.2 Theme → ratatui Style 변환

```rust
// theme.rs 에 추가
impl Theme {
    /// Convert to ratatui Style
    pub fn to_style(&self) -> Style {
        Style::default()
            .fg(self.colors.foreground.to_ratatui())
            .bg(self.colors.background.to_ratatui())
    }
}

impl Color {
    pub fn to_ratatui(&self) -> ratatui::style::Color {
        match self {
            Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(*r, *g, *b),
            Color::Indexed(n) => ratatui::style::Color::Indexed(*n),
            // ...
        }
    }
}
```

### 5.3 StatefulWidget 패턴 (ChatView 예시)

```rust
// widgets/chat.rs
use ratatui::{
    widgets::StatefulWidget,
    frame::Frame,
    layout::Rect,
    style::Style,
};

#[derive(Default)]
pub struct ChatViewState {
    messages: Vec<ChatMessage>,
    streaming: Option<StreamingState>,
    scroll_offset: u16,
    focused_thinking: Option<(usize, usize)>,
}

pub struct ChatView<'a> {
    theme: &'a Theme,
}

impl StatefulWidget for ChatView<'_> {
    type State = ChatViewState;

    fn render(
        self,
        area: Rect,
        buf: &mut Buffer,
        state: &mut Self::State,
    ) {
        // 현재 chat_view.rs의 렌더링 로직을 ratatui Buffer에 그림
    }
}
```

### 5.4 Overlay Manager (ratatui 기반)

```rust
// widgets/overlay.rs
pub struct OverlayManager {
    stack: Vec<OverlayEntry>,
}

impl OverlayManager {
    pub fn push(&mut self, widget: Box<dyn StatefulWidget>) { ... }
    pub fn pop(&mut self) -> Option<Box<dyn StatefulWidget>> { ... }
    pub fn render(&self, f: &mut Frame) { ... }
}
```

---

## 6. 컴포넌트 상세 설계

### 6.1 ChatView (가장 복잡한 컴포넌트)

**현재 상태 (oxi-tui::ChatView):**
- `rendered_lines: Vec<RenderedMessage>` 캐시
- `reflow_if_needed()` 최적화 (streaming 시 마지막 메시지만 갱신)
- Collapsible thinking blocks
- Tool call/result 카드 렌더링

**리팩토링:**
```rust
pub struct ChatView<'a> {
    theme: &'a Theme,
}

#[derive(Default)]
pub struct ChatViewState {
    messages: Vec<ChatMessageDisplay>,
    streaming: Option<StreamingState>,
    scroll_offset: u16,
    // Rendering cache - built once per message update, not per frame
    cached_lines: Vec<RenderedLine>,
    content_height: u16,
    last_area_width: u16,
}
```

**주요 변경:**
- `RenderedMessage` / `RenderedLine` / `StyledCell` 대신 `ratatui::Buffer` 직접 채우기
- `reflow_if_needed` → 상태 변경 시마다 전체 재구성 (ratatui가 최적화)
- `paint()` → 직접 `buf.set()` 호출

### 6.2 Editor

**현재 상태:**
- `lines: Vec<Line>`, `current_line: usize`
- Undo stack (스냅샷 기반)
- 파일 경로 / @ 멘션 자동완성
- Ctrl+←/→ 단어 이동

**리팩토링:**
```rust
pub struct EditorState {
    lines: Vec<LineContent>,
    current_line: usize,
    cursor_col: usize,
    scroll_offset: usize,
    // Undo stack
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    // Completion
    completions: Vec<Completion>,
    completion_active: bool,
}

pub struct Editor<'a> {
    theme: &'a Theme,
    options: EditorOptions,
}
```

### 6.3 Input

**현재 상태:**
- `value: String`, `cursor_pos: usize`
- 파일/멘션 자동완성
- placeholder 렌더링

**리팩토링:**
```rust
pub struct InputState {
    text: String,
    cursor: usize,
    completions: Vec<Completion>,
    completion_index: usize,
    completion_active: bool,
}

pub struct Input<'a> {
    theme: &'a Theme,
    placeholder: Option<&'a str>,
}
```

### 6.4 Footer

**현재 상태:**
- `FooterData` 구조체 (Arc<AtomicU32> 토큰 카운터)
- 토큰/비용/브랜치/세션 표시

**리팩토링:**
```rust
pub struct FooterState {
    data: FooterData,  // 유지 (Arc<AtomicU32> 등)
}

pub struct Footer<'a> {
    theme: &'a Theme,
}

// render()에서:
// - model_name + provider
// - ↑↓ tokens with cache read/write
// - cost
// - @branch, context%, thinking level
// - session duration
```

---

## 7. tui_interactive.rs 마이그레이션

### 7.1 마이그레이션 전략

`oxi-cli/src/tui_interactive.rs`의 1858줄을 새 컴포넌트로 교체:

**Before:**
```rust
// tui_interactive.rs - 현재
fn render_chat(f: &mut Frame, area: Rect, ...) {
    // 모든 메시지를 Vec<Line>으로 수동 구성
    let mut all_lines: Vec<Line> = Vec::new();
    for msg in messages { ... }
    let widget = Paragraph::new(chat_text)...
    f.render_widget(widget, area);
}
```

**After:**
```rust
// tui_interactive.rs - 리팩토링 후
fn render_chat(f: &mut Frame, area: Rect, ...) {
    let mut state = ChatViewState { messages, streaming, ... };
    ChatView::new(theme).render(area, f.buffer_mut(), &mut state);
}
```

### 7.2 상태 통합

현재 `tui_interactive.rs`의 상태 관리:
- `messages: Vec<ChatMessage>`
- `input: InputState`
- `is_agent_busy`, `scroll_offset`, `spinner_frame` 등

→ 각 컴포넌트의 StatefulWidget State로 분리. `AppState` 구조체가 이들을 조합.

```rust
pub struct AppState {
    pub chat: ChatViewState,
    pub input: InputState,
    pub footer: FooterState,
    pub overlay: OverlayState,
    // Shared
    pub is_agent_busy: bool,
    pub spinner_frame: usize,
    pub auto_scroll: bool,
}
```

### 7.3 이벤트 루프 통합

```rust
// tui_interactive.rs - 리팩토링 후 이벤트 루프
loop {
    // 1. 채팅 상태 업데이트 (UI 이벤트 수신)
    while let Ok(event) = ui_rx.try_recv() {
        update_chat_state(&mut app.chat, event);
    }

    // 2. ratatui draw
    terminal.draw(|f| {
        ChatView::new(theme).render(chat_area, f.buffer_mut(), &mut app.chat);
        Input::new(theme).render(input_area, f.buffer_mut(), &mut app.input);
        Footer::new(theme).render(footer_area, f.buffer_mut(), &mut app.footer);

        // Overlay management
        for overlay in &mut app.overlays {
            overlay.render(f);
        }
    })?;

    // 3. 이벤트 poll
    if event::poll(poll_timeout)? {
        let event = event::read()?;
        let consumed = handle_input(&event, &mut app);
        if !consumed {
            // Pass to focused component
        }
    }
}
```

---

## 8. 테스트 전략

### 8.1 단위 테스트

기존 `#[cfg(test)]` 모듈 유지:
- `chat_view.rs` 테스트 → `ChatViewState`의 메서드 테스트
- `editor.rs` 테스트 → `EditorState` 로직 테스트
- Widget 렌더링 자체는 Integration test로

### 8.2 통합 테스트

```rust
#[test]
fn test_chat_view_renders_messages() {
    let mut state = ChatViewState::default();
    state.messages.push(ChatMessageDisplay {
        role: MessageRole::User,
        content_blocks: vec![Text { content: "Hello".into() }],
        timestamp: 0,
    });

    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
    ChatView::new(&Theme::dark())
        .render(Rect::new(0, 0, 80, 24), &mut buffer, &mut state);

    // Verify buffer has content
    assert!(buffer.has_style());
}
```

### 8.3 컴파일 후 하위 호환성

- 기존 `oxi-tui` 사용처가 있는가? (`oxi-cli`에서 직접 import하는지 확인 필요)
- → `oxi-tui/src/lib.rs`에서 re-export 유지

---

## 9. 구현 순서

```
Phase 1: Foundation
  [ ] theme.rs에 to_ratatui() 변환 메서드 추가
  [ ] event.rs, keys.rs, keybindings.rs 유지 확인
  [ ] Cargo.toml에서 ratatui 의존성 확인 + 업그레이드

Phase 2: Core Widgets
  [ ] widgets/chat.rs - ChatView StatefulWidget
  [ ] widgets/footer.rs - Footer Widget
  [ ] widgets/input.rs - Input StatefulWidget

Phase 3: Advanced Widgets
  [ ] widgets/editor.rs - Editor StatefulWidget
  [ ] widgets/markdown.rs - Markdown Widget
  [ ] widgets/command_palette.rs - CommandPalette StatefulWidget

Phase 4: Integration
  [ ] widgets/overlay.rs - Overlay Manager
  [ ] tui_interactive.rs 리팩토링 (inline render → widget 사용)
  [ ] 삭제: surface.rs, cell.rs, renderer.rs, terminal.rs, layout.rs

Phase 5: Polish
  [ ] 테스트 작성
  [ ] 문서화
  [ ] 벤치마크 (기존 대비 렌더링 성능 비교)
```

---

## 10. 예상 비용 / 이점

| 항목 | 비용 | 이점 |
|---|---|---|
| ratatui 의존성 추가 | 낮음 (이미 tui_interactive에서 사용 중) | - |
| 컴포넌트 재구현 | 중간 (ChatView 가장 복잡) | - |
| 삭제 작업 | 낮음 | 유지보수 비용 50% 감소 |
| UX 개선 기반 구축 | 높음 | ratatui 위에서 빠른 iteration |
| 렌더링 성능 | - | ratatui의 버퍼 관리 + dirty tracking 활용 |

---

## 11. 결정 사항 (미결)

- [ ] `ChatView`에서 streaming 중 dirty 유지 방식 유지? (현재 매 프레임 전체 리플로우)
- [ ] `Editor` undo 스냅샷 방식 유지? (대안: operational transform)
- [ ] overlay z-order 렌더링 순서:ratatui의 draw call 순서와 어떻게 맞출지

---

## 12. 참고

- ratatui StatefulWidget 문서: https://ratatui.rs/how-to/widgets/stateful-widget/
- ratatui Buffer API: https://ratatui.rs/api/buffer/struct.Buffer.html
- 현재 oxi-tui ChatView: `/Volumes/MERCURY/PROJECTS/oxi/oxi-tui/src/components/chat_view.rs`
- 현재 tui_interactive: `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/tui_interactive.rs`