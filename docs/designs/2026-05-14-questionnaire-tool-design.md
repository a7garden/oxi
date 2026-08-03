# Design: Questionnaire Tool for oxicode

**Date:** 2026-05-14
**Status:** Final — Approved
**Author:** oxicode team

---

## 1. Overview

AI 코딩 에이전트가 작업 중 사용자의 결정이 필요할 때(예: 기술 스택 선택, 아키텍처 방향 결정, 설정 옵션 선택 등) 대화형 질문 UI를 표시하는 도구. pi-agent의 `questionnaire` 확장과 동일한 UX를 oxicode에 내장 도구로 제공.

### Goals

- LLM이 도구 호출로 사용자에게 질문을 던질 수 있게 함
- 단일/다중 질문, 객관식/주관식, 다중 선택 지원
- TUI 오버레이로 질문 렌더링, 사용자 응답을 도구 결과로 반환
- 기존 agent tool, overlay, event 아키텍처에 자연스럽게 통합

### Non-Goals

- CLI 싱글샷 모드 지원 (TUI 전용)
- WASM 확장으로의 분리 (내장 도구로 구현)
- 네트워크 기반 원격 질의 (로컬 TUI만)
- 조건부 분기 (`showIf`) — v2에서 검토. LLM이 여러 번 `questionnaire`를 호출하는 sequential 방식으로 충분

---

## 2. User Flows

### 2.1 Single Question (간단 선택)

```
AI: "이 프로젝트에 어떤 프레임워크를 사용할까요?"
┌──────────────────────────────────────────────┐
│  ? 이 프로젝트에 어떤 프레임워크를 사용할까요?   │
│                                              │
│  > 1. Actix-web                              │
│    2. Axum                                   │
│    3. Warp                                   │
│    4. Type something...                      │
│                                              │
│  ↑↓ navigate • Enter select • Esc cancel    │
└──────────────────────────────────────────────┘
```

### 2.2 Multi-Question (탭 UI)

```
AI: "설정을 확인해주세요"
┌──────────────────────────────────────────────┐
│  ← [■ Language] [□ Database] [□ Auth] ✓ Submit →  │
│                                              │
│  ? 프로그래밍 언어를 선택하세요                │
│                                              │
│  > 1. Rust                                   │
│    2. TypeScript                             │
│    3. Go                                     │
│    4. Python                                 │
│                                              │
│  Tab/←→ navigate • ↑↓ select • Enter confirm│
└──────────────────────────────────────────────┘
```

### 2.3 Free Text (allowOther)

```
옵션 선택 시 "Type something..." 선택 → 인라인 에디터로 전환
│  Your answer: _____________________________ │
│  Enter to submit • Esc to cancel            │
```

### 2.4 Multi-Select (복수 선택)

```
│  ? 지원할 플랫폼을 선택하세요 (Space로 선택/해제)  │
│                                              │
│  ☑ 1. macOS                                  │
│  ☑ 2. Linux                                  │
│  ☐ 3. Windows                                │
│  ☐ 4. Web (WASM)                             │
│                                              │
│  Space toggle • Enter confirm • Esc cancel   │
```

---

## 3. Architecture

### 3.1 Data Flow

```
LLM tool_call(questionnaire)
    │
    ▼
┌─────────────────────────────────────────────────────┐
│                   Agent Thread                       │
│                                                     │
│  QuestionnaireTool::execute()                       │
│    1. parse params → Vec<Question>                  │
│    2. create oneshot channel (tx, rx)               │
│    3. bridge.set(questions, tx)                     │
│    4. tokio::select! {                              │
│       rx.await  ─── user answered ──► build result  │
│       signal    ─── Ctrl+C abort ───► cancelled     │
│     }                                               │
└─────────────────────┬───────────────────────────────┘
                      │ Arc<QuestionnaireBridge>
                      │ (shared between threads)
┌─────────────────────▼───────────────────────────────┐
│                   TUI Thread                         │
│                                                     │
│  Main loop (app.rs while running)                   │
│    1. check: overlay inactive && bridge.has_pending │
│    2. bridge.try_take() → PendingQuestionnaire      │
│    3. create QuestionnaireOverlay                   │
│    4. state.overlay_state = Some(overlay)           │
│                                                     │
│  QuestionnaireOverlay::handle_key()                 │
│    → user selects options                           │
│    → on Submit: responder.send(QuestionnaireResponse)│
│    → on Esc:    responder.send(cancelled)            │
│    → return OverlayAction::Close                    │
└─────────────────────────────────────────────────────┘
```

