# Tool Context Transparency — 활동 투명성을 위한 구조화된 이벤트 보강

> **Status:** Design
> **Supersedes:** `2026-06-05-exploration-transparency.md` (해당 설계의 문제 인식은 유효하나, 해결책의 배치 레이어가 코드와 불일치)
> **Scope:** oxicode-agent (`events.rs`, `agent_loop/tool_exec.rs`, browse tools)
> **Depends on:** v0.27 browser observability (shipped — per-tab routing, `tab_id` on `ToolExecutionUpdate`)
> **Estimated effort:** ~200 LoC + tests. 3–4 days.

---

## 0. TL;DR

에이전트가 도구를 사용할 때, UI가 "무슨 일이 일어나고 있는지"를 **구조적으로** 이해할 수 있게 한다.

**핵심 아이디어:** `ToolExecutionUpdate` 이벤트에 `Option<ToolContext>` 필드를 추가한다. `ToolContext`는 툴 이름과 인자에서 자동 추론한 의미론적 정보다. 새 이벤트 파이프라인 없이, 기존 단일 채널에 의미를 더한다.

```
Before:  ToolExecutionUpdate { partial_result: "Opening https://github.com..." }
After:   ToolExecutionUpdate {
             partial_result: "Opening https://github.com...",
             context: PageVisit { url: "github.com/...", reason: DirectNavigation }
         }
```

구버전 UI는 `context`를 무시하고 `partial_result`만 표시. 신버전 UI는 `context`를 읽어 풍부한 렌더링. **하위 호환 완벽.**

---

## 1. 문제

### 1.1 사용자가 기대하는 것

에이전트가 "Rust headless browser 비교 정보 수집"을 할 때:

```
🔍 "Rust headless browser" 검색 중...
📄 https://github.com/.../fantoccini 열기...
📄 https://crates.io/crates/headless_chrome 열기...
📋 비교 정보 추출 중...
✅ 3개 사이트 탐색 완료 (12.4초)
```

각 단계가 **무엇을 의미하는지** 실시간으로 보여야 한다.

### 1.2 현재의 한계

v0.27에서 페이지 수명주기 이벤트는 `ToolExecutionUpdate { partial_result, tab_id }`로 끝까지 간다. 하지만:

| 한계 | 설명 |
|------|------|
| **의미가 문자열에 갇힘** | `partial_result: String`은 "Opening...", "Loaded..." 등 사실만 전달. "검색 중"인지 "페이지 방문"인지 구조적 정보가 없음 |
| **툴콜 간의 관계가 안 보임** | `web_search` → `browse` → `browse`가 하나의 탐색이라는 걸 UI가 모름. 독립된 툴콜 3개로만 보임 |
| **일부 툴이 불투명** | BrowseSessionTool (29개 액션), BrowseScriptTool (N개 스텝)이 `ToolExecutionUpdate`를 emit하지 않음 |
| **UI가 문자열을 파싱해야 함** | "Opening https://..."에서 URL을 뽑으려면 문자열 파싱. 깨지기 쉽고 비효율적 |

### 1.3 핵심 통찰

> **툴은 사실을 방출하고, 에이전트 루프가 의미를 부여한다.**

```
┌──────────────────────────────────────────────────────────┐
│ 툴 레이어 (BrowseTool, BashTool, ReadTool, ...)          │
│   "이 URL 열었어. 200 OK. 12KB."                         │
│   → 사실 보고. 의도(intent) 없음                         │
├──────────────────────────────────────────────────────────┤
│ 에이전트 루프 (tool_exec.rs)                             │
│   "이 툴콜은 browse 도구이고 url 인자가 있으니           │
│    PageVisit { url: ..., reason: DirectNavigation }다"    │
│   → 의미 보고. 왜 하는지 설명                             │
└──────────────────────────────────────────────────────────┘
```

의미는 툴 내부가 아니라 **에이전트 루프에서** 부여한다. 왜?

