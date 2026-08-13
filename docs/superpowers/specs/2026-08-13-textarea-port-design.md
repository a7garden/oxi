# 2026-08-13 — `xai-ratatui-textarea` Port Design

## 0. 배경

oxicode의 TUI 입력 필드는 ratatui `Paragraph` + 수동 cursor 계산으로 그려져 있어 다음
문제들이 있다:

- **CJK/emoji 입력 시 caret 어긋남** — `state.input_cursor as u16`로 byte offset을
  terminal column으로 잘못 계산. 한글 1글자(3 bytes) = 2 columns, emoji(4 bytes) = 2
  columns이라 caret이 입력 텍스트보다 점진적으로 오른쪽으로 밀려난다.
- **multi-line wrap 미지원** — `Block::default().borders(Borders::ALL)` 한 줄 박스에
  wrap이 켜져 있어도 caret은 항상 `area.top() + 1`. 2줄째 글자가 보여도 caret은
  1줄에 머문다.
- **horizontal scroll 없음** — long prompt에서 caret이 right border에 clamp되고 입력
  위치가 안 보인다.
- **selection 없음** — shift+arrow로 텍스트 선택이 불가능.
- **vim engine과 buffer mutation racing 위험** — `oxicode-vtui::vim::engine`이
  byte cursor 기반이고 `main_loop.rs`도 byte cursor를 직접 만진다. 키 라우팅이
  분산되어 undo/redo 같은 구조적 mutation history가 없다.

grok-build(`xai-org/grok-build`)의 `xai-ratatui-textarea` 12K LOC 풀 에디터는 이 모든
문제를 정공법으로 해결한다 — `EditCommand`/`EditPlan` 기반의 atomic mutation,
`TextElement`로 모델링된 atomic text (paste/file/image), `display_width_of_range()`
기반 정확한 caret, soft-wrap + `effective_scroll`, mouse hit-testing까지 포함.

## 1. 결정

**grok의 `xai-ratatui-textarea`를 통째로 fork해서 우리 workspace에 `oxicode-textarea`
crate로 들인다.** 라이선스는 Apache-2.0 (grok) → MIT (oxicode) 호환 (확인됨).
기존 호스트 이름 `xai-ratatui-textarea` → `oxicode_textarea`로 rename. vendoring이 아닌
독립 crate로 두어 `oxicode-cli`, `oxios` 등 다른 제품에서 재사용 가능하게 한다.

**vim은 grok의 textarea 내장 vim으로 단일화.** 기존 `oxicode-vtui::vim::engine`은
deprecated + 호출자 제거. 사용자 멘탈 모델 ("Normal/Insert 두 모드, 한 버퍼")은
유지되고, byte-cursor racing 문제는 한 mutation 모델로 통합되면서 사라진다.

**TextElement는 풀 도입** (Plain | Masked | FileRef | Image). secure input의
mask가 element로 자연스럽게 표현되고, 향후 image paste / file reference도 같은
data model 안에서 처리.

**작업은 단일 PR에 통합**하지만 PR 안에서 의존 방향으로 12 step을 순서대로 쌓아
중간마다 `cargo check` / `cargo nextest`가 컴파일 가능한 상태를 유지한다.

## 2. 목표 vs 비목표

### 2.1 목표 (in scope)

- `oxicode-textarea` crate 생성, `xai-ratatui-textarea` 코드 port (12K LOC).
- ratatui 0.28 → 0.30 API 차이 해소.
- `EditCommand`/`EditPlan` mutation 모델로 main_loop의 buffer mutation 통합.
- `cursor_pos_with_state(area, state) → Option<(u16, u16)>` API 노출.
- `screen_position_of(pos, area, state)` API 노출 (slash popup, secure input에서
  공용으로 사용).
- composer가 textarea 인스턴스를 보유, soft-wrap + horizontal scroll 동작.
- secure_input → textarea `MaskedTextElement` 한 개로 표현, caret positioning
  통합.
- vim engine → textarea 내장 vim으로 단일화. 기존 `vim::handle_key` 호출자 제거.
- mouse hit-testing은 기본만 (텍스트 drag → selection). 자동완성 popup 영역
  hit-test는 이번 PR 범위 밖.
