# 범용 셀렉터 통합 리팩토링 — 설계

> **작성:** 2026-06-21
> **전제:** [`2026-06-21-omp-ask-redesign.md`](./2026-06-21-omp-ask-redesign.md) 구현 완료 후 리뷰에서 식별된 설계 부채.
> **표준:** omp `HookSelectorComponent` — 모든 리스트형 선택 UI가 하나의 위젯을 공유.
> **원칙:** Clean cutover. 별칭·레거시 경로·더블 소스 오브 트루스 금지.

---

## 0. 문제 — 중복의 정량

7개 리스트형 오버레이가 각자 커서/필터/List 렌더/popup 로직을 재구현:

| 파일 | 줄 수 | cursor | filter/search | List widget | centered popup |
|------|------:|:------:|:------:|:------:|:------:|
| `ask.rs` | 994 | ✓ | ✓ | ✓ | ✓ |
| `provider_select.rs` | 874 | ✓ | — | — | ✓ |
| `model_select_inline.rs` | 225 | ✓ | ✓ | ✓ | ✓ |
| `model_select.rs` | 214 | ✓ | ✓ | ✓ | ✓ |
| `resume_select.rs` | 194 | ✓ | — | — | ✓ |
| `fork_select.rs` | 189 | ✓ | — | ✓ | ✓ |
| `logout_select.rs` | 150 | ✓ | — | ✓ | ✓ |
| **합계** | **2840** | | | | |

omp는 이 전부를 `HookSelectorComponent` 1개(660줄)로 처리한다. oxi의 `ask.rs`만 이미 994줄이다.

**설계 부채 4종:**

1. **커서/필터 로직 N중 구현** — 각 오버레이가 `cursor`, `filtered()`, `move_selection`을 따로 구현. ask.rs의 compact 모드 버그(리뷰에서 발견)가 다른 오버레이에도 잠재 존재.
2. **ask 오버레이가 오케스트레이션 + 선택을 한 구조체에 섞음** — omp는 `askSingleQuestion`(선택 1회) + 도구 루프(순차)로 분리. oxi는 `AskOverlay`가 두 역할을 모두 담당 → 994줄짜리 갈레오 클래스.
3. **툴 결과가 텍스트 문자열만 전달** — `AgentToolResult.metadata`가 이벤트 파이프라인에서 누락됨. `format_ask_result`가 텍스트를 정규식 파싱으로 복원 (취약).
4. **뒤로 가기(←) 사전 채움 누락** — omp는 `initialSelection`으로 이전 답을 커서에 반영. oxi는 recommended로 리셋.

---

## 1. 목표 아키텍처 — 3계층

```
┌─────────────────────────────────────────────────────────┐
│  oxi-tui/src/widgets/list_selector.rs                   │  Layer 1
│  ListSelectorState — 순수 위젯 (상태 + 렌더 + 입력)       │  (oxi-tui, 의존성 無)
│  • options + markers (radio/checkbox/none)              │
│  • cursor 이동, disabled 스킵, compact+fuzzy            │
│  • render() → Vec<Line>, handle_key() → SelectorAction  │
└────────────────────────┬────────────────────────────────┘
                         │ 사용
┌────────────────────────▼────────────────────────────────┐
│  oxi-cli/src/tui/overlay/selector.rs                    │  Layer 2
│  SelectorOverlay — OverlayComponent 래퍼                │  (oxi-cli)
│  • ListSelectorState를 중앙 popup에 합성                 │
│  • 제어 행(Other/Done/Custom) 주입                       │
│  • oneshot 채널 브리지                                    │
│  • 타임아웃/poll                                         │
└────────────────────────┬────────────────────────────────┘
                         │ 사용
┌────────────────────────▼────────────────────────────────┐
│  각 도메인 오버레이 (얇은 어댑터)                          │  Layer 3
│  AskCoordinator / ModelSelect / ResumeSelect / ...      │
│  • SelectorOverlay를 설정 (옵션·마커·콜백만 지정)         │
│  • 도메인 로직만 (모델 목록 로드, 세션 조회 등)            │
└─────────────────────────────────────────────────────────┘
```

