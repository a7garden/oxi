# oxi-tui: 안정성 + UX 이식 설계 (보충)

**날짜**: 2026-07-19
**상태**: 설계 (사용자 승인 대기)
**관계**: `2026-07-19-oxi-tui-grok-pattern-adoption-design.md`의 보충. 이전 설계가 렌더링 인프라(OSC8, color level, streaming checkpoint, tmTheme, PTY e2e)를 다뤘다면, 본 설계는 **UX 품질 자체**를 다룬다 — 사용자가 "불안정하고 불편하다"고 느끼는 증상의 원인을 진단하고, (A) 로컬 버그 수정과 (B) grok UX 패턴 이식으로 분리 해법을 제시.
**버전 타겟**: v0.56 patch ~ v0.58

---

## 배경 — 증상 분해 (symptom decomposition)

사용자 피드백: "oxi-tui가 불안정하고 불편하다."

`grep`으로 `TODO/FIXME/flicker/unstable/janky`를 oxi-tui/src에서 찾아봤으나 **문서화된 알려진 결함은 0건**. 따라서 "불안정하다"는 것은 **체감 UX 문제**이며, 이를 구체적 증상으로 분해해야 한다.

### "불안정"의 원인 (코드에서 직접 확인)

`oxi-tui/src/widgets/chat/state.rs`에서 3개 로컬 버그 확인:

| 버그 | 위치 | 증상 |
|---|---|---|
| **u16 scroll cap** | `state.rs:117, 125` (`content_height: u16`, `scroll_offset: u16`) | 65,535줄 초과 세션에서 스크롤이 자동으로 망가짐. 조용한 데이터 손실. |
| **auto_scroll race** | `state.rs:154-176` (binary `auto_scroll: bool`) | 사용자가 스트리밍 중 스크롤 올리면 `auto_scroll=false` → 새 내용이 아래에 계속 쌓여도 뷰가 얼어붙은 듯 보임. "왜 새 답변이 안 보이지?" |
| **clamp_scroll 매 프레임** | `state.rs:178-181` + `mod.rs:94` | `content_height`가 스트리밍 중 변하면(레이아웃 reflow, todo_panel 표시 등) `.min(max_off)`가 viewport를 위로 점프시킴. |

그리고 **마우스 스크롤 정규화 부재** — `crossterm::MouseEvent`를 처리하지 않음. 휠 한 번에 줄 수가 터미널 브랜드마다 달라서 (AppleTerminal=3, iTerm2=1, Ghostty=3) 사용자는 일관성 없는 스크롤 속도를 겪음. grok은 `input/mouse.rs`(1,443 LOC)에서 이것을 정규화한다.

### "불편"의 원인 (grok 대비 부재)

| 증상 | oxi-tui 현황 | grok 대응 |
|---|---|---|
| 명령어 발견 어려움 | 14개 slash builtin이 있으나 popup 방식. fuzzy/MRU 없음 | `slash/{mod,registry,mru,matcher}.rs` + nucleo fuzzy + 7일 반감기 MRU + inline ghost completion (총 ~3,500 LOC) |
| 단축키 발견 어려움 | 14개 overlay가 있으나 치트시트 없음 | `views/shortcuts_help.rs` 127KB — registry-driven 검색 가능한 카테고리별 단축키 모달 |
| 과거 내용 검색 불가 | 세로 스크롤로 일일이 찾아야 | `scrollback/search.rs` 1,025 LOC — 백그라운드 스레드 regex 검색 + 다음/이전 매치 |
| 진행 상황 피드백 부재 | 단순 spinner | `views/turn_status.rs` 54.5KB — 16개 상태 조합(Thinking/Responding/Running/Verifying/Retrying/Waiting) + phase timer + token count + cancel/bg 버튼 |
| 긴 대화에서 위치 실종 | 스크롤하면 어디 있는지 감각 상실 | `scrollback/sticky.rs` 1,430 LOC — "iOS-style" sticky 헤더로 turn prompt가 화면 상단에 걸쳐 있음 |

---

## 설계 원칙