- 기존 `composer_cursor_position` / secure input caret 코드는 `#[allow(dead_code)]`
  로 유지하여 textarea caret API가 노출될 때까지 컴파일 통과 + regression 안전망.
- 12 step 진행 중 매 step 끝마다 `cargo fmt --check` + `cargo clippy --workspace
  --all-targets -- -D warnings` + `cargo nextest run -p oxicode-cli` 통과.

### 2.2 비목표 (out of scope)

- image paste (Kitty/iTerm2 graphics protocol) — TextElement variant만 추가하고
  실제 렌더 경로는 별도 PR.
- 마우스 hover/click 자동완성 popup hit-test — 별도 PR.
- clipboard provider trait (image paste 등) — 별도 PR.
- ratatui 0.30 → 향후 0.31 마이그레이션 — 별도 PR.
- AGENTS.md의 "TUI only" 원칙에 따라 oxicode-tui(`ColorScheme`, glyph system)
  복구 — DEAD 상태 유지.
- 다른 제품(oxios 등)의 textarea 사용 — `oxicode-textarea` API는 노출되지만
  통합은 별도.

## 3. crate 레이아웃

```
oxicode-textarea/
├── Cargo.toml                workspace 멤버, edition 2024, MIT license
├── src/
│   ├── lib.rs                pub mod 5개 + re-export
│   ├── element.rs            TextElement enum, ElementRange, element_at_cursor
│   ├── command.rs            EditCommand, EditPlan, EditResult
│   ├── selection.rs          Selection, Anchor, Affinity
│   ├── wrap.rs               wrapped_lines, display_width_of_range, clip_*
│   ├── editor.rs             Editor state + EditPlan 적용
│   ├── editor_keys.rs        키 → EditCommand 매핑 (vim/normal/insert 모두)
│   ├── textarea.rs           Widget impl, cursor_pos_with_state,
│   │                          screen_position_of, screen_spans_of_range
│   └── tests/                textarea_tests.rs 포팅 (grok 원본)
└── LICENSE                   MIT (grok 원본 Apache-2.0 호환 확인)
```

**의존성 (grok Cargo.toml 그대로 + 우리 workspace 정렬)**:

```toml
[dependencies]
crossterm = { workspace = true, features = ["event-stream", "bracketed-paste"] }
ratatui = { workspace = true, features = ["crossterm", "unstable-widget-ref"] }
ratatui-core = { workspace = true }      # 0.30 분리 crate
textwrap = { workspace = true }          # 신규
tui-scrollbar = { workspace = true }     # 신규
unicode-segmentation = { workspace = true }
unicode-width = { workspace = true }

[dev-dependencies]
fuzzy-matcher = { workspace = true }
itertools = { workspace = true }
pretty_assertions = { workspace = true }
rand = { workspace = true }
```

`textwrap` + `tui-scrollbar`은 workspace member 추가 필요. 둘 다 가벼운 단일
crate라 PR에 추가.

## 4. architecture

### 4.1 data flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                            oxicode-cli                              │
│                                                                     │
│  ┌──────────────┐    owns    ┌─────────────────────────────────┐    │
│  │ RenderState  │◄──────────►│ composer: TextArea (insert mode)│    │
│  │              │            │ vim mode toggle → Normal/Insert │    │
│  │ input_buffer │  String    │                                 │    │
│  │ input_cursor │  usize     │ TextElement {                   │    │
│  │ vim_state    │            │   Plain("hello"),               │    │
│  └──────────────┘            │   FileRef("@main.rs:12"),       │    │
│                              │ }                               │    │
│                              └────┬────────────────────────────┘    │
│                                   │                                  │
│  ┌──────────────┐                 │ EditCommand                     │
│  │ secure_input │ owns    ┌────────▼───────────────────────────┐    │
│  │ OverlaySIn.. │◄───────►│ TextArea (mask = Masked elem)      │    │
│  └──────────────┘         │   TextElement::Masked("*****")     │    │
│                           └────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
                            ┌──────────────┐
                            │oxicode-textarea│
                            │  (this crate) │
                            └──────────────┘
                                    │
                                    ▼
                       ratatui 0.30 / unicode-width