1. 에이전트 루프는 모든 툴콜을 통과하는 유일한 지점이다
2. 툴 이름과 인자라는 **충분한 정보**가 이미 거기에 있다
3. 툴은 자기가 "탐색"인지 "편집"인지 알 필요 없다 — 그냥 일하면 된다

---

## 2. 설계 원칙

| # | 원칙 | 이유 |
|---|------|------|
| P1 | **툴은 사실만, 루프가 의미 부여** | 툴에 의미론적 지식을 넣지 않는다. `infer_context(tool_name, args)`가 의미를 생성 |
| P2 | **하나의 이벤트 채널** | 새 이벤트 variant를 만들지 않는다. `ToolExecutionUpdate` 하나에 사실과 의미를 함께 실운다 |
| P3 | **선언적 확장, 명령적 교체 아님** | 기존 `ProgressCallback`을 교체하지 않는다. 그 위에 구조화된 레이어를 얹는다 |
| P4 | **범용 메커니즘** | "탐색 투명성"이 아니라 "활동 투명성". 모든 툴에 적용 가능한 `ToolContext` |
| P5 | **툴콜 = 스텝** | 별도의 스텝 트리를 만들지 않는다. 툴콜의 생명주기(Start → Update → End)가 곧 스텝의 생명주기 |

---

## 3. 아키텍처

### 3.1 전체 파이프라인

```
┌──────────────────────────────────────────────────────────────┐
│ 에이전트 루프 (tool_exec.rs)                                  │
│                                                              │
│  execute_prepared_tool_call()                                 │
│    │                                                         │
│    ├── infer_context(tool_name, args) → Option<ToolContext>   │
│    │     "browse" + { url: "..." } → PageVisit { url, ... }  │
│    │     "web_search" + { query }  → WebSearch { query }     │
│    │                                                         │
│    ├── on_progress(cb)  ← 툴에 콜백 전달                     │
│    │     cb = |msg| emit(ToolExecutionUpdate {                │
│    │       partial_result: msg,                               │
│    │       context: inferred_context,   ← 여기에 의미 실음   │
│    │     })                                                   │
│    │                                                         │
│    └── tool.execute(...)  ← 툴은 그냥 일함                   │
│                                                               │
└───────────────────────┬──────────────────────────────────────┘
                        │ AgentEvent stream
                        ▼
                 ┌──────────────┐
                 │  UI / TUI    │
                 │              │
                 │  context가   │
                 │  있으면 풍부하게 렌더,     │
                 │  없으면 partial_result만   │
                 └──────────────┘
```

### 3.2 데이터 모델

#### `ToolContext` — 툴콜의 의미론적 맥락

```rust
/// 툴 실행 이벤트의 의미론적 보강.
///
/// UI가 이 필드를 이해하면 풍부하게 렌더하고,
/// 이해 못하면 무시하고 기존처럼 `partial_result` 문자열만 표시한다.
/// `Option` + `skip_serializing_if`로 하위 호환 보장.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolContext {
    // ── 웹 탐색 ──────────────────────────────────────
    /// 검색 엔진에 쿼리를 보냄
    WebSearch {
        query: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        engine: Option<String>,
    },

    /// 특정 URL의 페이지를 방문
    PageVisit {
        url: String,
        /// 이 방문이 어떤 맥락에서 나왔는지
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<VisitReason>,
    },

    /// 페이지에서 특정 데이터를 추출
    DataExtraction {
        /// 추출 대상 설명 (CSS selector 등)
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },

    /// 브라우저 세션 액션 (browse_session)
    SessionAction {
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },

    /// 스크립트 스텝 진행 (browse_script)
    ScriptStep {
        current: usize,
        total: usize,
        step: String,
    },

    // ── 파일 작업 (미래 확장) ──────────────────────────
    // FileRead { path: String },
    // FileEdit { path: String },
    // FileWrite { path: String },

    // ── 셸 작업 (미래 확장) ────────────────────────────
    // Command { command: String },
}

/// 페이지 방문 이유
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisitReason {
    /// 에이전트가 직접 지정
    DirectNavigation,
    /// 검색 결과에서 클릭
    SearchResult { position: usize },
    /// 페이지 내 링크 클릭
    LinkFollowed { from_url: String },
}
```

