# Browser Improvements Design

> oxi 내장 브라우저 기능 개선 설계
> Status: **Draft** | Date: 2026-05-25

## 0. 현재 문제 요약

```
Severity:  ████████░░  CRITICAL  (P0)
           ██████░░░░  HIGH     (P1)
           ████░░░░░░  MEDIUM   (P2)
           ██░░░░░░░░  LOW      (P3)
```

| # | 문제 | 심각도 | 영향 범위 |
|---|------|--------|-----------|
| 1 | Tab 생명주기 누수 (close 실패 시 탭 유출) | P0 Critical | 메모리 누수, 장기 세션 크래시 |
| 2 | `BrowseTool`이 `links` 포맷에서 이미 닫힌 탭으로 재요청 | P0 Critical | use-after-close, 의미 없는 재렌더링 |
| 3 | `BrowseExtractTool`이 engine-level 메서드로 별도 탭 열어 재렌더링 | P0 Critical | 이중 렌더링, 성능 2배 낭비 |
| 4 | `Select` 스텝이 value 무시 (`_value`) | P1 High | 폼 자동화 불가능 |
| 5 | `Screenshot` 스텝이 no-op | P1 High | 스크립트에서 스크린샷 불가능 |
| 6 | `Check/Uncheck`가 토글이 아닌 click로 구현 | P1 High | 체크박스 상태 보장 안 됨 |
| 7 | 탭 동시성 제한 없음 | P1 High | 리소스 고갈 |
| 8 | 렌더 캐시 없음 (동일 URL 반복 요청 시 매번 렌더) | P2 Medium | 네트워크/렌더 비용 낭비 |
| 9 | `wait_for` 타임아웃 10초 하드코딩 | P2 Medium | 느린 SPA 대기 불가 |
| 10 | 테스트 커버리지 5% 미만 | P2 Medium | 회귀 방어 없음 |
| 11 | `BrowseScriptTool`이 feature gate 뒤에만 존재 | P3 Low | 커스텀 엔진에서 스크립트 사용 불가 |
| 12 | `BrowseTab` trait이 `Clone` 미구현 | P3 Low | 멀티 에이전트 탭 공유 불가 |

---

## 1. Phase 1 — Critical Bug Fixes (P0)

> 예상 기간: 1-2일
> 목표: 올바른 Tab 생명주기 + 이중 렌더링 제거

### 1.1 Guard 패턴으로 Tab 생명주기 보장

**문제**: 현재 `let _ = tab.close().await;`로 close 실패를 무시함.
에러 발생 경로에서 탭이 절대 닫히지 않을 수 있음.

**해결**: `TabGuard` — RAII wrapper로 탭 생명주기를 강제.

```rust
// oxi-agent/src/tools/browse/tab_guard.rs (신규 파일)

use super::engine::{BrowserTab, BrowserError};

/// RAII guard that ensures a tab is closed when dropped.
///
/// Usage:
/// ```ignore
/// let mut guard = TabGuard::new(engine.new_tab().await?);
/// let page = guard.tab().goto(url).await?;
/// // ... work with tab ...
/// // guard drops here, tab is closed (or warned on failure)
/// ```
pub struct TabGuard {
    tab: Option<Box<dyn BrowserTab>>,
    /// Track whether close() was already called explicitly.
    explicitly_closed: bool,
}

impl TabGuard {
    pub fn new(tab: Box<dyn BrowserTab>) -> Self {
        Self {
            tab: Some(tab),
            explicitly_closed: false,
        }
    }

    /// Access the underlying tab.
    pub fn tab(&self) -> &dyn BrowserTab {
        self.tab.as_ref().expect("TabGuard already consumed")
    }

    /// Explicitly close the tab and consume the guard.
    /// Returns Ok(()) even if close fails (logs warning instead).
    pub async fn close(mut self) {
        self.explicitly_closed = true;
        if let Some(tab) = self.tab.take() {
            if let Err(e) = tab.close().await {
                tracing::warn!("Tab close failed: {}", e);
            }
        }
    }

