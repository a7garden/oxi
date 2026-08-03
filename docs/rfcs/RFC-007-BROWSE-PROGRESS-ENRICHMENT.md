# RFC-007: BrowseProgress Enrichment 확장 — 전체 이벤트 타입 ToolCallContext 반영

**상태**: 구현 완료 (oxicode-agent 0.29.1)
**우선순위**: P1 — oxios Web UI 브라우징 투명성 리치 렌더링의 전제 조건
**영역**: oxicode-agent (`agent_loop/tool_exec.rs`, `events.rs`)
**의존**: RFC-015 (oxios chat transparency)  

---

## 1. 동기

oxicode-agent 0.29에서 `BrowseProgress` enum은 5종의 구조화된 이벤트를 방출합니다:

| `BrowseProgress` variant | `make_browse_enrichment_cb` 처리 여부 |
|---|---|
| `DocumentReady` | ✅ `page_title`, `page_status`, `page_bytes`, `page_duration_ms` enrich |
| `NavigationStarted` | ❌ 무시 |
| `WaitingForSelector` | ❌ 무시 |
| `ScreenshotCaptured` | ❌ 무시 |
| `NavigationFailed` | ❌ 무시 |

`DocumentReady`만 처리되므로, oxios Web UI는 다음 정보를 **절대 수신할 수 없습니다**:

1. 페이지 로드 **실패** 사실과 에러 메시지
2. **스크린샷** 촬영 여부와 메타데이터
3. CSS 셀렉터 **대기** 상태

oxios 프론트엔드는 이미 이 필드들을 렌더링할 준비가 되어 있습니다(`ToolCallContext` 타입에 `navigation_error`, `screenshot`, `waiting_for_selector` 필드 추가 완료). oxicode-agent 측 enrichment만 추가되면 즉시 작동합니다.

## 2. 제안

### 2.1 `ToolCallContext::PageVisit`에 필드 2개 추가

```rust
// events.rs — PageVisit variant
PageVisit {
    url: String,
    reason: Option<VisitReason>,
    page_title: Option<String>,
    page_status: Option<u16>,
    page_bytes: Option<u64>,
    page_duration_ms: Option<u64>,
    // ── NEW ──
    /// Navigation error message (from BrowseProgress::NavigationFailed).
    #[serde(skip_serializing_if = "Option::is_none")]
    navigation_error: Option<String>,
    /// Screenshot metadata (from BrowseProgress::ScreenshotCaptured).
    #[serde(skip_serializing_if = "Option::is_none")]
    screenshot: Option<ScreenshotMeta>,
}

/// Screenshot metadata attached to PageVisit context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotMeta {
    /// PNG payload size in bytes.
    pub bytes: usize,
    /// Viewport width.
    pub width: u32,
    /// Capture duration in milliseconds.
    pub duration_ms: u64,
}
```

### 2.2 `ToolCallContext`에 variant 1개 추가

```rust
// events.rs
/// Waiting for a CSS selector to appear (BrowseProgress::WaitingForSelector).
WaitingForSelector {
    /// CSS selector being awaited.
    selector: String,
    /// Maximum wait time in milliseconds.
    timeout_ms: u64,
},
```

**참고:** `#[non_exhaustive]`이므로 variant 추가는 semver 호환입니다.

### 2.3 `make_browse_enrichment_cb`에 매치 암 추가

```rust
// agent_loop/tool_exec.rs — make_browse_enrichment_cb()

// 기쁜 DocumentReady 매치들은 그대로 유지

// ── NEW: NavigationFailed → PageVisit.navigation_error ──
(
    Some(ToolCallContext::PageVisit {
        navigation_error,
        ..
    }),
    crate::tools::browse::BrowseProgress::NavigationFailed {
        error,
        ..
    },
) => {
    *navigation_error = Some(error.clone());
}

// ── NEW: ScreenshotCaptured → PageVisit.screenshot ──
(
    Some(ToolCallContext::PageVisit {
        screenshot,
        ..
    }),
    crate::tools::browse::BrowseProgress::ScreenshotCaptured {
        bytes,
        width,
        duration_ms,
    },
) => {
    *screenshot = Some(ScreenshotMeta {
        bytes: *bytes,
        width: *width,
        duration_ms: *duration_ms,
    });
}

// ── NEW: WaitingForSelector → PageVisit 대기 상태 전환 ──
// PageVisit이 아직 DocumentReady를 받지 않은 상태에서
// 셀렉터 대기 이벤트가 오면, 아무 enrich도 하지 않음.
// 이 이벤트는 대기 시간이 길어질 때 "무엇을 기다리고 있는지"
// 알려주는 목적이며, DocumentReady가 오면 자연히 덮어씌워짐.
```