```

### 4.2 mutation 단일화

**Before** (현재):
```rust
// main_loop.rs key 라우팅
match code {
    KeyCode::Char('h') if vim_normal => vim::handle_key(&mut vim_state, ...),
    KeyCode::Char('h') => {
        s.input_buffer.insert(cursor, 'h');   // 직접 mutation 1
        s.input_cursor += 1;                  // 직접 mutation 2
    }
    ...
}
```

**After**:
```rust
// main_loop.rs
let cmd = textarea.handle_key(key, mouse);    // vim/normal/insert 통합
match cmd {
    EditCommand::Insert(c) => editor.apply(EditPlan::single(cmd))?,
    EditCommand::MoveWordLeft => editor.apply(...)?,
    EditCommand::DeleteRange { start, end } => editor.apply(...)?,
    EditCommand::Undo => editor.undo(),
    EditCommand::Redo => editor.redo(),
    EditCommand::None => {}
}
```

`EditPlan`은 atomic — 실패 시 원상복구. selection이 있어도 일관됨.

### 4.3 vim 통합

grok의 textarea가 vim mode를 내부에 보유. 호스트(`oxicode-cli`)는 단순히
`textarea.vim_mode_enabled()` boolean만 토글하고, key 라우팅은 textarea에 위임.

기존 `oxicode-vtui::vim::engine`은 deprecated:
- `oxicode-vtui/src/vim/mod.rs` 상단에 `#[deprecated(note = "moved to
  oxicode-textarea")]` 표시.
- 호출자 (`main_loop.rs`, `slash/registry.rs`)는 textarea vim으로 전환.
- 6개월 후 deprecated 표기 제거 + crate 자체 삭제 (별도 PR).

### 4.4 caret API

grok의 `cursor_pos_with_state(area, state) → Option<(u16, u16)>`을 그대로 노출.
호스트 코드는:

```rust
let pos = textarea.cursor_pos_with_state(textarea_area, TextAreaState::default());
if let Some((x, y)) = pos {
    frame.set_cursor_position(Position::new(x, y));
}
```

기존 `composer_cursor_position` 함수는 `#[allow(dead_code)]`로 남겨두되, 새
코드에서는 호출하지 않음. textarea 통합 후 dead code 확정되면 별도 cleanup PR.

### 4.5 secure input 통합

secure prompt의 mask 처리:
- 현재: `*`를 `secure.value.chars().count()`만큼 반복. caret 별도 계산.
- 이후: `TextElement::Masked { visible_len, char }` element 하나로 표현.
  textarea가 자체 mask rendering + caret positioning 통합.
- `OverlaySecureInput.value: String` → `OverlaySecureInput.element:
  TextElement::Masked`로 type 변경.

## 5. PR 안의 12 step (의존 방향, 단일 머지)

각 step 끝마다 `cargo check -p oxicode-textarea` (1-7) 또는 `cargo nextest
run -p oxicode-cli --lib` (8-12) 통과.

| step | 파일/산출물 | 검증 |
|------|------------|------|
| 1. Cargo.toml + lib.rs skeleton | workspace 등록, `pub mod ... ;`만 | `cargo check -p oxicode-textarea` |
| 2. element.rs port | `TextElement`, `ElementRange`, `element_at_cursor` | unit test |
| 3. command.rs port | `EditCommand`, `EditPlan`, `EditResult` | unit test |
| 4. selection.rs port | `Selection`, `Anchor`, `Affinity` | unit test |
| 5. wrap.rs port | `wrapped_lines`, `display_width_of_range`, `clip_*` | unit test |
| 6. editor.rs port | `Editor` state + `EditPlan::apply` | unit test |
| 7. editor_keys.rs port | 키 → `EditCommand` 매핑 | unit test |
| 8. textarea.rs port | `Widget`, `cursor_pos_with_state`, `screen_position_of` | textarea_tests.rs |
| 9. ratatui 0.30 패치 | `Buffer::set_string` 등 API 갭 해소 | `cargo check` |
| 10. composer 통합 | `RenderState.composer: TextArea`로 교체, 기존 byte cursor 제거 | `cargo nextest -p oxicode-cli` |
| 11. secure_input 통합 | `OverlaySecureInput.element: TextElement::Masked` | secure_input_tests |
| 12. vim 단일화 | 기존 `vim::handle_key` 호출 제거 + textarea vim 토글 | clippy + PTY |