    /// Take ownership of the tab without closing it.
    /// Useful when transferring tab ownership (e.g., for multi-step scripts).
    pub fn into_inner(mut self) -> Box<dyn BrowserTab> {
        self.explicitly_closed = true;
        self.tab.take().expect("TabGuard already consumed")
    }
}

impl Drop for TabGuard {
    fn drop(&mut self) {
        if !self.explicitly_closed {
            // Tab was not explicitly closed — this is a leak.
            // We can't call async close() in drop, so log a warning.
            tracing::warn!(
                "TabGuard dropped without explicit close — tab may leak. \
                 Call .close().await or .into_inner() to prevent this."
            );
        }
    }
}
```

### 1.2 BrowseTool: links 포맷 이중 렌더링 제거

**문제**: `BrowseTool::execute()`에서 `links` 포맷 시:
1. 탭 열기 + `goto(url)` → 첫 번째 렌더링
2. `self.engine.extract_links(url)` → 두 번째 탭 열기 + 두 번째 렌더링

```rust
// BEFORE (browse_tool.rs, "links" arm):
"links" => {
    let links = self.engine.extract_links(url).await  // ← NEW TAB + RE-RENDER!
        .map_err(|e| e.to_string())?;
    // ...
}
```

**해결**: 이미 열린 탭에서 JS로 링크 추출.

```rust
// AFTER:
"links" => {
    let js = r#"(function() {
        var links = document.querySelectorAll('a[href]');
        return Array.from(links).map(function(a) {
            return { text: a.textContent.trim(), href: a.href };
        });
    })()"#;
    let value = tab.tab().evaluate(js).await.map_err(|e| e.to_string())?;
    let links = parse_link_values(value);
    format_links(&links)
}
```

### 1.3 BrowseExtractTool: 탭 재사용

**문제**: `BrowseExtractTool`이 `extract_links`/`query_all` 호출 시
engine-level 메서드를 사용하여 **별도 탭을 열고 URL을 다시 렌더링**.

```rust
// BEFORE (browse_extract_tool.rs):
"links" => {
    let links = self.engine.extract_links(url).await  // ← NEW TAB!
        .map_err(|e| e.to_string())?;
}
"elements" => {
    let elements = self.engine.query_all(url, selector).await  // ← NEW TAB!
        .map_err(|e| e.to_string())?;
}
```

**해결**: 모든 추출을 이미 열린 탭에서 JS `evaluate()`로 수행.

```rust
// AFTER: 공통 추출 헬퍼
impl BrowseExtractTool {
    /// Extract from the already-loaded tab via JS.
    async fn extract_from_tab(
        tab: &dyn BrowserTab,
        selector: &str,
        extract: &str,
        all: bool,
    ) -> Result<String, BrowserError> {
        match extract {
            "links" => {
                let js = r#"(function() {
                    var links = document.querySelectorAll('a[href]');
                    return Array.from(links).map(function(a) {
                        return { text: a.textContent.trim(), href: a.href };
                    });
                })()"#;
                let value = tab.evaluate(js).await?;
                Ok(format_link_json(value, all))
            }
            "elements" => {
                let js = build_element_query_js(selector);
                let value = tab.evaluate(&js).await?;
                Ok(format_element_json(value, all))
            }
            "markdown" | "text" => {
                let texts = tab.query_all(selector).await?;
                let texts = if all { texts } else { texts.into_iter().take(1).collect() };
                let sep = if extract == "markdown" { "\n\n" } else { "\n" };
                Ok(texts.join(sep))
            }
            _ => Err(BrowserError::DomError(format!("Unknown extract mode: {}", extract))),
        }
    }
}
```

### 1.4 BrowseTool: 스크린샷 이중 렌더링 제거

**문제**: `want_screenshot` 시 `self.engine.screenshot(url, 800)`을 호출하여
별도 탭에서 **URL을 세 번째로 렌더링**.

```rust
// BEFORE (browse_tool.rs):
if want_screenshot {
    match self.engine.screenshot(url, 800).await {  // ← 3rd render!
```

**해결**: 이미 열린 탭에서 스크린샷.

```rust
// AFTER:
if want_screenshot {
    match tab.tab().screenshot(800).await {
        Ok(png) => {
            let b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, &png,
            );
            let img = oxi_ai::ContentBlock::Image(
                oxi_ai::ImageContent::new(b64, "image/png"),
            );
            result = result.with_content_blocks(vec![img]);
        }
        Err(e) => {
            tracing::warn!("screenshot failed for {}: {}", final_url, e);
        }
    }
}
```

### 1.5 변경 후 파일 구조

```
oxi-agent/src/tools/browse/
├── mod.rs                       # + pub mod tab_guard;
├── engine.rs                    # 변경 없음
├── tab_guard.rs                 # 신규 (TabGuard)
├── oxibrowser_backend.rs        # 변경 없음
├── browse_tool.rs               # 수정 (TabGuard + links/ screenshot 탭 재사용)
├── browse_extract_tool.rs       # 수정 (TabGuard + engine 메서드 대신 tab.evaluate)
├── browse_script_tool.rs        # 수정 (TabGuard + Select/Screenshot 수정)
└── tests.rs                     # 확장
```

---

## 2. Phase 2 — Missing Step Implementations (P1)

> 예상 기간: 1일
> 목표: 모든 스텝 타입이 올바르게 동작

### 2.1 Select 스텝 구현

**문제**: `_value` 필드가 무시되고 `click()`만 호출.

**해결**: `<select>` 요소의 값을 JS로 설정.

```rust
// browse_script_tool.rs — Step::Select

Step::Select { selector, value } => {
    // Use JS to set the <select> value and dispatch change event
    let sel_json = serde_json::to_string(&selector)
        .unwrap_or_default();
    let val_json = serde_json::to_string(&value)
        .unwrap_or_default();
    let js = format!(
        r#"(function() {{
            var sel = document.querySelector({sel_json});
            if (!sel) throw new Error('Element not found: ' + {sel_json});
            sel.value = {val_json};
            sel.dispatchEvent(new Event('change', {{ bubbles: true }}));
        }})()"#,
    );
    if let Err(e) = tab.evaluate(&js).await {
        result.error = Some(format!(
            "Step {} select '{}' failed: {}", i + 1, selector, e
        ));
        break;
    }
}
```

### 2.2 Screenshot 스텝 구현

**문제**: `Step::Screenshot`이 완전한 no-op.

**해결**: 스크린샷을 base64로 수집하여 결과에 포함.

```rust
// ScriptResult에 필드 추가
struct ScriptResult {
    steps_executed: usize,
    extracts: Vec<(usize, Vec<String>)>,
    screenshots: Vec<(usize, Vec<u8>)>,  // ← 추가
    final_content: Option<String>,
    final_url: Option<String>,
    final_title: Option<String>,
    error: Option<String>,
}

