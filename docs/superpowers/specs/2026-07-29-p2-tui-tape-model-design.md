# P2 — TUI omp Tape Model Realignment — Design Spec

- **날짜**: 2026-07-29
- **상태**: 설계 (사용자 승인 방침 T1 기반, 상세 설계 완료)
- **상위 문서**: `specs/2026-07-27-omp-realignment-design.md` §3.2 원칙 4, Phase 2
- **omp 소스**: `/tmp/omp/packages/tui/src/tui.ts` (4273 lines), `components/`, `terminal.ts` (66KB), `keys.ts` (17KB)
- **대상 크레이트**: `oxi-tui-legacy/` (22.5K LOC → `oxi-tui`로 rename), 현 `oxi-tui/` v2 (9.8K LOC → 폐기)

---

## 1. 문제 정의

### 1.1 이중 크레이트 교착

oxi-cli은 두 TUI 크레이트에 동시 의존한다:

| 크레이트 | LOC | 역할 | 렌더 패러다임 |
|---|---|---|---|
| `oxi-tui-legacy` | 22,499 | 모든 위젯·테마·심볼·키바인딩·오버레이·머메이드·이미지·LaTeX | ratatui Frame + DiffBackend (alt screen, row-level diff, CSI 2026, DECCARA) |
| `oxi-tui` (v2) | 9,792 | draw_frame_closure 파이프라인, CursorState, RetainedTree, capability detect | ratatui Frame (closure-based, cell-level diff) |

실제 렌더 경로는 hybrid다: v2 `draw_frame_closure`이 프레임 라이프사이클을 소유하고, 그 안에서 legacy `render::draw`가 위젯을 그린다. 터미널 백엔드는 `Terminal<DiffBackend<io::Stdout>>` — **legacy DiffBackend**가 실제 출력을 담당.

### 1.2 v2가 제공하는 고유 기능 (legacy에 없는 것)

조사 결과, v2 파이프라인이 legacy DiffBackend 위에 추가하는 것은 **cursor dedup 단 하나**다:

| 기능 | legacy DiffBackend | v2 pipeline | 비고 |
|---|:---:|:---:|---|
| Row-level diff (u64 checksum) | ✅ | — | legacy가 담당 |
| CSI 2026 synchronized output | ✅ | — | legacy가 담당 (`mod.rs:345-478`) |
| DECCARA background fills | ✅ | — | legacy가 담당 (`mod.rs:468-473`) |
| Terminal capability detection | ✅ | ✅ (별도) | 중복 — legacy `render/terminal.rs`, v2 `theme/capability.rs` |
| Cursor dedup (position-based) | ❌ | ✅ | **v2 유일 고유 기능** |
| Hash-skip (RetainedTree) | ❌ | ✅ (closure path 미사용) | `draw_frame` path 전용, 현재 미사용 |

**결론**: v2는 cursor dedup + 향후 RetainedTree 마이그레이션을 위한 스캐폴드. 현재 `draw_frame_closure`은 항상 렌더 (hash-skip 불가). cursor dedup은 ~60줄 로직이며 legacy로 이식 가능.

### 1.3 omp tape 모델과의 근본적 차이

| 측면 | 현재 oxi (legacy + v2) | omp tape 모델 |
|---|---|---|
| 스크린 모드 | **Alt screen** (1049h) — 종료 시 전체 소실 | **Main screen** (native scrollback) — 종료 후에도 대화 이력 잔존 |
| 렌더링 단위 | ratatui `Frame` → `Buffer` → `Cell[][]` | `Component.render(width) → string[]` (raw ANSI lines) |
| Diff 전략 | Row/cell-level diff (전체 프레임 재계산) | 3-전략 차등: component memo → native scrollback commit → ED3 replay |
| 메모이제이션 | content_hash (v2 RetainedTree, 미사용) | 참조 identity (TS): unchanged → 같은 배열 참조 반환 |
| 완료된 메시지 | 매 프레임 재렌더 (alt screen 내) | **scrollback에 commit (불변)**, 재렌더 없음 |
| 스트리밍 | 전체 viewport repaint | mutable 접미사만 in-place repaint |
| 입력 | crossterm 기본 이벤트 | Kitty protocol + bracketed paste + SGR 1006 mouse + kill ring |

