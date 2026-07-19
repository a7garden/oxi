# Phase 2 W1 — 가상 스크롤 좌표계 + FollowMode + Sticky 헤더

**날짜**: 2026-07-19
**상태**: v0.57.0
**스펙**: `docs/superpowers/specs/2026-07-19-oxi-tui-ux-stability-design.md` §W1
**Phase 1 완료**: A4 layout cache 안정화 + color level + PTY harness
**선행 조건 충족**: PTY e2e harness (Phase 1 Task 6-7)에서 회귀 기반 확보 완료

---

## 배경

Phase 1 spec이 식별한 4개 "불안정" 증상이 단일 근본 원인을 공유:
- `LayoutEntry.y/height: u16` → 65K줄 cap
- `scroll_offset: u16`, `content_height: u16` → 동일 cap
- binary `auto_scroll: bool` → 스트리밍 race
- `clamp_scroll` 매 프레임 호출 → reflow 시 viewport 점프

**해법**: 가상 좌표 (u32) + draw-time u16 변환 + FollowMode 상태 머신 + sticky 헤더.

---

## 위험 + 안전 메커니즘

| 위험 | 완화 |
|---|---|
| 모든 render 경로 변경 | 1) **TDD-first** — 가상 좌표 적용 전 후 byte-equivalent snapshot 테스트 작성 2) **단계적 commit** — 5개 독립 commit, 매 단계 `cargo nextest run -p oxi-tui` 통과 3) **PTY 회귀** — Phase 1 harness가 scroll/flicker 검증 |
| `LayoutEntry.y/height` 타입 변경이 광범위하게 전파 | 4) **불변 타입은 u32 유지, 그려질 때만 u16 변환**. 모듈 간 인터페이스는 u32. |
| FollowMode 전이 12+ 케이스 누락 | 5) 상태 머신 단위 테스트로 모든 전이 매트릭스 커버 |
| sticky 헤더가 viewport 일부를 덮음 | 6) sticky 영역은 render 시 별도 레이어로 분리, layout entries 위에 overlay |
| `pub scroll_offset: u16` 등 외부 API (`oxi-cli/src/tui/app.rs`)가 u16 가정 | 7) 외부 API는 `u16` 헬퍼 메서드 (`scroll_up_n(n: u16)`) 유지, 내부 상태는 u32 |

---

## 작업 분할 — 5개 커밋

### Commit 1: `feat(oxi-tui): virtual coordinate u32 layer for chat scroll`

**변경**:
- `layout.rs::LayoutEntry { y: u16 → u32, height: u16 → u32 }`
- `state.rs::ChatViewState { scroll_offset: u16 → u32, content_height: u16 → u32 }`
- `LayoutCache { total_height: u16 → u32 }`
- `compute_layout` 내부 `y: u32` 로직 변경 없음 (이미 u32), push 시 `as u16` 캐스팅만 제거
- mod.rs render 루프: `viewport_base: u32` 개념 도입, draw-time `as u16` 변환
- `clamp_scroll(visible_height: u16)` → `clamp_viewport(visible_height: u16)` 시그니처 동일, 내부 `u32` 산술

**호환성**:
- `oxi-cli/src/tui/app.rs::scroll_up(n: u16)` / `scroll_down(n: u16)` / `ensure_auto_scroll(visible_height: u16)` 시그니처 **불변**. 내부에서 `self.chat.scroll_up(n as u32)` 변환.
- `state.scroll_offset` 외부 노출은 `as u16` getter (legacy 호환) — 단, 새 코드에서는 u32 직접 사용 권장

**테스트**:
- `test_virtual_coord_large_session`: 100K 메시지 layout → u32 누적 → `u16::MAX` 초과도 정상
- `test_layout_entry_y_u32_takes_arbitrary_value`
- 기존 `test_scroll_offset_invariants` 등은 u32에 맞게 assert만 변경

**LOC**: ~250

---

### Commit 2: `feat(oxi-tui): FollowMode state machine replaces auto_scroll bool`

**변경**:
- 신규 `state.rs::FollowMode` enum:
  ```rust
  pub enum FollowMode {
      Following,
      FollowingGrace { until: Instant },
      Pinned { anchor_msg_idx: usize, anchor_y_in_msg: u32 },
  }
  ```