### 3.2 Bridge Plumbing — 생성 및 주입 흐름

Bridge는 `oxicode-cli`에서 생성되어 `Arc`로 양쪽에 전달됩니다.

```
                    oxicode-cli (lib.rs / main.rs)
                           │
                   Arc<QuestionnaireBridge>::new()
                           │
               ┌───────────┴───────────┐
               │                       │
    QuestionnaireTool             AppState
    (→ ToolRegistry)          (→ TUI main loop)
```

**주입 경로 — 상세 코드:**

```rust
// ── oxicode-cli/src/lib.rs — App::new() 또는 초기화 시점 ──

impl App {
    pub fn new(/* ... */) -> Result<Self> {
        // ... existing setup ...

        let questionnaire_bridge = Arc::new(
            oxicode_agent::tools::questionnaire::QuestionnaireBridge::new()
        );

        // Tool에 bridge 주입 (ToolRegistry에 등록)
        let tools = self.agent.tools();
        tools.register_arc(Arc::new(
            oxicode_agent::tools::questionnaire::QuestionnaireTool::new(
                questionnaire_bridge.clone()
            )
        ));

        // App에 bridge 보관 (TUI로 전달용)
        self.questionnaire_bridge = Some(questionnaire_bridge);

        Ok(Self { /* ... */ })
    }
}

// ── oxicode-cli/src/tui/app.rs — run_tui_interactive_impl() ──

async fn run_tui_interactive_impl(app: crate::App, resume_last: bool) -> Result<()> {
    let tools = app.agent().tools();
    let questionnaire_bridge = app.questionnaire_bridge().cloned();
    // ... existing setup ...

    // AppState에 bridge 전달
    let mut state = AppState::new();
    state.questionnaire_bridge = questionnaire_bridge;

    // 메인 루프 안에서 폴링
    while running {
        // ... existing rendering + event handling ...

        // Questionnaire bridge 폴링
        if state.overlay.is_none() && state.overlay_state.is_none() {
            if let Some(bridge) = &state.questionnaire_bridge {
                if let Some(pending) = bridge.try_take() {
                    state.overlay_state = Some(Box::new(
                        crate::tui::overlay::questionnaire::QuestionnaireOverlay::new(
                            pending.questions,
                            pending.responder,
                        )
                    ));
                }
            }
        }
    }
}
```

### 3.3 Core Types

#### 3.3.1 `QuestionnaireBridge` (oxicode-agent)

```rust
// oxicode-agent/src/tools/questionnaire.rs

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use parking_lot::Mutex;

/// Shared bridge between the questionnaire tool (agent thread)
/// and the TUI overlay (main thread).
///
/// Created in oxicode-cli, injected into both QuestionnaireTool and AppState.
pub struct QuestionnaireBridge {
    pending: Mutex<Option<PendingQuestionnaire>>,
}

impl QuestionnaireBridge {
    pub fn new() -> Self {
        Self { pending: Mutex::new(None) }
    }

    /// Store a pending questionnaire. Called by QuestionnaireTool::execute.
    /// Returns false if another questionnaire is already pending (should not happen
    /// in sequential tool execution, but guards against races).
    pub fn set(&self, pending: PendingQuestionnaire) -> bool {
        let mut lock = self.pending.lock();
        if lock.is_some() { return false; }
        *lock = Some(pending);
        true
    }

    /// Try to take the pending questionnaire. Called by TUI main loop polling.
    /// Returns None if nothing is pending or already taken.
    pub fn try_take(&self) -> Option<PendingQuestionnaire> {
        self.pending.lock().take()
    }
}

/// A pending questionnaire waiting for user interaction.
pub struct PendingQuestionnaire {
    pub questions: Vec<Question>,
    pub responder: tokio::sync::oneshot::Sender<QuestionnaireResponse>,
}

/// A single question to ask the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default = "default_true")]
    pub allow_other: bool,
    #[serde(default)]
    pub multi_select: bool,
}

fn default_true() -> bool { true }

/// An option within a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// Response from user interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionnaireResponse {
    pub answers: Vec<Answer>,
    pub cancelled: bool,
}

/// A single answer to a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub id: String,
    pub value: String,
    pub label: String,
    pub was_custom: bool,
    pub index: Option<usize>,
}
```