---

## 2. 타겟 아키텍처

### 2.1 비전: omp tape 모델의 Rust 구현

omp의 핵심 혁신은 **append-only native scrollback**이다:

1. 완료된 메시지는 터미널의 native scrollback에 commit — 불변, 재렌더 없음
2. 활성 스트리밍 메시지만 mutable 접미사로 in-place repaint
3. Component는 `render(width) → lines`로 순수 함수적 렌더링
4. Container는 unchanged child를 참조 비교로 skip

이것이 alt screen 패러다임과 근본적으로 다르다: alt screen에서는 매 프레임이 전체 viewport의 재계산이지만, tape 모델에서는 finalized content가 한 번 쓰이고 영원히 스크롤백에 남는다.

### 2.2 아키텍처 결정 (ADR)

#### ADR-1: 점진적 마이그레이션 — alt screen 유지하며 tape engine을 병렬 구축

**결정**: tape engine을 legacy와 병렬로 구축하고, 점진적으로 transcript를 전환한다.

**이유**: omp의 native scrollback은 터미널 엔진을 처음부터 다시 짓는 일이다 (4272 lines). 한 번에 전환하면 리스크가 너무 크다. alt screen 기반 DiffBackend를 유지하면서, 새 tape engine을 standalone으로 구축·테스트한 뒤 transcript를 점진 전환.

**트레이드오프**: 두 렌더 경로가 일시적 공존. 하지만 P2.1(v2 retirement)로 단일 DiffBackend 경로를 먼저 확보하면, tape engine은 그 위에 clean-room으로 구축 가능.

#### ADR-2: Component 모델 — content_hash 기반 메모이제이션

**결정**: `fn render(&self, width: u16) -> RenderResult` where `RenderResult { lines: Vec<Line>, hash: u64 }`. hash가 unchanged면 Container가 child를 skip.

**이유**: Rust는 TS의 참조 identity(GC 추적)를 사용할 수 없다. `content_hash`는 v2 RetainedTree가 이미 증명한 접근. omp의 `getRenderStablePrefixRows()` 고급 기능도 hash 접두사 비교로 번역 가능.

#### ADR-3: Line 타입 — 자체 도메인 타입, ratatui 비의존

**결정**: `Line`은 oxi-tui 자체 타입 (`Vec<Span>`, Span = `{ text: String, style: Style }`). ratatui `buffer::Cell`이나 `text::Line`에 의존하지 않는다.

**이유**: tape engine은 alt screen을 벗어나 main screen에 직접 write해야 한다. ratatui Buffer/Cell은 alt screen 프레임 렌더링에 최적화된 타입이다. Component가 `Vec<Line>`을 반환하면, tape engine이 이를 raw ANSI bytes로 직렬화하여 stdout에 write.

#### ADR-4: Overlays는 alt screen 유지

**결정**: 팝업/다이얼로그/설정 오버레이는 기존 ratatui Frame 렌더링을 유지. transcript 영역만 tape engine으로 전환.

**이유**: omp도 overlay에 alt screen을 사용한다 (resize 시 `enterResizeAltSequence`). overlay는 일시적(transient)이며 ratatui의 레이아웃 엔진이 유용. transcript는 append-only 영구 영역이므로 tape model이 적합.

#### ADR-5: v2 retirement 선행 (P2.1)

**결정**: P2.1로 v2를 즉시 폐기. cursor dedup ~60줄을 legacy로 이식하고, v2 crate를 삭제.

**이유**: v2는 omp 포팅이 아닌 grok-inspired clean-room 재작성. cursor dedup 외에 legacy가 없는 기능이 없음. 이중 크레이트가 모든 후속 작업의 복잡도를 증가시킴. P2.1은 ~300줄 변경으로 단일 크레이트를 확보하는 저비용 고가치 정리.

---

## 3. 단계 분해 (6개 독립 배포 가능 단계)

각 단계는 독립적으로 merge 가능하며, 이전 단계에 누적 의존.

### P2.1 — V2 Retirement + Rename (~300 lines, 1-2일)

