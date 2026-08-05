# TUI grok-build Parity — Phase 2 구현 명세

- **갱신**: 2026-08-05 (Phase 1 완료 직후)
- **브랜치**: `main`
- **전제 문서**: `2026-08-05-tui-grok-build-parity-remaining.md` (Phase 1 원본, 18개 항목 명세)

---

## Phase 1 완료 요약 (이번 세션)

| 항목 | 상태 | 비고 |
|------|------|------|
| B.4 Shift+E / Ctrl+E | ✅ 완료 | `e` 3-state 순환, `Shift+E` expand all, `Ctrl+E` collapse all |
| B.3 Esc cancel grace | ✅ 완료 | `route_cancel` 순수 함수로 streaming/idle 분기, 1s grace |
| A.3 스크롤바 | ✅ 완료 | `render_scrollbar`, follow-tail dim / explicit bright |
| A.2 3-state display | ✅ 완료 | `BlockDisplayMode { Collapsed, Truncated, Expanded }`, head + `… +N` gap + tail |
| A.1 Sticky 헤더 | ⚠️ 단순화 | 본문 영역 축소 pin (fade/push 수학 없음) — **깊이 보강 필요** |
| C.3 ModalConfirmation | ⚠️ 부분 | quit에만 연결 — **`/clear` 등 파괴적 액션에 확장 필요** |
| C.1 OSC 알림 | ✅ 완료 | `notifications.rs` 신규 모듈, OSC 9/99/777 + tmux passthrough |
| C.2 Ephemeral 팁 | ⚠️ 부분 | onboarding + clear-hint 2개만 — **나머지 5개 팁 + occlusion 필요** |
| B.1 프롬프트 큐 | ⚠️ 부분 | 추가/드레인/send-now — **인라인 편집·재정렬·`Ctrl+;` 토글 없음** |

**검증 기준선**: fmt clean, `clippy --workspace --all-targets -- -D warnings` clean, **3162 tests passed** (nextest), `oxicode-sdk --features native-browser` clippy clean.

---

## 코드베이스 맵 (Phase 2 진입용)

`oxicode-cli/src/tui_vt/` (5,469줄 총합):

| 파일 | 줄 | 역할 |
|------|----|------|
| `main_loop.rs` | 4224 | 렌더 + 입력 + 상태 통합 (단일 파일 모놀리스) |
| `slash/registry.rs` | 666 | 슬래시 명령 (11개) |
| `frame_layout.rs` | 312 | chrome geometry (status bar / shortcuts bar) |
| `notifications.rs` | 184 | OSC 알림 프로토콜 |
| `host.rs` | 83 | 호스트 어댑터 (테마/workspace) |

### main_loop.rs 주요 진입점 (라인 번호 = Phase 1 종료 시점)

| 심볼 | 라인 | 용도 |
|------|------|------|
| `RenderState` | 144 | 입력 스레드 ↔ 렌더 공유 상태 |
| `TranscriptLine` | 222 | 트랜스크립트 라인 (kind, segments, block_id) |
| `BlockDisplayMode` | 236 | 3-state enum |
| `OverlayState` | 297 | 오버레이 모달/리스트 |
| `ModalConfirmation` | 309 | y/n/x 다이얼로그 |
| `EphemeralTip` | 318 | TTL 팁 배너 |
| `route_cancel` | 792 | Esc Cancel 라우팅 (순수 함수) |
| `apply_command` | 806 | `InlineCommand` → 상태 |
| `map_agent_event` | 990 | `AgentEvent` → 트랜스크립트 |
| `handle_inline_event` | 1112 | `InlineEvent` → 액션 |
| `handle_interrupt` | 1292 | Ctrl+C/Esc 정책 + quit confirmation |
| `spawn_input_thread` | 1338 | crossterm 폴링 + 키 디스패치 |
| `handle_confirmation_key` | 1706 | confirmation y/n/x 라우팅 |
| `handle_overlay_key` | 1736 | 오버레이 키 네비게이션 |
| `render_frame` | 2161 | 렌더 루트 (모든 위젯 호출 집합) |
| `render_transcript` | 2495 | 트랜스크립트 + sticky + scrollbar + 3-state |
| `render_scrollbar` | 2736 | 우측 1컬럼 스크롤바 |
| `transcript_line_marked` | 2784 | 라인 → ratatui `Line` (fold marker, 검색 하이라이트) |
| `render_composer` | 2880 | 입력 박스 |
| `render_tip` | 3137 | TTL 팁 배너 |
| `InputEditor` | 3234 | vim 어댑터 |