### 핵심 원칙
- **Layer 1은 oxi-tui에 산다** — oxi-cli에 의존하지 않는 순수 위젯. `Symbols`, `ThemeStyles`만 사용.
- **Layer 1은 도메인을 모른다** — "질문", "모델", "세션"을 모름. 옵션 리스트 + 마커 타입 + 콜백만 안다.
- **Layer 2는 OverlayComponent 프로토콜을 구현** — app.rs의 기존 폴링 루프와 호환.
- **Layer 3은 설정만 한다** — 각 오버레이가 30-60줄로 줄어듦.

---

## 2. Layer 1 — `ListSelectorState` (oxi-tui)

### 타입

```rust
// oxi-tui/src/widgets/list_selector.rs

/// 행 마커 종류. omp `selectionMarker`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectorMarker {
    /// 마커 없음 (일반 리스트 — 모델 선택, 세션 선택)
    #[default]
    None,
    /// 라디오 (단일 선택 미리보기 — 커서 행이 채워짐)
    Radio,
    /// 체크박스 (다중 선택 — 행별 checked 상태)
    Checkbox,
}

/// 셀렉터 옵션.
#[derive(Debug, Clone)]
pub struct SelectorOption {
    pub label: String,
    pub description: Option<String>,
    pub disabled: bool,
}

/// 셀렉터의 완전한 표시 상태. 호출자가 소유한다 (위젯은 무상태).
#[derive(Debug, Clone)]
pub struct ListSelectorState {
    // ── 표시 ──
    pub title: String,
    pub options: Vec<SelectorOption>,
    pub marker: SelectorMarker,
    /// 체크박스용: 체크된 옵션 인덱스.
    pub checked: HashSet<usize>,
    /// 마커가 적용되는 옵션 수 (제어 행은 제외).
    pub markable_count: usize,
    /// 타이틀 옆 카운트다운 "(30s)" — None이면 표시 안 함.
    pub timeout_secs: Option<u64>,
    /// 다단계 진행률 "(2/3)" — None이면 표시 안 함.
    pub progress: Option<String>,
    pub help_text: String,

    // ── 내부 상태 ──
    cursor: usize,
    search: String,
    max_visible: usize,         // compact 임계값 (기본 12)
}

/// 키 입력의 결과 — 호출자가 해석한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorAction {
    /// 아무 일도 없음 (커서만 이동 등)
    None,
    /// 옵션 선택 (단일: 즉시 확정, 다중: 토글)
    Select { option_idx: usize },
    /// 체크박스 토글
    Toggle { option_idx: usize },
    /// 이전 질문으로 (←)
    NavBack,
    /// 다음 질문으로 (→)
    NavForward,
    /// 타임아웃 발생
    Timeout,
    /// 취소 (Esc)
    Cancel,
}
```

### 메서드

```rust
impl ListSelectorState {
    pub fn new(title: String, options: Vec<SelectorOption>) -> Self;

    /// 커서 초기 위치 설정 (recommended 인덱스 + 이전 답 사전 채움).
    pub fn set_initial_cursor(&mut self, idx: usize);

    /// 렌더 (ratatui Line 벡터). 마커·커서·설명·검색 상태·카운트다운 포함.
    pub fn render(&self, width: usize, styles: &ThemeStyles) -> Vec<Line<'static>>;

    /// 키 처리. 내부 커서/검색 상태를 갱신하고 SelectorAction 반환.
    pub fn handle_key(&mut self, key: KeyEvent) -> SelectorAction;

    /// 표시 중인 옵션 수 (필터링 후).
    pub fn visible_count(&self) -> usize;

    /// 표시 행 인덱스 → 실제 옵션 인덱스 (compact 모드 필터링 처리).
    pub fn display_to_option(&self, display_idx: usize) -> Option<usize>;

    /// 현재 커서의 실제 옵션 인덱스.
    pub fn cursor_option(&self) -> Option<usize>;

    /// 타임아웃 확인 (외부 타이머에서 호출).
    pub fn check_timeout(&mut self) -> bool;
}
```

### 왜 상태를 호출자가 소유하는가

omp의 `HookSelectorComponent`는 상태를 자체 필드로 보관하지만, oxi의 ratatui 모델에서는 위젯이 무상태(stateless)로 `render()`만 하는 것이 관례다 (`tool_renderer`, `footer` 등이 이 패턴). 상태 소유자가 `handle_key`를 호출해 상태를 갱신하고, 다음 프레임에 `render`에 넘긴다. 이렇게 하면:
- 상태 직렬화/복원이 자연스럽다 (오버레이가 `Clone + Debug`).
- 테스트가 쉽다 (상태 생성 → 키 입력 → assert).
- 같은 상태로 여러 번 렌더해도 부작용이 없다.

