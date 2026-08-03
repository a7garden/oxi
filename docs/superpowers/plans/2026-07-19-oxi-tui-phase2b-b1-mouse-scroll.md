# Phase 2b B1 — Mouse scroll normalization state machine

**날짜**: 2026-07-19
**상태**: v0.57.1
**스펙**: `docs/superpowers/specs/2026-07-19-oxicode-tui-ux-stability-design.md` §B1
**선행 조건**: Phase 2 W1 완료 (가상 좌표, FollowMode, sticky)

---

## 배경

현재 `oxicode-cli/src/tui/handlers.rs:54-66`의 마우스 핸들러는 naive:

```rust
MouseEventKind::ScrollUp => state.scroll_up(3),
MouseEventKind::ScrollDown => state.scroll_down(3),
```

**증상**: 터미널마다 wheel notch당 발생하는 이벤트 수가 달라서 스크롤 속도가 비일관적:
- Apple Terminal: notch당 3 events
- iTerm2: 1 event
- Ghostty: 3 events
- VS Code: 1 event

grok `input/mouse.rs`(1,443 LOC)이 이미 해결한 패턴을 oxicode 크기로 축소 이식.

---

## 이식 범위 (grok 1,443 LOC → ~600 LOC)

1. **EPT 테이블** (events-per-tick): 5개 터미널 환경별 보정값. 환경변수 `OXICODE_SCROLL_EPT`로 override 가능.
2. **Multiplexer 감지**: tmux/screen/zellij는 EPT=1 강제 (SGR 마우스 모드 비활성).
3. **Stream 기반 gesture grouping**: 80ms 이내 또는 방향 동일 시 한 stream으로 묶음 → flush.
4. **Acceleration bands**: fast (<8ms) 2.5x, medium (<20ms) 1.6x, base 1.0x.
5. **Per-flush delta cap**: viewport 절반 또는 최소 6줄 (한 프레임 화면 통째 스크롤 방지).
6. **Wheel vs Trackpad 자동 감지**: median interval 기반 (trackpad는 연속 짧은 이벤트, wheel은 간헐).

---

## 작업 분할 — 4개 커밋

### Commit 1: `feat(oxicode-tui): mouse scroll normalizer core`

**신규 모듈** `oxicode-tui/src/widgets/chat/mouse.rs`:
- `pub struct ScrollNormalizer` — EPT + gesture state + history (last 16 events)
- `pub enum TerminalKind { Iterm2, AppleTerminal, Ghostty, VSCode, Unknown, Multiplexer }`
- `pub fn detect_terminal() -> TerminalKind` — `TERM_PROGRAM`, `TMUX`, `STY`, `ZELLIJ` env vars
- `pub fn ept_for(kind: TerminalKind) -> u8` — AppleTerminal/Ghostty=3, others=1
- `ScrollNormalizer::push(event) -> Option<NormalizedScroll>` — 내부에서 flush 결정
- `ScrollNormalizer::flush() -> Option<NormalizedScroll>` — 누적된 stream을 정제된 delta로 반환
- `pub struct NormalizedScroll { pub delta_lines: i32, pub direction: ScrollDirection }`
- 환경변수 override: `OXICODE_SCROLL_EPT` (1-10), `OXICODE_SCROLL_MULT=1.0`, `OXICODE_SCROLL_FLUSH_MS=80`

**테스트**:
- EPT 테이블 검증 (5 terminal kinds × expected EPT)
- detect_terminal: env vars 조합 → TerminalKind 매핑
- push() + flush() 시뮬레이션: 3 events in 30ms → 1 normalized scroll (AppleTerminal의 경우)
- 80ms 갭 초과 → 새 stream
- 방향 전환 → 새 stream
- trackpad 시뮬레이션: 16 events in 200ms (median interval 12ms) → trackpad 감지 → 부드러운 누적

**LOC**: ~400

---

### Commit 2: `feat(oxicode-tui): wheel/trackpad detection + acceleration bands`

