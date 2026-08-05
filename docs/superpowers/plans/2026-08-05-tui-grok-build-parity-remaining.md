# TUI grok-build Parity — 남은 60% 구현 명세

- **갱신**: 2026-08-05
- **브랜치**: `main`
- **완료** (이전 세션): 테마 마이그레이션 (ciapre-dark → oxide-dark) + 11개 TUI 기능
  - /theme, /find, Shift+J/K, e/E 블록 폴딩, ? 치트시트
  - 액센트 레일 + 웨이브 애니메이션, Esc 사다리, 터미널 알림
  - 커맨드 팔레트 (Ctrl+P), 멀티라인 (Ctrl+M), 히스토리 recall (↑)
  - diff 렌더링 (gated on @@ hunk markers), 데드 코드 정리
- **기준선**: 749 tests passing, clippy clean, fmt clean
- **참조**: 3개 스카우트 분석 결과 (agent://GrokRenderAnalysis, GrokInputAnalysis, GrokOverlayAnalysis)

---

## 완료된 작업 요약

| Phase | 작업 | 효과 | 규모 |
|-------|------|------|:----:|
| 1차 | /theme, /find, Shift+J/K, e/E, ? | 슬래시 8→11개, 키바인딩 +5 | +189 lines |
| 1차 | 액센트 레일 + sin² 웨이브 | 시각 정체성 확보 | +55 lines |
| 1차 | Esc 사다리 (800ms 더블) | #1 UX-polish | +20 lines |
| 1차 | 터미널 알림 (벨 + 타이틀) | 백그라운드 인식 | +25 lines |
| 1차 | Ctrl+P 팔레트, Ctrl+M 멀티라인, ↑ 히스토리 | 입력 정교화 | +60 lines |
| 1차 | try_render_diff (@@ 게이트) | 편집 가시성 | +65 lines |
| 1차 | transcript_line 데드 코드 제거 | 코드 품질 | -45 lines |

**누적**: +369 net, 749 tests, 11/12 grok P0+P1 기능

---

## 남은 작업 (이전 P0+P1 미구현 + 신규 발견)

| 순위 | 작업 | grok-build 원본 | 영향 | 예상 규모 | 비고 |
|------|------|-----------------|------|-----------|------|
| 1 | **Sticky 헤더** | `scrollback/sticky.rs` (1431줄) | ★★★ | ~300 lines | iOS-style 1D math, 가장 큰 UX 단일 개선 |
| 2 | **프롬프트 큐** | `queue_pane.rs` (80KB) + `app/agent.rs` | ★★ | ~400 lines | FIFO + 종류별 라우팅 + send-now |
| 3 | **@ 파일 피커** | `file_search/{mod,context,dropdown,state,line_viewer}.rs` | ★★ | ~500 lines | fuzzy + dir drill-down + line range |
| 4 | **OSC 알림 확장** | `notifications/protocol.rs` (brand detection) | ★★ | ~200 lines | OSC 9/99/777 + tmux passthrough |
| 5 | **Three-state display** | `block.rs::next_fold_mode` | ★★ | ~250 lines | Collapsed/Truncated/Expanded + auto-collapse |
| 6 | **스크롤바 follow-dim** | `render/scrollbar.rs` (tui-scrollbar) | ★ | ~150 lines | follow-tail에서 dim, 스크롤 시 bright |
| 7 | **컨텍스트 팁 시스템** | `tips/ephemeral.rs` + 7 variants | ★ | ~350 lines | TTL 배너 + seen-cap + occlusion |
| 8 | **ModalConfirmation** | `views/modal.rs::ModalConfirmation<R>` | ★ | ~100 lines | y/n/x 차단 다이얼로그 패턴 |
| 9 | **Esc 후 취소 grace** | docs/03 사다리 (post-cancel ~1s) | ★ | ~30 lines | mashing Esc 방지 |
| 10 | **Shift+E expand-all** | grok `Shift+E` | ★ | ~10 lines | E=unfold-all, Shift+E=expand-all |
| 11 | **마우스 지원** | `input/mouse.rs` (62KB) | ★ | ~600 lines | 클릭, 휠, hover, middle-click paste |
| 12 | **쉘 모드 (!)** | `agent.rs::bash_mode` | ★ | ~200 lines | 빈 프롬프트에서 !→직접 bash |
| 13 | **Settings 오버레이** | `views/settings_modal/` (74KB registry) | ★ | ~800 lines | 카테고리 + 검색 + 에디터 |
| 14 | **세션 피커** | `views/session_picker.rs` | ★ | ~500 lines | repo 그룹핑 + delete 확인 |
| 15 | **하이퍼링크 (OSC 8)** | `link_map.rs` + `osc8.rs` | ★ | ~200 lines | file:// 클릭 가능 |
| 16 | **TTS/음성** | `voice/{auth,handle,mod}.rs` | ○ | ~1500 lines | STT 파이프라인 + hold-to-talk |
| 17 | **Mermaid 렌더** | `app/mermaid_worker.rs` (98KB) | ○ | ~2000 lines | ANSI art 백그라운드 worker |
| 18 | **Agent Dashboard** | `views/dashboard/` (전체) | ○ | ~3000 lines | 멀티 세션 풀스크린 |

**총 예상**: ~11,100 lines (1-15), 17-18은 별도 스프린트

---

## Phase A — 스크롤백 심화 (P1)

### A.1 — Sticky 헤더 (iOS-style)

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs` (render_transcript)

**grok 원본**: `scrollback/sticky.rs` (1431줄, exhaustive tests 포함)

**알고리즘** (1D 수학, 프레임워크 무관):
```
render_height = full_height - scroll_past.clamped(min_height)
push effect: clip_from_top with fade_region(fade_opacity = visible / (render_height + 1))
```

**현재 모델과의 호환성**:
- `RenderState.scroll_offset: usize`는 원본 인덱스 기반
- sticky는 **display line** 기반 좌표가 필요
- 새 좌표: `logical_row = skip_rows + row_in_viewport` (파라미터 안정)
- `ScrollbackState` 도입 없이 `RenderState`에 sticky metadata 추가:
  ```rust
  pub struct StickyHeader {
      pub entry_idx: usize,        // transcript 원본 인덱스
      pub y_virtual: u16,
      pub full_height: u16,
      pub min_height: u16,         // 가장 작을 때 높이
  }
  pub pinned_sticky: Option<StickyHeader>,
  pub pushed_sticky: Option<StickyHeader>,
  ```

**체크리스트**:
- [ ] `StickyHeader` struct + push/pin 결정 알고리즘
- [ ] `render_transcript`를 display line 순회 → sticky-aware 순회로 리팩토
- [ ] `push_fade_region` 구현: `blend_rgb(bg, accent, opacity)` per cell
- [ ] clip top: `frame.buffer_mut().set_stringn(x, y, "", width, style)` 으로 빈 셀로 클리핑
- [ ] 테스트: 50줄 transcript + 30줄 viewport에서 sticky 동작 검증
- [ ] 회귀 테스트: 기존 transcript/wrap 테스트 깨지지 않는지

**리스크**: 전체 transcript 렌더 파이프라인이 sticky-aware가 되어야 하므로 render_transcript가 가장 큰 변경. 단, `wave_brightness`의 `logical_row` 패턴이 이미 동일 좌표계를 쓰므로 호환.

**예상 시간**: 4-6시간

### A.2 — Three-State Display Mode (Collapsed/Truncated/Expanded)

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs`

**grok 원본**: `block.rs::BlockContent::default_display_mode`, `next_fold_mode`, `collapse_mode`, `finished_display_mode`

**현재 oxicode 모델과의 차이**:
- oxicode: 2-state (folded/unfolded)
- grok: 3-state (Collapsed → Truncated[default] → Expanded)
- Truncated: "…" + 마지막 N줄 + body de-emphasis (DIM+ITALIC blend)

**모델 확장**:
```rust
pub enum BlockDisplayMode { Collapsed, Truncated, Expanded }
pub struct BlockFoldMeta {
    pub mode: BlockDisplayMode,
    pub truncated_lines: usize,  // 기본 3 (생각 블록 grok default)
    pub finished: bool,          // turn 종료 시 자동 collapse
}
```

**체크리스트**:
- [ ] `BlockFoldMeta` 추가, transcript에 fold + meta 동시 저장
- [ ] `e` 키 동작: Collapsed → Truncated → Expanded → Collapsed 순환
- [ ] `TurnEnd` 이벤트 시 `finished: true`로 마킹
- [ ] Truncated 렌더링: "…" + 마지막 N줄 + "(ctrl+e to expand)" hint
- [ ] 자동-collapse: finished + 길이 > threshold면 Truncated로

**리스크**: TranscriptLine 구조 변경 (fold + meta → 2차원 데이터). RenderState에서도 관리 필요.

**예상 시간**: 3-4시간

### A.3 — 스크롤바 (follow-dim)

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs`

**grok 원본**: `render/scrollbar.rs` (tui-scrollbar crate, sub-cell 1/8 정밀도)

**현재 oxicode**: 스크롤바 자체가 없음. `scroll_offset == usize::MAX` (follow-tail) vs 명시적 오프셋만 구분.

**구현** (grok crate 의존 없이):
```rust
fn render_scrollbar(frame: &mut Frame, area: Rect, state: &RenderState) {
    if state.transcript.is_empty() { return; }
    let total = state.transcript.len();
    let follow = state.scroll_offset == usize::MAX;
    let pos = if follow { total.saturating_sub(1) } else { state.scroll_offset };
    let ratio = (pos as f64 / total as f64).clamp(0.0, 1.0);
    let height = area.height as usize;
    let thumb_h = (height * height / total.max(height)).max(1);
    let thumb_y = (ratio * (height - thumb_h) as f64) as u16;
    
    let dim = follow;
    let bar_color = if dim { bg_color } else { accent_color };
    for y in area.top()..area.bottom() {
        let is_thumb = (y - area.top()) >= thumb_y 
                    && (y - area.top()) < thumb_y + thumb_h as u16;
        let cell = &mut frame.buffer_mut().cell_mut((area.x + area.width - 1, y)).unwrap();
        cell.set_char(if is_thumb { '█' } else { '│' });
        cell.set_style(Style::default().fg(bar_color));
    }
}
```

**체크리스트**:
- [ ] 스크롤바 렌더 함수
- [ ] content_area를 1-col 줄여서 공간 확보
- [ ] follow-tail일 때 dim, 스크롤 시 bright
- [ ] 클릭 점프 (선택사항, mouse 미구현이면 skip)

**예상 시간**: 2시간

---

## Phase B — 입력 정교화 (P1)

### B.1 — 프롬프트 큐

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs` + `oxicode-cli/src/tui_vt/host.rs`

**grok 원본**: `queue_pane.rs` (80KB) + `app/agent.rs::enqueue_prompt`

**현재 oxicode**: `RenderState.queued_inputs: Vec<String>` 이미 존재하지만 UI가 미구현. 단순 FIFO (merge 안 함).

**상호작용**:
| 키 | 동작 |
|---|---|
| Enter (running) | 현재 입력 큐에 추가 |
| Enter (idle, 큐 비어있지 않음) | 큐 top 전송 |
| `Ctrl+;` | 큐 패널 토글 |
| `Ctrl+Enter` | send-now interrupt |
| `x` / `Delete` | 큐 아이템 삭제 |
| `e` / `Enter` (큐 위) | 인라인 편집 |
| `Shift+J/K` | 큐 아이템 재정렬 |

**체크리스트**:
- [ ] `QueueEntry { text, kind: Prompt\|Bash\|Command, is_in_flight }` 모델
- [ ] Enter 핸들러 분기: running → 큐, idle+큐비어있지않음 → 큐 전송
- [ ] 큐 패널: 헤더 `#1` `#2` + 본문 + 액션 버튼 (호버)
- [ ] send-now: turn cancel + 큐 top 즉시 전송
- [ ] 테스트: 큐 추가/삭제/재정렬/전송

**리스크**: `agent_loop`과의 메시지 프로토콜 변경 필요 가능. `InlineEvent::Submit`을 항상 즉시 처리하는 현재 모델과 충돌.

**예상 시간**: 6-8시간

### B.2 — @ 파일 피커

**대상 파일**: `oxicode-vtui/src/tui/file_search/` (신규 디렉토리)

**grok 원본**: `file_search/{mod,context,state,dropdown,line_viewer}.rs` 5개 파일

**상호작용**:
| 입력 | 동작 |
|---|---|
| `@` (단독) | fuzzy 파일 검색 드롭다운 |
| `@path:N-M` | 라인 범위 참조 |
| `@dir/` | 디렉토리 모드 (drill-down) |
| `@!` | hidden 파일 모드 |
| Tab / Enter | 선택 (trailing space) |
| `→` | 선택 (no space, drill-down용) |
| `:` / `Ctrl-L` | 선택 + 라인 뷰어 |

**컴포넌트**:
```rust
// file_search/context.rs: @-token 파서
// file_search/state.rs: 검색 상태 (query, results, scroll)
// file_search/dropdown.rs: 드롭다운 렌더링 (prompt 바로 위)
// file_search/line_viewer.rs: 선택 후 파일 미리보기
```

**체크리스트**:
- [ ] `FileSearchContext::parse_at_cursor(buffer, cursor) -> Option<AtToken>`
- [ ] email 가드 (`foo@bar.com` 무시)
- [ ] 백그라운드 fuzzy 검색 (ignore crate 또는 nucleo)
- [ ] 프롬프트 위 드롭다운 렌더링
- [ ] 삽입: `@path:N-M` → AtomicTextSegment (KIND_FILE_REF)
- [ ] 테스트: `@/usr/local/bin/`, `@!~/.config/`, `@file.rs:10-25`

**리스크**: 텍스트 입력 시스템을 "element chips"로 전환 필요 (현재 flat string). grok의 forked TextArea 도입 검토. 또는 최소: plain text replacement (grokked).

**예상 시간**: 8-10시간

### B.3 — Esc 후 취소 grace

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs`

**grok 패턴**: 1st Esc = cancel + grace(~1s), grace 중 mashing Esc는 무시

**현재 oxicode**: Esc 사다리에서 `last_esc_at`은 입력 클리어용. 별도 `cancel_grace_until` 필요.

**체크리스트**:
- [ ] `RenderState.cancel_grace_until: Option<Instant>`
- [ ] 1st Esc → cancel + grace 1초 설정
- [ ] grace 중 Esc 무시
- [ ] grace 만료 후 새 Esc는 다시 cancel

**예상 시간**: 1시간

### B.4 — Shift+E expand-all (vs E unfold-all)

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs`

**grok 패턴**: `e` = fold 토글, `Shift+E` = expand-all, `Ctrl+E` = expand all thinking

**현재 oxicode**: `E` = unfold all. grok 의미와 정반대.

**체크리스트**:
- [ ] `Shift+E` = `folded_blocks.clear()` (expand all)
- [ ] `E` = `folded_blocks.insert_all()` (fold all)
- [ ] cheatsheet 업데이트

**예상 시간**: 30분

---

## Phase C — 알림/UX (P1→P2)

### C.1 — OSC 알림 확장 (brand detection)

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs` (신규 `notifications.rs` 모듈)

**grok 원본**: `notifications/{protocol,mod,title,progress,focus,sleep,tmux}.rs` 7개 파일

**현재 oxicode**: 모든 터미널에 BEL만. 제목은 braille 스피너 + 모델명.

**구현**:
```rust
// protocol.rs
enum NotificationProtocol {
    Osc9,   // iTerm2, WezTerm, Warp
    Osc99,  // Kitty (i=grok;)
    Osc777, // Ghostty, VTE, Foot, Terminator
    Bel,
    None,
}

fn detect_protocol(terminal: &str) -> NotificationProtocol {
    if terminal.contains("ghostty") { Osc777 }
    else if terminal.contains("wezterm") { Osc9 }
    else if terminal.contains("iterm2") { Osc9 }
    // ...
}
```

**체크리스트**:
- [ ] 터미널 감지: `$TERM`, `$TERM_PROGRAM`, `$WEZTERM_EXECUTABLE` 등
- [ ] OSC 9/99/777/8 escape 시퀀스 정의
- [ ] tmux DCS passthrough: `\x1bPtmux;\x1b<seq>\x1b\\` 래핑
- [ ] Ghostty 1s keep-alive for OSC 9;4
- [ ] 알림 dedup: `ApprovalRequired`가 큐잉될 때 re-bell 방지
- [ ] sleep inhibitor (focus-gated 알림)

**예상 시간**: 4-5시간

### C.2 — 컨텍스트 팁 시스템

**대상 파일**: `oxicode-vtui/src/tips/` (신규)

**grok 원본**: `tips/{ephemeral,clear_detector,clipboard_focus,plan_nudge,send_now,small_screen,ssh_wrap,word_select}.rs`

**모델**:
```rust
pub struct EphemeralTip {
    pub key: &'static str,         // dedup 키
    pub text: String,
    pub ttl_ticks: u32,             // 기본 90 (~3s @ 30fps)
    pub seen_count: u32,            // per-session cap
    pub clear_on_submit: bool,
    pub ambient: bool,              // true = occluded 동안 TTL 일시정지
}

pub struct TipRegistry {
    active: Option<EphemeralTip>,
    seen: HashMap<&'static str, u32>,
}
```

**7개 팁**:
1. `clear_detector`: 2× Esc 감지 → "press again to clear" (이미 1차에서 구현)
2. `send_now`: 큐 hold 중 "Enter to send now"
3. `plan_nudge`: "plan" 키워드 입력 시 mode toggle nudge
4. `clipboard_focus`: text selection 후 "y to copy" hint
5. `small_screen`: < 40 cols 감지 시 compact mode 제안
6. `ssh_wrap`: SSH 세션 감지 시 tmux wrap 제안
7. `word_select`: 이중 클릭 후 단어 선택 hint

**체크리스트**:
- [ ] `EphemeralTip` + `TipRegistry` 타입
- [ ] render: 1줄 배너 (prompt 위 또는 footer)
- [ ] occlusion: `ambient=true`이고 occluded면 TTL 일시정지
- [ ] per-session seen cap (기본 3)
- [ ] 7개 팁 트리거 등록

**예상 시간**: 5-6시간

### C.3 — ModalConfirmation 패턴

**대상 파일**: `oxicode-vtui/src/tui/modal.rs` (신규 또는 확장)

**grok 원본**: `views/modal.rs::ModalConfirmation<R>`

**모델**:
```rust
pub enum ConfirmationResult<R> {
    Yes(R),  // 동적 결과 (e.g. "save & send" vs "discard & send")
    No,
    Cancel,
}

pub struct ModalConfirmation<R> {
    pub title: String,
    pub message: String,
    pub yes_label: String,
    pub no_label: Option<String>,
    pub cancel_label: Option<String>,
}
```

**체크리스트**:
- [ ] generic `ModalConfirmation<R>` 타입
- [ ] stdin_thread에서 y/n/x 키 라우팅
- [ ] 결과 이벤트 → main loop으로 전달
- [ ] 첫 사용 사례: `pending_quit` (Ctrl+C 2번째 확인) 또는 destructive 액션

**예상 시간**: 2시간

---

## Phase D — 모드/고급 (P2)

### D.1 — 마우스 지원

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs` (spawn_input_thread + render)

**grok 원본**: `input/mouse.rs` (62KB), `app/agent.rs`의 hit-testing

**상호작용**:
| 액션 | 동작 |
|---|---|
| Click | (1) scrollback: 텍스트 선택, (2) overlay: 아이템 선택, (3) prompt: 포커스 |
| Wheel | scroll line/page |
| Drag | 텍스트 선택 |
| Right-click | 컨텍스트 메뉴 (선택사항) |
| Middle-click | PRIMARY paste (X11) |

**체크리스트**:
- [ ] crossterm::event::MouseEvent 처리
- [ ] hit-testing: 각 overlay/dropdown이 hit rects 노출
- [ ] click-to-focus: prompt/overlay 토글
- [ ] wheel scroll: `ScrollLineUp/Down` 이벤트와 연결
- [ ] `mouse_capture` 설정 (raw mode에서 enable)
- [ ] 테스트: synthetic mouse event dispatch

**예상 시간**: 10-12시간 (매우 광범위)

### D.2 — 쉘 모드 (!)

**대상 파일**: `oxicode-cli/src/tui_vt/main_loop.rs` + `oxicode-cli/src/tui_vt/slash/registry.rs`

**grok 패턴**: 빈 프롬프트에서 `!` → bash 모드 진입, prompt prefix `! ` (yellow). Esc = 종료.

**체크리스트**:
- [ ] `RenderState.shell_mode: bool`
- [ ] `!` 단독 입력 시 `shell_mode = true`
- [ ] prompt prefix `! ` (yellow accent)
- [ ] submit 시 `InlineEvent::BashCommand` (새 이벤트 또는 기존 `Submit` 재사용)
- [ ] Esc = shell mode 해제
- [ ] cheatsheet에 `!` 섹션 추가

**리스크**: `InlineEvent::Submit`이 `String`만 받음. `BashCommand` 구분 필요 시 새 이벤트 추가.

**예상 시간**: 3-4시간

### D.3 — Settings 오버레이

**대상 파일**: `oxicode-vtui/src/tui/settings_modal/` (신규)

**grok 원본**: `views/settings_modal/{mod,render,input,state}.rs` 4개 + `settings/{defs,registry}.rs` (137KB)

**모델**:
```rust
// settings/registry.rs
pub struct SettingDef {
    pub key: String,
    pub label: String,
    pub category: SettingCategory,
    pub kind: SettingKind,  // Bool | Int | String | Enum(min/max/options)
    pub default: Value,
    pub description: String,
    pub validator: Option<fn(&Value) -> Result<(), String>>,
}

// state.rs
pub enum SettingsModalState {
    Browse { selected: usize, filter: String },
    EditingString { /* ... */ },
    PickingEnum { options: Vec<String>, current: usize },
}
```

**체크리스트**:
- [ ] `SettingDef` 레지스트리 (모든 [ui], [features], [session] 토글)
- [ ] `SettingsModalState` 상태 머신
- [ ] 필터 캐시 (mutate 시 재계산)
- [ ] 에디터: String (validator), Int (stepper), Enum (chooser)
- [ ] 리셋 확인 (ModalConfirmation 통합)
- [ ] mouse hit-testing

**예상 시간**: 12-15시간

### D.4 — 세션 피커

**대상 파일**: `oxicode-cli/src/tui_vt/` (sessions 모듈 확장)

**grok 원본**: `views/session_picker.rs`

**현재 oxicode**: 세션 resume는 기본 CLI, TUI 통합 없음.

**체크리스트**:
- [ ] repo 그룹핑: `repo_name_from_cwd` (마지막 2 path 컴포넌트)
- [ ] 현재 cwd 그룹 pin-to-top
- [ ] expandable rows: id/cwd/time
- [ ] content search: 전 세션 grep + snippet 미리보기
- [ ] `d` → `y/n` armed delete
- [ ] UUID 직접 paste 로드 (validator)

**예상 시간**: 8-10시간

### D.5 — OSC 8 하이퍼링크

**대상 파일**: `oxicode-vtui/src/render/osc8.rs` (신규)

**grok 원본**: `link_map.rs` + OSC 8 escape

**체크리스트**:
- [ ] `set_hyperlink(url, text)` → `"\x1b]8;;<url>\x1b\\<text>\x1b]8;;\x1b\\"`
- [ ] 스크롤백의 미디어 경로 (`./assets/foo.png`) → file:// link
- [ ] `description_lines`의 마지막 줄 underline + clickable
- [ ] hit-testing: link rect → on-click open

**예상 시간**: 3-4시간

---

## Phase E — 멀티미디어/대형 (P3+)

### E.1 — TTS/음성 입력

**grok 원본**: `voice/{auth,handle,mod}.rs` + STT 파이프라인

**컴포넌트**:
- hold-to-talk (Kitty protocol) 또는 toggle
- 마이크 캡 (cpal, portaudio)
- STT API 호출 (Whisper, Deepgram)
- VoiceTarget: agent prompt vs dashboard dispatch

**예상 시간**: 20-30시간 (별도 인프라)

### E.2 — Mermaid 렌더링

**grok 원본**: `app/mermaid_worker.rs` (98KB) — 백그라운드 ANSI art 렌더

**컴포넌트**:
- `mermaid` CLI 또는 JS 런타임 호출
- 백그라운드 thread (UI 블로킹 방지)
- ANSI art 결과 캐시 (width-keyed)
- progress indicator
- 에러 시 fallback (text-only)

**예상 시간**: 25-35시간

### E.3 — Agent Dashboard (멀티 세션)

**grok 원본**: `views/dashboard/` (전체) — 7개 파일

**컴포넌트**:
- 풀스크린 멀티 세션 뷰
- 세션별 peek panel
- dispatch input (한 세션→다른 세션 입력 라우팅)
- Ctrl+/ search/filter
- pin/rename/stop/delete
- Ctrl+G grouping (by state/directory)

**예상 시간**: 40-50시간

---

## 우선순위 권장 (P0+P1 완성)

1. **B.4** (Shift+E) — 30분, 즉시 가능
2. **B.3** (Esc grace) — 1시간, 실수 방지
3. **A.3** (스크롤바) — 2시간, 즉각적 가시성
4. **C.3** (ModalConfirmation) — 2시간, 재사용 가능한 패턴
5. **A.2** (Three-state) — 3-4시간, fold의 진화
6. **A.1** (Sticky 헤더) — 4-6시간, 가장 큰 단일 UX
7. **C.1** (OSC 확장) — 4-5시간, 백그라운드 UX
8. **B.1** (프롬프트 큐) — 6-8시간, 핵심 워크플로
9. **C.2** (팁 시스템) — 5-6시간, 발견성

**총 1-9**: ~30-40시간 작업으로 grok-build의 P0+P1 기능 90% 달성

Phase D (마우스, Settings, 세션 피커) + Phase E는 별도 스프린트 (각 ~20-50시간).

---

## 변경 시 주의사항

- **TranscriptLine 모델 확장 시**: A.1, A.2는 transcript 데이터 구조에 필드 추가. 기존 749 테스트의 TranscriptLine 인스턴스화에 영향.
- **마우스 도입 시**: `crossterm::event::EnableMouseCapture` 필수. 일부 터미널(Apple Terminal, VS Code 통합 터미널)에서 modifier-key 충돌 가능.
- **OSC 알림**: ESC sequence timing — 너무 빠르면 터미널이 무시, 너무 느리면 frame pipeline과 race. grok의 `post-flush escapes` 패턴 참고.
- **스트롤바**: `tui-scrollbar` 크레이트 의존 vs 자체 구현. 자체 구현이 grok-build와 차이 안 나면 자체로.
- **grok의 `BlockContent` trait (13 variants)**: oxicode는 1차에서 `block_id`만 도입. A.2까지는 flat 모델 유지. 진정한 13-variant 모델은 Phase 5+ 리팩토링에서.
- **clippy 경고**: `cargo clippy --workspace -- -D warnings` 매번 확인
- **fmt**: `cargo fmt --all` 매 commit 전
- **테스트**: 749개 모두 통과 유지. 새 기능마다 `TestBackend` 렌더링 테스트 추가

---

## 진행 추적

각 Phase 시작 시 todo list 업데이트. Phase A 완료 시 grok-build parity ~45%. Phase A+B 완료 시 ~60%. Phase A+B+C 완료 시 ~75%. Phase D까지 완료 시 ~90%.