### 3.3 `ToolExecutionUpdate` 확장

```rust
/// 단일 필드 추가. 나머지는 변경 없음.
ToolExecutionUpdate {
    tool_call_id: String,
    tool_name: String,
    partial_result: String,
    tab_id: Option<uuid::Uuid>,

    // ── NEW ───────────────────────────────────────────
    /// 이 툴콜의 의미론적 맥락.
    /// `None`이면 기존처럼 partial_result만 표시.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<ToolContext>,
}
```

### 3.4 `ToolExecutionStart`에도 context 추가

툴콜이 시작될 때도 UI가 의미를 알면 더 좋은 렌더링이 가능:

```rust
ToolExecutionStart {
    tool_call_id: String,
    tool_name: String,
    args: serde_json::Value,

    // ── NEW ───────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<ToolContext>,
}
```

---

## 4. 에이전트 루프 변경

### 4.1 `infer_context` — 순수 함수

```rust
// oxicode-agent/src/agent_loop/tool_exec.rs

/// 툴 이름과 인자로부터 의미론적 맥락을 추론.
///
/// 이 함수는 에이전트 루프에서 **유일하게** 의미를 생성하는 지점이다.
/// 툴 자체는 의미를 모른다 — 오직 루프가 안다.
fn infer_context(tool_name: &str, args: &Value) -> Option<ToolContext> {
    match tool_name {
        "web_search" => args["query"].as_str().map(|q| ToolContext::WebSearch {
            query: q.into(),
            engine: args["engines"].as_str().map(String::from),
        }),

        "browse" => args["url"].as_str().map(|u| ToolContext::PageVisit {
            url: u.into(),
            reason: Some(VisitReason::DirectNavigation),
        }),

        "browse_extract" => Some(ToolContext::DataExtraction {
            target: args["selector"]
                .as_str()
                .unwrap_or("data")
                .to_string(),
            url: args["url"].as_str().map(String::from),
        }),

        "browse_session" => Some(ToolContext::SessionAction {
            action: args["action"].as_str().unwrap_or("unknown").to_string(),
            url: args["url"].as_str().map(String::from),
        }),

        "browse_script" => {
            // 스텝 수는 args에서 알 수 없으므로, ScriptStep context는
            // BrowseScriptTool이 progress callback에서 직접 emit.
            // 여기서는 스크립트 실행 시작 정보만 제공.
            None
        }

        _ => None,
    }
}
```

### 4.2 `execute_prepared_tool_call` 수정

```rust
// 기존 코드에 2줄 추가:

let context = infer_context(&tool_name, &prepared.args);

let progress_cb: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |msg: String| {
    let tab_id = *tab_id_slot_cb.lock();
    emit_clone(AgentEvent::ToolExecutionUpdate {
        tool_call_id: tool_call_id_clone.clone(),
        tool_name: tool_name.clone(),
        partial_result: msg,
        tab_id,
        context: context.clone(),   // ← 추가
    });
});
```

`ToolExecutionStart` emit 지점에도:

```rust
emit(AgentEvent::ToolExecutionStart {
    tool_call_id: tc_id.clone(),
    tool_name: tc_name.clone(),
    args: tc_args,
    context: infer_context(&tc_name, &tc_args),   // ← 추가
});
```

---

## 5. 툴 변경

### 5.1 BrowseSessionTool, BrowseExtractTool — `on_progress` 연결

v0.27에서 이 두 툴은 `on_progress`를 구현하지 않았다. Phase 1에서 추가했다.

