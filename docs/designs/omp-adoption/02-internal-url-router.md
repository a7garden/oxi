# 세부 설계 ② — Internal URL Router 포트

> 상태: 설계 v1 (구현 전 합의용)
> 작성: 2026-06-19
> 선행: [`00-master-plan.md`](./00-master-plan.md), [`01-hashline-edit.md`](./01-hashline-edit.md) (read.rs 변경 지점 공유)
> omp 분석: `packages/coding-agent/src/internal-urls/*` (12개 프로토콜 핸들러)
> 후속: M2 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

omp는 `read`/`search`가 **로컬 파일뿐 아니라** PR·이슈·서브에이전트 결과·스킬·룰·메모리·아티팩트를 **같은 형태**(`scheme://path`)로 소비하게 한다. 도구 표면은 그대로(read/search/grep/find)면서 에이전트가 접근하는 정보 영역이 확장된다.

oxi는 이것을 **새 포트 `InternalUrlRouter`**(포트 12) + `ProtocolHandler` 트레잇 + `CompositeUrlRouter`로 구현한다. 미등록 시 일반 파일 경로로 100% 폴백 — 기존 동작 regression 제로.

### omp가 검증한 가치
- **도구 수 증가 없음** — `gh_issue_view`, `gh_pr_view` 같은 별도 도구 없이 `read issue://1428`, `read pr://owner/repo/1063`.
- **일관된 인터페이스** — 모델이 "하나의 read API"만 학습. omp README 피처 #10, #15.
- **selector 투명성** — `read pr://1428/diff/1`, `read agent://<id>/findings.0.path` (JSON path).
- **immutable 플래그** — sealed 아티팩트/요약은 hashline anchor 억제 (편집 불가 명시).

---

## 1. omp 메커니즘

### 1.1 구조 (`internal-urls/router.ts`)

- **프로세스 글로벌 라우터** 1개 (`InternalUrlRouter.instance()`).
- **스킴당 1개 핸들러** (`#handlers: Map<scheme, ProtocolHandler>`).
- `canHandle(input)` — `^([a-z][a-z0-9+.-]*)://` 정규식으로 스킴 추출, 등록 여부 확인.
- `resolve(input, context)` — 핸들러 디스패치, `immutable` 플래그 스탬프.
- `complete(scheme, query)` — 자동완성 지원 핸들러만.

### 1.2 ProtocolHandler 인터페이스 (`types.ts:112`)

```ts
interface ProtocolHandler {
  readonly scheme: string;          // "issue", "pr", "agent", ...
  readonly immutable: boolean;       // true → hashline anchor 억제
  resolve(url: InternalUrl, context?: ResolveContext): Promise<InternalResource>;
  complete?(query: string): Promise<UrlCompletion[]>;
}
interface InternalResource {
  url: string; content: string;
  contentType: "text/markdown" | "application/json" | "text/plain";
  size?; sourcePath?; notes?; immutable?;
}
interface ResolveContext {
  cwd?; settings?; signal?; localProtocolOptions?;
}
```

### 1.3 12개 스킴과 oxi 우선순위

| 스킴 | omp 용도 | oxi 우선순위 | 비고 |
|---|---|:-:|---|
| `issue://` | GitHub 이슈 본문/댓글 | **M2 1차** | 기존 `github` 도구 재사용 |
| `pr://` | PR 본문/diff/커밋 | **M2 1차** | 기존 `github` 도구 재사용 |
| `agent://` | 서브에이전트 결과 (JSON path) | M2 2차 | 서브에이전트 결과 저장소 |
| `memory://` | Hindsight 메모리 항목 | M3 (04 연동) | `MemoryStore` 포트 |
| `skill://` | SKILL.md 본문 | M2 2차 | `SkillLoader` 포트 |
| `rule://` | TTSR 룰 본문 | M3 (03 연동) | `RuleRegistry` 포트 |
| `local://` | 세션 artifacts 디렉토리 | M2 2차 | oxi-cli 세션 경로 |
| `artifact://` | sealed 산출물 | 검토 | oxios 연관 |
| `history://` | 세션 히스토리 | 검토 | 세션 트리 |
| `omp://` | 번들 문서 | 제외 | omp 전용 |
| `vault://` | Obsidian 금고 | 검토 | Obsidian 스킬 연동 |
| `mcp://` | MCP 리소스 URI | M2 2차 | 기존 `McpManager` |