1. **증량 우선 (symptom-first)** — 각 후보는 특정 증상에 매핑된다. "grok이 갖고 있어서"가 아니라 "이 증상을 치료하므로" 도입.
2. **스크롤 증상은 단일 workstream** — u16 cap, auto_scroll race, clamp jump, sticky 헤더는 서로 독립된 로컬 버그가 아니라 **가상 좌표계(virtual coordinate layer) 부재**라는 동일한 근본 원인을 공유. W1 workstream으로 통합 (grok `scrollback/{state,render,sticky}.rs`가 참조 구현). 단 A4(layout cache tuning)는 좌표계와 직교하므로 로컬 수정으로 남김.
3. **기존 v3 렌더링 설계와 상호보완** — 이전 설계의 5개 후보(OSC8, color level, streaming checkpoint, tmTheme, PTY e2e)는 여전히 유효. 본 설계의 후보들은 그 위에 UX 레이어를 올린다.
4. **oxi-tui 정체성 존중** — 호스트 의존(title bar, tmux, focus 감지, sleep)은 oxi-cli 영역으로 밀어냄. oxi-tui는 순수 위젯 라이브러리로 유지.

---

## Part A — 로컬 버그 수정 (grok 불필요, v0.56 patch)

### A4. 레이아웃 캐시 안정화

- **대상**: `state.rs:68-94` LayoutCache (msg_count, streaming_len, streaming_text_len, spinner_frame, width 기반 invalidation)
- **증상**: 모든 streaming delta가 전체 레이아웃을 무효화 → re-render storm
- **구현**:
  - `streaming_text_len` 임계값 증가 (byte 단위 검사를 line 단위로 변경)
  - tail-only invalidation: streaming 중에는 마지막 메시지의 레이아웃만 재계산, 과거는 캐시 유지
  - spinner_frame은 전체 무효화 안 함 (spinner만 별도 recomposite)
- **LOC**: ~150
- **리스크**: 중간. 캐시 invariant를 잘못 건드리면 visible 버그. 단위 테스트 필수.
- **좌표계와의 관계**: A4는 좌표계(u16 vs usize)와 직교하는 invalidation 정책 문제. W1과 독립적으로 진행 가능.

---

## Part W — 가상 스크롤 좌표계 workstream (grok 참조, v0.57)

→ "불안정" 증상 3개와 sticky 헤더(B3)를 **하나의 workstream**으로 묶은 이유: 이들은 모두 **가상 좌표계(virtual coordinate layer) 부재**라는 동일한 근본 원인을 공유한다. 로컬 버그 vs grok 이식의 경계가 모호한 영역 — grok의 `scrollback/{state,render,sticky}.rs`가 사실상의 참조 구현. **진단 정정 (advisory)**: 최초엔 A1/A2/A3를 독립된 "저비용 로컬 수정"으로 분류했으나, `LayoutEntry.y: u16`(layout.rs:28)와 ratatui Buffer/Rect의 u16 인덱스가 구조적으로 묶여 있어 `scroll_offset: usize` 단독 변경은 컴파일 안 됨. 가상 좌표계 도입이 선행해야 함.

### W1. 가상 스크롤 좌표계 + FollowMode 상태 머신 + sticky 헤더

**구조적 제약**: `LayoutEntry.y/height: u16` (layout.rs:28-29), `content_height/scroll_offset: u16` (state.rs:117,125). mod.rs:99-123의 render 루프는 모두 u16 산술 → ratatui Buffer/Rect u16 인덱스로 직접 전달.

**구현 (4개 하위 과제)**:

1. **논리 좌표계 (usize)**:
   - `LayoutEntry.y: usize`, `LayoutEntry.height: usize`
   - `content_height: usize`, `scroll_offset: usize` → `viewport_base: usize`로 개명 (의미 명확화)
2. **Draw-time u16 변환**:
   - render 루프에서 각 entry를 `viewport_base` 기준 u16 상대 좌표로 변환
   - `let rel_y = (entry.y.saturating_sub(viewport_base)).min(u16::MAX as usize) as u16;`
   - viewport 밖 entry는 스킵 (현재와 동일)