> **⚠️ 중요**: `render_frame`(2161)의 핵심 호출 체인(`render_transcript` → `render_queue_pane` → `render_todo_pane` → `render_follow_ups` → `render_composer`)은 Phase 1에서 한번 손실된 적이 있음. 이 파일을 편집한 후에는 반드시 `grep render_transcript\|render_queue_pane oxicode-cli/src/tui_vt/main_loop.rs`로 호출 존재를 확인하고, `render_frame_paints_transcript_content` 테스트가 통과하는지 확인할 것.

---

## Phase 2 — 남은 작업

### 우선순위 매트릭스

| 순위 | 작업 | 체감 효과 | 예상 규모 | 리스크 |
|------|------|-----------|-----------|--------|
| 1 | **@ 파일 피커** (B.2) | ★★★ +10~15% | ~600줄 | 높음 — 입력 시스템 전환 |
| 2 | **마우스 지원** (D.1) | ★★ +5~8% | ~500줄 | 중간 — 터미널 호환성 |
| 3 | **Settings 오버레이** (D.3) | ★★ +5% | ~600줄 | 낮음 |
| 4 | **큐 깊이 보강** | ★ +3% | ~200줄 | 낮음 |
| 5 | **Sticky fade/push** | ★ +2% | ~150줄 | 중간 — 렌더 파이프라인 |
| 6 | **팁 5종 추가** | ☆ +1% | ~150줄 | 낮음 |
| 7 | **세션 피커** (D.4) | ☆ +2% | ~400줄 | 낮음 |
| 8 | **쉘 모드** (D.2) | ☆ +1% | ~150줄 | 낮음 |
| 9 | **OSC 8 하이퍼링크** (D.5) | ☆ +1% | ~150줄 | 낮음 |
| 10 | **ModalConfirmation 확장** | ☆ +1% | ~80줄 | 낮음 |

P3 (TTS / Mermaid / Agent Dashboard)는 별도 스프린트 — 이 문서 범위 외.

---

## P2-1 — @ 파일 피커 (최우선, 가장 큰 효과)

**grok 원본**: `file_search/{mod,context,state,dropdown,line_viewer}.rs` (5파일)

**현재 상태**: oxicode 입력은 flat `String` (`RenderState.input_buffer`). `@` 트리거 자체가 없음.

### 상호작용 명세

| 입력 | 동작 |
|------|------|
| `@` (단독, 빈 단어 직후) | fuzzy 파일 검색 드롭다운 오픈 |
| `@path/` | 디렉토리 drill-down 모드 |
| `@path:N` 또는 `@path:N-M` | 라인 범위 참조 |
| `@!` | hidden 파일 표시 토글 |
| `Tab` / `Enter` | 선택 (trailing space) |
| `→` | 선택 (no space, drill-down) |
| `:` / `Ctrl+L` | 선택 + 라인 뷰어 |
| `Esc` | 취소 |

### 가드 규칙
- `foo@bar.com` 같은 이메일은 무시 (`@` 앞이 alphanumeric이면 트리거 안 함)
- `@`가 단어 시작(공백/버퍼 시작)일 때만 활성화

### 구현 전략 (두 가지 옵션)

