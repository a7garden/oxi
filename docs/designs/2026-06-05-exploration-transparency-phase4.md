# Exploration Transparency — 남은 개선사항 설계서

> **Status:** ✅ Implemented
> **목표:** 기능이 완성된 현재 코드를 **구조적으로 아름다운** 상태로 다듬기
> **범위:** oxicode-agent (핵심), oxicode-sdk (re-export), oxicode-cli (호환성)
> **Tests:** 2116/2116 pass (default), 514/514 pass (native-browser), 0 clippy errors

---

## 현황

| Phase | 내용 | 상태 |
|-------|------|------|
| Phase 1 | 툴 내부 visibility (on_progress + tab_id) | ✅ 완료 |
| Phase 2 | ToolCallContext enum + infer_context | ✅ 완료 |
| Phase 3 | BrowseProgress 구조적 파이프라인 | ✅ 완료 |
| **Phase 4** | **구조적 우아함 완성** | 🔶 이 설계서 |

---

## 0. 발견된 문제 (심각도 순)

| # | 문제 | 심각도 | 분류 |
|---|------|--------|------|
| P1 | **병렬 실행 경로에 context_cell/browse_cb 누락** | 🔴 Critical | 기능 결함 |
| P2 | **browse_session: SessionAction context가 enrichment 안 됨** | 🟡 Medium | 기능 누락 |
| P3 | **browse_script: infer_context가 None 반환** | 🟡 Medium | 기능 누락 |
| P4 | **DataExtraction.result_count가 항상 None** | 🟢 Low | 기능 누락 |
| D1 | **pending_browse_callback 4개 툴에 동일 패턴 복붙** | 🟡 Medium | 중복 |
| D2 | **TabCallbackRegistry에 이중 맵 (callbacks + browse_callbacks)** | 🟡 Medium | 구조 |
| D3 | **on_structured_progress / ToolProgress / StructuredProgressCallback 사용 안 함** | 🟢 Low | 죽은 코드 |
| D4 | **설계 문서 4개 산재** | 🟢 Low | 정리 |
| T1 | **BrowseProgress / invoke_browse 단위 테스트 없음** | 🟡 Medium | 테스트 |
| T2 | **oxibrowser_backend 통합 테스트에 browse callback 검증 없음** | 🟢 Low | 테스트 |

---

## P1. 병렬 실행 경로에 context_cell/browse_cb 누락

### 문제

`execute_prepared_tool_call` (순차 경로)에는 `context_cell` + `browse_cb`가 있지만,
`execute_prepared_tool_call_static` (병렬 경로)에는 없다.

```rust
// 순차 경로 (execute_prepared_tool_call) — ✅ context_cell + browse_cb 있음
let context_cell = Arc::new(parking_lot::Mutex::new(context));
tool.on_progress(progress_callback(...));
tool.on_browse_progress(browse_cb);

// 병렬 경로 (execute_prepared_tool_call_static) — ❌ 아무것도 없음
// on_progress, on_browse_progress 호출 없이 바로 execute
let mut result = AgentToolResult::success("");
if let Some(ref tool) = tool {
    match tool.execute(...).await { ... }
}
```

**영향:** 병렬 모드에서 browse 툴이 실행되면:
- `ToolExecutionUpdate`가 emit되지 않음 (partial_result 없음)
- `context`가 `ToolExecutionStart`에만 나오고 update에서는 사라짐
- BrowseProgress enrichment가 전혀 일어나지 않음

### 해결책

`execute_prepared_tool_call_static`에 동일한 context_cell + callback 패턴을 추가.

또는 더 우아하게: **두 경로를 하나의 함수로 통합**.

```rust
// Before: 두 개의 분리된 함수
async fn execute_prepared_tool_call(...) -> ExecutedToolCallOutcome  // 순차
async fn execute_prepared_tool_call_static(...) -> ExecutedToolCallOutcome  // 병렬

// After: 하나의 함수로 통합
async fn execute_tool(
    tool_call: ToolCall,
    tool: Arc<dyn AgentTool>,
    args: Value,
    after_hook: Option<AfterToolCallHook>,
    emit: EmitFn,
    ctx: &ToolExecContext,
) -> ExecutedToolCallOutcome
```

호출부에서 공통 로직(context_cell, callbacks, on_progress, on_browse_progress)을
한 번만 수행하고, 순차/병렬은 실행 스케줄링만 담당.

### 변경 파일