// Step::Screenshot 구현
Step::Screenshot => {
    match tab.screenshot(800).await {
        Ok(png) => {
            result.screenshots.push((i, png));
        }
        Err(e) => {
            result.error = Some(format!(
                "Step {} screenshot failed: {}", i + 1, e
            ));
            break;
        }
    }
}

// 결과 포맷에 스크린샷 포함
for (step_idx, png) in &result.screenshots {
    output_parts.push(format!("### Step {} — Screenshot\n", step_idx + 1));
    output_parts.push(format!("![screenshot](data:image/png;base64,{})",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD, png
        )
    ));
}
```

### 2.3 Check/Uncheck 올바른 구현

**문제**: `click()`으로 토글 — 현재 상태를 모르므로 원하는 상태 보장 불가.

**해결**: JS로 현재 상태 확인 후 필요한 경우에만 클릭.

```rust
Step::Check { selector } => {
    let sel_json = serde_json::to_string(&selector).unwrap_or_default();
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel_json});
            if (!el) throw new Error('Element not found');
            if (!el.checked) el.click();
        }})()"#,
    );
    if let Err(e) = tab.evaluate(&js).await {
        result.error = Some(format!(
            "Step {} check '{}' failed: {}", i + 1, selector, e
        ));
        break;
    }
}