**목표**: v2 crate 폐기, `oxi-tui-legacy` → `oxi-tui` rename, 단일 크레이트 확보.

**변경 범위**:
- legacy DiffBackend에 cursor dedup 추가 (~60 lines)
- oxi-cli에서 v2 import 전부 제거 (~50 lines)
- `v2_render.rs` (35 lines), `v2_bridge.rs` (57 lines), `v2_overlay_adapter.rs` (341 lines) 삭제
- `oxi-tui/` v2 crate 삭제, workspace에서 제거
- `oxi-tui-legacy/` → `oxi-tui/` rename (Cargo.toml, workspace, import paths)

**수락 기준**: `oxi-tui` 단일 크레이트. build + clippy + nextest green. 렌더링 시각적 동일.

### P2.2 — Native Scrollback Tape Engine (~3000 lines, 2-3주)

**목표**: omp `NativeScrollbackLiveRegion` + committed prefix + ED3 replay의 Rust 구현.

**핵심 설계**:

```
┌──────────────────────────────────────────────────┐
│                  TapeEngine                       │
│                                                   │
│  ┌─────────────────────────────────────────┐     │
│  │ Committed Prefix (불변, scrollback)      │     │
│  │ - finalized rows, terminal에 write 완료   │     │
│  │ - 재렌더 대상 아님                        │     │
│  └─────────────────────────────────────────┘     │
│  ┌─────────────────────────────────────────┐     │
│  │ Live Region (mutable, viewport)          │     │
│  │ - 활성 스트리밍 메시지                    │     │
│  │ - in-place repaint (differential)        │     │
│  │ - finalize 시 committed prefix로 승격    │     │
│  └─────────────────────────────────────────┘     │
│  ┌─────────────────────────────────────────┐     │
│  │ Sticky Region (input, footer, panels)    │     │
│  │ - viewport 하단 고정                     │     │
│  │ - 매 프레임 repaint (불변 영역 아님)      │     │
│  └─────────────────────────────────────────┘     │
│                                                   │
│  Output: raw ANSI bytes → stdout (CSI 2026 wrap) │
└──────────────────────────────────────────────────┘
```

**Rust 구조**:
- `TapeEngine` struct: committed_prefix (`Vec<String>`), live_window (`Vec<String>`), previous_frame (diff용)
- `Component` trait: `fn render(&self, width: u16) -> RenderResult`
- `NativeScrollbackLiveRegion` trait: `fn live_region_start(&self) -> Option<usize>`
- `commit()`: live region을 finalized → committed prefix로 승격
- `paint()`: differential write (committed + live + sticky)
- `replay()`: ED3 (CSI 3 J) erase + full repaint (resize/session-replace)
- `ViewportTailProvider` trait: resize fast-path (하단 N줄만 렌더)

**alt screen 전략**:
- Main screen 모드 (1049h 미사용) — omp 정렬
- Resize 시 임시 alt screen 진입 → geometry rebuild → alt screen 종료 (omp `enterResizeAltSequence`)
- Overlay는 별도 alt screen frame으로 composite

**테스트**: TestBackend 없이 raw byte 비교. `TapeEngine`에 frame을 feed하고 stdout bytes를 snapshot 테스트.

### P2.3 — Component Model + Transcript Migration (~3000 lines, 2-3주)

**목표**: 채팅 transcript를 ratatui Frame 렌더링에서 Component `string[]` 렌더링으로 전환.

**변경 범위**:
- `Component` trait 구현체: UserMessage, AssistantMessage, ToolCallBlock, ThinkingBlock
- 각 Component의 `render(width)` — markdown, code, diff를 `Vec<Line>`으로 출력
- `ChatContainer`: children 관리, content_hash memoization
- oxi-cli `render.rs`의 transcript 부분을 tape engine 경로로 전환
- Legacy ratatui Frame 렌더링은 overlay 전용으로 잔존

**핵심 위젯 마이그레이션**:
- `widgets/chat/markdown.rs` → `Component` 기반 markdown renderer (pulldown-cmark 유지)
- `widgets/chat/render.rs` → tape engine 통합
- `widgets/tool_renderer.rs` → tool call block Component