**옵션 A — chip 모델 전환 (grok 정통)**:
- `input_buffer: String` → `Vec<AtomicTextSegment>` 구조 도입
- segment 종류: `Text(String)` | `FileRef { path, line_range }`
- grok의 forked TextArea 도입 검토
- **리스크**: InputEditor(3234), render_composer(2880), 모든 입력 처리 재작성. 규모 ~1000줄.

**옵션 B — plain text replacement (최소, 권장)**:
- `input_buffer`는 그대로 String 유지
- `@path:N-M` 형태를 plain text로 삽입
- 에이전트가 텍스트에서 경로를 파싱 (이미 oxicode read 도구가 경로 인식)
- 드롭다운만 추가: `@` 입력 시 파일 검색 팝업
- **규모**: ~500줄. 기존 시스템 파괴 없음.

### 체크리스트 (옵션 B 기준)
- [ ] `oxicode-cli/src/tui_vt/file_search.rs` 신규 모듈
- [ ] `FileSearchContext::parse_at_cursor(buffer: &str, cursor: usize) -> Option<AtToken>`
  - `@` 위치 탐지, 이메일 가드, `path:line-range` 파싱
- [ ] `FileSearchState { query, results, selected, hidden_mode }`
- [ ] 백그라운드 fuzzy 검색 (workspace 크기에 따라 동기/비동기)
  - 작은 workspace: 동기 `ignore` crate 순회
  - 큰 workspace: 별도 스레드 + 채널
- [ ] `RenderState.file_search: Option<FileSearchState>` 필드 추가
- [ ] `spawn_input_thread`에서 `@` 입력 시 `file_search` 활성화
- [ ] `render_file_search_dropdown` — composer 바로 위 (slash_popup과 유사한 패턴, `render_slash_popup` 3156 참조)
- [ ] 선택 시 `input_buffer`에 `@path ` 또는 `@path:N-M ` 삽입
- [ ] `:` / `Ctrl+L` → 라인 뷰어 (선택 파일 미리보기 오버레이)
- [ ] `mod.rs`에 `pub mod file_search;` 등록
- [ ] 테스트: `parse_at_cursor` 순수 함수 단위 테스트 (`@/usr/local/bin/`, `@!~/.config/`, `@file.rs:10-25`, `foo@bar.com` 무시)
- [ ] 회귀: 기존 3162 테스트 통과 유지

### 의존성
- `ignore` crate (이미 workspace에 있을 가능성 높음 — `grep ignore oxicode-cli/Cargo.toml` 확인)
- 없으면 `nucleo-matcher` (fuzzy) 또는 자체 구현

**예상 시간**: 8~12시간 (옵션 B)

---

## P2-2 — 마우스 지원

**grok 원본**: `input/mouse.rs` (62KB), `app/agent.rs` hit-testing

### 상호작용

| 액션 | 동작 |
|------|------|
| Click (scrollback) | 텍스트 선택 시작 |
| Click (overlay/item) | 항목 선택 |
| Click (prompt) | 포커스 |
| Wheel up/down | 라인/페이지 스크롤 |
| Drag | 텍스트 선택 |
| Middle-click | PRIMARY paste (X11) |

### 체크리스트
- [ ] `Tui::enter`(main_loop.rs `impl Tui`)에 `enable_mouse_capture` / `disable_mouse_capture` 추가
- [ ] `spawn_input_thread`에서 `Event::Mouse(MouseEvent)` 처리 분기
- [ ] hit-testing: 각 overlay/dropdown이 hit rect 노출
  - `OverlayState`에 `hit_test(x, y) -> Option<Action>` 메서드
  - file_search dropdown, slash_popup, confirmation 모달
- [ ] wheel scroll → `ScrollLineUp/Down` 이벤트와 연결 (이미 `InlineEvent::ScrollLineUp` 존재)
- [ ] click-to-focus: prompt/overlay 토글
- [ ] 터미널 호환성 주의: Apple Terminal, VS Code 통합 터미널에서 modifier-key 충돌 가능
- [ ] 테스트: synthetic `MouseEvent` dispatch (TestBackend는 마우스 미지원 → 로직 단위 테스트만)

