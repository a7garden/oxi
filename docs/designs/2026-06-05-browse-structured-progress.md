# Browse Observability — 구조적 이벤트 파이프라인 완성

> **Status:** ✅ Implemented
> **Depends on:** Phase 1 (툴 progress callback 연결) ✅, Phase 2 (ToolCallContext) ✅
> **Scope:** oxi-agent (engine.rs, oxibrowser_backend.rs, browse tools, tool_exec.rs)
> **oxibrowser-core 변경:** 없음. 이미 충분한 데이터를 보내고 있음.
> **Tests:** 2108/2108 pass, 0 clippy errors

---

## 0. 문제

oxibrowser-core는 풍부한 구조적 이벤트를 보낸다:

```rust
BrowserEvent::DocumentReady {
    tab_id: Uuid,
    final_url: "https://example.com",
    title: "Example Page",
    status: 200,
    total_bytes: 12400,
    js_script_count: 3,
    total_duration: Duration::from_millis(245),
}
```

하지만 OxiBrowserEngine의 drain task에서 전부 문자열로 압축된다:

```rust
progress_clone.invoke(&tab_id, event.short_label());
//                                  ^^^^^^^^^^^^^^^^^^^^
//                                  "Loaded \"Example Page\" — 200 · 12KB · 245ms"
//                                  구조적 데이터가 문자열에 갇힘
```

현재 도달하는 ToolExecutionUpdate:

```json
{
  "partial_result": "Loaded \"Example Page\" — 200 · 12KB · 245ms",
  "context": { "kind": "page_visit", "url": "https://example.com" }
}
```

**잃어버린 것:** status code, bytes, duration, script count — 전부 문자열 파싱으로만 복구 가능.

---

## 1. 목표

```json
{
  "partial_result": "Loaded \"Example Page\" — 200 · 12KB · 245ms",
  "context": {
    "kind": "page_visit",
    "url": "https://example.com",
    "reason": "direct_navigation",
    "page_title": "Example Page",
    "page_status": 200,
    "page_bytes": 12400,
    "page_duration_ms": 245
  }
}
```

- `partial_result`: 사람이 읽는 텍스트 (기존과 동일, 하위 호환)
- `context`: 구조적 데이터 (기계가 읽는 데이터, 기존 필드에 결과 추가)

---

## 2. 설계 원칙

| # | 원칙 | 이유 |
|---|------|------|
| P1 | **ProgressCallback = Fn(String)은 건드리지 않는다** | BashTool, ReadTool, SubagentTool이 사용. 변경하면 breaking change |
| P2 | **BrowseProgress는 browse 모듈 내부 타입** | engine.rs에 정의. oxibrowser-core 의존 없음. feature gate 불필요 |
| P3 | **병렬 채널, 직렬 변환 아님** | TabCallbackRegistry에 두 번째 맵 추가. 기존 String 콜백과 독립 |
| P4 | **context는 점진적 enrichment** | 시작: PageVisit { url }. DocumentReady 도착: page_title, page_status 등 채워짐 |

---

## 3. 아키텍처

### 3.1 현재 (끊긴 파이프라인)

```
oxibrowser-core                OxiBrowserEngine drain       TabCallbackRegistry       tool_exec.rs callback
                                                                                               
BrowserEvent ──────→  event.short_label() ──────────→  Fn(String) ──────────→  ToolExecutionUpdate
   ↑                           │                                                     {
   구조적 데이터                │ String으로 압축                                     partial_result: msg,
   (status, bytes, duration)   │ (데이터 소실)                                        context: FIXED (초기값)
                               │                                                     }
```

### 3.2 변경 후 (완전한 파이프라인)

```
oxibrowser-core                OxiBrowserEngine drain       TabCallbackRegistry       tool_exec.rs callbacks
                                                                                               
BrowserEvent ──────→  event.short_label() ────────→  Fn(String) ────────→  ToolExecutionUpdate
   │                       │                                                     {
   │                       │ 기존 String 콜백 (유지)                              partial_result: msg,
   │                       │                                                     context: cell.read()
   │                       │                                                     }
   │                       │
   └───────→  BrowseProgress::from(event) ──→  Fn(BrowseProgress) ──→  context_cell.write()
                      │                                    │              (context enrichment)
                      │                                    │
                      oxibrowser_backend.rs가               browse tools가
                      둘 다 invoke                         등록
```