3. **FollowMode 상태 머신** (binary `auto_scroll: bool` 대체):
   ```rust
   pub enum FollowMode {
       Following,                                          // 바닥 추적
       FollowingGrace { until: Instant },                  // 2초 유예
       Pinned { anchor_msg_idx: usize, anchor_y_in_msg: usize },  // 특정 메시지 고정
   }
   ```
   - 사용자 스크롤 업 → `FollowingGrace { until: now + 2s }`
   - grace 만료 → `Pinned` (현재 보이는 첫 메시지를 anchor로 — 논리 좌표 추적이 필수)
   - 새 내용 + `Following`/`FollowingGrace` → viewport_base를 content_height-vh로 이동
   - 새 내용 + `Pinned` → viewport_base 유지 + **"↓ 새 답변" 배지**
4. **Sticky 헤더 (구 B3 흡수)**:
   - 각 사용자 메시지를 sticky 후보로 등록
   - `compute_sticky_layout(viewport_base, viewport_height, prompts)` → pinned/pushed 헤더
   - viewport 상단에 overlay 레이어로 렌더 (기존 렌더 순서 변경 최소)
   - 점진적 collapse: scroll 지남에 따라 full_height → min_height

**치료 증상 (4개 통합)**:
- u16 cap → 65K줄 초과 세션 조용한 파손 (구 A1)
- auto_scroll race → 스트리밍 중 스크롤 올리면 뷰 얼어붙음 (구 A2)
- clamp 매 프레임 점프 → 레이아웃 reflow 시 viewport 도약 (구 A3)
- 긴 대화 위치 상실 → sticky 헤더로 turn prompt가 상단에 걸침 (구 B3)

**참조 구현** (grok source):
- `scrollback/state/mod.rs` (3,478 LOC) — usize scroll position 모델 확인
- `scrollback/render.rs` (4,513 LOC) — draw-time 변환 알고리즘
- `scrollback/sticky.rs` (1,430 LOC) — sticky 헤더 알고리즘
- `scrollback/entry.rs` (834 LOC) — 다중 레벨 캐싱 전략

**LOC**: ~2,000 (grok 9,400+ LOC에서 oxi 모델로 대폭 축소. 핵심은 좌표계 + 상태 머신 + sticky 알고리즘)

**외부 의존**: 없음

**리스크**: **HIGH**. 모든 render 경로 건드림. LayoutEntry/LayoutKind 소비자(tool_renderer, dashboard, list_selector 등) 전부 업데이트 필요.

**안전 메커니즘** (v3 설계의 "decoupled safety" 패턴 차용):
- Feature flag `--cfg oxi_legacy_scroll`로 구형 좌표계 롤백
- Snapshot 테스트: 동일 메시지 시퀀스에서 신/구 렌더러 byte-identical 출력 검증
- Interleaving unit test: 스트리밍 + 사용자 스크롤 + 새 내용 도착 + todo_panel 토글 시나리오
- Viewport stability baseline: 100K 토큰 더미 응답에서 viewport 점프 0회 검증

---

## Part B — grok UX 이식 (v0.57 ~ v0.58)

각 후보는 특정 증상을 치료한다.

### B1. 마우스 스크롤 정규화 상태 머신 — "jumpy wheel" 치료

- **대상**: `oxi-tui/src/widgets/chat/mouse.rs` (신규) + `oxi-cli/src/tui/handlers.rs` (마우스 이벤트 라우팅)
- **근거**: grok `input/mouse.rs`(1,443 LOC) + `input/mouse/tests.rs`(1,860 LOC). 현재 oxi-tui는 crossterm `MouseEvent` 처리 전혀 없음 — 마우스 휠이 터미널마다 다른 속도로 동작.
- **이식 내용**:
  - **Stream 기반 gesture grouping**: 80ms 갭 또는 방향 전환까지 하나의 stream으로 묶음
  - **Per-terminal events-per-tick (EPT)**: AppleTerminal=3, Ghostty=3, iTerm2=1, VSCode=1. 터미널 브랜드별 wheel notch당 실제 이벤트 수 보정
  - **Wheel vs Trackpad 자동 감지**: interval histogram 기반
  - **Acceleration bands**: fast (<8ms → 2.5x), medium (<20ms → 1.6x), base (1.0x)
  - **Per-flush delta cap**: viewport 절반, 최소 6줄 — 한 프레임에 화면 통째 스크롤 방지
  - **Multiplexer awareness**: tmux/screen/zellij는 EPT=1 강제
