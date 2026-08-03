# 재구현 계획: 유실된 Browser + Vision Routing

> 병합 과정에서 유실된 파일/변경을 재구현하기 위한 작업 계획
> Date: 2026-05-25

## 0. 현재 상태

### 디스크에 있는 것 (살아있음)

```
oxicode-agent/src/tools/browse/
├── config.rs        ✅ BrowseConfig (118줄)
├── helpers.rs       ✅ JS 헬퍼 + 파서 (238줄)
└── tab_guard.rs     ✅ TabGuard RAII (189줄)

oxicode-ai/src/router/    ✅ 라우터 전체 (비전 없이)
oxicode-store/src/router_config.rs  ✅ 라우터 설정 (비전 없이)
oxicode-cli/              ✅ CLI 전체 (비전 없이)

docs/designs/browser-improvements.md   ✅ 설계 문서
docs/designs/vision-routing.md         ✅ 설계 문서
```

### 유실된 것

```
oxicode-agent/src/tools/browse/
├── mod.rs                ❌ 모듈 진입점
├── engine.rs             ❌ BrowserEngine/BrowserTab trait + 공유 타입
├── browse_tool.rs        ❌ BrowseTool (AgentTool)
├── browse_extract_tool.rs ❌ BrowseExtractTool (AgentTool)
├── browse_script_tool.rs  ❌ BrowseScriptTool (AgentTool) [feature-gated]
├── oxibrowser_backend.rs  ❌ OxicodeBrowserEngine impl [feature-gated]
└── tests.rs              ❌ 단위 테스트

oxicode-agent/src/tools.rs     ❌ "pub mod browse;" 선언 + re-exports

oxicode-ai/src/router/signals.rs  ❌ VisionSignal 추가분
oxicode-ai/src/router/types.rs    ❌ ScoringWeights.vision + RoutingDecision vision 필드
oxicode-ai/src/router/scoring.rs  ❌ compute_score() vision 파라미터
oxicode-ai/src/router/mod.rs      ❌ route_with_vision() + ensure_vision_model()

oxicode-store/src/router_config.rs ❌ ScoringWeights.vision 필드 + 파싱
oxicode-cli/src/main.rs            ❌ vision 필드 매핑
oxicode-cli/src/router_integration.rs ❌ vision 필드 매핑
```

## 1. 구현 순서 (의존성 순)

```
Phase A: 브라우저 엔진 기반층 (trait + 백엔드)
    engine.rs → mod.rs → tools.rs 모듈 선언

Phase B: 브라우저 도구 3개
    browse_tool.rs → browse_extract_tool.rs → browse_script_tool.rs

Phase C: 비전 라우팅
    signals.rs → types.rs → scoring.rs → mod.rs → store → cli

Phase D: 테스트 + clippy
```

---

## Phase A: engine.rs + mod.rs + tools.rs

### A1. `engine.rs` (약 175줄)

**역할**: BrowserEngine, BrowserTab 트레이트 + 공유 타입들
**의존**: `async_trait`, `serde`, `thiserror` (모두 Cargo.toml에 이미 있음)
**oxibrowser-core 의존**: 없음 — 항상 컴파일됨

```
포함 내용:
- BrowserError (thiserror enum)
- PageContent (struct, 5 fields: url, title, status, markdown, html)
- LinkInfo (struct: text, href)
- ElementInfo (struct: tag, text, attributes)
- BrowserEngine trait (6 methods: fetch, extract_links, query_all, screenshot, new_tab, close)
- BrowserTab trait (11 methods: goto, click, type, fill, press, wait_for, content, query_all, evaluate, screenshot, close)
```

### A2. `mod.rs` (약 50줄)

```
포함 내용:
- pub mod engine, config, tab_guard, helpers
- pub mod browse_extract_tool, browse_tool
- #[cfg(feature = "native-browser")] pub mod browse_script_tool
- #[cfg(feature = "native-browser")] pub mod oxibrowser_backend
- #[cfg(test)] mod tests
- re-exports: BrowseTool, BrowseExtractTool, BrowserEngine, BrowserTab, BrowseConfig, etc.
- #[cfg(feature = "native-browser")] re-exports: BrowseScriptTool, OxicodeBrowserEngine
```

### A3. `tools.rs` 수정 (1줄 추가)

```rust
// "Built-in tools" 주석 아래에 추가:
pub mod browse;
```

그리고 re-export 섹션에 추가:
```rust
pub use browse::{BrowseExtractTool, BrowseTool, BrowserEngine, BrowserTab};
#[cfg(feature = "native-browser")]
pub use browse::BrowseScriptTool;
```