### 2.4 `WaitingForSelector` — context 전환 vs enrich

`WaitingForSelector` 이벤트는 `PageVisit`이 **아직 로드 중**일 때 발생합니다. 두 가지 접근이 가능합니다:

**옵션 A (권장): `PageVisit` enrich만**
- `WaitingForSelector` 이벤트는 `AgentEvent::ToolExecutionUpdate`의 `partial_result` (문자열)로 이미 UI에 전달됨
- `context`를 변경하지 않고 문자열 progress로 충분
- 구현이 단순하고 기존 동작 변경 없음

**옵션 B: context를 `WaitingForSelector`로 전환**
- context cell을 `ToolCallContext::WaitingForSelector { selector, timeout_ms }`로 교체
- UI가 구조화된 셀렉터 정보를 리치하게 렌더링 가능
- 하지만 DocumentReady가 오면 다시 PageVisit으로 덮어씌워야 하는 복잡도

**권장: 옵션 A.** `ScreenshotCaptured`와 `NavigationFailed`만 enrich 하고, `WaitingForSelector`는 문자열 progress로 충분합니다.

## 3. 변경 파일

| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/events.rs` | `ToolCallContext::PageVisit`에 `navigation_error`, `screenshot` 필드 추가. `ScreenshotMeta` 구조체 추가. `WaitingForSelector` variant 추가 (옵션). |
| `oxicode-agent/src/agent_loop/tool_exec.rs` | `make_browse_enrichment_cb`에 `NavigationFailed` → `navigation_error`, `ScreenshotCaptured` → `screenshot` 매치 암 추가 |
| `oxicode-sdk/src/lib.rs` | re-export 변경 없음 (`ToolCallContext`는 이미 re-export됨) |

## 4. 하위 호환성

- `ToolCallContext`는 `#[non_exhaustive]` — 새 variant와 필드 추가가 semver 호환
- 새 필드는 `#[serde(skip_serializing_if = "Option::is_none")]` — 직렬화 시 기존과 동일
- `make_browse_enrichment_cb`의 `_ => {}` catch-all이 새 이벤트를 무시하므로, **enrichment를 추가하지 않아도 동작에는 영향 없음**

## 5. 소비자 영향

### oxios (oxios-kernel + oxios-web)

| 필드 | 현재 oxios 프론트엔드 준비 상태 |
|------|------|
| `page_visit.navigation_error` | ✅ 타입 정의됨, 리치 에러 뱃지/상세 구현 완료 |
| `page_visit.screenshot` | ✅ 타입 정의됨, 📷 아이콘 + 상세 정보 구현 완료 |
| `waiting_for_selector` | ✅ 타입 정의됨, 셀렉터/타임아웃 뱃지/상세 구현 완료 |

oxios는 이미 `AgentEvent::ToolExecutionUpdate { context }`를 `serde_json::Value`로 pass-through하므로, **oxios 커널 변경은 필요 없습니다.** oxicode-agent에서 enrichment만 추가되면 WS를 통해 그대로 흘러갑니다.

### oxicode-cli / oxicode-tui

`ToolCallContext`의 새 필드와 variant는 `#[serde(skip_serializing_if)]`와 `#[non_exhaustive]`로 보호되므로 영향 없음.

## 6. 구현 예상 분량

- `events.rs`: ~20줄 (필드 2개 + 구조체 1개 + variant 1개)
- `tool_exec.rs`: ~20줄 (매치 암 2-3개)
- 테스트: ~30줄 (기존 `browse_progress_serde_roundtrip` 확장 + enrichment 단위 테스트)

**총 ~70줄.** 30분 이내 구현 가능.