- `ChatViewState { auto_scroll: bool → follow: FollowMode }`
- `scroll_to_bottom(vh)` → `follow_bottom(vh)`: `follow = Following`
- `scroll_up(n)` → `follow = FollowingGrace { now + 2s }`; grace 만료 → `Pinned { anchor: 현재 visible 첫 메시지 }`
- `scroll_down(n)`: bottom 닿으면 `Following` 복귀
- mod.rs render:
  - 새 content 추가 시:
    - `Following` / `FollowingGrace` → viewport_base = content_height - vh (점프 안 함, 그냥 따라감)
    - `Pinned` → viewport_base 불변 + `new_answer_badge: bool` 표시
- grace 만료 처리: `ensure_auto_scroll` 호출 시 `now >= until`이면 `Pinned` 전환 + anchor 계산
- anchor 계산: 현재 viewport_base에서 가장 가까운 message의 msg_idx + y_in_msg
- 외부 API: `App::ensure_auto_scroll(visible_height: u16)` 시그니처 **불변**

**테스트**:
- 12+ FollowMode 전이 케이스:
  1. `Following` + 새 content → viewport_base updates
  2. `Following` + `scroll_up` → `FollowingGrace { until = now+2s }`
  3. `FollowingGrace` + 새 content → viewport_base updates (grace 활성)
  4. `FollowingGrace` + 2s 경과 + 새 render → `Pinned { anchor = topmost_visible }`
  5. `Pinned` + 새 content → viewport_base **불변** + badge true
  6. `Pinned` + `scroll_down` to bottom → `Following`
  7. `Pinned` + 새 메시지 추가 (anchor 메시지는 stable) → Pinned 유지
  8. `Pinned` + anchor 메시지가 drain됨 → `Following`
  9. `FollowingGrace` + `scroll_down` to bottom → `Following`
  10. initial state → `Following`
  11. `scroll_to_top` → `Pinned { anchor = 0, 0 }`
  12. `scroll_to_bottom` → `Following`

**LOC**: ~400

---

### Commit 3: `feat(oxi-tui): draw-time u16 conversion in chat render loop`

**변경**:
- mod.rs::render:
  - `let viewport_base: u32 = state.viewport_base`
  - 각 entry 순회 시:
    - `let rel_y: u32 = entry.y.saturating_sub(viewport_base)`
    - viewport 밖 (`rel_y + entry.height > vh`) → continue
    - `let dst_y: u16 = (area.y as u32 + rel_y.min(u16::MAX as u32 - area.y as u32)) as u16`
  - `let vp_bottom_u32 = viewport_base.saturating_add(area.height as u32)` → `entry.y >= vp_bottom_u32` 으로 비교
- `Rect::new(area.x, area.y + rel_y, ...)` 부분을 `Rect::new(area.x, (area.y as u32 + rel_y).min(u16::MAX as u32) as u16, ...)`
- temp buffer 복사 부분의 `inner_width as usize` 캐스팅 유지 (u16 → usize는 안전)

**테스트**:
- `test_render_loop_handles_u32_overflow`: viewport_base = 70_000, area.y = 5, area.height = 20 → render가 정상, dst_y는 u16 범위로 clamp
- `test_entry_above_viewport_skipped`: entry.y = 100, viewport_base = 200 → skip
- `test_entry_partially_clipped`: entry.y = 195, viewport_base = 200, vh = 20 → 위에서 5줄 잘려서 그려짐

**LOC**: ~150

---

### Commit 4: `feat(oxi-tui): clamp jump prevention — reflow-safe viewport`

**변경**:
- 기존 `clamp_scroll(vh)` 호출을 `try_clamp_to_existing_anchor(vh)`로 교체:
  - 새 content_height이 더 작아져서 anchor 메시지가 사라지지 않는 한 viewport_base 불변
  - 사라진 경우에만 `Following`으로 fallback (점프 안 함, 그냥 현재 viewport_base 유지하되 content 끝에 도달하면 0으로)
- 즉: viewport_base는 **사용자 의도**에 따라 잡힌 값이므로 reflow가 그걸 깨면 안 됨
- 새 메시지 추가 시 viewport_base를 자동으로 content 끝으로 점프시키지 않음 (FollowMode가 처리)