변경 내용 (Phase 1에서 이미 구현):
- `pending_callback` + `tab_id_slot` 필드 추가
- `on_progress` / `set_tab_id_slot` / `current_tab_id` trait 메서드 구현
- `open` 액션에서 `engine.callback_registry().set(tab_id, cb)`로 콜백 등록
- `close` 액션에서 `tab_id_slot` 정리

**이제 BrowseSessionTool의 29개 액션이 `BrowserEvent`를 받아 `ToolExecutionUpdate`로 전달한다.** `infer_context("browse_session", args)`가 `SessionAction { action: "goto", url: Some("...") }`를 생성하므로, UI는 "브라우저 세션에서 goto 액션을 수행 중"이라는 걸 구조적으로 안다.

### 5.2 BrowseScriptTool — 스텝별 진행 + `on_progress` 연결

Phase 1에서 추가한 것:
- `pending_callback` + `tab_id_slot` 필드 추가
- trait 메서드 구현
- 스텝마다 `[N/M] action_label` 형태의 progress emit

여기서 더 나아가, **context도 함께 전달**하도록 한다.

#### 방법: BrowseScriptTool이 자체 context를 emit

BrowseScriptTool은 `infer_context`에서 처리할 수 없는 동적 정보(current/total)를 가진다. 이 툴은 progress callback에서 직접 `ToolContext::ScriptStep`을 생성해야 한다.

하지만 `ProgressCallback`은 `Fn(String)` 시그니처다. context를 전달할 수 없다.

**해결: `on_structured_progress`를 연결한다.**

`tools.rs`에 이미 정의된 `StructuredProgressCallback`과 `ToolProgress` enum이 있다. 현재 `tool_exec.rs`에서 연결하지 않고 있다. 여기서 연결:

```rust
// tool_exec.rs의 execute_prepared_tool_call에서 추가:

let structured_context = context.clone();
tool.on_structured_progress(Arc::new(move |progress: ToolProgress| {
    if let ToolProgress::Status { message } = progress {
        let tab_id = *tab_id_slot_structured.lock();
        emit_structured(AgentEvent::ToolExecutionUpdate {
            tool_call_id: tool_call_id_structured.clone(),
            tool_name: tool_name_structured.clone(),
            partial_result: message,
            tab_id,
            context: structured_context.clone(),
        });
    }
}));
```

하지만 이건 복잡하다. 더 단순한 방법이 있다.

#### 더 단순한 방법: `infer_context`가 스크립트 진행도 처리

`BrowseScriptTool`이 emit하는 progress 문자열(`[3/10] Clicking element`)을 `infer_context`가 파싱하는 대신, **툴이 `AgentToolResult::metadata`에 스텝 정보를 넣고**, `tool_exec.rs`가 그걸 읽어서 context를 갱신하는 건 너무 복잡하다.

**가장 단순한 해결:** BrowseScriptTool의 progress callback에서 이미 `[3/10] action_label`을 보내므로, `infer_context`는 `browse_script`에 대해 항상 `None`을 반환한다. 대신 UI는 `partial_result` 문자열의 패턴으로 스텝 진행을 감지한다.

```
UI 측 로직:
  if tool_name == "browse_script" && partial_result.matches("[N/M]") {
    // 스텝 진행 표시
  }
```

이건 "문자열 파싱"으로 보일 수 있지만, `[N/M]`은 우리가 정의한 고정 포맷이므로 안정적이다. 그리고 `partial_result` 자체가 이미 그 용도로 설계되었다.

**결정:** Phase 1에서 구현한 대로 BrowseScriptTool은 `[N/M] label`을 progress로 보내고, `ToolContext`는 툴콜 시작 시에만 `ScriptStep` context를 제공하지 않는다. 미래에 `on_structured_progress`가 연결되면 그때 `ScriptStep { current, total, step }`을 추가한다.

---

## 6. 선행 작업 (Phase 1, 이미 구현됨)