### P2.4 — Input System (~2000 lines, 1-2주)

**목표**: omp 수준의 입력 처리.

| 항목 | omp 소스 | 현재 oxi | 목표 |
|---|---|---|---|
| Kitty keyboard protocol | `keys.ts` (17KB) | ❌ | crossterm 확장 + custom CSI parsing |
| Bracketed paste | `bracketed-paste.ts` (5KB) | ❌ | crossterm `EnableBracketedPaste` |
| Keybinding system | `keybindings.ts` (10KB) | ✅ (legacy) | 강화: conflict resolution, per-mode |
| Mouse SGR 1006 | `mouse.ts` (4KB) | 부분 (crossterm 기본) | SGR 1006 명시적 |
| Kill ring / undo | editor (123KB) | ❌ | input state에 kill-ring 추가 |
| stdin buffer | `stdin-buffer.ts` (28KB) | crossterm poll | omp 수준 partial-read 처리 |

### P2.5 — Rich Content (~1500 lines, 1-2주)

| 항목 | omp 소스 | 현재 legacy | 작업 |
|---|---|---|---|
| LaTeX inline | `latex-to-unicode.ts` (53KB) | `render/latex.rs` | omp 수준 보강 |
| LaTeX block | `latex-block.ts` (44KB) | ❌ | ANSI art block renderer |
| Mermaid | — | `render/mermaid.rs` (85KB) ✅ | tape engine Line 타입으로 이관 |
| Image (Kitty/iTerm2) | `kitty-graphics.ts` (9KB) + `image.ts` (17KB) | `render/image.rs` ✅ | tape engine 통합 |
| Markdown | `markdown.ts` (117KB) | `widgets/chat/markdown.rs` | OSC 66 headings, LaTeX 통합 |
| Autocomplete | `autocomplete.ts` (38KB) | `completion/` | fuzzy 강화 |

### P2.6 — Theme/Glyph Unification (~500 lines, 3-5일)

- 단일 `ColorScheme` (legacy 26 slots 기준, v2 28 slots 흡수)
- 단일 `Symbols` / `GlyphSet` (legacy `symbols.rs` 기준)
- Theme file format (TOML) 단일화
- `THEME_NAMES`, `ThemeRegistry` 단일 소스

---

## 4. P2.1 상세 설계 (즉시 실행)

### 4.1 Cursor Dedup 이식

v2 `CursorState`는 커서 위치/가시성을 추적하고, 동일 위치면 cursor move escape를 생략한다.

**이식 위치**: `oxi-tui-legacy/src/render/mod.rs`의 `DiffBackend` 또는 별도 `cursor.rs` 모듈.

**설계**:
```rust
/// Cursor dedup state — tracks last cursor position/visibility to
/// avoid redundant cursor move escape sequences.
pub struct CursorState {
    last_pos: Option<(u16, u16)>,
    last_visible: bool,
}

impl CursorState {
    pub fn reconcile(&mut self, want: Option<Position>, term: &mut Terminal<DiffBackend<W>>) -> Result<()>;
}
```

`DiffBackend::draw()` 종료 후 호출. want cursor position이 last와 동일하면 skip.

### 4.2 v2 Import 제거 맵

| 파일 | v2 사용 | 대체 |
|---|---|---|
| `tui/app.rs:44-45` | `V2CursorState`, `V2Theme`, `TerminalCaps` | legacy `CursorState`, legacy `Theme`, legacy `TerminalCapabilities` |
| `tui/app.rs:53` | `v2_theme_from_legacy()` | 삭제 (legacy Theme 직접 사용) |
| `tui/app.rs:297,303` | `v2_chat: ChatLog`, `v2_chat_view: ChatView` | 삭제 (dead code) |
| `tui/app.rs:409` | `cursor_state: V2CursorState` | legacy `CursorState` |
| `tui/app.rs:515-516` | `v2_chat`, `v2_chat_view` init | 삭제 |
| `tui/app.rs:564` | `cursor_state: V2CursorState::new()` | legacy init |
| `tui/app.rs:1607-1646` | `draw_frame_closure` | `terminal.draw()` + `cursor_state.reconcile()` |
| `tui/handlers.rs:14` | `V2MessageRole` | legacy `MessageRole` |
| `tui/v2_render.rs` | 전체 | 삭제 |
| `tui/v2_bridge.rs` | 전체 | 삭제 |
| `tui/v2_overlay_adapter.rs` | 전체 | 삭제 |