| 파일 | 변경 |
|------|------|
| `tool_exec.rs` | `execute_prepared_tool_call`과 `execute_prepared_tool_call_static`을 `execute_tool`로 통합 |

---

## P2. browse_session: SessionAction context가 enrichment 안 됨

### 문제

`browse_session`의 `goto` 액션은 `SessionAction` context를 생성하지만,
`browse_cb` match arm은 `PageVisit`과 `DataExtraction`만 처리한다.
`SessionAction`은 enrichment에서 무시됨.

```rust
// infer_context
"browse_session" => Some(ToolCallContext::SessionAction {
    action: "goto",
    url: Some("https://example.com"),
}),

// browse_cb — SessionAction match arm이 없음!
// PageVisit은 enrichment 됨
// DataExtraction은 enrichment 됨
// SessionAction은 _ => {} 로 무시됨
```

### 해결책

두 가지 접근 중 선택:

**A) SessionAction에 결과 필드 추가 + browse_cb에 match arm 추가:**

```rust
SessionAction {
    action: String,
    url: Option<String>,
    // ── Result fields ──
    #[serde(skip_serializing_if = "Option::is_none")]
    page_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_duration_ms: Option<u64>,
}
```

**B) browse_session "goto" → PageVisit으로 추론 (semantic upgrade):**

`goto` 액션은 본질적으로 페이지 방문이므로, `SessionAction` 대신
`PageVisit { reason: DirectNavigation }`으로 추론.

선택: **B가 더 우아함.** SessionAction은 `click`, `fill`, `type` 같은
비-탐색 액션에만 사용. `goto`는 PageVisit.

### 변경 파일

| 파일 | 변경 |
|------|------|
| `tool_exec.rs` | `infer_context`에서 browse_session "goto" → PageVisit |
| `tool_exec.rs` | `browse_cb`에 SessionAction enrichment 추가 (A를 선택한 경우) |
| `events.rs` | SessionAction에 결과 필드 추가 (A를 선택한 경우) |

---

## P3. browse_script: infer_context가 None 반환

### 문제

```rust
// browse_script: step progress is emitted via progress callback
// with [N/M] format. Dynamic ScriptStep context is a future
// enhancement (requires on_structured_progress wiring).
_ => None,
```

browse_script는 복잡한 YAML 인자를 받아서 infer_context가
동적으로 ScriptStep을 생성하기 어렵다.

### 해결책

browse_script 인자에서 `steps` 배열의 길이를 읽어서
초기 ScriptStep 컨텍스트를 생성:

```rust
"browse_script" => {
    let total = args["steps"].as_array().map(|a| a.len()).unwrap_or(0);
    if total > 0 {
        Some(ToolCallContext::ScriptStep {
            current: 0,
            total,
            step: "starting".into(),
        })
    } else {
        None
    }
}
```

그리고 `browse_script_tool`의 `execute_steps`에서
각 스텝마다 context_cell을 업데이트하도록
progress callback에 context_cell clone을 전달.

### 변경 파일

| 파일 | 변경 |
|------|------|
| `tool_exec.rs` | `infer_context`에 browse_script case 추가 |
| `browse_script_tool.rs` | `execute_steps`에서 context_cell 업데이트 |

---

## P4. DataExtraction.result_count가 항상 None

### 문제

`DataExtraction.result_count` 필드가 정의되어 있지만,
아무도 이 값을 채우지 않는다. browse_extract_tool이
결과를 반환할 때 몇 개의 요소를 추출했는지 알 수 있지만,
그 정보가 context로 역전파되지 않는다.

### 해결책

browse_extract_tool이 execute 완료 후
`ToolCallContext`에 result_count를 쓸 수 있는 채널이 필요.

**접근:** `AgentToolResult.metadata`에 추출 결과 수를 담고,
`tool_exec.rs`에서 execute 완료 후 metadata를 읽어 context_cell을 업데이트.