- **증상 치료**: jumpy/inconsistent wheel feel
- **LOC**: ~1,200 (grok 1,443 LOC를 압축)
- **외부 의존**: 없음. crossterm 만으로 가능.
- **리스크**: 중간. EPT 테이블 하드코딩은 유지보수 부담 — `terminal_support.rs` 패턴으로 환경변수 오버라이드 허용.

### B2. Slash fuzzy dropdown + MRU — "명령어 발견" 치료

- **대상**: `oxi-tui/src/widgets/slash_dropdown.rs` (신규) + 기존 `oxi-cli/src/tui/slash/registry.rs` 확장
- **근거**: grok `views/slash_dropdown.rs`(23KB) + `slash/{registry,mru,matcher}.rs`. 현재 oxi-tui는 단순 popup.
- **이식 내용**:
  - **nucleo fuzzy matching**: ranked results, highlight indices
  - **MRU**: 7일 반감기 decay (사용자가 자주 쓰는 명령이 상단)
  - **Inline ghost completion**: `/com` → `/comm`**it** (회색 ghost)
  - **Mid-text token recognition**: 프롬프트 중간에 `/model` 등 감지 → teal 하이라이트
  - **Two-bit completeness**: `takes_args` + `args_required` 로 자동 완성 품질
  - **Preview**: `/theme` 등 arg 탐색 시 실시간 미리보기
- **증상 치료**: 사용자가 14개 명령 이름을 외워야 하는 불편
- **LOC**: ~1,500
- **외부 의존**: `nucleo` workspace dep 추가 (Helix에서 fork한 고품질 fuzzy matcher)
- **리스크**: 낮음. 기존 `oxi-cli/src/tui/slash/registry.rs`는 그대로 두고, 위에 dropdown widget 추가.

### B4. Scrollback 검색 — "과거 내용 찾기" 치료

- **대상**: `oxi-tui/src/widgets/chat/search.rs` (신규)
- **근거**: grok `scrollback/search.rs`(1,025 LOC). 백그라운드 스레드 regex 검색.
- **이식 내용**:
  - **SearchIndex**: 각 메시지의 searchable 텍스트를 `content_generation` 카운터로 캐시
  - **Background daemon**: UI 스레드 차단 없이 regex 스캔 (mpsc channel로 결과 전달)
  - **Match iteration**: next/prev 매치 점프 + 하이라이트
  - **단축키**: `/` 입력 시 검색 박스 토글, `n`/`N` 다음/이전
- **증상 치료**: "아까 본 그 메시지 어디 있지?"
- **LOC**: ~700 (grok 1,025 LOC 축소 — oxi는 메시지 수가 적어 인덱싱 단순)
- **외부 의존**: `regex` workspace dep (이미 있을 것)
- **리스크**: 중간. 백그라운드 스레드와 UI 동기화. tokio channel 사용.

### B5. Turn status indicator — "진행 상황 불투명" 치료

- **대상**: `oxi-tui/src/widgets/turn_status.rs` (신규) + 기존 `widgets/footer.rs` 대체 또는 확장
- **근거**: grok `views/turn_status.rs`(54.5KB). 16개 상태 조합.
- **이식 내용**:
  - **Braille spinner** (7.5fps): ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏
  - **상태 라벨**: Thinking / Responding / Running tool / Verifying / Retrying / Waiting approval
  - **Phase timer**: 현재 단계 경과 시간
  - **Turn timer + token count**: 전체 턴 경과, 소비 토큰
  - **Cancel [stop] button**: 키보드 `Esc` 매핑
  - **Send-to-background [↓] button**: 백그라운드로 보내고 다른 작업 계속