### 4.3 렌더 경로 전환

**Before** (v2 closure):
```rust
let result = oxi_tui::pipeline::draw_frame_closure(
    &mut tui.terminal,
    &mut cursor_state,
    FocusTarget::None,
    &v2_theme,
    &caps,
    |ctx| {
        ctx.with_frame(|frame| render::draw(frame, &mut state, &theme));
        if let Some(p) = state.last_input_cursor { ctx.set_cursor(p); }
    },
);
```

// CRITICAL: read last_input_cursor AFTER draw (render_input_area sets it during render).
let want_cursor = {
    state.last_input_cursor = None;
    tui.terminal.draw(|frame| {
        render::draw(frame, &mut state, &theme);
    })?;
    state.last_input_cursor
};
state.cursor_state.reconcile(want_cursor, &mut tui.terminal)?;

`DiffBackend::draw()`가 이미 CSI 2026 + DECCARA + row diff를 담당하므로, `terminal.draw()` 한 호출로 끝.

### 4.4 Rename 계획

1. `oxi-tui-legacy/` → `oxi-tui/` (디렉토리 rename)
2. `oxi-tui-legacy/Cargo.toml` `[package] name = "oxi-tui"` + version 유지
3. `Cargo.toml` workspace members에서 `oxi-tui-legacy` 제거 (이미 `oxi-tui` 존재 → 디렉토리 교체)
4. 모든 `oxi_tui_legacy::` → `oxi_tui::` import 치환
5. 모든 `use oxi_tui_legacy` → `use oxi_tui`
6. AGENTS.md, README 등 문서 업데이트
7. `.github/workflows/`에서 `oxi-tui-legacy` 참조 확인

**주의**: 현 `oxi-tui/` v2를 먼저 삭제한 후 rename해야 충돌 회피.

---

## 5. P2.2 Tape Engine 아키텍처 (핵심 혁신)

### 5.1 omp 3-전략 차등 렌더링

```
Frame paint sequence:
1. Component.render(width) → Vec<Line>      [component memo: hash 비교]
2. Container가 unchanged children skip        [전략 1: component memoization]
3. finalized rows → committed prefix          [전략 2: native scrollback commit]
4. live region → in-place differential repaint [전략 3: ED3 replay on divergence]
```

### 5.2 Rust TapeEngine 설계

```rust
/// Append-only tape rendering engine. Writes finalized rows to the
/// terminal's native scrollback (immutable), repaints only the live
/// suffix + sticky region.
pub struct TapeEngine {
    /// Finalized rows committed to scrollback. Never re-rendered.
    committed_prefix: Vec<RenderedLine>,
    /// Previous full frame for differential comparison.
    previous_frame: Vec<RenderedLine>,
    /// Terminal dimensions.
    width: u16,
    height: u16,
    /// Whether we're inside a resize alt-screen sequence.
    in_resize: bool,
    /// Hardware cursor state for dedup.
    cursor: CursorState,
    /// Output writer.
    out: W,
}

impl TapeEngine {
    /// Paint one frame from a composed Component tree.
    pub fn paint(&mut self, root: &dyn Component) -> Result<()>;

    /// Finalize the current live region — promote to committed prefix.
    pub fn commit(&mut self);

    /// Destructive replay — ED3 erase + full repaint (resize, session-replace).
    pub fn replay(&mut self, root: &dyn Component) -> Result<()>;
}
```

### 5.3 Component Trait

```rust
/// Rendering component — omp `Component` interface의 Rust 번역.
pub trait Component: Send {
    /// Render to lines at the given width. Returns content hash for memoization.
    fn render(&self, width: u16) -> RenderResult;

    /// Optional: the mutable suffix starts at this line index.
    /// Rows above are FINAL (byte-stable, commit to scrollback).
    fn live_region_start(&self) -> Option<usize> { None }

    /// Optional: invalidate cached state (theme change, etc.)
    fn invalidate(&mut self) {}

    /// Optional: rehydrate for destructive replay.
    fn prepare_replay(&mut self) {}
}

pub struct RenderResult {
    pub lines: Vec<RenderedLine>,
    pub hash: u64,
}
```