| 작업 | 파일 | 상태 |
|------|------|------|
| BrowseSessionTool: `on_progress` + `tab_id_slot` + `current_tab_id` | `browse_session_tool.rs` | ✅ 완료 |
| BrowseSessionTool: `open`에서 callback 등록, `close`/timeout에서 정리 | 동일 | ✅ 완료 |
| BrowseExtractTool: `on_progress` + `tab_id_slot` + `current_tab_id` | `browse_extract_tool.rs` | ✅ 완료 |
| BrowseExtractTool: 탭 열 때 callback 등록, 닫을 때 정리 | 동일 | ✅ 완료 |
| BrowseScriptTool: `on_progress` + `tab_id_slot` + `current_tab_id` | `browse_script_tool.rs` | ✅ 완료 |
| BrowseScriptTool: 스텝별 `[N/M] label` progress emit | 동일 | ✅ 완료 |
| BrowseScriptTool: `step_label()` 헬퍼 | 동일 | ✅ 완료 |

이 선행 작업만으로도 모든 browse 툴이 `ToolExecutionUpdate`를 emit하게 되었다.

---

## 7. 이번 작업 (Phase 2)

### 7.1 `ToolContext` + `VisitReason` 타입 정의

**파일:** `oxicode-agent/src/events.rs`

`AgentEvent` enum 앞에 두 개의 타입을 추가.

### 7.2 `AgentEvent` variant에 `context` 필드 추가

**파일:** `oxicode-agent/src/events.rs`

`ToolExecutionStart`와 `ToolExecutionUpdate`에 `context: Option<ToolContext>` 추가.

`type_name()` 매치 암에 새 필드는 영향 없음 — variant 이름이 바뀌지 않으므로.

### 7.3 `infer_context` 함수

**파일:** `oxicode-agent/src/agent_loop/tool_exec.rs`

순수 함수. 툴 이름 + args → `Option<ToolContext>`.

### 7.4 `execute_prepared_tool_call` 수정

**파일:** `oxicode-agent/src/agent_loop/tool_exec.rs`

`infer_context` 호출 + progress callback에 context 전달.

### 7.5 sequential/parallel 실행 경로에 context 전달

**파일:** `oxicode-agent/src/agent_loop/tool_exec.rs`

`execute_tool_calls_sequential`과 `execute_tool_calls_parallel` 모두에서
`ToolExecutionStart` emit 시 context를 포함.

### 7.6 `execute_prepared_tool_call_static` (병렬 경로) 수정

**파일:** `oxicode-agent/src/agent_loop/tool_exec.rs`

병렬 실행에서는 `on_progress`가 연결되지 않는다 — `execute_prepared_tool_call_static`이
progress callback을 설정하지 않기 때문. 여기도 동일하게 context를 포함하려면
callback 설정이 필요하지만, 병렬 경로는 현재 progress emit을 지원하지 않는다.

**결정:** 병렬 경로의 `ToolExecutionStart`에만 context를 포함. `ToolExecutionUpdate`는
병렬 경로에서 emit되지 않으므로 (callback 연결 없음), 이건 기존 동작과 동일.

### 7.7 테스트

| 테스트 | 설명 |
|--------|------|
| `infer_context_web_search` | `web_search` + `{ query }` → `WebSearch` |
| `infer_context_browse` | `browse` + `{ url }` → `PageVisit` |
| `infer_context_browse_extract` | `browse_extract` + `{ url, selector }` → `DataExtraction` |
| `infer_context_browse_session` | `browse_session` + `{ action, url }` → `SessionAction` |
| `infer_context_unknown` | 알 수 없는 툴 → `None` |
| `infer_context_missing_args` | 필수 인자 누락 → `None` |
| `serde_roundtrip` | `ToolContext` 직렬화/역직렬화 |
| `serde_backward_compat` | `context` 필드 없는 JSON → `None` |

---

## 8. 파일 변경 요약

