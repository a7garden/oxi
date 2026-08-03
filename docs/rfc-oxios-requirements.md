# RFC: oxicode-sdk 단일 의존 + Agent OS 브라우저 요구사항

> **From**: oxios (Agent OS)
> **To**: oxicode-sdk / oxicode-agent / oxibrowser-core

---

## Part 1: 단일 의존 구조

### 원칙

oxios는 **`oxicode-sdk`만을 유일한 oxicode 의존성**으로 가져야 한다. `oxicode-ai`, `oxicode-agent`, `oxibrowser-core`는 모두 SDK를 통해서만 접근.

```
# 목표
oxios → oxicode-sdk    ← 끝

# 현재 (제거해야 할 것)
oxios → oxicode-sdk
      → oxicode-ai          ← 제거
      → oxibrowser-core ← 제거
```

### 요청 1: Provider 구현체 re-export

`oxicode-ai`의 concrete provider 타입을 SDK에서 re-export.

```rust
// oxicode-sdk/src/lib.rs 에 추가:
pub use oxicode_ai::providers::{
    AnthropicProvider, OpenAiProvider, GoogleProvider, DeepSeekProvider,
    MistralProvider, VertexProvider, BedrockProvider, AzureProvider,
};
```

**이유**: `oxios-ouroboros` 테스트에서 `oxicode_ai::OpenAiProvider::with_base_url_and_key()` 직접 사용. `oxicode-ai` 의존 없이 `oxicode_sdk::OpenAiProvider`로 교체해야 함.

---

## Part 2: 브라우저 툴 아키텍처

### 현재 설계 (유지)

| 툴 | 용도 | 탭 수명 |
|----|------|---------|
| `browse` | 1회성 페이지 읽기 | per-request |
| `browse_extract` | CSS selector 데이터 추출 | per-request |
| `browse_script` | YAML 멀티스텝 자동화 | per-request |
| `browse_session` | **새로 필요** — 대화형 persistent 탭 | multi-request |

### 요청 2: `BrowseSessionTool` 추가

에이전트가 여러 툴콜에 걸쳐 하나의 브라우저 탭을 유지하며 작업.

```text
Agent → browse_session(action="open")            → 탭 생성
Agent → browse_session(action="goto", url=...)    → 이동
Agent → browse_session(action="fill", ...)        → 폼 입력
Agent → browse_session(action="click", ...)       → 제출
Agent → browse_session(action="content")          → 결과 읽기
Agent → browse_session(action="back")             → 뒤로
Agent → browse_session(action="close")            → 탭 종료
```

**구현**: `Mutex<Option<Box<dyn BrowserTab>>>`로 탭을 보관. `open`에서 생성, `close`에서 해제. drop 시 `tracing::warn!` (기존 `TabGuard` 패턴과 동일).

**Schema**:
```json
{
  "name": "browse_session",
  "parameters": {
    "action": {
      "enum": ["open", "goto", "back", "forward", "reload",
               "click", "fill", "type", "press",
               "select", "check", "uncheck",
               "wait_for", "content", "query_all", "evaluate",
               "screenshot", "close"]
    },
    "url": {},
    "selector": {},
    "value": {},
    "combo": {},
    "javascript": {},
    "timeout_ms": {},
    "width": {}
  }
}
```

### 요청 3: `BrowserTab` trait 확장

`back/forward/reload`는 default 메서드로 제공 (JS evaluate 기반). `select/check/uncheck`는 백엔드 구현 필요.