---

## Phase B: 브라우저 도구 3개

### B1. `browse_tool.rs` (약 200줄)

**역할**: 페이지 렌더링 → markdown/html/text/links
**원칙**: 1 Request = 1 Tab, helpers 사용

```
구조체: BrowseTool { engine: Arc<dyn BrowserEngine>, config: BrowseConfig }
AgentTool 구현:
  - name: "browse"
  - params: url, format, selector, wait_for, screenshot
  - execute:
    1. engine.new_tab() → TabGuard
    2. tab.goto(url)
    3. wait_for (옵션)
    4. format에 따라 출력:
       - "html": page.html 또는 tab.query_all(sel)
       - "links": helpers::extract_links(tab) → helpers::format_links()
       - "text": page.markdown 또는 tab.query_all(sel)
       - "markdown": page.markdown 또는 tab.query_all(sel)
    5. screenshot (옵션): tab.screenshot() → ContentBlock::Image
    6. guard.close()
```

### B2. `browse_extract_tool.rs` (약 170줄)

**역할**: CSS selector로 구조화된 데이터 추출
**원칙**: 1 Tab, helpers 사용, timeout 적용

```
구조체: BrowseExtractTool { engine, config }
AgentTool 구현:
  - name: "browse_extract"
  - params: url, selector, extract, all, timeout
  - execute:
    1. tokio::time::timeout()로 전체 감쌈
    2. engine.new_tab() → TabGuard
    3. tab.goto(url)
    4. extract_from_tab():
       - "links": helpers::js_links_within(selector) → parse_link_values
       - "elements": helpers::js_query_elements(selector) → parse_element_values
       - "markdown": tab.query_all(selector)
       - "text": tab.query_all(selector)
    5. guard.close()
```

### B3. `browse_script_tool.rs` (약 480줄, feature-gated)

**역할**: YAML 기반 다단계 브라우저 자동화
**feature**: `native-browser` (serde_yaml 필요)
**helpers 사용**: js_set_select_value, js_check, js_uncheck

```
포함 내용:
- Step enum (15 variants: Goto, Click, Fill, Type, Press, Wait, Extract, Evaluate, Check, Uncheck, Select, Scroll, Screenshot, Content, Sleep)
- ScriptResult struct
- parse_steps() — YAML → Vec<Step>
- parse_selector_value(), parse_extract()
- execute_script() — 단일 탭에서 전체 스텝 실행
  - OxicodeTab을 TabGuard로 감쌈
  - deadline 기반 timeout
  - max_script_steps 제한
- BrowseScriptTool (AgentTool)
  - name: "browse_script"
  - params: script, timeout
  - 파일 경로 감지 (존재하면 파일에서 로드)
  - 스텝별 실행:
    - Select: helpers::js_set_select_value()
    - Check: helpers::js_check()
    - Uncheck: helpers::js_uncheck()
    - Screenshot: tab.screenshot() → base64 PNG
    - Content: tab.content() → markdown
- #[cfg(test)] mod tests (10개 테스트)
```

### B4. `oxibrowser_backend.rs` (약 260줄, feature-gated)

**역할**: BrowserEngine trait의 oxibrowser-core 구현체
**feature**: `native-browser`

```
포함 내용:
- OxicodeBrowserEngine { browser: oxibrowser_core::Browser }
  - new(config) / new_default()
  - BrowserEngine impl: fetch, extract_links, query_all, screenshot, new_tab, close
- OxicodeTab { inner: oxibrowser_core::Tab }
  - BrowserTab impl: goto, click, type, fill, press, wait_for, content, query_all, evaluate, screenshot, close
```

---

## Phase C: 비전 라우팅

### C1. `router/signals.rs` — VisionSignal 추가 (약 100줄 추가)

기존 StructuralSignal, BehavioralSignal, ContextBudgetSignal 아래에 추가:

```rust
pub struct VisionSignal {
    pub recent_image_count: usize,
    pub has_image_in_latest_turn: bool,
    pub image_producing_tools: Vec<String>,
}

impl VisionSignal {
    pub fn extract(messages: &[Message], window: usize) -> Self
    pub fn requires_vision(&self) -> bool
    pub fn normalized(&self) -> f64  // 0→0.0, 1→0.7, 2+→0.9~1.0
}

// 10개 테스트:
// no_images, user_image, tool_result_image, browse_screenshot,
// normalized_zero, normalized_single, normalized_multiple,
// window_respected, text_only_tool_result
```

### C2. `router/types.rs` — 3곳 수정