### 리스크
- 일부 터미널에서 마우스 캡처가 스크롤백 선택을 차단 (사용자 불편)
- `Shift` + drag로 마우스 캡처 우회 허용 옵션 권장

**예상 시간**: 10~14시간

---

## P2-3 — Settings 오버레이

**grok 원본**: `views/settings_modal/` (74KB) + `settings/registry.rs`

### 현재 상태: `/settings` 미구현. 설정은 `~/.oxicode/settings.toml` 직접 편집.

### 모델
```rust
pub struct SettingDef {
    pub key: String,
    pub label: String,
    pub category: SettingCategory,  // Ui | Features | Session | Model
    pub kind: SettingKind,           // Bool | Int | String | Enum(options)
    pub default: serde_json::Value,
    pub description: String,
}
```

### 체크리스트
- [ ] `oxicode-cli/src/tui_vt/settings_modal.rs` 신규 모듈
- [ ] `SettingDef` 레지스트리 (모든 `[ui]`, `[features]`, `[session]` 토글 매핑)
  - `crate::store::settings::Settings` 필드와 동기화
- [ ] `SettingsModalState` 상태 머신: `Browse { selected, filter }` | `EditingString` | `PickingEnum`
- [ ] 검색 필터 (overlay의 `OverlaySearchState` 패턴 재활용, 328)
- [ ] 에디터: Bool (토글), Int (stepper), String (validator), Enum (chooser)
- [ ] 저장: `Settings` 변경 → `~/.oxicode/settings.toml` atomic write (temp+rename)
- [ ] `/settings` 슬래시 명령 → overlay 오픈 (registry.rs에 `SettingsCommand` 추가)
- [ ] 리셋 확인 (ModalConfirmation 통합 — P2-10 선행)
- [ ] mouse hit-testing (P2-2 선행하면 자연 통합)
- [ ] 테스트: 레지스트리 완전성 (모든 Settings 필드 매핑), 필터 동작

**예상 시간**: 12~15시간

---

## P2-4 — 프롬프트 큐 깊이 보강

**현재**: `queued_inputs.push` + `drain_queue_head` + Ctrl+Enter send-now.
**부족**: UI 상호작용 (편집/재정렬/삭제/토글).

### 체크리스트
- [ ] `RenderState.queue_panel_open: bool` 필드
- [ ] `Ctrl+;` → 큐 패널 토글 (`spawn_input_thread` 컨트롤 키 영역, ~1360)
- [ ] 큐 패널 포커스 시 키 라우팅:
  - `x` / `Delete` → 현재 항목 삭제
  - `e` / `Enter` → 인라인 편집 (input_buffer로 로드 + 큐에서 제거)
  - `Shift+J` / `Shift+K` → 순서 변경
- [ ] `render_queue_pane`(3039) 확장: 헤더 `#1 #2`, 본문, 선택 하이라이트
- [ ] 포커스 상태 `RenderState.queue_focused: bool`
- [ ] 테스트: 큐 추가/삭제/재정렬/편집

**예상 시간**: 4~6시간

---

## P2-5 — Sticky 헤더 fade/push (iOS-style 1D 수학)

**현재**: 본문 영역 1줄 축소 + 단순 pin (`render_transcript` 2624~2659).
**grok 원본**: `scrollback/sticky.rs` (1431줄), `fade_region` + `clip_from_top`.

### 알고리즘 (plan Phase 1 A.1 참조)
```
render_height = full_height - scroll_past.clamped(min_height)
push effect: 다음 block 헤더가 위로 스크롤업되면 이전 sticky를 fade-out하며 밀어냄
fade_opacity = visible / (render_height + 1)
```