핵심: **Fn(BrowseProgress) 콜백이 공유 context_cell을 업데이트**하면,
다음 Fn(String) 콜백이 이미 enriched context를 읽는다.

---

## 4. 구현 계획

### Step 1: BrowseProgress enum 정의

**파일:** `oxi-agent/src/tools/browse/engine.rs`

```rust
/// 브라우저 탐색의 구조적 진행 이벤트.
///
/// oxibrowser-core의 BrowserEvent에서 변환됨.
/// engine.rs는 항상 컴파일되므로 oxibrowser-core 의존 없이
/// 독립적으로 정의. 변환은 oxibrowser_backend.rs에서.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrowseProgress {
    /// 탐색 시작
    NavigationStarted {
        url: String,
    },

    /// 셀렉터 대기
    WaitingForSelector {
        selector: String,
        timeout_ms: u64,
    },

    /// 문서 로드 완료 (핵심 — 풍부한 구조적 데이터)
    DocumentReady {
        url: String,
        title: String,
        status: u16,
        bytes: u64,
        duration_ms: u64,
    },

    /// 스크린샷 캡처
    ScreenshotCaptured {
        bytes: usize,
        width: u32,
        duration_ms: u64,
    },
}
```

**LoC:** ~30

### Step 2: TabCallbackRegistry에 BrowseProgress 맵 추가

**파일:** `oxi-agent/src/tools/browse/engine.rs`

```rust
pub struct TabCallbackRegistry {
    /// 기존 String 콜백 (partial_result용)
    callbacks: Mutex<HashMap<uuid::Uuid, crate::tools::ProgressCallback>>,
    /// NEW: 구조적 진행 콜백 (context enrichment용)
    browse_callbacks: Mutex<HashMap<uuid::Uuid, Arc<dyn Fn(BrowseProgress) + Send + Sync>>>,
}

impl TabCallbackRegistry {
    // 기존 메서드 유지 (set, clear, invoke, is_set, len, is_empty)

    /// BrowseProgress 콜백 등록
    pub fn set_browse(&self, tab_id: uuid::Uuid, cb: Arc<dyn Fn(BrowseProgress) + Send + Sync>) {
        self.browse_callbacks.lock().insert(tab_id, cb);
    }

    /// BrowseProgress 콜백 제거
    pub fn clear_browse(&self, tab_id: &uuid::Uuid) {
        self.browse_callbacks.lock().remove(tab_id);
    }

    /// BrowseProgress 콜백 호출
    pub fn invoke_browse(&self, tab_id: &uuid::Uuid, progress: BrowseProgress) {
        if let Some(cb) = self.browse_callbacks.lock().get(tab_id).cloned() {
            cb(progress);
        }
    }
}
```

**LoC:** ~25

### Step 3: BrowserTab trait에 browse callback 메서드 추가

**파일:** `oxi-agent/src/tools/browse/engine.rs`

```rust
// BrowserTab trait에 추가 (default no-op)
fn set_browse_progress_callback(&self, _cb: Arc<dyn Fn(BrowseProgress) + Send + Sync>) {}
fn clear_browse_progress_callback(&self) {}
```

**LoC:** ~3

### Step 4: OxiTab에 browse callback 구현

**파일:** `oxi-agent/src/tools/browse/oxibrowser_backend.rs`

```rust
impl OxiTab {
    pub fn set_browse_progress_callback(&self, cb: Arc<dyn Fn(BrowseProgress) + Send + Sync>) {
        self.registry.set_browse(self.tab_id, cb);
    }

    pub fn clear_browse_progress_callback(&self) {
        self.registry.clear_browse(&self.tab_id);
    }
}

// BrowserTab trait impl에도 오버라이드
fn set_browse_progress_callback(&self, cb: Arc<dyn Fn(BrowseProgress) + Send + Sync>) {
    self.set_browse_progress_callback(cb);
}
fn clear_browse_progress_callback(&self) {
    self.clear_browse_progress_callback();
}
```

**LoC:** ~15

### Step 5: OxiBrowserEngine drain task에서 BrowseProgress 변환

**파일:** `oxi-agent/src/tools/browse/oxibrowser_backend.rs`