**회귀 방지**: step 10부터 main_loop.rs의 cursor mutation 코드를 textarea 호출로
바꾸면서 매 PR/commit에서 `cargo nextest run -p oxicode-cli`로
`render_tests` 9개 + secure_input_tests가 통과해야 다음 step 진행.

## 6. 위험 & 완화

| 위험 | 영향 | 완화 |
|------|------|------|
| ratatui 0.28 → 0.30 API 차이 누락 | 컴파일 실패 또는 runtime crash | step 9에서 grok의 textarea_tests.rs를 우리 workspace에 port, 100+ 테스트로 검증 |
| vim model 변경으로 사용자 멘탈 모델 깨짐 | Normal/Insert 모드 동작 변화 | 기존 keymap (i/a/o/ESC, h/j/k/l, w/b/e, 0/$)을 grok의 vim 그대로 유지. docs/TUI.md에 변경점 명시 |
| TextElement 도입으로 기존 input_buffer String API 사라짐 | 호출자 6곳 컴파일 실패 | step 10에서 일괄 변환, `text/plain` accessor로 String 추출 가능 |
| multi-line composer로 layout 영향 | transcript/shortcuts 위치 이동 | composer는 여전히 max-3 rows, 내부 soft-wrap만. layout 계산 변화 없음 |
| PR 너무 큼 (12K LOC) | 리뷰 부담 | PR description에 12 step 표기 + 각 step의 diff 요약. rust-review 스킬 적용 |
| 기존 composer_cursor_position dead_code clippy 경고 | clippy -D warnings 실패 | step 10 이후 `#[allow(dead_code)]` 유지, dead 확정 후 별도 cleanup PR |
| vendoring으로 인한 upstream drift | grok 보안 패치 놓침 | LICENSE에 `derived from xai-org/grok-build @ <commit>` 명시. 별도 `scripts/sync-textarea.sh` (옵션) |

## 7. 테스트 전략

### 7.1 oxicode-textarea 자체

grok의 `textarea_tests.rs` (~1100 LOC)를 그대로 port + 우리 ratatui 0.30에 맞게
조금 수정. 핵심 검증:

- `display_width_of_range` — ASCII/CJK/emoji/ZWJ 정확성
- `wrapped_lines` — soft-wrap boundary, trailing space 처리
- `cursor_pos_with_state` — wrap boundary, horizontal scroll 시 caret 위치
- `screen_position_of` — 임의 byte offset의 screen 좌표
- `EditPlan::apply` — selection 유지, undo/redo roundtrip

### 7.2 oxicode-cli 통합

기존에 추가한 9개 cursor 테스트 + secure_input_tests + 새 textarea 통합 테스트:

- composer가 textarea 인스턴스를 보유하고 매 프레임 cursor 위치를 textarea API에서 가져오는지
- secure prompt open/close 사이클이 textarea element로 일관되는지
- slash popup이 textarea cursor 위치 기반으로 anchor 좌표 잡는지
- vim normal mode (`Esc` 진입) → insert (`i`/`a`/`o`) 전환 시 caret 모양 변화
- long prompt 입력 시 horizontal scroll 동작
- multi-line paste 시 wrap boundary 정확

### 7.3 PTY

기존 `tests/pty_e2e.rs` + `tests/sessions_resume.rs` + 새 `tests/textarea_pty.rs`:

- vim `dd` 한 줄 삭제 + `p` paste roundtrip
- shift+arrow로 selection + ctrl+c 복사
- soft-wrap 발생 시 caret이 다음 줄로 이동
- Korean prompt 입력 → caret이 텍스트와 정렬

## 8. 마이그레이션 노트