### 체크리스트
- [ ] `StickyHeader { entry_idx, y_virtual, full_height, min_height }` struct
- [ ] push 감지: viewport 상단으로 다음 block 헤더가 올라오는 임계 계산
- [ ] `push_fade_region`: `blend_rgb(bg, accent, opacity)` per cell (이미 `blend_rgb` 2130 존재)
- [ ] `render_transcript`의 sticky 렌더 블록(2640~2659)을 fade-aware로 확장
- [ ] 테스트: 50줄 transcript + 30줄 viewport에서 push 전환 검증

### 리스크
- `render_transcript` 렌더 루프(2665~2704)와 강결합. 변경 시 wrap/accent rail/scrollbar 회귀 주의.
- **반드시** `render_frame_paints_transcript_content` + `transcript_wraps_long_lines` + sticky 테스트 통과 확인.

**예상 시간**: 4~6시간

---

## P2-6 — 컨텍스트 팁 5종 추가

**현재**: onboarding + clear-hint (2개). **grok**: 7개 variants + occlusion.

### 추가할 팁
1. `send_now`: 큐 hold 중 "Enter to send now" (큐 비어있지 않을 때)
2. `plan_nudge`: "plan" 키워드 입력 시 "try /compact to plan" 제안
3. `clipboard_focus`: 텍스트 선택 후 "y to copy"
4. `small_screen`: width < 40 감지 시 compact mode 제안
5. `ssh_wrap`: `$SSH_CONNECTION` 감지 시 tmux wrap 제안

### 체크리스트
- [ ] `TipRegistry { seen: HashMap<&'static str, u32> }` — per-session seen cap (기본 3)
- [ ] occlusion: `ambient: true` 팁은 overlay/confirmation 활성 시 TTL 일시정지
- [ ] 각 팁 트리거 지점:
  - send_now: `queued_inputs` 비어있지 않을 때 (B.1 연동)
  - plan_nudge: input_buffer에 "plan" 포함 시
  - small_screen: render_frame에서 area.width 체크
  - ssh_wrap: run_tui 시작 시 env 체크 (1회)
- [ ] `EphemeralTip`에 `key: &'static str`, `ambient: bool` 필드 추가
- [ ] seen cap 도달 시 동일 팁 재표시 안 함
- [ ] 테스트: seen cap, TTL 일시정지 (occlusion)

**예상 시간**: 4~5시간

---

## P2-7 — 세션 피커

**grok 원본**: `views/session_picker.rs`

### 현재: 세션 resume은 CLI 전용. TUI 통합 없음.

### 체크리스트
- [ ] `/sessions` 슬래시 명령 → overlay 오픈
- [ ] `SessionManager`(crate::store::session)에서 세션 목록 로드
- [ ] repo 그룹핑: `repo_name_from_cwd` (마지막 2 path 컴포넌트)
- [ ] 현재 cwd 그룹 pin-to-top
- [ ] expandable rows: id / cwd / time
- [ ] content search: 전 세션 grep + snippet 미리보기
- [ ] `d` → ModalConfirmation "Delete session?" → 삭제
- [ ] UUID 직접 paste → 로드 (validator)
- [ ] 테스트: 그룹핑, 검색, 삭제 확인 플로우

**예상 시간**: 8~10시간

---

## P2-8 — 쉘 모드 (!)

**grok 패턴**: 빈 프롬프트에서 `!` → bash 모드, prefix `! ` (yellow). Esc = 종료.

### 체크리스트
- [ ] `RenderState.shell_mode: bool` 필드
- [ ] `spawn_input_thread` Char arm에서 빈 버퍼 + `!` → `shell_mode = true`
- [ ] `render_composer`(2880)에서 shell_mode 시 prefix `! ` 노란색
- [ ] submit 시: `InlineEvent::Submit` 대신 bash 실행 경로
  - 옵션: 새 `InlineEvent::BashCommand(String)` 또는 기존 bash 도구 라우팅