#### 3.3.2 `QuestionnaireTool` (oxicode-agent)

```rust
pub struct QuestionnaireTool {
    bridge: Arc<QuestionnaireBridge>,
}

impl QuestionnaireTool {
    pub fn new(bridge: Arc<QuestionnaireBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl AgentTool for QuestionnaireTool {
    fn name(&self) -> &str { "questionnaire" }
    fn label(&self) -> &str { "Questionnaire" }
    fn description(&self) -> &str {
        "Ask the user one or more questions. Use for clarifying requirements, \
         getting preferences, or confirming decisions. For single questions, \
         shows a simple option list. For multiple questions, shows a tab-based \
         interface."
    }
    fn parameters_schema(&self) -> Value { /* JSON Schema — Section 4 참조 */ }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        // 1. 파싱 + 검증
        let questions = parse_questions(&params)?;

        // 2. oneshot 채널 생성
        let (tx, rx) = tokio::sync::oneshot::channel();

        // 3. 브릿지에 저장 → TUI가 폴링으로 감지
        if !self.bridge.set(PendingQuestionnaire {
            questions,
            responder: tx,
        }) {
            return Ok(AgentToolResult::error(
                "Another questionnaire is already pending"
            ));
        }

        // 4. TUI 응답 대기 — abort signal과 select
        let result = tokio::select! {
            response = rx => {
                match response {
                    Ok(resp) => {
                        if resp.cancelled {
                            Ok(AgentToolResult::success(
                                "User cancelled the questionnaire"
                            ))
                        } else {
                            Ok(AgentToolResult::success(format_answers(&resp.answers)))
                        }
                    }
                    Err(_) => {
                        // oneshot Sender dropped (overlay closed without sending)
                        Ok(AgentToolResult::success(
                            "Questionnaire dismissed"
                        ))
                    }
                }
            }
            _ = await_abort_signal(signal) => {
                // Agent aborted (Ctrl+C) — clean up pending
                self.bridge.try_take(); // drop the pending, which drops tx
                Ok(AgentToolResult::success(
                    "Questionnaire cancelled by user interrupt"
                ))
            }
        };

        result
    }
}

/// Helper: await the abort signal, or pending forever if no signal provided.
async fn await_abort_signal(signal: Option<tokio::sync::oneshot::Receiver<()>>) {
    if let Some(mut sig) = signal {
        let _ = sig.await;
    } else {
        std::future::pending::<()>().await;
    }
}
```

#### 3.3.3 `QuestionnaireOverlay` (oxicode-cli)