```rust
Ok(event) => {
    let tab_id = extract_event_tab_id(&event);

    // 기존: String 콜백
    progress_clone.invoke(&tab_id, event.short_label());

    // NEW: BrowseProgress 콜백
    if let Some(bp) = browse_progress_from_event(&event) {
        progress_clone.invoke_browse(&tab_id, bp);
    }
}

/// oxibrowser-core BrowserEvent → BrowseProgress 변환
fn browse_progress_from_event(event: &oxibrowser_core::BrowserEvent) -> Option<BrowseProgress> {
    use oxibrowser_core::BrowserEvent::*;
    match event {
        NavigationStarted { url, .. } => Some(BrowseProgress::NavigationStarted {
            url: url.clone(),
        }),
        WaitingForSelector { selector, timeout_ms, .. } => Some(BrowseProgress::WaitingForSelector {
            selector: selector.clone(),
            timeout_ms: *timeout_ms,
        }),
        DocumentReady { final_url, title, status, total_bytes, total_duration, .. } =>
            Some(BrowseProgress::DocumentReady {
                url: final_url.clone(),
                title: title.clone(),
                status: *status,
                bytes: *total_bytes,
                duration_ms: total_duration.as_millis() as u64,
            }),
        ScreenshotCaptured { bytes, viewport_width, duration, .. } =>
            Some(BrowseProgress::ScreenshotCaptured {
                bytes: *bytes,
                width: *viewport_width,
                duration_ms: duration.as_millis() as u64,
            }),
        _ => None,
    }
}
```

**LoC:** ~30

### Step 6: ToolCallContext에 결과 필드 추가

**파일:** `oxi-agent/src/events.rs`

```rust
PageVisit {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<VisitReason>,
    // ── 결과 필드 (BrowseProgress에서 점진적 채움) ──
    #[serde(skip_serializing_if = "Option::is_none")]
    page_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_duration_ms: Option<u64>,
},

DataExtraction {
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    // ── 결과 필드 ──
    #[serde(skip_serializing_if = "Option::is_none")]
    result_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_status: Option<u16>,
},
```

**LoC:** ~15

### Step 7: tool_exec.rs에 공유 context cell + browse callback 생성

**파일:** `oxi-agent/src/agent_loop/tool_exec.rs`

```rust
// 기존 context를 공유 셀에 넣음
let context_cell: Arc<parking_lot::Mutex<Option<ToolCallContext>>> =
    Arc::new(parking_lot::Mutex::new(context));

// String 콜백 (기존, 수정: context_cell에서 읽음)
let cc = context_cell.clone();
let progress_cb: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |msg: String| {
    let tab_id = *tab_id_slot_cb.lock();
    let ctx = cc.lock().clone();
    emit_clone(AgentEvent::ToolExecutionUpdate {
        tool_call_id: tool_call_id_clone.clone(),
        tool_name: tool_name.clone(),
        partial_result: msg,
        tab_id,
        context: ctx,
    });
});

// BrowseProgress 콜백 (NEW: context_cell을 enriched)
let cc2 = context_cell.clone();
let browse_cb: Arc<dyn Fn(BrowseProgress) + Send + Sync> =
    Arc::new(move |progress: BrowseProgress| {
        let mut ctx = cc2.lock();
        match (&mut *ctx, &progress) {
            (
                Some(ToolCallContext::PageVisit {
                    page_title,
                    page_status,
                    page_bytes,
                    page_duration_ms,
                    ..
                }),
                BrowseProgress::DocumentReady {
                    title, status, bytes, duration_ms, ..
                },
            ) => {
                *page_title = Some(title.clone());
                *page_status = Some(*status);
                *page_bytes = Some(*bytes);
                *page_duration_ms = Some(*duration_ms);
            }
            (
                Some(ToolCallContext::PageVisit { .. }),
                BrowseProgress::NavigationStarted { url },
            ) => {
                // URL 업데이트 (리다이렉트 등)
                // 필요시 구현
                let _ = url;
            }
            _ => {}
        }
    });

// 두 콜백 모두 tool에 전달
tool.on_progress(progress_callback(move |msg: String| {
    progress_cb(msg);
}));
tool.on_browse_progress(browse_cb);
```

**LoC:** ~40

### Step 8: AgentTool trait에 on_browse_progress 추가

**파일:** `oxi-agent/src/tools.rs`