### compact 모드 내장

`options.len() > max_visible`이면 자동으로:
- 라벨만 표시 (설명은 커서 행만)
- 퍼지 검색 활성화 (타이핑 → `search` 누적 → `visible_count` 축소)
- `display_to_option`이 필터링된 인덱스를 반환

이 로직이 **한 곳에만** 존재한다 — ask.rs의 `cursor_to_option_idx`/`visible_option_count`/`filtered_options` tangle이 제거된다.

---

## 3. Layer 2 — `SelectorOverlay` (oxi-cli)

```rust
// oxi-cli/src/tui/overlay/selector.rs

/// 제어 행 — 옵션 목록 끝에 자동으로 붙는 특수 행.
#[derive(Debug, Clone)]
pub enum ControlRow {
    /// "Other (type your own)" — 선택 시 인라인 편집기 열림.
    Other,
    /// "Done selecting" — multi-select 확정.
    Done,
    /// 도메인별 커스텀 행.
    Custom { label: String, style: ControlRowStyle },
}

#[derive(Debug, Clone)]
pub enum ControlRowStyle {
    /// 항상 활성.
    Active,
    /// 조건부 (예: Done은 1개 이상 선택 시에만 활성).
    EnabledWhen(Box<dyn Fn(&SelectorOverlay) -> bool>),
}

/// 셀렉터 결과.
#[derive(Debug, Clone)]
pub struct SelectorResult {
    /// 선택된 옵션 인덱스 목록.
    pub selected: Vec<usize>,
    /// Other 편집기의 자유 텍스트.
    pub custom_input: Option<String>,
    pub cancelled: bool,
    pub timed_out: bool,
    /// ←/→ 네비게이션 신호.
    pub nav: Option<SelectorNav>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorNav { Back, Forward }

/// 범용 셀렉터 오버레이. OverlayComponent 구현.
pub struct SelectorOverlay {
    state: ListSelectorState,
    control_rows: Vec<ControlRow>,
    allow_back: bool,
    allow_forward: bool,
    responder: Option<oneshot::Sender<SelectorResult>>,
    input_mode: bool,
    input_text: String,
    started_at: Instant,
    timeout: Option<Duration>,
}

impl SelectorOverlay {
    pub fn new(
        state: ListSelectorState,
        control_rows: Vec<ControlRow>,
        responder: oneshot::Sender<SelectorResult>,
    ) -> Self;

    /// ←/→ 네비게이션 허용 여부 (다단계 흐름용).
    pub fn with_nav(mut self, allow_back: bool, allow_forward: bool) -> Self;
}

impl OverlayComponent for SelectorOverlay { /* render + handle_key + poll + hint */ }
```

### handle_key 흐름

```
handle_key(key):
  if input_mode:                    # Other 편집기
    Enter → custom_input 확정 → responder 전송 → Close
    Esc → input_mode 종료
    Char/Backspace → 텍스트 편집
    return None

  if 제어 행이 커서에 있고 Enter/Space:
    Other → input_mode = true
    Done → selected 확정 → responder 전송 → Close
    Custom → 해당 액션

  action = state.handle_key(key)    # Layer 1에 위임
  match action:
    Select { idx } → 단일: selected=[idx], responder 전송, Close
                    다중: state.checked 토글, 유지
    NavBack → nav=Back, responder 전송, Close
    NavForward → nav=Forward, responder 전송, Close
    Cancel → cancelled=true, responder 전송, Close
    Timeout → timed_out=true, 자동선택, responder 전송, Close
    None → None
```

---

## 4. Layer 3 — 도메인 오버레이 (얇은 어댑터)

### AskCoordinator (도구 구동 순차 흐름)

omp `askSingleQuestion` + 도구 루프 모델. **bridge가 한 질문씩 전달**한다.