### 8.1 호출자 변경 목록

| 호출자 | Before | After |
|--------|--------|-------|
| `main_loop.rs` 키 라우팅 (~50 곳) | `s.input_buffer.insert(cursor, ch)` 직접 | `composer.apply(EditPlan::insert_char(ch, cursor))` |
| `main_loop.rs` cursor move (~8 곳) | `s.input_cursor += 1` | `composer.apply(EditPlan::move_right())` |
| `main_loop.rs` render_composer | `composer_cursor_position(area, state, prefix_columns)` | `composer.cursor_pos_with_state(area, state)` |
| `main_loop.rs` vim_state 호출 (~12 곳) | `vim::handle_key(&mut s.vim_state, ...)` | textarea vim mode 토글 + `composer.handle_key(...)` |
| `secure_input_tests` (~6 곳) | `secure.value = ...; secure.cursor = ...` | `secure.element = TextElement::Masked{...}` |
| `slash/registry.rs` | vim_normal 분기 | textarea가 vim mode 보유, 호출자는 toggle만 |

### 8.2 데이터 타입 변경

| 타입 | Before | After |
|------|--------|-------|
| `RenderState.input_buffer` | `String` | `oxicode_textarea::Editor` (TextArea 내부) |
| `RenderState.input_cursor` | `usize` | `Editor.cursor()` 메서드로 |
| `RenderState.vim_state` | `vim::State` | textarea vim mode bool + state |
| `OverlaySecureInput.value` | `String` | `OverlaySecureInput.element: TextElement::Masked` |
| `OverlaySecureInput.cursor` | `usize` | `element.cursor()` |

### 8.3 deprecated 처리

`oxicode-vtui::vim::engine`:
```rust
// oxicode-vtui/src/vim/mod.rs
#[deprecated(
    since = "0.75.0",
    note = "moved to oxicode-textarea; vim is now inside TextArea"
)]
pub mod engine;
```

6개월 후 (next minor release) deprecated 표기 제거 + module 삭제.

## 9. 일정 추정

| step | 시간 | 누적 |
|------|------|------|
| 1-4. data types | 0.5일 | 0.5 |
| 5-7. wrap/editor/keys | 1.5일 | 2.0 |
| 8. textarea.rs | 1.0일 | 3.0 |
| 9. ratatui 0.30 패치 | 0.5일 | 3.5 |
| 10. composer 통합 | 1.0일 | 4.5 |
| 11. secure_input | 0.5일 | 5.0 |
| 12. vim 단일화 + clippy + PTY | 1.0일 | 6.0 |
| rust-review + fix | 1.0일 | 7.0 |

**총 ~7일**. user가 "총력"이라 했으니 그 범위 내 진행.

## 10. 성공 기준

- [ ] `oxicode-textarea` crate가 workspace의 일부로 빌드/clippy/test 통과
- [ ] `cargo nextest run --workspace` 3374+ 테스트 모두 통과
- [ ] 기존 cursor 수정 PR의 9개 테스트 + 새 textarea 통합 테스트 모두 통과
- [ ] PTY e2e 테스트 (`test_pty_tui_renders_and_exits`) 통과
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 통과
- [ ] `cargo clippy -p oxicode-cli --features native-browser -- -D warnings` 통과
- [ ] Korean prompt + emoji + long prompt로 manual smoke 시 caret이 텍스트와 정렬
- [ ] vim mode `ddp`, `yw`, `>>` 동작
- [ ] shift+arrow selection + Ctrl+C copy 동작
- [ ] secure prompt mask 동작 (5글자 키 입력 → `*****` 표시, caret 정확)

## 11. 참고

- grok-build 분석: `docs/ref-porter/xai-org-grok-build-tui.md` 38행 (textarea 466
  LOC vs grok의 12K LOC 비교)
- 기존 cursor 수정: PR `4096a6ef` (작업 시작점)
- vtui 채택 결정: `docs/superpowers/design/2026-07-30-vtcode-ui-adoption.md`
- grok 클론: `/tmp/ref-porter/xai-org-grok-build`
  (depth 1, 2026-08-13 기준 latest commit)