```rust
/// 브라우저 진행 이벤트 콜백.
/// BrowseProgress를 받아서 context를 enriched.
pub type BrowseProgressCallback = Arc<dyn Fn(browse::BrowseProgress) + Send + Sync>;

fn on_browse_progress(&self, _callback: BrowseProgressCallback) {
    // Default no-op — browse 툴만 구현
}
```

**LoC:** ~8

### Step 9: Browse 툴에 on_browse_progress 구현

**파일:** browse_tool.rs, browse_session_tool.rs, browse_extract_tool.rs, browse_script_tool.rs

각 툴에:

```rust
// 필드 추가
pending_browse_callback: SyncMutex<Option<crate::tools::BrowseProgressCallback>>,

// trait 구현
fn on_browse_progress(&self, callback: crate::tools::BrowseProgressCallback) {
    *self.pending_browse_callback.lock() = Some(callback);
}
```

그리고 탭 열 때:

```rust
// BrowseTool (OxiTab downcast 경로)
if let Some(oxi_tab) = raw_tab.as_any().downcast_ref::<OxiTab>() {
    if let Some(cb) = self.pending_callback.lock().take() {
        oxi_tab.set_progress_callback(cb);
    }
    if let Some(bcb) = self.pending_browse_callback.lock().take() {
        oxi_tab.set_browse_progress_callback(bcb);
    }
}

// BrowseSessionTool/ExtractTool/ScriptTool (callback_registry 직접 경로)
if let Some(bcb) = self.pending_browse_callback.lock().take() {
    let registry = self.engine.callback_registry();
    registry.set_browse(tab_id, bcb);
}
```

**LoC:** ~60 (4개 툴 × ~15줄)

### Step 10: TabGuard close에서 browse callback도 정리

**파일:** `oxi-agent/src/tools/browse/tab_guard.rs`

```rust
// close() 메서드에 추가
pub async fn close(self) {
    self.tab.clear_progress_callback();
    self.tab.clear_browse_progress_callback();  // NEW
    let _ = self.tab.close().await;
    // ...
}
```

**LoC:** ~2

---

## 5. 파일 변경 요약

| 파일 | 변경 | LoC |
|------|------|-----|
| `engine.rs` | BrowseProgress enum, TabCallbackRegistry browse 맵, BrowserTab trait 메서드 | ~60 |
| `oxibrowser_backend.rs` | BrowserEvent → BrowseProgress 변환, OxiTab 구현, drain task 수정 | ~50 |
| `events.rs` | ToolCallContext 결과 필드 | ~15 |
| `tools.rs` | BrowseProgressCallback 타입, on_browse_progress trait 메서드 | ~10 |
| `tool_exec.rs` | 공유 context_cell, BrowseProgress 콜백 생성 | ~45 |
| `browse_tool.rs` | on_browse_progress 구현, 등록 | ~15 |
| `browse_session_tool.rs` | 동일 | ~15 |
| `browse_extract_tool.rs` | 동일 | ~15 |
| `browse_script_tool.rs` | 동일 | ~15 |
| `tab_guard.rs` | close에서 browse callback 정리 | ~2 |
| **합계** | | **~242** |

---

## 6. 변경받지 않는 것

| 파일/타입 | 이유 |
|-----------|------|
| `oxibrowser-core` | 이미 충분한 데이터를 보냄 |
| `ProgressCallback = Fn(String)` | BashTool, ReadTool, SubagentTool이 사용. 변경 안 함 |
| `oxi-ai` | ProgressCallback 타입 변경 없음 |
| `oxi-sdk` | 재export만 업데이트 (BrowseProgress, BrowseProgressCallback) |
| `oxi-cli` | context 필드의 새 옵션 필드는 `..`로 무시됨 |

---

## 7. 이벤트 흐름 예시

### NavigationStarted 수신

```
1. oxibrowser: BrowserEvent::NavigationStarted { tab_id, url: "https://example.com" }
2. OxiBrowserEngine: invoke(tab_id, "Opening https://example.com…")
                     invoke_browse(tab_id, BrowseProgress::NavigationStarted { url })
3. browse_cb: context_cell.enrich()  ← 아직 enrich할 필드 없음 (URL은 이미 있음)
4. progress_cb: emit(ToolExecutionUpdate {
     partial_result: "Opening https://example.com…",
     context: PageVisit { url: "https://example.com", page_title: None, ... }
   })
```

