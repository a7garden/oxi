# 설계 개정: 코드 검증 기반 수정 (v1 → v2)

> 상태: 개정 v1 (모든 하위 설계에 적용)
> 작성: 2026-06-19
> 선행: [`00-master-plan.md`](./00-master-plan.md) (v1) + 리뷰
> 목적: v1 설계 문서들의 코드 검증 결과를 반영, 실제 oxi API에 맞게 패턴을 수정

이 문서는 모든 하위 설계 문서(⑤~⑫)에 적용되는 **교차적(cross-cutting) 수정사항**을 정의한다. 각 하위 문서의 코드 스니펫은 이 개정 문서의 패턴으로 대체된다.

---

## 1. 🔴 P0: ToolContext — 능력(ability) 특성 주입 패턴

### 1.1 문제

v1 설계들은 `ToolContext`에 새 필드를 직접 추가한다고 가정했다 (`todo_state`, `session_writer`, `event_tx`, `agent_pool`, `lsp_writethrough`). **실제 코드는 능력 특성 주입 패턴을 사용한다.**

### 1.2 실제 ToolContext (`oxi-agent/src/tools.rs:77-94`, 검증 완료)

```rust
pub struct ToolContext {
    pub workspace_dir: PathBuf,
    pub root_dir: Option<PathBuf>,
    pub session_id: Option<String>,
    pub snapshot_store: Option<Arc<dyn oxi_hashline::SnapshotStore>>,
    pub memory: Option<Arc<dyn MemoryBackend>>,
    pub url_resolver: Option<Arc<dyn UrlResolver>>,
}
```

모든 확장은 `Option<Arc<dyn Trait>>` 능력 주입으로 이루어진다. `MemoryBackend`, `UrlResolver`가 이미 이 패턴이다.

### 1.3 수정된 설계 — 새 능력 특성들

todo, LSP, agent pool은 각각 능력 특성을 정의하고 `ToolContext`에 추가한다:

```rust
// oxi-agent/src/tools.rs — ToolContext 확장 (v2)

/// Todo 상태 접근 능력. todo 도구와 sticky panel이 공유.
pub trait TodoStateProvider: Send + Sync {
    fn get_phases(&self) -> Vec<TodoPhase>;
    fn apply_ops(&self, ops: &[TodoOp]) -> Result<Vec<TodoPhase>, String>;
}

/// LSP 관리자 접근 능력. lsp 도구와 writethrough가 사용.
pub trait LspProvider: Send + Sync {
    fn execute_action(&self, action: &LspAction) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
    fn writethrough(&self, path: &Path, content: &str) -> Pin<Box<dyn Future<Output = Result<FileDiagnosticsResult, String>> + Send + '_>>;
}

/// Agent 풀 접근 능력. Agent Hub와 서브에이전트 매칭이 사용.
pub trait AgentPoolProvider: Send + Sync {
    fn list_agents(&self) -> Vec<AgentInfo>;
    fn get_agent(&self, id: &str) -> Option<AgentInfo>;
}

pub struct ToolContext {
    // 기존 필드 (변경 없음)...
    pub workspace_dir: PathBuf,
    pub root_dir: Option<PathBuf>,
    pub session_id: Option<String>,
    pub snapshot_store: Option<Arc<dyn oxi_hashline::SnapshotStore>>,
    pub memory: Option<Arc<dyn MemoryBackend>>,
    pub url_resolver: Option<Arc<dyn UrlResolver>>,
    // ── v2 추가 (능력 주입) ──
    pub todo: Option<Arc<dyn TodoStateProvider>>,
    pub lsp: Option<Arc<dyn LspProvider>>,
    pub agent_pool: Option<Arc<dyn AgentPoolProvider>>,
}
```

```rust
// 빌더 메서드 (기존 패턴 준수)
impl ToolContext {
    pub fn with_todo(mut self, todo: Arc<dyn TodoStateProvider>) -> Self {
        self.todo = Some(todo);
        self
    }
    pub fn with_lsp(mut self, lsp: Arc<dyn LspProvider>) -> Self {
        self.lsp = Some(lsp);
        self
    }
    pub fn with_agent_pool(mut self, pool: Arc<dyn AgentPoolProvider>) -> Self {
        self.agent_pool = Some(pool);
        self
    }
}
```