Step::Uncheck { selector } => {
    let sel_json = serde_json::to_string(&selector).unwrap_or_default();
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel_json});
            if (!el) throw new Error('Element not found');
            if (el.checked) el.click();
        }})()"#,
    );
    if let Err(e) = tab.evaluate(&js).await {
        result.error = Some(format!(
            "Step {} uncheck '{}' failed: {}", i + 1, selector, e
        ));
        break;
    }
}
```

---

## 3. Phase 3 — Resource Management (P1)

> 예상 기간: 1일
> 목표: 동시성 제한 + 탭 풀링

### 3.1 TabPool

```rust
// oxi-agent/src/tools/browse/tab_pool.rs (신규 파일)

use super::engine::{BrowserEngine, BrowserTab, BrowserError};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

/// Limits concurrent tabs and provides tab lifecycle management.
pub struct TabPool {
    engine: Arc<dyn BrowserEngine>,
    semaphore: Semaphore,
    active_tabs: AtomicUsize,
    max_tabs: usize,
}

impl TabPool {
    pub fn new(engine: Arc<dyn BrowserEngine>, max_tabs: usize) -> Self {
        Self {
            engine,
            semaphore: Semaphore::new(max_tabs),
            active_tabs: AtomicUsize::new(0),
            max_tabs,
        }
    }

    /// Acquire a tab permit and open a new tab.
    /// Waits if the pool is at capacity.
    pub async fn acquire(&self) -> Result<PooledTab, BrowserError> {
        let permit = self.semaphore.acquire().await
            .map_err(|_| BrowserError::EngineClosed)?;
        let tab = self.engine.new_tab().await?;
        let active = self.active_tabs.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(active, max = self.max_tabs, "tab opened");
        Ok(PooledTab {
            tab: Some(tab),
            permit: Some(permit),
            active_tabs: &self.active_tabs,
        })
    }

    /// Current number of active tabs.
    pub fn active(&self) -> usize {
        self.active_tabs.load(Ordering::Relaxed)
    }
}

/// A tab that releases its permit on drop.
pub struct PooledTab<'a> {
    tab: Option<Box<dyn BrowserTab>>,
    permit: Option<tokio::sync::SemaphorePermit<'a>>,
    active_tabs: &'a AtomicUsize,
}

impl<'a> PooledTab<'a> {
    pub fn tab(&self) -> &dyn BrowserTab {
        self.tab.as_ref().expect("tab already released")
    }
}

impl<'a> Drop for PooledTab<'a> {
    fn drop(&mut self) {
        self.active_tabs.fetch_sub(1, Ordering::Relaxed);
        // Permit is released automatically when dropped
        // Note: can't call async close() in drop
        // TabGuard handles async close via explicit .close().await
        drop(self.permit.take());
    }
}
```

### 3.2 도구에 TabPool 주입

```rust
// BrowseTool, BrowseExtractTool, BrowseScriptTool 모두
// Arc<dyn BrowserEngine> 대신 Arc<TabPool> 사용

pub struct BrowseTool {
    pool: Arc<TabPool>,
}

impl BrowseTool {
    pub fn new(pool: Arc<TabPool>) -> Self {
        Self { pool }
    }
}

// execute 내부:
let tab = self.pool.acquire().await
    .map_err(|e| format!("Tab pool exhausted: {}", e))?;
let guard = TabGuard::new(tab.into_inner());
// ... 작업 ...
guard.close().await;
```

### 3.3 기본값

```rust
// OxiBuilder / App 초기화 시
let pool = TabPool::new(engine, /* max_tabs = */ 4);
```

`settings.toml`:
```toml
[browser]
enabled = true
max_tabs = 4       # 동시 열린 탭 수 제한
```

---

## 4. Phase 4 — Render Cache (P2)

> 예상 기간: 1일
> 목표: 동일 URL 반복 요청 시 캐시된 결과 반환

### 4.1 RenderCache

```rust
// oxi-agent/src/tools/browse/render_cache.rs (신규 파일)