```rust
#[async_trait]
pub trait BrowserTab: Send + Sync {
    // — 기존 메서드 (변경 없음) —
    async fn goto(&self, url: &str) -> Result<PageContent, BrowserError>;
    async fn click(&self, selector: &str) -> Result<(), BrowserError>;
    async fn type_(&self, selector: &str, text: &str) -> Result<(), BrowserError>;
    async fn fill(&self, selector: &str, value: &str) -> Result<(), BrowserError>;
    async fn press(&self, combo: &str) -> Result<(), BrowserError>;
    async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<(), BrowserError>;
    async fn content(&self) -> Result<PageContent, BrowserError>;
    async fn query_all(&self, selector: &str) -> Result<Vec<String>, BrowserError>;
    async fn evaluate(&self, js: &str) -> Result<Value, BrowserError>;
    async fn screenshot(&self, width: u32) -> Result<Vec<u8>, BrowserError>;
    async fn close(&self) -> Result<(), BrowserError>;

    // — 새로 추가 —
    async fn back(&self) -> Result<PageContent, BrowserError> {
        let _ = self.evaluate("history.back()").await;
        self.content().await
    }

    async fn forward(&self) -> Result<PageContent, BrowserError> {
        let _ = self.evaluate("history.forward()").await;
        self.content().await
    }

    async fn reload(&self) -> Result<PageContent, BrowserError> {
        let _ = self.evaluate("location.reload()").await;
        self.content().await
    }

    async fn select_option(&self, selector: &str, value: &str) -> Result<(), BrowserError>;
    async fn check(&self, selector: &str) -> Result<(), BrowserError>;
    async fn uncheck(&self, selector: &str) -> Result<(), BrowserError>;
}
```

### 요청 4: `BrowserError` 배리언트 추가

```rust
pub enum BrowserError {
    // — 기존 —
    Navigation(String),
    ElementNotFound(String),
    Timeout(String),
    Evaluation(String),
    Screenshot(String),
    TabClosed(String),
    Backend(String),

    // — 추가 —
    #[error("no active session — call 'open' first")]
    NoActiveSession,
}
```

`browse_session`에서 `open` 없이 다른 액션을 호출했을 때 반환.

---

## Part 3: 설정 통합

### 요청 5: `BrowseConfig` 확장

Agent OS의 `config.toml` 브라우저 설정을 SDK `BrowseConfig`로 통합.

```rust
pub struct BrowseConfig {
    // — 기존 필드 유지 —
    pub default_wait_timeout_ms: u64,    // 10_000
    pub page_timeout_secs: u64,          // 30
    pub screenshot_width: u32,           // 800
    pub max_script_steps: usize,         // 100
    pub cache_ttl_secs: u64,             // 300
    pub cache_max_entries: usize,        // 50
    pub max_concurrent_tabs: usize,      // 4
    pub max_output_bytes: usize,         // 512_000

    // — 추가 요청 —
    pub user_agent: Option<String>,      // 커스텀 User-Agent
    pub obey_robots: bool,               // robots.txt 준수 (default: true)
    pub js_timeout_ms: u64,              // JS 실행 타임아웃 (default: 10_000)
}
```

---

## Part 4: SDK 진입점

### 요청 6: `AgentBuilder`에 `browsing_with_session` 추가

```rust
impl<'a> AgentBuilder<'a> {
    /// browse + browse_extract + browse_session
    pub fn browsing_with_session(self, engine: Arc<dyn BrowserEngine>) -> Self {
        self.tools.register(BrowseTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseExtractTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseSessionTool::new(engine));
        self
    }

    /// 위 + browse_script (native-browser feature)
    #[cfg(feature = "native-browser")]
    pub fn full_browsing(self, engine: Arc<dyn BrowserEngine>) -> Self {
        self.tools.register(BrowseTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseExtractTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseSessionTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseScriptTool::new(engine));
        self
    }
}
```

---

## 정리

| # | 요청 | 중요도 | 범위 |
|---|------|--------|------|
| 1 | Provider 구현체 re-export | **필수** | oxicode-sdk |
| 2 | `BrowseSessionTool` 구현 | **필수** | oxicode-agent |
| 3 | `BrowserTab` trait 확장 (back/forward/reload/select/check/uncheck) | **필수** | oxicode-agent, oxibrowser-core |
| 4 | `BrowserError::NoActiveSession` | **필수** | oxicode-agent |
| 5 | `BrowseConfig` 확장 (user_agent, obey_robots, js_timeout_ms) | 권장 | oxicode-agent |
| 6 | `AgentBuilder` 편의 메서드 | 권장 | oxicode-sdk |

필수 항목이 모두 구현되면 oxios에서:
- `oxicode-ai`, `oxibrowser-core` 직접 의존 제거
- `tools/browser/` 제거 → SDK 툴로 교체
- `BrowserApi` → `Arc<dyn BrowserEngine>` 전환