```rust
// oxi-agent/src/tools/ask.rs — 브리지 페이로드 변경

pub struct PendingAsk {
    pub question: Question,          // 단일 질문 (was: Vec<Question>)
    pub context: AskContext,
    pub responder: oneshot::Sender<AskResponse>,
}

pub struct AskContext {
    pub progress: Option<String>,    // "(2/3)"
    pub allow_back: bool,
    /// 뒤로 가기 시 이전 답을 커서에 사전 채움 (omp initialSelection).
    pub initial_selection: Option<Answer>,
}

// 도구 execute() 루프:
async fn execute(...) {
    let questions = parse_questions(&params)?;
    let mut idx = 0;
    let mut answers = Vec::new();
    while idx < questions.len() {
        let q = &questions[idx];
        let (tx, rx) = oneshot::channel();
        bridge.set(PendingAsk {
            question: q.clone(),
            context: AskContext {
                progress: Some(format!("{}/{}", idx + 1, questions.len())),
                allow_back: idx > 0,
                initial_selection: answers.iter().find(|a| a.id == q.id),
            },
            responder: tx,
        });
        let resp = select_with_abort(rx, signal, &bridge).await?;
        match resp.nav {
            Some(Back) => { idx = idx.saturating_sub(1); continue; }
            Some(Forward) | None => { /* 답 저장 */ idx += 1; }
        }
        if resp.cancelled { return cancelled; }
    }
    format_answers(&answers)
}
```

app.rs 폴링은 그대로 — `bridge.try_take()` → `SelectorOverlay::new(...)` 생성. **AskOverlay 994줄 삭제, SelectorOverlay 설정 ~30줄로 대체.**

### ModelSelect (얇은 어댑터)

```rust
// Before: model_select.rs 214줄 (커서/필터/List/popup 전부 직접 구현)
// After:
pub fn model_select_overlay(
    models: Vec<String>,
    responder: oneshot::Sender<SelectorResult>,
) -> SelectorOverlay {
    let options = models.iter()
        .map(|m| SelectorOption { label: m.clone(), description: None, disabled: false })
        .collect();
    let state = ListSelectorState::new("Select Model".into(), options)
        .with_max_visible(12);
    SelectorOverlay::new(state, vec![], responder)
}
// ~15줄
```

동일 패턴이 `resume_select`, `logout_select`, `fork_select`, `model_select_inline`에 적용.

---

## 5. Layer 4 (선택) — 구조화된 메타데이터 파이프라인

### 현재 문제
```
AgentToolResult { output: String, metadata: Option<Value> }
                          ↓ 에이전트 루프
AgentEvent::ToolExecutionEnd { result: ToolResult }    // metadata 누락!
                          ↓ oxi-cli 핸들러
stream_tool_result(id, name, content, is_error)        // metadata 전달 안 됨
                          ↓
ContentBlock::ToolCall { result: (String, bool) }       // 텍스트만
                          ↓
format_tool_result(name, result, ...)                   // 텍스트 파싱
```

`format_ask_result`가 result 텍스트를 정규식 파싱 → 취약 (cancel 위양성, 쉼표 포함 라벨 등).

### 제안 파이프라인
```
AgentToolResult { output, metadata }
    ↓ 에이전트 루프: metadata를 이벤트에 복사
AgentEvent::ToolExecutionEnd { result, is_error, metadata: Option<Value> }   // NEW
    ↓ oxi-cli 핸들러
stream_tool_result(id, name, content, is_error, metadata)                    // NEW
    ↓
ContentBlock::ToolCall { result: (String, bool), metadata: Option<Value> }   // NEW
    ↓
ToolCallView { ..., metadata: Option<Value> }                                // NEW
    ↓
format_tool_result(name, result, is_error, arguments, metadata, ...)         // NEW
```

ask 도구가 설정하는 메타데이터:
```json
{
  "answers": [
    {
      "id": "auth",
      "prompt": "Which auth method?",
      "options": ["JWT", "OAuth2", "Session cookies"],
      "selected": ["JWT"],
      "multi": false,
      "custom": null,
      "timed_out": false
    }
  ]
}
```

`format_ask_result`가 이 JSON을 직접 읽음 — 텍스트 파싱 제거, `parse_ask_result` 삭제.

### 비용
- `events.rs`: `ToolExecutionEnd` variant에 필드 추가 (1줄)
- `events.rs`: `ToolComplete` (legacy)에도 추가 (1줄)
- `chat/types.rs`: `ContentBlock::ToolCall`에 필드 추가
- `chat/state.rs`: `stream_tool_result` 시그니처 + 2개 호출처
- `chat/render.rs`: ToolCallView에 메타데이터 전달
- `tool_renderer.rs`: `format_tool_result` + cache 시그니처
- 에이전트 루프: `AgentToolResult.metadata`를 이벤트에 복사 (1줄)