### 1.4 selector 문법 (`path-utils.ts`)

- `:1-50` — 행 범위 (read 도구 재사용).
- `:raw` — 가공 없는 원본.
- `:conflicts` — 충돌 마커 영역.
- **opaque 스킴**(`mcp://`) — 리소스 URI가 colon을 포함할 수 있어 selector 파싱 제외 (`OPAQUE_RESOURCE_SCHEMES`).

---

## 2. oxi화 설계

### 2.1 새 포트: `InternalUrlRouter` (포트 12)

`oxi-sdk/src/ports/mod.rs`:

```rust
// ═══════════════════════════════════════════════════════════════════════════
// Port 12 — InternalUrlRouter: 프로토콜별 가상 경로 해석.
// ═══════════════════════════════════════════════════════════════════════════

/// 라우터가 해석한 가상 경로 결과. read/search가 소비.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedUrl {
    /// 정규화된 원본 URL (디버그/로그용).
    pub url: String,
    /// 해석된 텍스트 콘텐츠.
    pub content: String,
    /// MIME: "text/markdown" | "application/json" | "text/plain".
    pub content_type: String,
    /// 바이트 크기 (선택).
    pub size: Option<usize>,
    /// 디버그용 소스 경로 (모델에 노출 안 함).
    pub source_path: Option<String>,
    /// 추가 노트 (해석 관련 경고 등).
    pub notes: Vec<String>,
    /// true → 편집 불가 (hashline anchor 억제). 핸들러의 immutable에서 스탬프.
    pub immutable: bool,
    /// 행 맵 (selector `:1-50` 적용을 read 도구에 위임하기 위한 메타).
    pub line_map: Option<LineMap>,
}

#[derive(Debug, Clone, Default)]
pub struct LineMap {
    pub total_lines: u32,
    /// 1-indexed 표시 가능 행全集 (생략 영역 표현).
    pub displayable: Option<Vec<(u32, u32)>>,
}

/// 라우터 호출 컨텍스트 (호출 세션 식별).
#[derive(Debug, Clone, Default)]
pub struct ResolveContext {
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
}

pub trait InternalUrlRouter: Send + Sync + 'static {
    /// `pr://owner/repo/1428`, `agent://<id>/findings.0.path` 해석.
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
        selector: Option<&'a str>,
        ctx: &'a ResolveContext,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedUrl, SdkError>> + Send + 'a>>;

    /// 이 라우터가 다루는 스킴 목록. read 도구가 dispatch 여부 판단용.
    fn schemes(&self) -> &[&str] { &[] }

    /// 자동완성 (선택).
    fn complete<'a>(
        &'a self,
        _scheme: &'a str,
        _query: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<UrlCompletion>, SdkError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlCompletion {
    pub value: String,
    pub label: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopInternalUrlRouter;
impl InternalUrlRouter for NoopInternalUrlRouter {
    fn resolve<'a>(&'a self, _: &'a str, _: Option<&'a str>, _: &'a ResolveContext)
        -> Pin<Box<dyn Future<Output = Result<ResolvedUrl, SdkError>> + Send + 'a>>
    { Box::pin(async { Err(SdkError::PortNotConfigured { port: "InternalUrlRouter" }) }) }
}
```

> **포트 버저닝 원칙 준수** (AGENTS.md): additive, noop 폴백. 미등록 시 `Err(PortNotConfigured)`.

### 2.2 ProtocolHandler 트레잇 + CompositeUrlRouter

`ProtocolHandler` 트레잇은 제품이 구현하는 **계약**이므로 `ports/` 레벨에,
`CompositeUrlRouter`(참조 구현)는 `ports/inmem/`에 둔다:

```rust
use async_trait::async_trait;
/// 단일 스킴 처리기. 제품이 각자 구현 (oxi-cli: IssueHandler, PrHandler, ...).
/// — `oxi-sdk/src/ports/mod.rs` (또는 `ports/url_router.rs`) —
#[async_trait]
pub trait ProtocolHandler: Send + Sync {
    /// 이 핸들러가 다루는 스킴 ("issue", "pr", ...). 소문자.
    fn scheme(&self) -> &str;
    /// 결과가 편집 불가면 true (hashline anchor 억제).
    fn immutable(&self) -> bool { false }
    /// URL 해석. selector는 read 도구가 peel-off 후 전달 (`:1-50`, `:raw`).
    async fn resolve(
        &self,
        url: &str,                    // 스킴 제외 path (omp rawPathname)
        selector: Option<&str>,
        ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError>;
    /// 자동완성 (선택).
    async fn complete(&self, _query: &str) -> Result<Vec<UrlCompletion>, SdkError> { Ok(vec![]) }
}

/// 다중 핸들러를 묶어 InternalUrlRouter 구현. omp InternalUrlRouter와 동등.
pub struct CompositeUrlRouter {
    handlers: parking_lot::RwLock<HashMap<String, Arc<dyn ProtocolHandler>>>,
}

impl CompositeUrlRouter {
    pub fn new() -> Self { Self { handlers: parking_lot::RwLock::new(HashMap::new()) } }
    pub fn register(&self, handler: Arc<dyn ProtocolHandler>) {
        self.handlers.write().insert(handler.scheme().to_lowercase(), handler);
    }
    pub fn unregister(&self, scheme: &str) -> bool {
        self.handlers.write().remove(&scheme.to_lowercase()).is_some()
    }
}

#[async_trait]
impl InternalUrlRouter for CompositeUrlRouter {
    fn schemes(&self) -> Vec<&str> { /* 등록된 스킴 목록 */ }
    async fn resolve(&self, uri: &str, selector: Option<&str>, ctx: &ResolveContext)
        -> Result<ResolvedUrl, SdkError>
    {
        let (scheme, path) = parse_scheme_and_path(uri)?;          // ^([a-z][a-z0-9+.-]*)://(.*)
        let handler = self.handlers.read().get(&scheme).cloned()
            .ok_or_else(|| SdkError::UnknownScheme { scheme: scheme.clone() })?;
        let mut resolved = handler.resolve(&path, selector, ctx).await?;
        resolved.immutable = handler.immutable();                  // 핸들러 immutable 스탬프
        Ok(resolved)
    }
}
```

### 2.3 read/search 도구 진입점 변경

**공통 dispatch 함수** (`oxi-agent/src/tools/path_security.rs` 또는 신규 `url_dispatch.rs`):

```rust
/// 경로가 내부 URL이면 라우터로, 아니면 파일로.
pub async fn resolve_path_or_url(
    input: &str,
    ctx: &ToolContext,
) -> Result<PathOrUrl, ToolError> {
    if let Some(scheme) = parse_internal_scheme(input) {           // ^scheme://
        let router = ctx.internal_url_router.as_ref()
            .ok_or("Internal URL routing not configured")?;
        let (path, selector) = peel_selector(input);               // :1-50, :raw 분리
        let resolved = router.resolve(&path, selector.as_deref(), &ctx.resolve_ctx()).await
            .map_err(|e| e.to_string())?;
        Ok(PathOrUrl::Url(resolved))
    } else {
        let guard = PathGuard::new(ctx.root());
        let validated = guard.validate_traversal(Path::new(input)).map_err(|e| e.to_string())?;
        Ok(PathOrUrl::File(validated))
    }
}

pub enum PathOrUrl {
    File(PathBuf),
    Url(ResolvedUrl),
}
```

**read.rs 변경**: `resolve_path_or_url` 후 분기. `Url`이면 `resolved.content`를 (selector 적용 + 행 번호 부여 + **immutable이면 tag 생략**) 출력. `File`이면 기존 경로.

**search/grep/find.rs**: 검색 대상이 URL이면 `resolved.content`를 메모리에서 검색. omp의 "search walks a diff like a directory" 지원.

### 2.4 selector 처리

omp는 path-utils의 정규식群으로 selector peel. oxi는 표준화:
- `LineMap` 타입으로 selector 해석을 read 도구에 위임 (이미 read는 offset/limit 지원).
- opaque 스킴(`mcp://`)은 selector 해석 안 함 — omp 정책 동일.

### 2.5 ToolContext 확장

```rust
pub struct ToolContext {
    pub workspace_dir: PathBuf,
    pub root_dir: Option<PathBuf>,
    pub session_id: Option<String>,
    pub snapshot_store: Option<Arc<dyn oxi_hashline::SnapshotStore>>,           // ①
    pub internal_url_router: Option<Arc<dyn InternalUrlRouter>>,                 // ② 신규
}
impl ToolContext {
    pub fn resolve_ctx(&self) -> ResolveContext {
        ResolveContext { cwd: Some(self.workspace_dir.clone()), session_id: self.session_id.clone() }
    }
}
```

---

## 3. oxi-cli 구현체 (M2)

### 3.1 핸들러 (기존 도구 재사용)

`oxi-cli/src/internal_urls/`:
- `issue_handler.rs` — `IssueProtocolHandler`, 기존 `github` 도구의 GitHub API 클라이언트 재사용. `issue://1428` → 현재 repo의 이슈, `issue://owner/repo/1428` → 명시 repo.
- `pr_handler.rs` — `pr://1428`, `pr://owner/repo/1428/diff/N`. diff는 unified diff를 markdown으로.
- `agent_handler.rs` — 서브에이전트 결과 저장소(신규)에서 JSON path 조회.
- `skill_handler.rs` — `SkillLoader` 포트로 SKILL.md 본문.
- `memory_handler.rs` — `MemoryStore` 포트 (M3 연동).
- `rule_handler.rs` — `RuleRegistry` 포트 (M3 연동).
- `local_handler.rs` — 세션 artifacts 디렉토리.

### 3.2 bootstrap.rs 등록

```rust
let url_router = Arc::new(CompositeUrlRouter::new());
url_router.register(Arc::new(IssueProtocolHandler::new(gh_client.clone())));
url_router.register(Arc::new(PrProtocolHandler::new(gh_client.clone())));
// ... 추가 핸들러
OxiBuilder::new().with_port_internal_url_router(url_router).build()
```

---

## 4. 의존성 & 마일스톤 (M2)

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| M2.1 | `InternalUrlRouter` 포트 + `Noop` + `ResolvedUrl`/`ResolveContext` 타입 | — |
| M2.2 | `CompositeUrlRouter` + `ProtocolHandler` 트레잇 + scheme 파서 | M2.1 |
| M2.3 | `url_dispatch.rs` + `PathOrUrl` + read.rs 분기 (라우터 None 시 폴백) | M2.2, ① M1.12 완료 후 |
| M2.4 | `IssueProtocolHandler` + `PrProtocolHandler` (기존 github 도구 재사용) | M2.2 |
| M2.5 | selector peel + `LineMap` + immutable tag 억제 | M2.3 |
| M2.6 | search/grep/find URL dispatch | M2.5 |
| M2.7 | 추가 핸들러 (agent/skill/local) | M2.2 |

> **M1과의 충돌 회피**: M2.3(read.rs 변경)은 M1.12(read.rs tag 발행) 완료 후 진행. 같은 파일 수정이므로.

---

## 5. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| 글로벌 라우터 vs 인스턴스 | 🟢 Composite 주입 | omp는 싱글톤. oxi는 포트 주입(세션별 가능, 더 유연) |
| immutable 리소스 + hashline | 🟢 tag 억제 | immutable=true 시 read가 `[path#TAG]` 헤더 생략 |
| selector 문법 표준화 | 🟡 `LineMap` | omp 정규식 호환 + 표준 타입. 세부는 M2.5 |
| MCP opaque 처리 | 🟢 동일 정책 | `OPAQUE_RESOURCE_SCHEMES = {mcp}` |
| `agent://` 결과 저장소 | 🟢 신규 | 서브에이전트 결과를 어디 저장? 세션 트리 또는 별도. M2.7에서 결정 |
| 인증 컨텍스트 (issue:// GitHub 토큰) | 🟢 기존 재사용 | `AuthProvider` 포트 |

---

## 6. 부록: omp → oxi 매핑

| omp 파일 | oxi 위치 |
|---|---|
| `internal-urls/router.ts` | `oxi-sdk/src/ports/inmem/url_router.rs` (`CompositeUrlRouter`) |
| `internal-urls/types.ts` (`ProtocolHandler`) | `oxi-sdk/src/ports/mod.rs` (또는 `ports/url_router.rs`) — 제품이 구현하는 계약 |
| `internal-urls/types.ts` (`InternalResource`) | `oxi-sdk/src/ports/mod.rs` (`ResolvedUrl`) |
| `internal-urls/parse.ts` | `oxi-sdk/src/ports/inmem/url_router.rs` (parse_scheme_and_path) |
| `internal-urls/issue-pr-protocol.ts` | `oxi-cli/src/internal_urls/issue_handler.rs`, `pr_handler.rs` |
| `internal-urls/local-protocol.ts` | `oxi-cli/src/internal_urls/local_handler.rs` |
| `tools/path-utils.ts` (selector) | `oxi-agent/src/tools/url_dispatch.rs` |