- **증상 치료**: 긴 작업 중 "지금 뭐 하고 있는 거야?" 불안
- **LOC**: ~1,500 (grok 54.5KB를 oxi 모델로 축소 — MCP init/drain 등 제품 기능 제외)
- **외부 의존**: 없음
- **리스크**: 낮음. 단순 위젯. oxi-cli가 상태 데이터를 공급해야 (agent 상태 연동).

### B6. Shortcuts help modal — "단축키 발견" 치료

- **대상**: `oxi-tui/src/widgets/shortcuts_help.rs` (신규)
- **근거**: grok `views/shortcuts_help.rs`(127KB). registry-driven.
- **이식 내용**:
  - **KeyBinding registry 확장**: 기존 `oxi-tui/src/keybindings/registry.rs`에 description + category 필드 추가
  - **Search/filter**: 입력 즉시 단축키 필터링
  - **카테고리 그룹핑**: Navigation / Editing / Slash Commands / Overlays
  - **Collapsible sections**: 카테고리별 접기/펼치기
  - **Context dimming**: 현재 활성 컨텍스트가 아닌 단축키는 회색
- **증상 치료**: 14개 overlay가 있는데 사용자가 모름
- **LOC**: ~2,000 (grok 127KB를 oxi 크기로 대폭 축소)
- **외부 의존**: 없음
- **리스크**: 낮음. 키바인딩 registry에 description 메타데이터를 추가하는 작업이 대부분.

### B7. Tips ephemeral framework — "기능 발견" 치료

- **대상**: `oxi-tui/src/widgets/tips.rs` (신규)
- **근거**: grok `tips/{ephemeral,render}.rs`(총 ~600 LOC). 프레임워크 수준.
- **이식 내용**:
  - **EphemeralTipState**: TTL 기반 단일 슬롯 힌트
  - **Dedup key**: 세션당 1회 표시
  - **Ambient mode**: 다른 UI 요소와 충돌 안 함
  - **render_ephemeral_tip**: 배너 영역 렌더링, modifier bleed 방지
- **트리거 로직**은 oxi-cli에 남김 (clipboard 감지, plan 키워드, undo 패턴 등은 호스트가 판단)
- **증상 치료**: 숨겨진 기능 (예: 단축키, 명령어)을 사용자에게 알림
- **LOC**: ~400 (프레임워크만)
- **외부 의존**: 없음
- **리스크**: 낮음. 단순 데이터 구조.

---

## 명시적 비목표 (본 설계에서 배제)

1. **Mouse text selection** (grok `text_selection.rs` 3,107 LOC) — alt-screen에서 자체 구현하는 비용이 너무 큼. 차라리 사용자에게 " native terminal의 스크롤백을 쓰고 싶으면 `--inline` 모드 사용"을 안내하는 게 나음. 향후 별도 설계.
2. **Atomic TextElement** (~10,000 LOC, ratatui-textarea fork 필요) — 이미 `ratatui-textarea 0.9`를 쓰고 있어 upstream 기능으로 충분. fork 비용이 가치 안 맞음.
3. **Welcome screen animation** (8,000 LOC) — cosmetic. 우선순위 아님.
4. **Tasks pane** (5,000 LOC) — 기존 `widgets/todo_panel.rs`와 기능 겹침. 통합은 별도 설계.
5. **Title bar / tmux / focus 감지** — 호스트 의존. oxi-cli 또는 oxios로 이관.
6. **Notifications 시스템** — 호스트 의존. oxi-cli로.
7. **Subagent catalog pane** — 제품 관심사. oxi-cli로.

---

## 우선순위 — 증상 커버리지 + 의존성 기반

### Phase 1: 안정성 독립 수정 (v0.56 patch, ~150 LOC)

→ 좌표계 재작업과 무관한 로컬 빠른 수정.

| 후보 | LOC | 증상 | 위험 |
|---|---:|---|---|
| A4 (layout cache tuning) | ~150 | re-render storm | 중간 |