**크로스 크레이트 4곳, 약 15개 호출처.** 기계적 변경이지만 파이프라인 전체를 건드린다.

### 의사결정
- **Layer 1-3만 먼저**: 셀렉터 통합 (ask + 기존 오버레이). 텍스트 파싱은 유지하되 `arguments` 기반 복원으로 충분.
- **Layer 4는 별도 트랙**: 메타데이터 파이프라인은 독립적 가치가 있지만 (모든 툴이 혜택), 범위가 크다. ask만 당장 필요하면 `arguments` 기반이 이미 동작하므로 후순위.

---

## 6. 마이그레이션 계획 (단계별)

```
Phase 1: ListSelectorState (oxi-tui)         ←─ 토대, 독립
   │
   ├─→ Phase 2: SelectorOverlay (oxi-cli)    ←─ Phase 1 의존
   │       │
   │       └─→ Phase 3: AskCoordinator        ←─ bridge 단일 질문 + 도구 루프
   │               │                          ←─ AskOverlay 994줄 → ~80줄
   │               │
   │               └─→ Phase 5: 기존 오버레이 마이그레이션
   │                       model_select, resume_select, fork_select,
   │                       logout_select, model_select_inline, provider_select
   │                       각 150-874줄 → 15-60줄
   │
   └─→ Phase 4 (선택): 메타데이터 파이프라인   ←─ Phase 1과 독립, 병렬 가능
```

| Phase | 대상 | 위험 | 노력 | 삭제되는 코드 |
|-------|------|------|------|---------------|
| 1 | `oxi-tui/widgets/list_selector.rs` 신규 | 낮음 | M | — |
| 2 | `oxi-cli/tui/overlay/selector.rs` 신규 | 낮음 | S | — |
| 3 | ask 도구 + bridge + app.rs | **중간** | M | ask.rs 994줄 → ~80줄 |
| 4 | events → state → render 파이프라인 | 중간 | M | parse_ask_result |
| 5a | model_select + model_select_inline | 낮음 | S | 439줄 → ~40줄 |
| 5b | resume_select + fork_select + logout_select | 낮음 | S | 533줄 → ~60줄 |
| 5c | provider_select (복잡, 다단계) | **중간** | M | 874줄 → ~100줄 |

**총 예상:** 순 삭제 ~2500줄, 순 추가 ~600줄. net **-1900줄**.

---

## 7. Phase 3 상세 — Ask 재아키텍처

### 현재 (단일 오버레이, 순차 내부 상태)
```
AskTool::execute:
  bridge.set(PendingAsk { questions: Vec<Question>, responder })   ← 전체
  rx.await                                                          ← 전체 응답

App poll loop:
  bridge.try_take() → AskOverlay::new(all_questions, responder)
  AskOverlay: 내부적으로 current 인덱스로 순차, ←/→, 멀티 토글
              994줄 (오케스트레이션 + 렌더 + 입력 전부)
```

### 목표 (도구 구동, 한 질문 = 한 오버레이)
```
AskTool::execute:
  loop over questions:
    bridge.set(PendingAsk { question: ONE, context, responder })
    resp = rx.await
    if Back: idx--; re-push with initial_selection
    if Forward/answer: idx++; store answer
    if cancelled: abort
  format all answers

App poll loop (변경 없음):
  bridge.try_take() → SelectorOverlay::new(state_for_one_question, responder)
  SelectorOverlay: 범용, 한 질문만 표시
                   Layer 1 ListSelectorState에 렌더/입력 위임
```

### 이점
1. **AskOverlay 994줄 삭제** → 도구 루프 ~40줄 + SelectorOverlay 설정 ~30줄.
2. **←/→ 사전 채움** 자연스러움 — 도구가 `initial_selection`을 브리지에 실어 보냄.
3. **순차 흐름이 도구에 있음** — omp와 동일 구조. 오버레이는 순수 선택기.
4. **멀티 토글 루프** — SelectorOverlay 내부에서 Done/Other 처리. 도구는 Done 응답을 받으면 다음 질문으로.

### 리스크
- **브리지 페이로드 변경** (`Vec<Question>` → 단일 `Question`) — app.rs 폴링 + 테스트 수정.
- **←/→ 시 오버레이 닫힘/재생성** — 한 질문에서 ← 누르면 오버레이가 닫히고(nav=Back 응답), 도구가 이전 질문을 재생성. 시각적으로 깜빡임 가능 → 하지만 omp도 동일 동작(각 질문 = 별도 셀렉터 호출).
- **기존 ask.rs 테스트 전면 재작성** — 7개 단위 테스트가 AskOverlay 구조에 결합.