| 프로젝트 | 파일 | 액션 |
|----------|------|------|
| **oxicode-agent** | `src/events.rs` | `ToolContext`, `VisitReason` 타입 추가. `ToolExecutionStart`, `ToolExecutionUpdate`에 `context` 필드 추가 |
| | `src/agent_loop/tool_exec.rs` | `infer_context()` 함수 추가. progress callback에 context 전달. Start emit에 context 포함 |
| | `src/tools/browse/browse_session_tool.rs` | Phase 1에서 이미 완료 (callback 연결) |
| | `src/tools/browse/browse_extract_tool.rs` | Phase 1에서 이미 완료 (callback 연결) |
| | `src/tools/browse/browse_script_tool.rs` | Phase 1에서 이미 완료 (callback + 스텝 progress) |
| **oxibrowser** | *(변경 없음)* | — |
| **oxicode-sdk** | *(변경 없음 — 재export 불필요)* | — |
| **oxios-kernel** | *(변경 없음 — 기존 KernelEvent 매핑이 AgentEvent를 그대로 전달)* | — |
| **oxios-web** | *(나중에 — context 인식 렌더링)* | — |

**총: ~120 LoC 변경, 새 파일 없음.**

---

## 9. 직렬화 예시

### 9.1 WebSearch

```json
{
  "type": "toolExecutionStart",
  "toolCallId": "call_abc123",
  "toolName": "web_search",
  "args": { "query": "rust headless browser" },
  "context": {
    "kind": "web_search",
    "query": "rust headless browser"
  }
}
```

### 9.2 PageVisit (progress)

```json
{
  "type": "toolExecutionUpdate",
  "toolCallId": "call_def456",
  "toolName": "browse",
  "partialResult": "Loaded \"fantoccini — Rust\" — 200 · 12KB · 245ms",
  "tabId": "a1b2c3d4-...",
  "context": {
    "kind": "page_visit",
    "url": "https://github.com/jonhoo/fantoccini",
    "reason": "direct_navigation"
  }
}
```

### 9.3 하위 호환 (구버전 UI가 받는 JSON)

구버전 UI는 `context` 필드를 무시:

```json
{
  "type": "toolExecutionUpdate",
  "toolCallId": "call_def456",
  "toolName": "browse",
  "partialResult": "Loaded \"fantoccini — Rust\" — 200 · 12KB · 245ms",
  "tabId": "a1b2c3d4-..."
}
```

`#[serde(default)]` 덕분에 구버전 JSON을 신버전 코드가 읽어도 `context: None`.

### 9.4 툴에 context가 없는 경우 (대부분의 툴)

```json
{
  "type": "toolExecutionStart",
  "toolCallId": "call_789",
  "toolName": "bash",
  "args": { "command": "cargo test" }
}
```

`context` 필드 자체가 생략됨 (`skip_serializing_if = "Option::is_none"`).

---

## 10. `infer_context` 확장성

`ToolContext`는 `#[non_exhaustive]`이므로, 새 툴에 대한 context를 추가하는 건
`infer_context`의 match arm 하나와 `ToolContext` variant 하나를 추가하는 것으로 끝난다.

**미래 확장 예시:**

```rust
// read 툴
"read" => args["path"].as_str().map(|p| ToolContext::FileRead { path: p.into() }),

// edit 툴
"edit" => args["path"].as_str().map(|p| ToolContext::FileEdit { path: p.into() }),

// bash 툴
"bash" => args["command"].as_str().map(|c| ToolContext::Command { command: c.into() }),
```

이렇게 하면 모든 툴콜에 의미론적 맥락이 붙는다. "탐색 투명성"이 아니라 **"활동 투명성"**으로
자연스럽게 확장된다.

---

## 11. 이전 설계와의 비교

### 11.1 폐기한 것