- [ ] Esc = shell_mode 해제
- [ ] cheatsheet에 `!` 섹션 추가

### 리스크
- `InlineEvent::Submit`이 `String`만 받음. bash 명령 구분 필요하면 `oxicode_vtui`의 `InlineEvent` enum 확장 (상위 크레이트 변경).

**예상 시간**: 3~4시간

---

## P2-9 — OSC 8 하이퍼링크

**grok 원본**: `link_map.rs` + OSC 8 escape

### 체크리스트
- [ ] `set_hyperlink(url, text)` 헬퍼: `"\x1b]8;;<url>\x1b\\<text>\x1b]8;;\x1b\\"`
- [ ] `notifications.rs` 또는 신규 `osc8.rs`에 배치
- [ ] 스크롤백의 미디어 경로 (`./assets/foo.png`) → `file://` link
- [ ] `transcript_line_marked`(2784)에서 URL 패턴 감지 시 하이퍼링크 래핑
- [ ] hit-testing: link rect → 클릭 시 open (P2-2 마우스 선행)
- [ ] 테스트: escape 시퀀스 생성 (순수 함수)

**예상 시간**: 3~4시간

---

## P2-10 — ModalConfirmation 확장

**현재**: quit에만 연결. **확장**: 파괴적 액션에 범용 적용.

### 체크리스트
- [ ] `ModalConfirmation`에 `action: ConfirmationAction` 필드 추가
  ```rust
  pub enum ConfirmationAction {
      Quit,
      ClearConversation,
      DeleteSession { id: String },
      Custom(Box<dyn FnOnce(&mut RenderState)>),
  }
  ```
- [ ] `handle_confirmation_key`에서 `Yes` 시 action 실행
  - `ClearConversation` → `session.reset()` + transcript clear (ClearCommand 로직, registry.rs 248)
  - session 접근 필요 → `handle_inline_event`로 라우팅 또는 `InlineEvent` 확장
- [ ] `/clear` → confirmation 설정 (ClearCommand.execute 변경)
- [ ] 테스트: 각 action의 Yes/No 동작

### 리스크
- session 접근이 입력 스레드에서 불가 → 이벤트 기반 라우팅 필요. 가장 깔끔한 방법:
  - `RenderState.pending_confirmation_action: Option<ConfirmationAction>`
  - `Yes` → 해당 action을 `handle_inline_event`로 전달할 이벤트 추가

**예상 시간**: 2~3시간

---

## 전체 검증 체크리스트 (매 작업 후)

```bash
# 1. 포맷
cargo fmt --all -- --check

# 2. 린트 (반드시 -D warnings)
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings

# 3. 테스트
cargo nextest run --workspace

# 4. 렌더 회귀 (main_loop.rs 편집 시 필수)
cargo nextest run -p oxicode-cli tui_vt
# 특히 render_frame_paints_transcript_content 통과 확인
```

**기준선**: Phase 1 종료 시 3162 tests passed. 각 작업 후 이 이상 유지.

---

## 진행 추적

각 Phase 2 작업 시작 시 `todo`로 항목 init. 완료 시 이 표의 ✅ 표시.

| 작업 | 상태 | 테스트 수 |
|------|------|-----------|
| P2-1 @ 파일 피커 | ☐ | |
| P2-2 마우스 | ☐ | |
| P2-3 Settings | ☐ | |
| P2-4 큐 깊이 | ☐ | |
| P2-5 Sticky fade/push | ☐ | |
| P2-6 팁 5종 | ☐ | |
| P2-7 세션 피커 | ☐ | |
| P2-8 쉘 모드 | ☐ | |
| P2-9 OSC 8 | ☐ | |
| P2-10 ModalConfirmation 확장 | ☐ | |

**완성도 추정**: P2-1~P2-3 완료 시 grok-build parity ~80%. P2-4~P2-10 완료 시 ~90%. P3(TTS/Mermaid/Dashboard)는 별도.