### 1.4 적용 대상

| 문서 | v1 (오류) | v2 (수정) |
|---|---|---|
| 05-todo | `ctx.todo_state`, `ctx.session_writer`, `ctx.event_tx` | `ctx.todo: Option<Arc<dyn TodoStateProvider>>` |
| 06-panel | `AgentEvent::TodoUpdate` 직접 발생 | `TodoStateProvider`가 콜백으로 TUI 갱신 |
| 07-hub | `ctx.agent_pool`, `ctx.lifecycle_tx` | `ctx.agent_pool: Option<Arc<dyn AgentPoolProvider>>` |
| 08-commit | `ctx.provider`, `ctx.model` | `oxi_ai::high_level::complete(model, ctx, opts)` |
| 09-compaction | `self.inline_transformer` | `ContextTransformer` 능력 특성 |
| 10-lsp | `ctx.lsp_writethrough` | `ctx.lsp: Option<Arc<dyn LspProvider>>` |
| 12-hindsight | `ctx.memory_store` | `ctx.memory: Option<Arc<dyn MemoryBackend>>` (기존) |

---

## 2. 🔴 P0: ToolError — String 타입 별칭

### 2.1 실제 코드 (검증 완료)

```rust
// oxi-agent/src/tools.rs:173
pub type ToolError = String;
```

`ToolError`는 **단순 String**이다. 변형이 없다.

### 2.2 수정

모든 설계 문서에서 `ToolError::InvalidParams(...)`, `ToolError::ExecutionFailed(...)`를 제거하고 `String` 에러를 사용:

```rust
// v1 (오류)
return Err(ToolError::InvalidParams("content required".into()));
return Err(ToolError::ExecutionFailed(e.to_string()));

// v2 (수정)
return Err("content required".into());
return Err(e.to_string());
```

---

## 3. 🔴 P0: oxi_ai API — 검증된 시그니처

### 3.1 high_level::complete (검증 완료, `oxi-ai/src/high_level.rs:22`)

```rust
pub async fn complete(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<AssistantMessage, Error>
```

**provider 인자가 없다** — 글로벌 `ProviderRegistry`에서 자동 해결. v1 설계들의 `complete(provider, model, ...)`는 모두 오류.

```rust
// v2 올바른 사용법
use oxi_ai::{high_level::complete, Context, Message, UserMessage, Tool};

let mut ctx = Context::new()
    .with_system_prompt(system_prompt);
ctx.add_message(Message::User(UserMessage::new(user_text)));
ctx.tools = vec![analysis_tool];

let result = complete(model, &ctx, Some(StreamOptions {
    max_tokens: Some(2400),
    ..Default::default()
})).await?;
```

### 3.2 Tool (검증 완료, `oxi-ai/src/tools.rs:20`)

```rust
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
}

impl Tool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: JsonValue) -> Self;
}
```

`Default` 파생이 없다. `..Default::default()` 사용 불가.

```rust
// v2 올바른 사용법
let tool = Tool::new(
    "create_conventional_analysis",
    "Analyze a git diff and produce a conventional commit analysis",
    json!({"type": "object", "properties": {...}}),
);
```

### 3.3 Context (검증 완료, `oxi-ai/src/context.rs:8`)

```rust
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
}
```

`system_prompt` 필드로 직접 접근 가능. Hindsight의 `<memories>` 블록 주입 지점.

### 3.4 providers::stream (검증 완료, `oxi-ai/src/providers/mod.rs:281`)

```rust
pub async fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>
```

compaction inline imaging은 **이 stream 호출 직전**에 context를 변환해야 한다.

---

## 4. 🔴 P0: MemoryBackend — 기존 특성 재활용

### 4.1 실제 코드 (검증 완료, `oxi-agent/src/tools.rs:31-51`)

```rust
pub trait MemoryBackend: Send + Sync {
    fn put<'a>(&'a self, content: &'a str, kind: &'a str, subject: &'a str)
        -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;
    fn search<'a>(&'a self, query: &'a str, k: usize)
        -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>>;
    fn list<'a>(&'a self, subject: &'a str)
        -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>>;
    fn delete<'a>(&'a self, id: &'a str)
        -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>>;
}
```