```rust
// oxicode-cli/src/tui/overlay/questionnaire.rs

use super::{OverlayAction, OverlayComponent};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxicode_agent::tools::questionnaire::{Answer, Question, QuestionnaireResponse};
use ratatui::{Frame, layout::Rect};
use oxicode_tui::Theme;
use std::collections::HashMap;

pub struct QuestionnaireOverlay {
    questions: Vec<Question>,
    current_tab: usize,           // 현재 활성 탭 (0..questions.len()), len=Submit 탭
    option_cursor: usize,         // 현재 옵션 커서 위치
    selected_indices: HashMap<usize, Vec<usize>>, // multi_select: tab_idx → selected option indices
    answers: HashMap<String, Answer>,
    input_mode: bool,             // "Type something" 인라인 에디터 활성
    input_text: String,           // 인라인 에디터 텍스트
    responder: tokio::sync::oneshot::Sender<QuestionnaireResponse>,
}

impl QuestionnaireOverlay {
    pub fn new(
        questions: Vec<Question>,
        responder: tokio::sync::oneshot::Sender<QuestionnaireResponse>,
    ) -> Self {
        // questions가 비어있으면 즉시 빈 응답 전송
        if questions.is_empty() {
            let _ = responder.send(QuestionnaireResponse {
                answers: vec![],
                cancelled: false,
            });
            // 주의: 빈 questions로 생성되면 안 됨 (Tool에서 검증)
        }

        Self {
            questions,
            current_tab: 0,
            option_cursor: 0,
            selected_indices: HashMap::new(),
            answers: HashMap::new(),
            input_mode: false,
            input_text: String::new(),
            responder,
        }
    }

    fn is_multi(&self) -> bool {
        self.questions.len() > 1
    }

    fn submit_tab_index(&self) -> usize {
        self.questions.len()
    }

    fn total_tabs(&self) -> usize {
        self.questions.len() + 1  // questions + Submit
    }

    fn current_question(&self) -> Option<&Question> {
        self.questions.get(self.current_tab)
    }

    /// 현재 질문의 표시 옵션 (allow_other 포함)
    fn current_options(&self, q: &Question) -> Vec<RenderOption> {
        let mut opts: Vec<RenderOption> = q.options.iter().map(|o| RenderOption {
            value: o.value.clone(),
            label: o.label.clone(),
            description: o.description.clone(),
            is_other: false,
        }).collect();
        if q.allow_other {
            opts.push(RenderOption {
                value: "__other__".to_string(),
                label: "Type something...".to_string(),
                description: None,
                is_other: true,
            });
        }
        opts
    }

    fn all_answered(&self) -> bool {
        self.questions.iter().all(|q| self.answers.contains_key(&q.id))
    }

    fn submit(&mut self, cancelled: bool) {
        let answers: Vec<Answer> = self.answers.drain().map(|(_, v)| v).collect();
        let _ = self.responder.send(QuestionnaireResponse { answers, cancelled });
    }

    fn save_answer(
        &mut self,
        question_id: String,
        value: String,
        label: String,
        was_custom: bool,
        index: Option<usize>,
    ) {
        self.answers.insert(question_id, Answer {
            id: /* 복원 불가: drained */ question_id.clone(),
            value,
            label,
            was_custom,
            index,
        });
    }

    fn advance_after_answer(&mut self) {
        if !self.is_multi() {
            // 단일 질문 → 즉시 제출
            self.submit(false);
            return;
        }
        // 다음 미답변 탭으로 이동, 없으면 Submit
        for i in (self.current_tab + 1)..self.total_tabs() {
            if i == self.submit_tab_index() || !self.answers.contains_key(&self.questions[i].id) {
                self.current_tab = i;
                self.option_cursor = 0;
                return;
            }
        }
        self.current_tab = self.submit_tab_index();
        self.option_cursor = 0;
    }
}

struct RenderOption {
    value: String,
    label: String,
    description: Option<String>,
    is_other: bool,
}

impl std::fmt::Debug for QuestionnaireOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionnaireOverlay")
            .field("questions", &self.questions.len())
            .field("current_tab", &self.current_tab)
            .field("input_mode", &self.input_mode)
            .finish()
    }
}

impl OverlayComponent for QuestionnaireOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        // ── Input mode (allowOther 에디터) ──
        if self.input_mode {
            return self.handle_input_mode_key(key);
        }

        // ── Tab navigation (multi-question) ──
        if self.is_multi() {
            match key.code {
                KeyCode::Tab | KeyCode::Right => {
                    self.current_tab = (self.current_tab + 1) % self.total_tabs();
                    self.option_cursor = 0;
                    return OverlayAction::None;
                }
                KeyCode::BackTab | KeyCode::Left => {
                    self.current_tab = (self.current_tab + self.total_tabs() - 1) % self.total_tabs();
                    self.option_cursor = 0;
                    return OverlayAction::None;
                }
                _ => {}
            }
        }

        // ── Submit tab ──
        if self.current_tab == self.submit_tab_index() {
            match key.code {
                KeyCode::Enter if self.all_answered() => {
                    self.submit(false);
                    return OverlayAction::Close;
                }
                KeyCode::Esc => {
                    self.submit(true);
                    return OverlayAction::Close;
                }
                _ => return OverlayAction::None,
            }
        }

        // ── Question tab ──
        let q = match self.current_question() {
            Some(q) => q.clone(), // clone to avoid borrow issues
            None => return OverlayAction::None,
        };
        let opts = self.current_options(&q);
        let q_id = q.id.clone();

        match key.code {
            KeyCode::Up => {
                self.option_cursor = self.option_cursor.saturating_sub(1);
            }
            KeyCode::Down => {
                if !opts.is_empty() {
                    self.option_cursor = (self.option_cursor + 1).min(opts.len() - 1);
                }
            }
            KeyCode::Enter => {
                if let Some(opt) = opts.get(self.option_cursor) {
                    if opt.is_other {
                        self.input_mode = true;
                        self.input_text.clear();
                    } else if q.multi_select {
                        self.toggle_multi_select(self.current_tab, self.option_cursor, &q, &opt);
                    } else {
                        self.save_answer(
                            q_id,
                            opt.value.clone(),
                            opt.label.clone(),
                            false,
                            Some(self.option_cursor + 1),
                        );
                        self.advance_after_answer();
                    }
                }
            }
            KeyCode::Char(' ') if q.multi_select => {
                if let Some(opt) = opts.get(self.option_cursor) {
                    if !opt.is_other {
                        self.toggle_multi_select(self.current_tab, self.option_cursor, &q, &opt);
                    }
                }
            }
            KeyCode::Esc => {
                self.submit(true);
                return OverlayAction::Close;
            }
            _ => {}
        }

        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        // Section 3.4 참조 — 렌더링 상세
    }

    fn hint(&self) -> &str {
        if self.input_mode {
            " Enter submit • Esc cancel"
        } else if self.is_multi() {
            " Tab/←→ navigate • ↑↓ select • Enter confirm • Esc cancel"
        } else {
            " ↑↓ navigate • Enter select • Esc cancel"
        }
    }
}

impl QuestionnaireOverlay {
    fn handle_input_mode_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Enter => {
                let q = match self.current_question() {
                    Some(q) => q.clone(),
                    None => return OverlayAction::None,
                };
                let text = if self.input_text.trim().is_empty() {
                    "(no response)".to_string()
                } else {
                    self.input_text.trim().to_string()
                };
                self.save_answer(q.id, text.clone(), text, true, None);
                self.input_mode = false;
                self.input_text.clear();
                self.advance_after_answer();
            }
            KeyCode::Esc => {
                self.input_mode = false;
                self.input_text.clear();
            }
            KeyCode::Backspace => {
                self.input_text.pop();
            }
            KeyCode::Char(c) => {
                self.input_text.push(c);
            }
            _ => {}
        }
        OverlayAction::None
    }

    fn toggle_multi_select(
        &mut self,
        tab_idx: usize,
        opt_idx: usize,
        q: &Question,
        opt: &RenderOption,
    ) {
        let selected = self.selected_indices.entry(tab_idx).or_default();
        if let Some(pos) = selected.iter().position(|&i| i == opt_idx) {
            selected.remove(pos);
        } else {
            selected.push(opt_idx);
            selected.sort();
        }

        // Update the answer from all selected options
        if selected.is_empty() {
            self.answers.remove(&q.id);
        } else {
            let values: Vec<String> = selected.iter()
                .filter_map(|&i| q.options.get(i))
                .map(|o| o.value.clone())
                .collect();
            let labels: Vec<String> = selected.iter()
                .filter_map(|&i| q.options.get(i))
                .map(|o| o.label.clone())
                .collect();
            self.answers.insert(q.id.clone(), Answer {
                id: q.id.clone(),
                value: values.join(", "),
                label: labels.join(", "),
                was_custom: false,
                index: Some(selected[0] + 1), // first selected index
            });
        }
    }
}
```