```rust
// browse_extract_tool.rs — execute 결과에 metadata 추가
Ok(AgentToolResult::success(output)
    .with_metadata(json!({ "result_count": items.len() })))

// tool_exec.rs — execute 완료 후 metadata로 context enrichment
if let Some(ref meta) = result.metadata {
    if let Some(count) = meta["result_count"].as_usize() {
        let mut guard = context_cell.lock();
        if let Some(ToolCallContext::DataExtraction { result_count, .. }) = &mut *guard {
            *result_count = Some(count);
        }
    }
}
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `browse_extract_tool.rs` | execute 결과에 `metadata: { result_count }` 추가 |
| `tool_exec.rs` | execute 완료 후 metadata로 context enrichment |

---

## D1. pending_browse_callback 4개 툴에 동일 패턴 복붙

### 문제

4개 browse 툴에 완전히 동일한 패턴이 반복:

```
1. pending_browse_callback 필드 선언
2. 생성자에서 Mutex::new(None)
3. on_browse_progress에서 lock → store
4. execute에서 lock → take → registry.set_browse 또는 OxicodeTab.set_browse_progress_callback
```

**20줄 × 4개 툴 = 80줄 중복.**

### 해결책

**`BrowseCallbackMixin` 공유 구조체:**

```rust
/// Browse 툴의 공통 callback 관리.
/// on_progress/on_browse_progress → pending → registry/OxicodeTab 등록의
/// 공통 패턴을 캡슐화.
pub(crate) struct BrowseCallbackMixin {
    pending_callback: SyncMutex<Option<ProgressCallback>>,
    pending_browse_callback: SyncMutex<Option<BrowseProgressCallback>>,
}

impl BrowseCallbackMixin {
    pub fn new() -> Self { ... }

    /// on_progress trait 메서드 구현용.
    pub fn store_progress(&self, cb: ProgressCallback) { ... }

    /// on_browse_progress trait 메서드 구현용.
    pub fn store_browse(&self, cb: BrowseProgressCallback) { ... }

    /// 탭이 열렸을 때 두 콜백을 모두 등록.
    /// - registry 경로: BrowseSessionTool, BrowseExtractTool, BrowseScriptTool
    /// - OxicodeTab downcast 경로: BrowseTool
    pub fn register_on_tab(&self, tab: &dyn BrowserTab, registry: Option<&TabCallbackRegistry>) {
        if let Some(cb) = self.pending_callback.lock().take() {
            // registry 또는 OxicodeTab에 등록
        }
        if let Some(bcb) = self.pending_browse_callback.lock().take() {
            // 동일
        }
    }
}
```

각 툴은:

```rust
pub struct BrowseTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    callbacks: BrowseCallbackMixin,  // 두 필드를 하나로
    tab_id_slot: ...,
}
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `engine.rs` (또는 새 파일 `callback_mixin.rs`) | `BrowseCallbackMixin` 정의 |
| `browse_tool.rs` | pending_callback/pending_browse_callback → callbacks 필드 |
| `browse_session_tool.rs` | 동일 |
| `browse_extract_tool.rs` | 동일 |
| `browse_script_tool.rs` | 동일 |

---

## D2. TabCallbackRegistry에 이중 맵

### 문제

```rust
pub struct TabCallbackRegistry {
    callbacks: Mutex<HashMap<Uuid, ProgressCallback>>,
    browse_callbacks: Mutex<HashMap<Uuid, BrowseProgressCallback>>,
}
```

동일한 생명주기(set → invoke → clear)를 가진 두 개의 독립적인 맵.
tab이 열릴 때 두 맵에 set, 닫힐 때 두 맵에서 clear.
두 맵의 키 셋은 항상 동일해야 함 (불일치 = 버그).

### 해결책

**단일 맵에 복합 콜백:**

```rust
struct TabCallbacks {
    progress: Option<ProgressCallback>,
    browse: Option<BrowseProgressCallback>,
}

pub struct TabCallbackRegistry {
    callbacks: Mutex<HashMap<Uuid, TabCallbacks>>,
}

impl TabCallbackRegistry {
    pub fn set(&self, tab_id: Uuid, cb: ProgressCallback) {
        self.callbacks.lock()
            .entry(tab_id)
            .or_insert(TabCallbacks::default())
            .progress = Some(cb);
    }

    pub fn set_browse(&self, tab_id: Uuid, cb: BrowseProgressCallback) {
        self.callbacks.lock()
            .entry(tab_id)
            .or_insert(TabCallbacks::default())
            .browse = Some(cb);
    }

    pub fn invoke(&self, tab_id: &Uuid, msg: String) {
        if let Some(entry) = self.callbacks.lock().get(tab_id) {
            if let Some(ref cb) = entry.progress { cb(msg); }
        }
    }

    pub fn invoke_browse(&self, tab_id: &Uuid, progress: BrowseProgress) {
        if let Some(entry) = self.callbacks.lock().get(tab_id) {
            if let Some(ref cb) = entry.browse { cb(progress); }
        }
    }

    pub fn clear(&self, tab_id: &Uuid) {
        self.callbacks.lock().remove(tab_id);
    }
}
```