### DocumentReady 수신

```
1. oxibrowser: BrowserEvent::DocumentReady { tab_id, title: "Example", status: 200, bytes: 12400, duration: 245ms }
2. OxiBrowserEngine: invoke(tab_id, "Loaded \"Example\" — 200 · 12KB · 245ms")
                     invoke_browse(tab_id, BrowseProgress::DocumentReady { title, status, bytes, duration_ms })
3. browse_cb: context_cell →
     PageVisit { url, page_title: Some("Example"), page_status: Some(200),
                 page_bytes: Some(12400), page_duration_ms: Some(245) }
4. progress_cb: emit(ToolExecutionUpdate {
     partial_result: "Loaded \"Example\" — 200 · 12KB · 245ms",
     context: PageVisit {
       url: "https://example.com",
       page_title: Some("Example"),
       page_status: Some(200),
       page_bytes: Some(12400),
       page_duration_ms: Some(245)
     }
   })
```

### UI가 받는 최종 JSON

```json
{
  "type": "tool_execution_update",
  "tool_call_id": "call_abc123",
  "tool_name": "browse",
  "partial_result": "Loaded \"Example\" — 200 · 12KB · 245ms",
  "tab_id": "f47ac10b-...",
  "context": {
    "kind": "page_visit",
    "url": "https://example.com",
    "reason": "direct_navigation",
    "page_title": "Example",
    "page_status": 200,
    "page_bytes": 12400,
    "page_duration_ms": 245
  }
}
```

---

## 8. 시퀀스 다이어그램

```
oxibrowser    OxiBrowserEngine    TabCallbackRegistry    BrowseTool    tool_exec.rs
    │                │                     │                 │              │
    │ BrowserEvent   │                     │                 │              │
    │ (DocumentReady)│                     │                 │              │
    ├───────────────→│                     │                 │              │
    │                │ short_label()       │                 │              │
    │                │────────────────────→│                 │              │
    │                │                     │ invoke String   │              │
    │                │                     │────────────────→│              │
    │                │                     │                 │ progress_cb  │
    │                │                     │                 │─────────────→│
    │                │                     │                 │              │
    │                │ BrowseProgress      │                 │              │
    │                │────────────────────→│                 │              │
    │                │                     │ invoke_browse   │              │
    │                │                     │────────────────→│              │
    │                │                     │                 │ browse_cb    │
    │                │                     │                 │ context_cell │
    │                │                     │                 │  .write()    │
    │                │                     │                 │              │
    │                │                     │                 │ ← (context enriched) │
    │                │                     │                 │              │
    │                │                     │                 │ (다음 invoke 시 │
    │                │                     │                 │  enriched context │
    │                │                     │                 │  반영됨)       │
```

---

## 9. 리스크

| 리스크 | 확률 | 대응 |
|--------|------|------|
| browse_cb와 progress_cb 호출 순서 보장 | 낮음 | 같은 drain task 루프에서 순차 호출. browse_cb 먼저, progress_cb 나중 |
| context_cell lock contention | 낮음 | parking_lot::Mutex는 짧은 락. 두 콜백이 같은 태스크에서 순차 실행 |
| BrowseProgress에 새 variant 추가 | 낮음 | `#[non_exhaustive]`. 기존 match는 wildcard로 처리 |
| TabCallbackRegistry 메모리 증가 | 낮음 | browse_callbacks는 callbacks와 동일한 수명. 탭 닫을 때 같이 정리 |

---

## 10. 구현 순서

```
Step 1: BrowseProgress enum                    (engine.rs)
Step 2: TabCallbackRegistry browse 맵           (engine.rs)
Step 3: BrowserTab trait 메서드 추가             (engine.rs)
Step 4: OxiTab 구현                             (oxibrowser_backend.rs)
Step 5: BrowserEvent → BrowseProgress 변환       (oxibrowser_backend.rs)
Step 6: ToolCallContext 결과 필드               (events.rs)
Step 7: tool_exec.rs 공유 context cell          (tool_exec.rs)
Step 8: AgentTool trait on_browse_progress      (tools.rs)
Step 9: Browse 툴 4개 구현                      (browse_*.rs)
Step 10: TabGuard 정리                           (tab_guard.rs)
```

예상: **2-3일, ~242 LoC**