이미 `put`/`search`/`list`/`delete`를 갖춘 완전한 인터페이스다. ⑩ Mnemopi는 **이 특성을 구현하는 구체체**여야 한다.

### 4.2 수정된 아키텍처

```
oxi-sdk MemoryStore 포트 (1차 ④)
        ↑ 구현
oxi-mnemopi Mnemopi (⑩ 백엔드)
        ↑ 브리지
oxi-agent MemoryBackend (기존 특성)
        ↑ 사용
memory_retain/recall/reflect/edit 도구 (⑨)
```

`ToolContext.memory: Option<Arc<dyn MemoryBackend>>`에 Mnemopi 브릿지를 주입. ⑩ Mnemopi 설계의 "MemoryStore 포트 충전" 섹션(§3)을 "MemoryBackend 구현"으로 대체.

### 4.3 Mnemopi → MemoryBackend 브리지

```rust
// oxi-cli/src/store/memory_bridge.rs
pub struct MnemopiMemoryBackend {
    mnemopi: Arc<Mnemopi>,
}

#[async_trait]
impl MemoryBackend for MnemopiMemoryBackend {
    async fn put(&self, content: &str, kind: &str, subject: &str) -> Result<String, String> {
        self.mnemopi.remember(content, RememberOptions {
            kind: kind.into(),
            scope: subject.into(),
            ..Default::default()
        }).await.map_err(|e| e.to_string())
    }
    // search, list, delete 동일 패턴
}
```

> **1차 ④ 관계**: `MemoryStore` 포트(oxi-sdk)는 SDK 소비자용. `MemoryBackend`(oxi-agent)는 도구용. Mnemopi는 **둘 다** 구현 — 포트는 SDK 직접 사용자에게, MemoryBackend는 oxi-cli 도구에.

---

## 5. 🔴 P0: 철학적 모순 정정 — 1차 "영구 제외" 재검토

### 5.1 1차 선언 (`omp-adoption/00-master-plan.md:32`)

> **영구 제외**: LSP 통합 · ... · omp commit · ... → "가벼운 임베더블 엔진" 원칙과 충돌

### 5.2 2차 재검토 근거

1차는 "가벼운 임베더블 엔진" 원칙을 LSP/Commit 도입의 정면 충돌로 보았다. 2차는 이를 **feature 게이트 + 독립 크레이트 격리**로 양립시킨다:

| 기능 | 1차 판정 | 2차 정정 | 근거 |
|---|---|---|---|
| LSP | 영구 제외 | **도입** (feature gate) | 독립 `oxi-lsp` 크레이트, `--features lsp` 미활성화 시 바이너리 크기 영향 0. 코딩 에이전트 핵심 기능 (rename 안전성) |
| Commit | 영구 제외 | **도입** (opt-in 도구) | `disabled_tools`로 비활성화 가능. LLM 비용이지만 `commit_tool_enabled: false` 기본 |
| DAP | 영구 제외 | **유지** (후순위) | LSP 안정화 후 별도 검토 |
| eval 커널 | 영구 제외 | **유지** | Python/Bun 런타임 의존. oxios 제품 |
| ACP | 영구 제외 | **유지** | 에디터 결합. 별도 제품 |

**정정**: 1차의 "영구 제외"는 과도했다. feature 게이트로 격리 가능한 기능(LSP, Commit)은 "가벼운 엔진" 원칙을 위반하지 않는다 — 미사용 시 바이너리에 포함되지 않기 때문. 반면 런타임 의존을 가져오는 것(eval, ACP, brush)은 여전히 제외.

> **1차 마스터 플랜 갱신 필요**: §0 "영구 제외" 목록에서 LSP/Commit을 제거하고 "feature-gated 도입"으로 이동.

---

## 6. 🟠 P1: snapcompact 네이티브 렌더러 — 검증 완료

### 6.1 확인 결과

omp `crates/pi-natives/src/snapcompact.rs` (1,194줄)는 **순수 Rust**로 확인됨:

- 공개 API: `render_snapcompact_png(text: String, options: SnapcompactRenderOptions) -> Result<Latin1String>`
- 폰트: 번들 BDF 파일 (`5x8.bdf`, `6x12.bdf`, `8x13.bdf`, `unscii-8.hex`) — **공개 도메인**
- 핵심 로직: `render_bitmap()`, Lanczos3 리샘플링, PNG 인코딩 — 모두 순수 Rust
- `#[napi]` 속성은 Node.js FFI 래퍼일 뿐 — 코어 로직과 무관

### 6.2 이식 전략 (수정)

`#[napi]` 속성과 `Latin1String` 반환을 제거하고, `Vec<u8>` (PNG 바이트)을 반환하도록 수정:

```rust
// oxi-ai/src/snapcompact/renderer.rs (이식 후)
pub fn render_snapcompact_png(
    text: &str,
    options: &SnapcompactRenderOptions,
) -> Result<Vec<u8>, SnapcompactError> {
    // omp render_snapcompact_png에서 #[napi] 제거, Latin1String → Vec<u8>
    // 코어 render_bitmap(), encode_png()는 그대로 재사용
}
```

폰트 파일은 `include_bytes!`로 번들. 라이선스 문제 없음 (공개 도메인).

---

## 7. 🟠 P1: Mnemopi 트랜잭션 — 설계 수정

### 7.1 문제

v1의 클로저 바운드 `F: FnOnce(&Connection) -> Result<T> + Send + 'static`는 참조 캡처를 불가능하게 함.

### 7.2 수정 — 직접 잠금 패턴

```rust
// v2 — tokio::sync::Mutex를 잡고 블록 내에서 직접 조작
impl MnemopiDb {
    pub async fn transaction<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&Connection) -> anyhow::Result<T> + Send,
        T: Send,
    {
        let mut conn = self.conn.lock().await;
        // rusqlite의 트랜잭션은 동기 — tokio::task::spawn_blocking 고려
        tokio::task::spawn_blocking(move || {
            conn.execute("BEGIN DEFERRED", [])?;
            let result = f(&conn);
            match &result {
                Ok(_) => { conn.execute("COMMIT", [])?; }
                Err(_) => { let _ = conn.execute("ROLLBACK", []); }
            }
            result
        }).await?
    }
}
```

> `rusqlite::Connection`은 `Send`이지만 동기 블로킹이므로 `spawn_blocking`으로 래핑하여 async 런타임 블로킹 방지.

---

## 8. 🟠 P1: bank 스코핑 — git 루트 사용

### 8.1 oxi 기존 자산 (검증 완료)

`oxi-cli/src/storage/resource_loader.rs:1342`에 `find_git_root(dir)`가 이미 존재한다:

```rust
pub fn find_git_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    let root = PathBuf::from("/");
    while current != root {
        if current.join(".git").exists() {
            return Some(current);
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    None
}
```

### 8.2 수정된 bank 스코핑 (`12-hindsight-memory.md` 대체)

```rust
// v2 — git 루트 기반 project label (omp #2412 수정 반영)
pub fn compute_bank_scope(config: &HindsightConfig, cwd: &Path) -> BankScope {
    let project_label = oxi_cli::storage::find_git_root(cwd)
        .and_then(|root| root.file_name()?.to_str().map(String::from))
        .unwrap_or_else(|| {
            cwd.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("default")
                .to_string()
        });
    // ... 나머지는 동일
}
```

---

## 9. 🟠 P1: mental-models 백엔드 — Mnemopi 스키마 확장

### 9.1 문제

`12-hindsight-memory.md`는 `memory.get_mental_model()` / `memory.save_mental_model()`을 호출하지만 `11-mnemopi-backend.md` 스키마에 mental-models 테이블이 없다.

### 9.2 수정 — Mnemopi 스키마에 mental_models 테이블 추가

```sql
-- 11-mnemopi-backend.md schema.rs에 추가
CREATE TABLE IF NOT EXISTS mental_models (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scope TEXT NOT NULL,              -- project_id
    content TEXT NOT NULL,            -- 큐레이션된 정신 모델 텍스트
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_mental_models_scope ON mental_models(scope);
```