### Phase 2a: 스크롤/피드백 workstream (v0.57.0, ~3,900 LOC)

→ "불안정" 직격 + 피드백. W1이 다른 Phase 2b 후보들과 병렬 진행 가능 (독립 파일).

| 후보 | LOC | 증상 | 위험 |
|---|---:|---|---|
| W1 (가상 좌표계 + FollowMode + sticky) | ~2,000 | 스크롤 불안정 4개 증상 + 위치 상실 | **HIGH** |
| B5 (turn status indicator) | ~1,500 | 진행 불투명 | 낮음 |
| B7 (tips framework) | ~400 | 숨겨진 기능 | 낮음 |

### Phase 2b: 명령어 발견 (v0.57.1, ~3,500 LOC)

→ "불편"의 명령 발견 축. W1과 병렬 가능.

| 후보 | LOC | 증상 | 위험 |
|---|---:|---|---|
| B2 (slash fuzzy dropdown) | ~1,500 | 명령 발견 | 낮음 |
| B6 (shortcuts help modal) | ~2,000 | 단축키 발견 | 낮음 |

### Phase 3: W1 의존 이식 (v0.58.0, ~1,900 LOC)

→ W1의 논리 좌표계를 소비하는 후보들.

| 후보 | LOC | 증상 | 위험 |
|---|---:|---|---|
| B1 (scroll normalization) | ~1,200 | jumpy wheel | 중간 |
| B4 (scrollback search) | ~700 | 과거 내용 검색 | 중간 |

**총 LOC**: ~9,450 (A: 150 + W: 2,000 + B: 7,300)

---

## 기존 v3 렌더링 설계와의 관계

이전 설계(`2026-07-19-oxi-tui-grok-pattern-adoption-design.md`)의 5개 후보와 본 설계의 후보는 **상호보완적**이다:

| 이전 (렌더링 인프라) | 본 설계 (UX) | 상호작용 |
|---|---|---|
| 후보 1 OSC8 | B4 (search) | 검색 결과 내 URL을 OSC8으로 클릭커블 |
| 후보 2 streaming checkpoint | A4 (layout cache) | checkpoint가 layout cache 안정화와 시너지 |
| 후보 3 color level | B5 (turn status) | spinner 색상도 color level 존중 |
| 후보 4 tmTheme | — | 독립 |
| 후보 5 PTY e2e | 모든 UX 후보 | PTY가 모든 UX 변경의 회귀 테스트 기반 |

**권장 순서 (이전 v3 렌더링 설계와 통합)**:
1. v0.56 patch: A4 (본) + 후보 3 color level (이전) — 독립적 안정성/기반
2. v0.57.0: W1 (본) + 후보 1 OSC8, 후보 4 tmTheme (이전) — 독립 병렬
3. v0.57.1: B2, B6 (본) + 후보 2 streaming checkpoint (이전) — checkpoint가 W1/A4와 시너지
4. v0.58: B1, B4 (본, W1 의존) + 후보 5 PTY e2e (이전) — PTY가 모든 변경 회귀 테스트

---

## 위험 요약

| 후보 | 주요 위험 | 완화 |
|---|---|---|
| A4 | 캐시 invariant 미스 | snapshot 테스트로 before/after 비교 |
| W1 | **HIGH** — 모든 render 경로 건드림 | feature flag `oxi_legacy_scroll` + snapshot 테스트 + interleaving unit test + stability baseline |
| B1 | EPT 테이블 유지보스 | 환경변수 오버라이드 (W1 이후) |
| B2 | MRU decay 백그라운드 스레드 | 단순 in-memory + 주기적 flush |
| B4 | 백그라운드 스레드 동기화 | tokio mpsc channel (W1 이후) |
| B5 | agent 상태 연동 | oxi-cli에서 상태 push 모델 |
| B6 | keybindings registry 확장 | description을 Option<String>으로 점진적 |
| B7 | tips 충돌 | 단일 슬롯 모델 |

---

## 완료 기준

각 후보별:

- **A4**: 50K 토큰 응답에서 re-render 횟수 50% 절감
- **B1**: 터미널 브랜드 4개(AppleTerminal/Ghostty/iTerm2/VSCode)에서 스크롤 속도 일관적
- **B2**: `/mod` 입력 시 `model`, `mcp` 등이 fuzzy rank로 정렬, ghost completion 동작
- **W1**: 100K 줄 더미 세션에서 스크롤 정상 + 스트리밍 중 사용자 스크롤 업 → 2초 grace 후 Pinned 전환 → "↓ 새 답변" 배지 + 레이아웃 토글 시 viewport 점프 0회 + 직전 turn prompt가 sticky로 보임
- **B4**: `/` 입력 → 검색 박스 → `error` 입력 시 모든 error 메시지로 점프
- **B5**: 턴 진행 중 phase timer + token count 실시간 표시
- **B6**: `Ctrl+/` 입력 시 모든 단축키 카테고리별 표시, 검색 동작
- **B7**: 클립보드 이미지 복사 시 "이미지 붙여넣기 가능" 팁 1회 표시

워크스페이스 차원:
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace` 통과
- 이전 v3 설계의 후보 5 (PTY e2e)가 Phase 3와 병행 도입되면 모든 후보 회귀 테스트 가능

---

## 부록 — scout 조사 결과 요약

**ViewsScout** (grok `views/` + `app/`):
- `views/shortcuts_help.rs` 127KB (oxi-tui: NONE) → B6
- `views/slash_dropdown.rs` 23KB (oxi-tui: simple completion) → B2
- `views/turn_status.rs` 54.5KB (oxi-tui: NONE) → B5
- `views/tasks_pane.rs` 110KB — 비목표 (todo_panel과 겹침)
- `views/welcome/` 160KB — 비목표 (cosmetic)
- `views/status_bar.rs + session_title.rs + shortcuts_bar.rs` 29KB — 비목표 (대부분 oxi-cli)
- `views/subagent_catalog_pane.rs` 17KB — 비목표 (제품)

**ScrollbackScout** (grok `scrollback/`):
- `scrollback/text_selection.rs` 3,107 LOC — 비목표 (비용 과대)
- `scrollback/search.rs` 1,025 LOC → B4
- `scrollback/sticky.rs` 1,430 LOC → W1 (sticky 헤더 알고리즘)
- `scrollback/state/mod.rs` 3,478 LOC — `scroll position: usize` 확인 → W1 근거 (가상 좌표계 필요성 입증)
- `scrollback/render.rs` 4,513 LOC, `scrollback/block.rs` 1,695 LOC, 기타 — 비목표

**InputSlashScout** (grok `input/` + `slash/`):
- `input/mouse.rs` 1,443 LOC → B1
- `input/keyboard_normalizer.rs` + macOS CoreGraphics probe — 비목표 (low ROI for oxi)
- `slash/{mod,registry,mru,matcher}.rs` → B2
- `xai-ratatui-textarea` atomic elements — 비목표 (fork 비용)

**FeedbackScout** (grok `notifications/` + `tips/`):
- `notifications/{title,focus,tmux,sleep,mod,protocol,progress,config,hooks}.rs` — 비목표 (호스트 의존, oxi-cli 영역)
- `tips/{ephemeral,render}.rs` ~600 LOC → B7 (프레임워크만)
- `tips/{clipboard_focus,plan_nudge,clear_detector,send_now,small_screen,ssh_wrap,word_select}.rs` — 비목표 (트리거는 oxi-cli)

---

## 참고 문서

- `docs/superpowers/specs/2026-07-19-oxi-tui-grok-pattern-adoption-design.md` — 본 설계의 전 단계 (렌더링 인프라)
- `docs/ref-porter/xai-org-grok-build-tui.md` — 비교분석 보고서
- `oxi-tui/src/widgets/chat/state.rs` — 로컬 버그 3건의 발견 장소
- grok source: `xai-grok-pager/src/{input/mouse.rs, scrollback/{sticky,search,state/mod}.rs, views/{turn_status,shortcuts_help,slash_dropdown}.rs, tips/{ephemeral,render}.rs}`