**이점:**
- clear 한 번이 두 콜백 모두 정리 (불일치 불가)
- lock 한 번으로 두 콜백 접근 가능
- 항상 동일한 키 셋 보장

### 변경 파일

| 파일 | 변경 |
|------|------|
| `engine.rs` | `TabCallbackRegistry`를 단일 맵으로 재구조화 |
| `tab_guard.rs` | clear_browse_progress_callback 제거 (clear 하나로 충분) |
| `engine.rs` | BrowserTab trait에서 `clear_browse_progress_callback` 제거 |
| `oxibrowser_backend.rs` | OxicodeTab의 clear_browse_progress_callback_impl 제거 |

---

## D3. 죽은 코드: on_structured_progress / ToolProgress

### 문제

`tools.rs`에 정의된 `ToolProgress` enum, `StructuredProgressCallback` 타입,
`on_structured_progress` trait 메서드가 **아무도 사용하지 않음.**

```rust
// tools.rs — 사용처 없음
pub enum ToolProgress {
    Status { message: String },
    PartialOutput { output: String, is_error: bool },
    Percentage { current: f64, total: Option<f64>, message: Option<String> },
    FileOperation { operation: FileOp, path: PathBuf, ... },
}

pub type StructuredProgressCallback = Arc<dyn Fn(ToolProgress) + Send + Sync>;

fn on_structured_progress(&self, _callback: StructuredProgressCallback) {}
```

### 해결책

Phase 3에서 `BrowseProgress`가 구조적 진행 이벤트의 역할을 대신함.
`ToolProgress`는 제거하거나, `BrowseProgress` 패턴을 일반화하여 교체.

**선택: 제거.** `ToolProgress`가 제공하려던 `Percentage`, `FileOperation`은
현재 아무 툴도 사용하지 않음. YAGNI.

```rust
// 제거 대상:
// - ToolProgress enum
// - FileOp enum
// - StructuredProgressCallback type
// - AgentTool::on_structured_progress trait 메서드
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `tools.rs` | `ToolProgress`, `FileOp`, `StructuredProgressCallback`, `on_structured_progress` 제거 |

---

## D4. 설계 문서 4개 산재

### 문제

```
docs/designs/2026-06-04-browser-observability-integration.md  (초기 설계)
docs/designs/2026-06-05-exploration-transparency.md           (v1 설계, BrowseTool 가정 오류)
docs/designs/2026-06-05-tool-context-transparency.md          (v2 설계, Phase 2)
docs/designs/2026-06-05-browse-structured-progress.md         (v3 설계, Phase 3)
```

4개 문서가 서로 참조하며, 일부는 이미 구식.

### 해결책

**하나의 통합 설계서로 병합:**

```
docs/designs/2026-06-05-exploration-transparency.md  ← 통합본 (다른 3개는 보관)
```

구조:
1. 배경과 동기
2. 아키텍처 다이어그램 (최종 상태)
3. Phase 1-3 구현 요약
4. Phase 4 개선사항 (이 설계서의 내용)
5. 이벤트 흐름 예시

---

## T1. BrowseProgress / invoke_browse 단위 테스트 없음

### 문제

`engine.rs`의 기존 `TabCallbackRegistry` 테스트는 String 콜백만 검증.
`set_browse`, `invoke_browse`, `clear_browse`에 대한 테스트가 없음.

### 해결책

`engine.rs` 테스트 모듈에 추가:

```rust
#[test]
fn tab_callback_registry_browse_set_and_invoke() { ... }

#[test]
fn tab_callback_registry_browse_clear() { ... }

#[test]
fn tab_callback_registry_browse_isolation_per_tab() { ... }

#[test]
fn browse_progress_serde_roundtrip() {
    // 모든 BrowseProgress variant가 serde 왕복 테스트를 통과하는지
}
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `engine.rs` | 테스트 3-4개 추가 |

---

## T2. oxibrowser_backend 통합 테스트에 browse callback 검증 없음

### 문제

기존 3개의 통합 테스트는 String 콜백만 검증.
`invoke_browse`가 실제로 호출되는지, BrowseProgress가 올바른지 확인 안 됨.

### 해결책