### 5.4 Terminal I/O 전략

omp는 stdout에 직접 raw bytes를 write. Rust에서도 동일:

```rust
// Main screen mode — NOT alt screen
write!(out, "{}", HIDE_CURSOR)?;
write!(out, "{}", SYNC_BEGIN)?;  // CSI 2026h
// ... write committed prefix (scroll naturally) ...
// ... write live region (in-place) ...
// ... write sticky region (bottom-fixed) ...
write!(out, "{}", SYNC_END)?;    // CSI 2026l
write!(out, "{}", show_cursor_if_needed)?;
```

**핵심 차이점** (alt screen vs main screen):
- Main screen에서는 `println!`이 자동 스크롤 → finalized content가 자연스럽게 scrollback에 진입
- Live region은 cursor positioning으로 in-place repaint
- Sticky region은 하단 고정 (매 프레임 repaint)

---

## 6. 리스크 분석

### 6.1 Native scrollback 호환성

**리스크**: 모든 터미널이 main screen 모드 TUI를 잘 지원하지 않을 수 있음.

**완화**: omp가 이미 이 모델로 작동하며, 주요 터미널(Ghostty, Kitty, iTerm2, Windows Terminal, tmux)에서 검증됨. ED3(CSI 3 J) 미지원 터미널은 duplication fallback (omp 패턴).

### 6.2 ratatui 의존도

**리스크**: transcript를 ratatui에서 분리하면, 기존 위젯 코드를 재작성해야 함.

**완화**: P2.3에서 점진적 전환. 먼저 `Component` trait + tape engine을 standalone으로 구축·테스트. 각 메시지 타입을 하나씩 전환. overlay는 ratatui 유지.

### 6.3 규모

**리스크**: P2 전체는 ~10,000 lines, 다-월간 작업.

**완화**: 6개 독립 배포 가능 단계로 분해. P2.1(정리) → P2.2(engine) → P2.3(transcript) 순으로 각 단계가 독립 가치 제공. P2.4-P2.6은 부가 기능이며 P2.3 이후 언제든 삽입 가능.

### 6.4 테스트 전략

- **P2.1**: 기존 nextest 전부 green + 시각적 동일 (수동 확인)
- **P2.2**: TapeEngine standalone 테스트 — raw byte snapshot, commit/replay/invariant 검증
- **P2.3**: Component 단위 테스트 — render(width) 출력 비교, hash 안정성
- **P2.4**: 입력 파서 단위 테스트 — Kitty sequence, paste marker, SGR mouse

---

## 7. 성공 기준 (P2 전체)

- [x] `oxi-tui` 단일 크레이트 (v2/legacy 이중 제거) — **P2.1**
- [ ] Native scrollback + append-only tape 동작 — **P2.2**
- [ ] Component 모델 기반 transcript 렌더링 — **P2.3**
- [ ] Kitty/bracketed-paste/keybinding/mouse/kill-ring — **P2.4**
- [ ] LaTeX/mermaid/image/autocomplete tape engine 통합 — **P2.5**
- [ ] Theme/glyph 시스템 단일화 — **P2.6**
- [ ] `cargo nextest run --workspace` + clippy green 전 단계

---

## 8. 실행 순서

```
P2.1 (V2 retirement + rename)
  ↓ [단일 크레이트 확보]
P2.2 (Tape engine standalone)
  ↓ [엔진 검증]
P2.3 (Component model + transcript migration)
  ↓ [tape model 활성화]
  ├── P2.4 (Input system)        [병렬 가능]
  ├── P2.5 (Rich content)        [병렬 가능]
  └── P2.6 (Theme/glyph)         [병렬 가능]
```

P2.1 → P2.2 → P2.3은 직렬 (각각 이전 단계의 산출물에 의존).
P2.4, P2.5, P2.6은 P2.3 완료 후 병렬 진입 가능.