---

## 8. Phase 5 상세 — 기존 오버레이 마이그레이션

### 마이그레이션 패턴 (각 오버레이 공통)

```rust
// Before: 150-874줄 (OverlayComponent 직접 구현)
impl OverlayComponent for ModelSelectOverlay {
    fn handle_key(&mut self, key) { /* 50줄 커서/필터 로직 */ }
    fn render(&mut self, f, area, theme) { /* 80줄 List/Paragraph/popup */ }
    fn hint(&self) -> &str { " ↑↓ navigate  enter select  esc cancel" }
}

// After: 15-60줄 (팩토리 함수)
pub fn model_select(models: Vec<String>, on_select: ...) -> SelectorOverlay {
    let state = ListSelectorState::new("Select Model".into(), options);
    SelectorOverlay::new(state, vec![], responder)
}
```

### provider_select.rs 특이점 (874줄)
이 오버레이는 **다단계** (provider 선택 → API 키 입력 → 모델 선택). Phase 3의 도구 구동 순차 패턴과 유사. SelectorOverlay를 여러 번 재생성하거나, `Custom` 제어 행으로 단계를 표현. 별도 분석 필요.

---

## 9. 리스크 & 완화

| 리스크 | 완화 |
|--------|------|
| ListSelectorState가 모든 오버레이의 요구를 충족 못 함 | `Custom` 제어 행 + `SelectorAction` 확장으로 도메인별 액션 수용 |
| provider_select 다단계 흐름이 단일 셀렉터에 안 맞음 | Phase 5c를 별도로 분리, 필요시 SelectorOverlay 체이닝 |
| 기존 오버레이 테스트 전면 재작성 | 각 Phase마다 기존 테스트를 새 구조로 이식 후 삭제 |
| bridge 단일 질문 변경이 기존 세션 호환성 깨기 | 세션 직렬화에 bridge 상태가 없으므로 영향 없음 (bridge는 런타임 전용) |
| ←/→ 시 오버레이 재생성 깜빡임 | ratatui 더블 버퍼링으로 프레임 간 깜빡임 최소화; omp도 동일 동작 |

---

## 10. 수용 기준 (전체 완료 시)

- [ ] `ListSelectorState` 단위 테스트: 마커 3종, compact 전환, 퍼지 필터, disabled 스킵, 타임아웃
- [ ] `SelectorOverlay` 통합 테스트: 단일/다중/Other/Done/nav/cancel/timeout 전 시나리오
- [ ] ask 도구: 순차 흐름, ←/→ 사전 채움, 멀티 토글, Other 편집 — 기존 7 테스트 이상
- [ ] model_select/resume_select/fork_select/logout_select/model_select_inline 각각 동작 확인
- [ ] provider_select 다단계 흐름 유지
- [ ] net 줄 수 -1500 이상 (측정 가능)
- [ ] `cargo nextest run --workspace` 통과
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` (todo.rs 기존 버그 제외) 통과
- [ ] `cargo fmt --check` 통과

---

## 부록 — omp와의 구조 대응

| omp (TypeScript) | oxi (Rust) 목표 |
|---|---|
| `HookSelectorComponent` (660줄, Container 서브클래스) | `ListSelectorState` (oxi-tui, 무상태 위젯) |
| `ExtensionUiController.showHookSelector` | `SelectorOverlay` (OverlayComponent 래퍼) |
| `ui.select()` / `ui.editor()` 원시 | `SelectorOverlay::new()` + `ControlRow::Other` |
| `askSingleQuestion` + 도구 루프 | `AskTool::execute` 루프 + bridge 단일 질문 |
| `AskToolDetails` (구조화된 결과) | `AgentToolResult.metadata` → (Layer 4) 파이프라인 |
| `selectionMarker: radio\|checkbox` | `SelectorMarker::Radio\|Checkbox` |
| `checkedIndices` + `markableCount` | `checked: HashSet<usize>` + `markable_count` |
| `onLeft`/`onRight` 콜백 | `SelectorAction::NavBack/NavForward` |
| `framedBlock` + `renderStatusLine` | 기존 `chat/render.rs` 프레임 재사용 |