**키 바인딩:**

| 키 | 동작 |
|----|------|
| `↑` / `↓` | 옵션 탐색 |
| `Enter` | 옵션 선택 / 입력 확정 / Submit |
| `Space` | multiSelect 토글 (question 탭에서) |
| `Tab` / `→` | 다음 탭 (다중 질문) |
| `Shift+Tab` / `←` | 이전 탭 |
| `Esc` | 취소 / 입력 모드 종료 |
| `Backspace` | 입력 모드에서 글자 삭제 |
| `Char(c)` | 입력 모드에서 글자 입력 |

**OverlayComponent 트레이드오프:**

`QuestionnaireOverlay`는 `OverlayAction` enum을 사용하지 않고,
`responder` oneshot 채널로 직접 응답을 전송합니다.
이것은 기존 패턴(overlay action → app handler)과 다른 사이드 채널이지만,
`OverlayAction`에 설문 응답 액션을 추가하는 것은 불필요한 결합을 만듭니다.
`responder`가 overlay 소유권 안에 캡슐화되어 있어 클린합니다.

### 3.4 Rendering

QuestionnaireOverlay의 `render()`는 기존 `render.rs`의 패턴을 따릅니다:

```
┌──────────────────────────────────────────────┐  ← Clear + dimmed bg + border
│  ────────────────────────────────────────     │  ← accent separator
│  ← [■ Language] [□ Database] [✓ Submit] →    │  ← tab bar (multi only)
│                                              │
│  ? 프로그래밍 언어를 선택하세요                │  ← prompt
│                                              │
│  > 1. Rust                                   │  ← options (cursor highlighted)
│    2. TypeScript                             │
│    3. Go                                     │  ← description (muted, below label)
│       Fast, simple, reliable                 │
│    4. Type something...                      │  ← allowOther
│                                              │
│  Tab/←→ • ↑↓ • Enter • Esc                  │  ← hint bar (muted)
│  ────────────────────────────────────────     │  ← accent separator
└──────────────────────────────────────────────┘
```