```rust
// ScoringWeights: vision 필드 추가
pub struct ScoringWeights {
    pub structural: f64,      // 기존 0.40 → 0.35
    pub behavioral: f64,      // 유지 0.35
    pub context_budget: f64,  // 기존 0.25 → 0.20
    pub vision: f64,          // 신규 0.10
}

// RoutingDecision: vision 필드 2개 추가
pub struct RoutingDecision {
    // ... 기존 필드 ...
    pub is_vision_triggered: bool,
    pub vision_images: usize,
}
```

### C3. `router/scoring.rs` — compute_score 시그니처 변경

```rust
// 기존:
pub fn compute_score(s, b, c, weights) -> f64

// 변경:
pub fn compute_score(s, b, c, vision: Option<&VisionSignal>, weights) -> f64
// vision이 None이면 기존과 동일 (v_raw = 0.0)
// vision이 Some이면 vision 가중치 추가
```

### C4. `router/mod.rs` — 3가지 추가

1. **re-export**: `pub use signals::VisionSignal;`
2. **route_with_vision()**: `RouterPipeline`에 vision-aware 라우팅 메서드
3. **ensure_vision_model()**: `RouterProvider`에 비전 모델 필터
   - 1순위: 현재 모델이 비전 지원 → 그대로
   - 2순위: fallback에서 비전 모델 → 교체
   - 3순위: 상위 티어에서 비전 모델 → 승격
   - 4순위: 없음 → 경고 후 원래 유지
4. **stream()**: VisionSignal.extract → route_with_vision → ensure_vision_model

### C5. `store/router_config.rs` — ScoringWeights.vision

```rust
pub struct ScoringWeights {
    pub structural: f64,   // 0.35
    pub behavioral: f64,   // 0.35
    pub context_budget: f64, // 0.20
    pub vision: f64,       // 0.10 (신규)
}

// parse_weights()도 vision 필드 추가
```

### C6. `cli/main.rs` + `router_integration.rs`

ScoringWeights 생성 시 `vision` 필드 추가:
```rust
oxicode_ai::router::ScoringWeights {
    structural: ...,
    behavioral: ...,
    context_budget: ...,
    vision: store_cfg.weights().vision,  // 추가
}
```

---

## Phase D: Cargo.toml + 테스트

### D1. `oxicode-agent/Cargo.toml` feature 확인

```toml
[features]
default = []
native-browser = ["oxibrowser-core", "serde_yaml"]
```

### D2. 테스트

```
browse/tests.rs — engine 타입 serde, BrowserError display
helpers — parse_link_values, format_links, parse_element_values, JS 빌더
tab_guard — close, into_inner, drop_warn, tab_access
browse_script — parse_steps 10개 테스트, JS 헬퍼 3개
router/signals — VisionSignal 10개 테스트
router/scoring — vision 가중치 2개 테스트
```

---

## 파일별 예상 줄수

| 파일 | 줄수 | 신규/수정 |
|------|------|-----------|
| `browse/engine.rs` | ~175 | 신규 |
| `browse/mod.rs` | ~50 | 신규 |
| `browse/browse_tool.rs` | ~170 | 신규 |
| `browse/browse_extract_tool.rs` | ~170 | 신규 |
| `browse/browse_script_tool.rs` | ~480 | 신규 |
| `browse/oxibrowser_backend.rs` | ~260 | 신규 |
| `browse/tests.rs` | ~80 | 신규 |
| `tools.rs` | +10 | 수정 |
| `router/signals.rs` | +100 | 수정 |
| `router/types.rs` | +15 | 수정 |
| `router/scoring.rs` | +30 | 수정 |
| `router/mod.rs` | +90 | 수정 |
| `router_config.rs` | +5 | 수정 |
| `cli/main.rs` | +1 | 수정 |
| `cli/router_integration.rs` | +1 | 수정 |
| **총계** | **~1,637** | |

## 실행 순서 요약

```
A1: engine.rs       — 기반 trait (다른 모든 것의 의존)
A2: mod.rs          — 모듈 연결
A3: tools.rs 수정   — pub mod browse 추가
B1: browse_tool.rs
B2: browse_extract_tool.rs
B3: browse_script_tool.rs
B4: oxibrowser_backend.rs
C1: router/signals.rs    — VisionSignal
C2: router/types.rs      — ScoringWeights.vision
C3: router/scoring.rs    — compute_score vision
C4: router/mod.rs        — route_with_vision + ensure_vision_model
C5: router_config.rs     — vision 파싱
C6: cli 2파일            — vision 매핑
D:  tests.rs + clippy
```
