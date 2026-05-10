# TUI 정리 설계서 v1

평가에서 도출된 10개 이슈를 6개 작업 단위로 묶어 순차 해결.

---

## 작업 1: 데드 코드 제거 (markdown.rs, FontScheme)

### 문제
- `markdown.rs`의 `parse_inline()`, `Segment`, `LineType`이 완전히 사용되지 않음
- `FontScheme`, `Attributes`가 정의만 되고 아무 곳에서도 사용되지 않음

### 해결
1. `markdown.rs`에서 데드 코드 제거:
   - `parse_inline()`, `Segment` enum → 삭제
   - `LineType` enum → 삭제 (이미 `detect_line_type removed` 상태)
   - 스타일 헬퍼 함수들(`code_style`, `bold_style` 등) → chat.rs에서 직접 스타일 지정하므로 삭제
2. `cell.rs`에서 `Attributes` 제거
3. `theme.rs`에서 `FontScheme` 제거, `Theme.fonts` 필드 제거
4. `lib.rs`에서 `Attributes`, `FontScheme` export 제거

### 영향 파일
- `oxi-tui/src/widgets/markdown.rs` → 대폭 축소
- `oxi-tui/src/cell.rs` → Attributes 제거
- `oxi-tui/src/theme.rs` → FontScheme 제거
- `oxi-tui/src/lib.rs` → export 정리

---

## 작업 2: 이벤트 타입 통일 (oxi_tui::Event → crossterm 직접 사용)

### 문제
- `oxi-tui/event.rs`가 자체 `KeyCode`, `KeyEvent`, `Event`를 정의
- 실제 핸들러는 `crossterm::event::Event` 직접 사용
- `CommandPalette`만 `oxi_tui::Event` 사용 — 유일한 소비자
- 두 시스템이 병존하며 혼란 발생

### 해결
1. `CommandPalette::handle_key()` → `crossterm::event::KeyEvent` 직접 받도록 변경
2. `oxi-tui/event.rs` 모듈 삭제
3. `lib.rs`에서 Event 관련 export 제거
4. `oxi-tui/Cargo.toml`에 `crossterm` 의존성 추가 (이미 ratatui가 끌어오지만 명시적 선언)

### 설계 선택: crossterm 타입을 선택한 이유
- oxi-tui의 자체 이벤트는 "백엔드 독립"을 목표로 하지만, 실제로 crossterm만 사용
- 위젯 라이브러리가 백엔드 의존적이 되는 것은 ratatui 생태계에서 일반적
- termion 등 다른 백엔드 전환 가능성이 현재로선 0%

### 영향 파일
- `oxi-tui/src/event.rs` → 삭제
- `oxi-tui/src/widgets/command_palette.rs` → crossterm 타입 사용
- `oxi-tui/src/lib.rs` → export 정리
- `oxi-tui/Cargo.toml` → crossterm 추가

---

## 작업 3: Tool call 매칭 로직 분리 (ChatViewState 책임 감소)

### 문제
- `ChatViewState`가 `active_tool_calls: HashMap<String, usize>`를 관리
- `stream_tool_call()`, `stream_tool_result()`, `set_tool_status()`에 ID 기반 매칭 로직이 복잡하게 섞임
- "위젯 상태"에 비즈니스 로직이 과다

### 해결
1. `ToolCallTracker` 타입을 새로 만들어 ID 매칭 로직을 캡슐화:
```rust
pub(crate) struct ToolCallTracker {
    active: HashMap<String, usize>,
}

impl ToolCallTracker {
    pub fn register(&mut self, id: String, index: usize) -> bool { ... }
    pub fn find_and_remove(&mut self, id: &str) -> Option<usize> { ... }
    pub fn clear(&mut self) { ... }
}
```
2. `ChatViewState`는 `ToolCallTracker`에 위임만

### 영향 파일
- `oxi-tui/src/widgets/chat.rs` — 내부 리팩터링

---

## 작업 4: Line clone 최적화 (성능)

### 문제
- 매 프레임마다 `Vec<Line<'static>>`을 clone해서 Paragraph에 넘김
- 긴 마크다운 문서에서 수백 줄이 매 80ms마다 복사됨
- `measure_wrapped_height()`가 렌더링 전에 `Paragraph::line_count()`로 높이를 계산 → 렌더링과 동일 로직 이중 실행

### 해결
1. **라인 캐시**: 마크다운 텍스트가 변경되지 않으면 이전에 생성한 `Vec<Line<'static>>`를 재사용
2. **Paragraph 스킵 최적화**: visible lines만 건네는 대신, 전체 lines를 넘기고 clip rect로 제한
   - ratatui Paragraph는 내부적으로 visible lines만 렌더링하므로, skip/take 로직이 불필요
3. **높이 캐시**: 세그먼트 높이를 width와 함께 캐시, width가 변경되면 재계산

### 설계
```rust
struct CachedLines {
    source_hash: u64,     // 원본 텍스트 해시
    lines: Vec<Line<'static>>,
    /// (width, height) 캐시
    height_cache: Option<(u16, u16)>,
}
```

### 영향 파일
- `oxi-tui/src/widgets/chat.rs` — 캐시 메커니즘 추가

---

## 작업 5: Buffer 직접 조작 최소화

### 문제
- `input.rs`: prompt, cursor를 `buf[(x,y)].set_char()`로 직접 그림
- `command_palette.rs`: 선택 하이라이트를 위해 버퍼 전체를 순회하며 스타일 덮어씀

### 해결
1. **Input prompt**: `Paragraph`로 렌더링 후 cursor만 buf 직접 조작 (cursor는 불가피)
2. **CommandPalette 하이라이트**: 
   - 현재: 라인 렌더링 후 빈 셀을 순회하며 bg 색 칠함
   - 변경: 선택된 아이템을 전체 폭의 Span으로 구성하여 Paragraph가 bg fill 하도록
   - `\u{00a0}` (NBSP)로 나머지 폭을 채워 bg color가 자동 적용되게

### 영향 파일
- `oxi-tui/src/widgets/input.rs` — prompt 렌더링 변경
- `oxi-tui/src/widgets/command_palette.rs` — 하이라이트 로직 변경

---

## 작업 6: Setup Wizard overlay 중복 제거

### 문제
- `handle_setup_step_key()`와 `handle_provider_step_key()`가 `SetupStep` variant를 수동으로 매칭
- `AppOverlay::Setup` vs `AppOverlay::ProviderConfig` — 같은 `SetupStep`을 쓰면서 핸들러가 복제
- `render_setup_step()`은 두 overlay를 통합 처리하지만, 키 핸들러는 분리되어 있음

### 해결
1. `handle_setup_step_key()` → 제네릭 함수로 통합:
```rust
async fn handle_setup_key_inner(
    key: KeyEvent,
    state: &mut AppState,
    is_setup: bool,  // true=Setup, false=ProviderConfig
) -> Option<Action>
```
2. `handle_setup_step_key`와 `handle_provider_step_key`가 이 함수를 호출

### 영향 파일
- `oxi-cli/src/tui/handlers.rs` — 핸들러 통합

---

## 실행 순서

1. **작업 1** (데드 코드) → 가장 안전, 의존성 없음
2. **작업 2** (이벤트 통일) → CommandPalette만 영향
3. **작업 3** (Tool call 분리) → chat.rs 내부 리팩터링
4. **작업 4** (Line clone 최적화) → chat.rs 성능 개선
5. **작업 5** (Buffer 조작 최소화) → input.rs, command_palette.rs
6. **작업 6** (Overlay 중복) → handlers.rs 정리

각 작업 후 `cargo check` → `cargo test` 로 검증.