**Submit 탭:**
```
│  ✓ Ready to submit                           │  ← accent + bold
│                                              │
│  Language: Rust                              │  ← answered items
│  Database: PostgreSQL                        │
│  Auth: (wrote) We'll use JWT...              │  ← was_custom 표시
│                                              │
│  Press Enter to submit                       │  ← success / warning
```

**인라인 에디터 (allowOther 활성):**
```
│  ? 프로젝트 이름을 입력하세요                  │
│                                              │
│  > 1. Option A                               │  ← options for reference
│    2. Option B                               │
│    3. Type something... ✎                    │  ← 편집 중 표시
│                                              │
│  Your answer: my project name_               │  ← cursor block
│                                              │
│  Enter to submit • Esc to cancel             │
```

렌더링은 `render.rs`의 기존 헬퍼(`centered_popup`, `render_popup_frame`,
`render_selectable_list`, `render_title`, `render_hint`)를 재사용합니다.

### 3.5 Tool Call / Tool Result 렌더링

기존 chat view의 tool 렌더링(`tool_renderer.rs`)에 questionnaire 전용 렌더링 추가:

**Tool call (요청):**
```
📋 questionnaire  2 questions (Language, Database)
```

**Tool result (성공):**
```
✓ Language: 1. Rust
✓ Database: 2. PostgreSQL
✓ Auth: (wrote) We'll use JWT with refresh tokens
```

**Tool result (취소):**
```
⚠ Cancelled
```

이것은 `ToolRenderer` trait에 `questionnaire` 케이스를 추가하거나,
tool name 기반 분기에서 처리합니다.

---

## 4. JSON Schema

### 4.1 Tool Parameters

```json
{
  "type": "object",
  "properties": {
    "questions": {
      "type": "array",
      "description": "Questions to ask the user",
      "items": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "description": "Unique identifier for this question"
          },
          "label": {
            "type": "string",
            "description": "Short contextual label for tab bar (defaults to Q1, Q2)"
          },
          "prompt": {
            "type": "string",
            "description": "The full question text to display"
          },
          "options": {
            "type": "array",
            "description": "Available options to choose from. Can be empty when allowOther is true for free-text questions.",
            "default": [],
            "items": {
              "type": "object",
              "properties": {
                "value": {
                  "type": "string",
                  "description": "The value returned when selected"
                },
                "label": {
                  "type": "string",
                  "description": "Display label for the option"
                },
                "description": {
                  "type": "string",
                  "description": "Optional description shown below label"
                }
              },
              "required": ["value", "label"]
            }
          },
          "allowOther": {
            "type": "boolean",
            "description": "Allow 'Type something' option (default: true)",
            "default": true
          },
          "multiSelect": {
            "type": "boolean",
            "description": "Allow multiple selections with Space toggle (default: false)",
            "default": false
          }
        },
        "required": ["id", "prompt"]
      }
    }
  },
  "required": ["questions"]
}
```

**설계 의사결정:** `options`는 required가 아닙니다.
`allowOther: true` + 빈 `options` = 순수 자유 응답 질문을 지원합니다.

### 4.2 Tool Result Format

**성공 시 (단일 답변):**
```
Language: user selected: 1. Rust
```

**성공 시 (다중 답변):**
```
Language: user selected: 1. Rust
Database: user selected: 2. PostgreSQL
Auth: user wrote: We'll use JWT with refresh tokens
Platforms: user selected: 1. macOS, 2. Linux
```

**취소 시:**
```
User cancelled the questionnaire
```