Mnemopi 파사드에 mental-model CRUD 추가:

```rust
impl Mnemopi {
    pub async fn get_mental_model(&self, scope: &str) -> anyhow::Result<Option<String>> { ... }
    pub async fn save_mental_model(&self, scope: &str, content: &str) -> anyhow::Result<()> { ... }
    pub async fn get_mental_model_history(&self, scope: &str) -> anyhow::Result<Vec<MentalModelSnapshot>> { ... }
}
```

---

## 10. 🟠 P1: LSP 메시지 리더 — 버퍼 기반

### 10.1 문제

v1은 헤더를 1바이트씩 읽는다 (비효율).

### 10.2 수정 — `tokio::io::AsyncBufReadExt`

```rust
// v2 — read_until로 헤더 경계 탐색
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

async fn message_reader(self: Arc<Self>, stdout: ChildStdout) {
    let mut reader = tokio::io::BufReader::new(stdout);
    
    loop {
        // 헤더를 \r\n\r\n까지 읽기
        let mut header = Vec::new();
        loop {
            let n = reader.read_until(b'\n', &mut header).await.unwrap_or(0);
            if n == 0 { return; } // EOF
            if header.ends_with(b"\r\n\r\n") { break; }
            if header.len() > 8192 {
                tracing::warn!("LSP header too long, resetting");
                header.clear();
                continue;
            }
        }
        
        // Content-Length 파싱
        let header_str = String::from_utf8_lossy(&header);
        let content_length: usize = header_str
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: ")
                .and_then(|v| v.trim().parse().ok()))
            .unwrap_or(0);
        if content_length == 0 { continue; }
        
        // 본문 읽기
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await.unwrap_or(0);
        
        if let Ok(msg) = serde_json::from_slice::<jsonrpc::Message>(&body) {
            self.route_message(msg);
        }
    }
}
```

---

## 11. 🟠 P1: AgentHandle — 메타데이터 분리

### 11.1 수정 — 표시용 필드를 별도 구조체로

```rust
// v2 — 표시 메타데이터를 별도 구조체로 분리
#[derive(Debug, Clone, Default)]
pub struct AgentMetadata {
    pub display_name: String,
    pub kind: AgentKind,
    pub last_activity_ms: u64,
    pub current_task: Option<String>,
    pub session_file: Option<PathBuf>,
}

// AgentHandle은 메타데이터를 Arc<RwLock>으로 참조
#[derive(Clone)]
pub struct AgentHandle {
    // 기존 핵심 필드 (변경 없음)...
    agent_id: String,
    status: Arc<AtomicU8>,
    agent: Arc<oxi_agent::Agent>,
    config: Arc<RwLock<AgentConfig>>,
    metrics: Arc<AgentMetrics>,
    lifecycle_tx: broadcast::Sender<AgentLifecycleEvent>,
    // ── v2: 메타데이터는 별도 ──
    metadata: Arc<RwLock<AgentMetadata>>,
}

impl AgentHandle {
    pub fn metadata(&self) -> parking_lot::RwLockReadGuard<'_, AgentMetadata> {
        self.metadata.read()
    }
    pub fn touch_activity(&self) {
        self.metadata.write().last_activity_ms = now_ms();
    }
}
```

> `unread_irc` 필드 제거 — IRC 미구현, dead code.

---

## 12. 🟠 P1: Compaction async transform — 설계 완성

### 12.1 통합점 (수정)

provider stream 호출 직전에 `ContextTransformer` 실행:

```rust
// oxi-ai/src/high_level.rs에 transform 옵션 추가 (또는 agent_loop에서)

// agent_loop/streaming.rs — stream 호출 전
let context = if let Some(transformer) = &self.context_transformer {
    transformer.transform(context, model).await
} else {
    context.clone()
};

let stream = oxi_ai::providers::stream(model, &context, options).await?;
```

```rust
// ContextTransformer는 능력 특성 (ToolContext가 아닌 AgentLoop가 보유)
pub trait ContextTransformer: Send + Sync {
    fn transform<'a>(
        &'a self,
        context: &'a Context,
        model: &'a Model,
    ) -> Pin<Box<dyn Future<Output = Context> + Send + 'a>>;
}
```