**테스트**:
- `test_reflow_does_not_jump_viewport`: 5개 메시지 + viewport_base = 100, todo_panel 토글로 content_height 변동 → viewport_base 유지
- `test_anchor_disappears_falls_back_to_following`: Pinned anchor 메시지 drain → Following

**LOC**: ~80

---

### Commit 5: `feat(oxi-tui): sticky turn-prompt header overlay`

**변경**:
- 신규 `state.rs::compute_sticky_candidates(messages, viewport_base, vh, n=3)` → 직전 N개 사용자 메시지 (`msg_idx`, `entry_y`)
- 신규 `sticky.rs` (또는 state.rs 내부): `render_sticky_headers(layout, candidates, viewport_base, area, buf, styles)` → viewport 상단에 overlay 레이어로 그림
- mod.rs::render: layout entries 그린 후, sticky overlay 마지막에 그림 (얇은 1-2줄, viewport 상단)

**제약**:
- sticky는 **읽기 전용** — input/ToolBox은 sticky 영역 사용 안 함 (input area는 별도 viewport)
- sticky가 부분적으로만 보이도록 (R10 정정 — N=3, max sticky height = 3 lines)
- 위치: viewport 상단 고정. scroll 따라 sticky 메시지가 바뀌면 fade transition (단순: 1 프레임 fade 없이 swap)

**테스트**:
- `test_sticky_candidates_picks_last_n_user_messages`
- `test_sticky_does_not_render_when_no_user_messages`
- `test_sticky_overlay_at_viewport_top`: viewport_base = 50, area.y = 0 → sticky는 area.y=0에 그려짐
- `test_sticky_messages_change_with_scroll`: viewport_base 이동 → 다른 사용자 메시지가 sticky로

**LOC**: ~250

---

## 완료 기준 (acceptance criteria)

### 단위 테스트
- 5개 commit 합쳐서 신규 테스트 ≥ 25개
- 기존 oxi-tui 테스트 (363개) 모두 통과
- `cargo clippy --workspace --all-targets -- -D warnings` 통과

### 통합
- `cargo nextest run --workspace` 통과
- `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` 통과 (oxi-tui 변경이 SDK에 영향 X)
- `cargo fmt -p oxi-tui -- --check` 통과

### UX 증상 (PTY harness로 검증)
- 100K 줄 더미 세션 스크롤 정상 (u16 cap 증상 해결)
- 스트리밍 중 사용자 scroll up → 2초 grace → Pinned + badge (auto_scroll race 해결)
- todo_panel 토글 → viewport 점프 0회 (clamp jump 해결)
- 100개 메시지 후에도 직전 turn prompt 보임 (sticky)

### 외부 API 호환
- `oxi-cli/src/tui/app.rs::App::scroll_up(n: u16)` 등 시그니처 불변
- `oxi-cli/src/tui/handlers.rs` 변경 없음
- `chat::ChatView` widget render 결과 시각적으로 동일 (단, sticky/overflow 추가 효과는 의도된 변화)

---

## 총 LOC 추정: ~1,130

(스펙의 3,500 추정은 B3 sticky 헤더가 별도 workstream이었던 v0 설계 기준. v3 설계에서 W1 sticky를 흡수했지만, 전체 grok sticky.rs 1,430 LOC를 oxi-tui 축소판으로 가져오지 않음 — 250 LOC이면 "직전 N개 사용자 메시지를 viewport 상단에 그리기" 충분)

---

## 외부 의존성

없음. 모든 변경은 oxi-tui 내부. 외부 API (`App::scroll_*`)는 시그니처 유지 + 내부 변환만.

---

## 롤백 계획

각 commit이 독립적으로 revert 가능:
- Commit 1 revert → u16 복귀 (마이그레이션 가능)
- Commit 2 revert → `auto_scroll: bool` 복귀, FollowMode 제거
- Commit 3 revert → draw-time 변환 제거, u16 직접 산술
- Commit 4 revert → `clamp_scroll` 복귀
- Commit 5 revert → sticky overlay 제거

전체 revert 시 Phase 1 상태로 복귀.