**Interrupt 시:**
```
Questionnaire cancelled by user interrupt
```

---

## 5. Conditional Branching Strategy

### v1: AI-Sequential Only

LLM이 복잡한 분기 로직을 처리합니다:

```
1st call: questionnaire({ questions: [{ id: "project_type", ... }] })
→ user selects "web"

2nd call: questionnaire({ questions: [
    { id: "frontend_framework", ... },
    { id: "state_management", ... }
]})
→ user answers both

3rd call (if needed): follow-up questions based on previous answers
```

**이유:**
- LLM은 JSON 조건문보다 자연어 추론에 능함
- 구현 복잡도 0 (추가 코드 없음)
- pi-agent도 이 방식만 사용
- LLM이 문맥을 더 잘 이해하고 적절한 후속 질문을 구성

### v2 (future): showIf 조건부 분기

추후 필요 시 `showIf` 필드를 추가하여 단일 questionnaire 내에서 조건부 분기 지원.
이 경우 동적 재평가(showIf 재계산 → 탭 추가/제거 → 답변 정리)가 필요.

---

## 6. Implementation Phases

### Phase 0: Plumbing

**목표:** `cargo build` 성공, bridge 생성/주입 흐름 확립

| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/tools/questionnaire.rs` | **신규** — types + Bridge + Tool 골격 (execute는 빈 응답 반환) |
| `oxicode-agent/src/tools.rs` | `pub mod questionnaire;` 추가 (with_builtins에는 아직 등록 안 함) |
| `oxicode-cli/src/lib.rs` | App에 `questionnaire_bridge: Option<Arc<...>>` 필드 + 접근자 |
| `oxicode-cli/src/tui/app.rs` | AppState에 `questionnaire_bridge` 필드 추가 |

**완료 기준:** `cargo build` 성공

### Phase 1: Single Question

**목표:** 단일 객관식 질문 E2E 동작

| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/tools/questionnaire.rs` | parse_questions, format_answers, Tool execute (full) |
| `oxicode-agent/src/tools.rs` | with_builtins에 QuestionnaireTool 등록 |
| `oxicode-cli/src/lib.rs` | App::new에서 bridge 생성 → Tool 등록 → 필드 저장 |
| `oxicode-cli/src/tui/overlay/questionnaire.rs` | **신규** — 단일 질문 오버레이 (옵션 탐색, 선택, 취소) |
| `oxicode-cli/src/tui/overlay/mod.rs` | `pub mod questionnaire;` + factory 함수 |
| `oxicode-cli/src/tui/app.rs` | 메인 루프에 bridge 폴링 추가 |

**완료 기준:** LLM이 `questionnaire` 호출 → TUI에 옵션 목록 표시 → 선택 → 결과 반환

### Phase 2: Multi-Question + Tabs

**목표:** 다중 질문 탭 UI + Submit + allowOther

| 파일 | 변경 |
|------|------|
| `oxicode-cli/src/tui/overlay/questionnaire.rs` | 탭 바 렌더링, Submit 탭, 답변 상태 추적, 인라인 에디터 |

**완료 기준:** 다중 질문에서 탭 네비게이션, 모든 질문 답변 후 Submit

### Phase 3: multiSelect

**목표:** Space 토글 복수 선택 + 복수 답변 포맷

| 파일 | 변경 |
|------|------|
| `oxicode-cli/src/tui/overlay/questionnaire.rs` | Space 토글, ☑/☐ 렌더링, 복수 답변 저장 |
| `oxicode-agent/src/tools/questionnaire.rs` | format_answers에 multiSelect 결과 포맷 추가 |

**완료 기준:** multiSelect 질문에서 여러 옵션 선택, 결과에 모든 선택 포함

---

## 7. Key Design Decisions

### 7.1 Bridge Pattern vs Event-Only

**결정:** Bridge pattern (`Arc<QuestionnaireBridge>` + oneshot 채널)

**이유:**
- 기존 AgentEvent는 `Serialize + Clone`이 필요 → `oneshot::Sender`는 직렬화 불가
- 이벤트 파이프라인은 단방향 (agent → TUI) → 역방향 채널이 필요
- Bridge는 양방향 통신을 캡슐화하면서 기존 아키텍처를 침범하지 않음

### 7.2 Bridge 생성 위치: oxicode-cli