**mouse.rs 확장**:
- `enum InputDevice { Wheel, Trackpad }`
- `ScrollNormalizer::detect_device() -> InputDevice` — median interval histogram
- `fn acceleration_band(median_ms: u64) -> f32` — fast (<8ms) 2.5x, medium (<20ms) 1.6x, base 1.0x
- `ScrollNormalizer::flush()` 수정: detected device + acceleration 적용
- `fn per_flush_cap(viewport_height: u16) -> u16` — min(viewport_height/2, 6)

**테스트**:
- trackpad: 16 events median=8ms → device=Trackpad, accel=2.5x, flush delta = ept * accel * events / 16
- wheel: 1 event / 200ms → device=Wheel, accel=1.0x
- per-flush cap: viewport_height=24, delta 계산 = 30 → capped to 12 (24/2)
- per-flush cap: viewport_height=10, delta = 50 → capped to 6 (min)

**LOC**: ~150

---

### Commit 3: `feat(oxicode-cli): wire scroll normalizer into mouse handler`

**변경**:
- `AppState` (in `oxicode-cli/src/tui/app.rs`) — `pub scroll_normalizer: ScrollNormalizer`
- `handlers.rs::handle_event`:
  - `ScrollUp`/`ScrollDown` → `state.scroll_normalizer.push(...)` → `state.scroll_normalizer.flush()` 결과에 따라 `state.scroll_up/down(n)`
- `app.rs::new()`: 초기화 + `ScrollNormalizer::with_terminal(detect_terminal())`

**테스트**:
- handlers.rs integration test: 3 rapid ScrollUp events → ept=3 → 1 scroll_up(9) or scroll_up(3) depending on terminal
- tmux 환경 시뮬레이션: `TMUX` env → EPT=1 강제 → 3 events → 3 scroll_up(3) (각각 3줄)

**LOC**: ~150

---

### Commit 4: `docs(oxicode-tui): terminal_support pattern for EPT overrides`

**신규** `oxicode-tui/src/widgets/chat/terminal_support.rs`:
- 문서 + 헬퍼: `fn ept_with_override(kind: TerminalKind) -> u8`
- `OXICODE_SCROLL_EPT` 환경변수가 있으면 그것으로, 없으면 테이블 값
- `OXICODE_SCROLL_FLUSH_MS=80` (기본값), `OXICODE_SCROLL_ACCEL=2.5` 등도 문서화

**테스트**:
- override: `OXICODE_SCROLL_EPT=5` → EPT=5 반환 (table 무시)
- 0/음수/너무 큰 값 → 기본값 fallback

**LOC**: ~50

---

## 완료 기준

### 단위 테스트
- 4개 commit 합쳐서 신규 테스트 ≥ 20개
- 기존 388개 + 신규 = ≥ 408개 통과
- `cargo clippy -p oxicode-tui --all-targets -- -D warnings` clean
- `cargo clippy -p oxicode-cli --tests -- -D warnings` clean
- `cargo fmt --check` clean

### UX 증상
- AppleTerminal에서 wheel 3 events/notch → 다른 터미널(iTerm2)과 동일하게 느껴짐 (정규화됨)
- trackpad 사용 시 부드러운 가속 (2.5x)
- tmux 안에서 EPT=1 강제 → 예측 가능한 스크롤

### 환경변수 호환
- `OXICODE_SCROLL_EPT=5` env로 override 가능
- invalid value는 silently fallback (default table)

---

## 외부 의존성

없음. crossterm `MouseEvent`는 이미 사용 중.

---

## 롤백

- mouse.rs 신규 모듈 — 삭제만 하면 됨
- handlers.rs 3-line 변경 — `state.scroll_up(3)` 복귀
- app.rs 필드 추가 — `scroll_normalizer: ScrollNormalizer::default()` 제거

각 commit이 독립 revert 가능.

---

## 총 LOC 추정

400 + 150 + 150 + 50 = **~750 LOC** (grok 1,443 LOC를 ~52% 압축)