> **위치**: `ContextTransformer`는 `AgentLoopConfig`에 `Option<Arc<dyn ContextTransformer>>`로 주입. ToolContext가 아님 — 컨텍스트 변환은 도구 실행이 아닌 스트림 호출 단계.

---

## 13. P2: 번호 정렬

### 13.1 수정 — 파일명을 원번호에 정렬

| 원번호 | 파일 (v2 제안) | 기능 |
|:-:|---|---|
| ⑤ | `05-todo-tool.md` | todo 도구 |
| ⑤b | `06-todo-panel.md` | sticky panel |
| ⑥ | `07-agent-hub.md` | Agent Hub |
| ⑦ | `08-compaction.md` | Compaction 모드 |
| ⑧ | `09-lsp.md` | LSP 통합 |
| ⑨ | `10-hindsight.md` | Hindsight 응용 |
| ⑩ | `11-mnemopi.md` | Mnemopi 백엔드 |
| ⑪ | `12-commit.md` | Commit 도구 |
| ⑫ | `13-mermaid.md` | Mermaid 렌더링 |

> 파일명 변경은 선택 — 내용의 상호 참조가 정확하면 파일명은 부차적. 각 문서 헤더의 원번호(⑤⑥⑦...)가 일차 식별자.

---

## 14. P2: 설정 중첩화

### 14.1 수정 — 기능별 TOML 섹션

```toml
# ~/.oxi/settings.toml (v2 제안)

[todo]
enabled = true
clear_delay_secs = 30
strikethrough_animation = true

[lsp]
enabled = false              # 무거운 의존, opt-in
format_on_write = true
diagnostics_on_write = true

[memory]
enabled = false
backend = "mnemopi"          # | "hindsight" | "none"
auto_recall = true
auto_retain = true
retain_every_n_turns = 3
reflect = false

[memory.mental_models]
enabled = true
auto_seed = true

[compaction]
strategy = "soft"            # | "snapcompact" | "hybrid"
snapcompact_enabled = false
snapcompact_inline = false

[commit]
enabled = false
default_dry_run = true
auto_changelog = true

[mermaid]
enabled = true
renderer = "auto"            # | "mmdc" | "builtin" | "disabled"
```

---

## 15. 적용 체크리스트

각 하위 문서에 대한 수정 체크리스트:

| 문서 | P0 수정 | P1 수정 | 상태 |
|---|---|---|:-:|
| 05-todo | ToolContext 능력, ToolError String | — | ✅ |
| 06-panel | TodoStateProvider 콜백 | unicode width 수정 | ✅ |
| 07-hub | AgentPoolProvider 능력 (개정문서 §11) | AgentMetadata 분리 (개정문서 §11) | ✅ 헤더 |
| 08-commit | oxi_ai::complete 시그니처, Tool::new | git_utils 재사용 (개정문서 §8) | ✅ |
| 09-compaction | ContextTransformer 능력 (개정문서 §12) | snapcompact 확인 (개정문서 §6), async transform (개정문서 §12) | ✅ 헤더 |
| 10-lsp | LspProvider 능력 (개정문서 §1) | read_until (개정문서 §10), lsp-types | ✅ 헤더 |
| 11-mnemopi | MemoryBackend 구현 (§3 교체) | FTS5 bundled, spawn_blocking (개정문서 §7), mental_models 테이블 (개정문서 §9) | ✅ |
| 12-hindsight | MemoryBackend 기반 (§3.1 수정) | find_git_root (§2.6 수정), mental-models 백엔드 (개정문서 §9) | ✅ |
| 13-mermaid | — | which 캐싱 (후순위) | ⚪ 경미 |
| 00-master | 1차 "영구 제외" 정정 (§0 추가) | 번호 정렬 (개정문서 §13), 설정 중첩 (개정문서 §14) | ✅ |
| **00-revisions** | **본 문서 (모든 수정사항 정의)** | — | ✅ |

> 이 개정 문서가 권위적 출처(authoritative source)다. 각 하위 문서의 코드 스니펫과 충돌 시 **본 문서가 우선**한다.