use super::engine::PageContent;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use std::time::{Duration, Instant};

/// TTL-based cache for rendered page content.
pub struct RenderCache {
    entries: RwLock<HashMap<String, CachedPage>>,
    ttl: Duration,
    max_entries: usize,
}

struct CachedPage {
    content: PageContent,
    inserted_at: Instant,
}

impl RenderCache {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    /// Get cached content for a URL, if still fresh.
    pub fn get(&self, url: &str) -> Option<PageContent> {
        let entries = self.entries.read();
        entries.get(url).and_then(|cached| {
            if cached.inserted_at.elapsed() < self.ttl {
                Some(cached.content.clone())
            } else {
                None
            }
        })
    }

    /// Store rendered content for a URL.
    pub fn insert(&self, url: &str, content: PageContent) {
        let mut entries = self.entries.write();
        // Evict expired entries if at capacity
        if entries.len() >= self.max_entries {
            entries.retain(|_, v| v.inserted_at.elapsed() < self.ttl);
        }
        // If still at capacity, remove oldest
        if entries.len() >= self.max_entries {
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, v)| v.inserted_at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }
        entries.insert(
            url.to_string(),
            CachedPage {
                content,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        self.entries.write().clear();
    }
}
```

### 4.2 BrowseTool에 캐시 통합

```rust
pub struct BrowseTool {
    pool: Arc<TabPool>,
    cache: Arc<RenderCache>,
}

// execute 내부:
// 1. 캐시 확인 (wait_for, selector가 없을 때만)
if wait_for.is_none() && selector.is_none() {
    if let Some(cached) = self.cache.get(url) {
        tracing::debug!(url = %url, "cache hit");
        return Ok(AgentToolResult::success(format_output(&cached, format))
            .with_metadata(json!({
                "url": cached.url,
                "title": cached.title,
                "status": cached.status,
                "cached": true,
            })));
    }
}

// 2. 캐시 미스 → 렌더
let page = guard.tab().goto(url).await...;

// 3. 캐시에 저장 (wait_for가 없을 때만)
if wait_for.is_none() {
    self.cache.insert(url, page.clone());
}
```

---

## 5. Phase 5 — Configurable Timeouts (P2)

> 예상 기간: 0.5일
> 목표: 하드코딩된 타임아웃 제거

### 5.1 BrowseConfig

```rust
// oxi-agent/src/tools/browse/config.rs (신규 파일)

/// Configuration for browser tools behavior.
#[derive(Debug, Clone)]
pub struct BrowseConfig {
    /// Default wait_for timeout in milliseconds.
    pub default_wait_timeout_ms: u64,
    /// Default page load timeout in seconds.
    pub page_timeout_secs: u64,
    /// Screenshot width in pixels.
    pub screenshot_width: u32,
    /// Maximum script steps per execution.
    pub max_script_steps: usize,
    /// Render cache TTL in seconds (0 = disabled).
    pub cache_ttl_secs: u64,
    /// Maximum render cache entries.
    pub cache_max_entries: usize,
    /// Maximum concurrent tabs.
    pub max_concurrent_tabs: usize,
    /// Maximum output size in bytes (truncation threshold).
    pub max_output_bytes: usize,
}

impl Default for BrowseConfig {
    fn default() -> Self {
        Self {
            default_wait_timeout_ms: 10_000,  // 10s
            page_timeout_secs: 30,
            screenshot_width: 800,
            max_script_steps: 100,
            cache_ttl_secs: 300,              // 5min
            cache_max_entries: 50,
            max_concurrent_tabs: 4,
            max_output_bytes: 512_000,        // 512KB
        }
    }
}
```

### 5.2 settings.toml 매핑

```toml
[browser]
enabled = true
max_tabs = 4
wait_timeout_ms = 10000
page_timeout_secs = 30
cache_ttl_secs = 300
max_cache_entries = 50
max_output_bytes = 512000
```

### 5.3 BrowseScriptTool wait 타임아웃

```rust
// BEFORE:
Step::Wait { selector } => {
    if let Err(e) = tab.wait_for(&selector, 10_000).await {  // ← hardcoded
```

```rust
// AFTER:
Step::Wait { selector } => {
    if let Err(e) = tab.wait_for(&selector, config.default_wait_timeout_ms).await {
```

---

## 6. Phase 6 — Feature Gate Restructuring (P3)

> 예상 기간: 0.5일
> 목표: BrowseScriptTool을 모든 엔진에서 사용 가능하게

### 6.1 문제

현재 `BrowseScriptTool`이 `#[cfg(feature = "native-browser")]` 뒤에 있어서
커스텀 `BrowserEngine` 구현체를 사용할 때 스크립트 도구를 사용할 수 없음.

```rust
// oxi-agent/src/tools/browse/mod.rs
#[cfg(feature = "native-browser")]        // ← 문제
pub mod browse_script_tool;

#[cfg(feature = "native-browser")]        // ← 문제
pub use browse_script_tool::BrowseScriptTool;
```

### 6.2 해결

`browse_script_tool.rs`는 `oxibrowser-core`에 의존하지 않음 — 
`BrowserEngine` trait만 사용. feature gate를 제거:

```rust
// AFTER:
pub mod browse_script_tool;
pub use browse_script_tool::BrowseScriptTool;
```

`serde_yaml` 의존성을 `native-browser` feature에서 분리:

```toml
# oxi-agent/Cargo.toml
[features]
default = []
native-browser = ["oxibrowser-core"]
# serde_yaml은 이제 항상 포함 (script 파싱에 필요)
```

또는 `serde_yaml`을 `browse-script` feature로 분리:

```toml
[features]
default = []
native-browser = ["oxibrowser-core", "browse-script"]
browse-script = ["serde_yaml"]
```

SDK와 CLI에서:

```rust
// oxi-sdk/src/tool_factory.rs
pub fn browse_tools(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(engine.clone()));
    registry.register(BrowseExtractTool::new(engine.clone()));
    #[cfg(feature = "browse-script")]
    registry.register(BrowseScriptTool::new(engine));
    Arc::new(registry)
}
```

---

## 7. Phase 7 — Test Infrastructure (P2)

> 예상 기간: 2일
> 목표: 80%+ 코드 커버리지

### 7.1 MockBrowserEngine

```rust
// oxi-agent/src/tools/browse/mock_engine.rs (신규, #[cfg(test)])

use super::engine::*;

pub struct MockBrowserEngine {
    pages: std::collections::HashMap<String, PageContent>,
    links: std::collections::HashMap<String, Vec<LinkInfo>>,
}

impl MockBrowserEngine {
    pub fn new() -> Self {
        Self {
            pages: std::collections::HashMap::new(),
            links: std::collections::HashMap::new(),
        }
    }

    pub fn with_page(mut self, url: &str, content: PageContent) -> Self {
        self.pages.insert(url.to_string(), content);
        self
    }

    pub fn with_links(mut self, url: &str, links: Vec<LinkInfo>) -> Self {
        self.links.insert(url.to_string(), links);
        self
    }
}

#[async_trait]
impl BrowserEngine for MockBrowserEngine {
    async fn fetch(&self, url: &str) -> Result<PageContent, BrowserError> {
        self.pages.get(url).cloned()
            .ok_or_else(|| BrowserError::NavigationFailed(
                format!("Mock: no page for {}", url)
            ))
    }

    async fn extract_links(&self, url: &str) -> Result<Vec<LinkInfo>, BrowserError> {
        self.links.get(url).cloned()
            .ok_or_else(|| BrowserError::DomError(
                format!("Mock: no links for {}", url)
            ))
    }

    async fn query_all(&self, url: &str, selector: &str)
        -> Result<Vec<ElementInfo>, BrowserError>
    {
        // Return mock elements based on selector
        Ok(vec![])
    }

    async fn screenshot(&self, _url: &str, _width: u32)
        -> Result<Vec<u8>, BrowserError>
    {
        Ok(vec![0x89, 0x50, 0x4E, 0x47])  // fake PNG header
    }

    async fn new_tab(&self) -> Result<Box<dyn BrowserTab>, BrowserError> {
        Ok(Box::new(MockTab {
            current_url: String::new(),
            pages: self.pages.clone(),
        }))
    }

    async fn close(&self) -> Result<(), BrowserError> {
        Ok(())
    }
}

struct MockTab {
    current_url: String,
    pages: std::collections::HashMap<String, PageContent>,
}

#[async_trait]
impl BrowserTab for MockTab {
    async fn goto(&mut self, url: &str) -> Result<PageContent, BrowserError> {
        self.current_url = url.to_string();
        self.pages.get(url).cloned()
            .ok_or_else(|| BrowserError::NavigationFailed(
                format!("Mock: no page for {}", url)
            ))
    }
    // ... 기타 메서드 mock 구현
}
```

### 7.2 테스트 매트릭스

```
oxi-agent/src/tools/browse/tests.rs
│
├── engine_tests          (기존 — 유지)
│   ├── page_content_empty
│   ├── page_content_serde_roundtrip
│   ├── link_info_serde
│   ├── element_info_serde
│   └── browser_error_display
│
├── tab_guard_tests       (신규)
│   ├── test_guard_close_success
│   ├── test_guard_drop_without_close_warns
│   └── test_guard_into_inner
│
├── browse_tool_tests     (신규)
│   ├── test_browse_markdown_default
│   ├── test_browse_html_format
│   ├── test_browse_links_format_uses_single_tab
│   ├── test_browse_text_with_selector
│   ├── test_browse_wait_for_selector
│   ├── test_browse_screenshot_includes_image
│   ├── test_browse_screenshot_failure_nonfatal
│   ├── test_browse_missing_url_param
│   └── test_browse_navigation_failure
│
├── browse_extract_tests  (신규)
│   ├── test_extract_text_default
│   ├── test_extract_links_from_loaded_tab
│   ├── test_extract_elements_json
│   ├── test_extract_first_only
│   └── test_extract_missing_selector
│
├── browse_script_tests   (신규)
│   ├── test_parse_simple_goto
│   ├── test_parse_fill_with_selector_value
│   ├── test_parse_extract_with_all_flag
│   ├── test_parse_unknown_step_fails
│   ├── test_parse_empty_steps_fails
│   ├── test_parse_invalid_yaml_fails
│   ├── test_script_goto_and_extract
│   ├── test_script_form_fill_flow
│   ├── test_script_select_sets_value
│   ├── test_script_check_uncheck_state
│   ├── test_script_screenshot_captures
│   ├── test_script_timeout_stops_execution
│   ├── test_script_file_path_loading
│   └── test_script_partial_result_on_error
│
├── render_cache_tests    (신규)
│   ├── test_cache_hit_within_ttl
│   ├── test_cache_miss_after_ttl
│   ├── test_cache_eviction_at_capacity
│   └── test_cache_clear
│
└── tab_pool_tests        (신규)
    ├── test_pool_acquire_and_release
    ├── test_pool_respects_max_tabs
    └── test_pool_active_count
```

---

## 8. Phase 8 — BrowserTab Trait 개선 (P3)

> 예상 기간: 1일
> 목표: 탭 공유 + 멀티 에이전트 지원

### 8.1 BrowserTab::url() 추가

```rust
#[async_trait]
pub trait BrowserTab: Send + Sync {
    /// Current page URL.
    async fn url(&self) -> Result<String, BrowserError>;

    // ... 기존 메서드
}
```

### 8.2 향후 확장: Tab Handle

```rust
/// A shareable handle to an open tab.
/// Multiple agents can reference the same tab for collaborative browsing.
pub struct TabHandle {
    id: String,
    engine: Arc<dyn BrowserEngine>,
}

impl TabHandle {
    /// Get the tab's current URL.
    pub fn id(&self) -> &str { &self.id }
}

// MessageBus를 통한 탭 공유 (향후)
// agent_a.publish(TabShared { handle, url }).await
// agent_b receives and can interact with the same tab
```

---

## 9. 구현 우선순위 및 일정

```
Week 1:
┌─────────────────────────────────────────────────────────┐
│ Day 1-2: Phase 1 — Critical Bug Fixes                   │
│   TabGuard, 이중 렌더링 제거, 탭 재사용                   │
│   → cargo test -p oxi-agent --features native-browser    │
├─────────────────────────────────────────────────────────┤
│ Day 3:   Phase 2 — Missing Step Implementations          │
│   Select, Screenshot, Check/Uncheck                      │
│   → 수동 테스트로 폼 자동화 검증                          │
├─────────────────────────────────────────────────────────┤
│ Day 4:   Phase 3 — Resource Management                   │
│   TabPool, 동시성 제한                                    │
├─────────────────────────────────────────────────────────┤
│ Day 5:   Phase 5 — Configurable Timeouts                 │
│   BrowseConfig, settings.toml 매핑                       │
└─────────────────────────────────────────────────────────┘

Week 2:
┌─────────────────────────────────────────────────────────┐
│ Day 1-2: Phase 7 — Test Infrastructure                   │
│   MockBrowserEngine + 전체 테스트 매트릭스                │
├─────────────────────────────────────────────────────────┤
│ Day 3:   Phase 4 — Render Cache                          │
│   RenderCache + BrowseTool 통합                          │
├─────────────────────────────────────────────────────────┤
│ Day 4:   Phase 6 — Feature Gate Restructuring            │
│   BrowseScriptTool 분리                                  │
├─────────────────────────────────────────────────────────┤
│ Day 5:   Phase 8 — BrowserTab Trait                      │
│   url() 추가 + 문서 업데이트                              │
└─────────────────────────────────────────────────────────┘
```

## 10. 검증 체크리스트

### Phase 1 완료 기준
- [ ] 모든 탭이 `TabGuard`로 관리됨
- [ ] `BrowseTool`에서 탭이 정확히 1개만 열림
- [ ] `BrowseExtractTool`에서 engine-level 메서드 호출 제거
- [ ] 스크린샷이 이미 열린 탭에서 캡처됨
- [ ] `cargo clippy --workspace --all-features -- -D warnings` 통과

### Phase 2 완료 기준
- [ ] `<select>` 요소에 값 설정 동작
- [ ] 스크립트에서 스크린샷이 base64로 반환
- [ ] checkbox가 원하는 상태로 설정됨
- [ ] 수동 테스트: 로그인 폼 자동화 성공

### Phase 3 완료 기준
- [ ] 동시 탭 수가 max_tabs로 제한됨
- [ ] 초과 요청이 대기 후 처리됨
- [ ] active 탭 카운트가 정확함

### Phase 7 완료 기준
- [ ] `cargo test -p oxi-agent --features native-browser` 30개+ 테스트 통과
- [ ] MockBrowserEngine으로 전체 도구 테스트 가능
- [ ] CI에서 두 feature 모드 모두 green

---

## 11. 리스크

| 리스크 | 영향 | 대응 |
|--------|------|------|
| `TabGuard` drop에서 async close 불가 | 탭 누수 가능 | tracing::warn + Drop에서 카운터 경고 |
| oxibrowser-core API 변경 | 백엔드 깨짐 | 버전 고정 (0.11.0), 타입 변환 계층 유지 |
| `RenderCache` 메모리 사용 | 장기 세션에서 증가 | max_entries + TTL로 제한 |
| `BrowseScriptTool` feature 분리 시 serde_yaml 항상 포함 | 바이너리 크기 증가 | 50KB (미미), 또는 browse-script feature로 선택 |
| `MockTab::goto` 시그니처가 `&self`여야 함 | trait 호환성 | trait 시그니처 변경 또는 interior mutability |