| 이전 설계 요소 | 폐기 이유 |
|----------------|-----------|
| `ExplorationTracker` + `StepGuard` | BrowseTool 안에 배치. 실제로 BrowseTool은 단일 URL만 처리. 틀린 레이어 |
| `AgentEvent::ExplorationProgress` 새 variant | 이중 파이프라인. 기존 `ToolExecutionUpdate`로 충분 |
| `ExplorationStep` enum | 계층 트리용이지만, 실제 탐색은 여러 툴콜에 분산. 트리 구조가 실제 패턴과 불일치 |
| `parent_step_id` 트리 | 툴콜 = 스텝이므로 불필요 |
| `StepGuard` RAII + `mem::forget` | 코드베이스의 `TabGuard` 철학(명시적 close)과 충돌 |
| oxibrowser 변경 | 처음부터 불필요했음. 의미는 툴 레이어가 아닌 루프 레이어에서 생성 |
| oxicode-sdk 재export | 새 타입이 범용 `ToolContext`이므로 `AgentEvent`와 함께 이미 노출됨 |
| oxios-kernel `KernelEvent::ExplorationStep` | 기존 매핑이 `AgentEvent`를 그대로 전달하므로 불필요 |

### 11.2 가져간 것

| 이전 설계 요소 | 현재 설계에서의 역할 |
|----------------|---------------------|
| 문제 정의 (1절) | 그대로 유효. 브라우저 레벨 vs 에이전트 레벨 구분 |
| 설계 원칙 P1, P3, P4 | 그대로 채택. P2(선언적 스텝)은 "툴콜 = 스텝"으로 단순화. P5(트리)는 폐기 |
| `VisitReason` | 그대로 채택. `PageVisit`의 `reason` 필드로 사용 |
| 하위 호환 전략 | `#[non_exhaustive]`, `skip_serializing_if`, unknown 필드 무시 |
| 최종 UI 모습 (8.5절) | 같은 UX. 다만 백엔드에서 `ExplorationProgress` 대신 `ToolExecutionUpdate + context`로 구현 |

---

## 12. 리스크 분석

| 리스크 | 확률 | 영향 | 대응 |
|--------|------|------|------|
| `infer_context`가 잘못된 context를 생성 | 낮음 | 낮음 | UI는 context를 무시하고 partial_result만 표시하면 됨. worst case: 의미 없는 context가 표시되는 것뿐 |
| `ToolContext` variant가 너무 많아짐 | 낮음 | 낮음 | `#[non_exhaustive]`로 새 variant 추가가 breaking change가 아님. 필요한 것만 추가 |
| BrowseScriptTool 스텝 진행이 `context` 없이 문자열만 전달 | 중 | 낮음 | `[N/M]` 포맷이 고정되어 있어 UI 파싱이 안정적. 향후 `on_structured_progress` 연결로 개선 가능 |
| 병렬 툴 실행 시 context 누락 | 낮음 | 낮음 | 병렬 실행에서는 progress callback이 연결되지 않는 기존 한계. Start 이벤트에는 context 포함 |

---

## 13. 열린 질문

1. **`ToolContext`를 oxicode-ai로 옮길까?**
   - 현재 `AgentEvent`가 `oxicode-agent`에 정의되어 있으므로 `ToolContext`도 같은 크레이트에.
   - 하지만 미래에 다른 크레이트에서 재사용하려면 `oxicode-ai`로 옮기는 게 좋을 수 있음.
   - **추천:** 일단 `oxicode-agent`에. 필요시 이동.

2. **`ToolContext`를 툴이 직접 제공하게 할까?**
   - `AgentTool` trait에 `fn infer_context(&self, args: &Value) -> Option<ToolContext>` 추가.
   - 툴이 자기 자신의 의미를 가장 잘 안다.
   - 하지만 툴에 의미론적 지식을 넣는 건 P1(툴은 사실만)에 위배.
   - **추천:** `infer_context`를 루프에 유지. 필요시 trait 메서드로 확장.

3. **`on_structured_progress` 연결 시점?**
   - 현재 `tool_exec.rs`에서 `on_structured_progress`를 호출하지 않음.
   - 연결하면 툴이 `ToolProgress::Status { message }`를 emit할 때 구조화된 context를 함께 보낼 수 있음.
   - **추천:** 이번 Phase 2에서는 연결하지 않음. Phase 3에서 고려.