**결정:** Bridge는 `oxicode-cli`에서 생성, `Arc`로 Tool과 AppState에 각각 주입

**이유:**
- Agent 루프는 별도 스레드에서 실행 (`std::thread::spawn` in `app.rs`)
- TUI 메인 루프는 또 다른 스레드
- 둘 다 bridge에 접근해야 하므로, 생성자(App)에서 Arc로 분배
- `oxicode-agent`는 bridge 타입만 정의, 인스턴스화는 `oxicode-cli`가 담당
- `with_builtins`에 questionnaire를 포함하지 않고, `App::new`에서 개별 등록

### 7.3 Tool Blocking: tokio::select!

**결정:** `tokio::select!`로 oneshot response와 abort signal을 동시에 대기

```rust
tokio::select! {
    response = rx => { /* user answered or overlay closed */ }
    _ = await_abort_signal(signal) => { /* Ctrl+C */ }
}
```

**이유:**
- `signal`을 무시하고 `rx.await`만 하면, Ctrl+C 시 도구가 응답할 때까지 agent 루프가 멈춤
- `select!`로 즉시 취소 가능
- `bridge.try_take()` 호출로 pending 정리 → tx drop → rx도 해제

### 7.4 Conditional Branching: AI-Sequential Only (v1)

**결정:** v1은 AI-sequential만 지원. showIf는 v2.

**이유:**
- LLM이 자연어로 분기 판단하는 것이 JSON 조건문보다 유연
- pi-agent도 showIf 없이 동작 중
- 구현 복잡도 대폭 감소
- 필요 시 v2에서 추가 가능 (역호환)

### 7.5 options는 Optional (빈 배열 허용)

**결정:** `options` 필드를 required에서 제외, 기본값 `[]`

**이유:**
- `allowOther: true` + 빈 options = 순수 자유 응답 질문
- 예: "프로젝트 이름을 입력하세요" (선택지 없이 텍스트만)
- pi-agent도 동일하게 허용

---

## 8. Resolved Questions

| # | 질문 | 결정 |
|---|------|------|
| 1 | 타임아웃 필요? | **No.** 무한 대기. 사용자가 Esc로 명시적 취소. |
| 2 | Ctrl+C abort 처리 | **tokio::select!** + signal await. bridge.try_take()로 정리. |
| 3 | 세션 복원 | **저장 안 함.** 세션에는 완료된 tool result만 기록됨. |
| 4 | WASM 노출 | **No.** LLM만 호출. v2에서 host function으로 검토. |
| 5 | OverlayComponent 사이드 채널 | **허용.** responder oneshot 채널로 직접 전송. OverlayAction 확장 불필요. |

---

## 9. Testing Strategy

### Unit Tests (cargo test)

- `parse_questions` — 유효/무효 JSON, 빈 questions, 빈 options, 기본값 적용
- `QuestionnaireBridge::set/try_take` — 정상 흐름, 이미 pending인 경우
- `format_answers` — 단일 답변, multiSelect, wasCustom 포맷

### Integration Tests

- Tool execute → bridge set → try_take → responder send → rx receive 전체 흐름
- Abort signal: tokio::select!에서 signal이 먼저 발생하는 시나리오
- 빈 questions: Tool이 에러 반환

### Manual Tests

| 시나리오 | 단계 | 기대 결과 |
|----------|------|-----------|
| 단일 질문 선택 | ↑↓ 이동 → Enter | 선택값 반환, 오버레이 닫힘 |
| 단일 질문 취소 | Esc | "cancelled" 반환 |
| allowOther 입력 | "Type something" 선택 → 텍스트 입력 → Enter | was_custom=true로 반환 |
| allowOther 취소 | "Type something" 선택 → Esc | 에디터 종료, 옵션 목록 복귀 |
| 다중 질문 탭 | Tab/Shift+Tab | 탭 전환 |
| 다중 질문 Submit | 모든 질문 답변 → Submit 탭 → Enter | 모든 답변 반환 |
| 미답변 Submit | Submit 탭에서 답변 누락 시 Enter | "Unanswered" 경고, 제출 불가 |
| multiSelect | Space 토글 | ☑/☐ 전환, 복수 선택값 반환 |
| Ctrl+C | 도구 실행 중 Ctrl+C | "interrupt" 반환, 오버레이 정리 |