기존 `engine_forwards_browser_events_to_progress_callback` 테스트에
BrowseProgress 검증을 추가:

```rust
#[tokio::test]
async fn engine_forwards_browse_progress_to_callback() {
    let engine = OxicodeBrowserEngine::new().await.unwrap();
    let registry = engine.callback_registry();

    let received: Arc<StdMutex<Vec<BrowseProgress>>> = ...;

    let tab = engine.new_tab().await.unwrap();
    let tab_id = /* extract */;

    registry.set(tab_id, /* String callback */);
    registry.set_browse(tab_id, /* BrowseProgress callback */);

    let _ = tab.goto("data:text/html,<title>Hi</title>").await;

    // BrowseProgress 검증
    let browse_events = received.lock().unwrap().clone();
    assert!(browse_events.iter().any(|bp| matches!(
        bp,
        BrowseProgress::DocumentReady { status: 200, .. }
    )));
}
```

### 변경 파일

| 파일 | 변경 |
|------|------|
| `oxibrowser_backend.rs` | 기존 테스트에 browse callback 검증 추가 + 전용 테스트 1개 |

---

## 구현 우선순위

```
P1 (병렬 경로 누락)           ← Critical, 기능 결함
 │
 ├→ D2 (이중 맵 통합)         ← P1 해결 시 registry API가 바뀌므로 먼저
 │
 ├→ D1 (Mixin으로 중복 제거)  ← D2 이후, 새 registry API에 맞춰서
 │
 ├→ P2 (SessionAction enrichment)
 ├→ P3 (browse_script infer_context)
 ├→ P4 (DataExtraction result_count)
 │
 ├→ D3 (죽은 코드 제거)
 ├→ T1 (BrowseProgress 단위 테스트)
 ├→ T2 (통합 테스트)
 │
 └→ D4 (설계 문서 통합)
```

### 권장 구현 순서

```
Batch 1 (구조 개선):
  D2 → D1 → P1

Batch 2 (기능 완성):
  P2 → P3 → P4

Batch 3 (정리):
  D3 → T1 → T2 → D4
```

---

## 파일 변경 예상

| Batch | 파일 | LoC 예상 |
|-------|------|---------|
| **Batch 1** | | **~200** |
| | `engine.rs` | TabCallbacks 통합 ~50 |
| | `callback_mixin.rs` (신규) | BrowseCallbackMixin ~60 |
| | `browse_tool.rs` | Mixin 적용 ~30 |
| | `browse_session_tool.rs` | Mixin 적용 ~30 |
| | `browse_extract_tool.rs` | Mixin 적용 ~15 |
| | `browse_script_tool.rs` | Mixin 적용 ~15 |
| | `tab_guard.rs` | clear 간소화 ~5 |
| | `oxibrowser_backend.rs` | OxicodeTab 간소화 ~10 |
| | `tool_exec.rs` | 병렬 경로 수정 ~50 |
| **Batch 2** | | **~60** |
| | `tool_exec.rs` | infer_context + browse_cb enrichment ~30 |
| | `browse_extract_tool.rs` | metadata에 result_count ~5 |
| | `browse_script_tool.rs` | context_cell 업데이트 ~25 |
| **Batch 3** | | **~100** |
| | `tools.rs` | 죽은 코드 제거 ~30 (줄어듦) |
| | `engine.rs` | 테스트 추가 ~40 |
| | `oxibrowser_backend.rs` | 테스트 추가 ~30 |
| | 설계 문서 | 통합 ~ |
| **합계** | | **~360** |

---

## 완료 기준

- [x] P1: 병렬 경로에서도 ToolExecutionUpdate + context enrichment가 동작
- [x] P2: browse_session "goto"에서 page_status/page_duration_ms가 enrichment됨
- [x] P3: browse_script에서 ScriptStep context가 추론됨
- [x] P4: browse_extract에서 result_count가 채워짐
- [x] D1: BrowseCallbackMixin으로 4개 툴의 중복 제거
- [x] D2: TabCallbackRegistry가 단일 맵으로 통합
- [x] D3: ToolProgress / on_structured_progress 죽은 코드 제거
- [x] T1: BrowseProgress 단위 테스트 4개+
- [x] T2: oxibrowser_backend 통합 테스트에 browse callback 검증
- [x] `cargo nextest run --workspace` 전체 통과
- [x] `cargo clippy --workspace -- -D warnings` 깨끗
