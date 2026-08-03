# 설계: 카탈로그를 Port 로 승격 — SDK 가 동적 갱신을 받는 단일 계약

> 상태: 설계 **v3** (구현 전). 사용자 합의: 즉시 교체(breaking) + lazy on-call 갱신.
> 작성: 2026-06-17
> v2 갱신: 7개 손보기 — `CatalogProtocol` enum 도입, `Oxicode::resolve_model` async,
>         `Oxicode` convenience 메서드 축소, `Refreshed` → `Updated`,
>         `current_source()` 제거, `init()` 단일화, entry-level `source`.
> v3 갱신: 구현 전 컴파일/빌드 브레이크 3개 정정 —
>         (1) 의존 방향 수정: bridge layer 를 `oxicode-sdk` 에 두고 `as_oxicode_api()` 항상
>         노출 (feature flag 제거). "oxicode-ai → SDK 의존이 0" 으로 주장 정정.
>         (2) `NoopModelCatalog` 의 derive(Debug) + manual impl 충돌 제거.
>         (3) `ProviderResolver` trait async ripple 명시 (§7.8 신설).
>         추가: 문서 내부 불일치 6건, SdkError 카탈로그 변종, RefreshOutcome::Failed
>         의미 doc 강화.
> 선행: `docs/designs/2026-06-17-dynamic-catalog-design.md` (데이터 소스 단일화).
>        본 설계는 **그 위에** SDK consumer 갱신 계층을 더한다.
> 후속: 기존 문서 §6.3 의 "`&'static` 계약 유지" 결정은 본 설계로 **폐기**.
>        dynamic-catalog §4.3 의 `protocol_for` 출력 타입도 본 설계로 동기화.

## 0. 핵심 (TL;DR)

기존 dynamic-catalog 설계는 데이터 흐름을 단일화했지만, **SDK consumer 가
새 데이터를 받는 메커니즘**이 빠졌다:

- `oxicode_sdk::get_all_models()`, `get_model_entry()` 등은 모두 `&'static` 를 반환한다.
- `&'static` 는 `OnceLock`/`LazyLock` 에서 오므로 **프로세스당 한 번 박히고 끝**.
- `init_models_dev()` 로 LIVE 캐시를 채워도, 그 데이터는 `OnceLock` 으로 들어가지
  못한다 (immutable). 결과적으로 oxicode-cli 의 `setup_wizard`, `tui/handlers`,
  `tui/slash`, `tui/overlay/settings` 가 보는 카탈로그는 **바이너리 빌드 시점의
  SNAP 으로 고정**되어 있다.
- 11 개 port (StateStore, ConfigStore, AuthProvider, EventBus, ...) 가 SDK 의
  계약인데, 카탈로그만 그 자리에 없다.

**본 설계의 한 문장 (v3)**: 카탈로그를 **12번째 port** 로 만들고 SDK 가 자기
타입 (`CatalogProtocol` enum) 을 갖게 한다 — `oxicode-ai → SDK` 역방향 의존만 없다
(SDK → oxicode-ai 정방향은 이미 존재; bridge layer 가 그 단일 소스). `OnceLock`/
`LazyLock` 글로벌과 `&'static` API 를 **전부 제거**한다. SDK consumer 는
`Oxicode::catalog()` 로 async 조회, `subscribe()` 로 갱신을 받고,
`Oxicode::resolve_model()` 은 dynamic registry 와 catalog 을 자동으로 이어준다.

## 1. 설계 원칙

| # | 원칙 | 트레이드오프 |
|---|---|---|
| 1 | **Port = SDK 의 유일한 카탈로그 진입점** | 기존 free fn (`get_all_models` 등) 제거 — breaking |
| 2 | **SDK 는 자기 타입을 갖는다** (`CatalogProtocol`, entry structs) | oxicode-ai → SDK 역방향 의존만 없음. SDK → oxicode-ai 정방향은 이미 존재 (재노출). bridge layer 가 그 단일 소스 (§7.5) |
| 3 | **Async + owned + typed** | `&'static` 최적화 + `String` 매칭 둘 다 포기. `Arc<dyn ModelCatalog>` + `CatalogProtocol` enum |
| 4 | **Lazy 갱신 (수동)** | 백그라운드 task 없음. consumer 가 `refresh().await` 호출 |
| 5 | **Broadcast 로 변경 통지** | `subscribe()` → `broadcast::Receiver<CatalogEvent>`. UI/캐시 무효화 |
| 6 | **데이터 흐름 단방향 유지** | models.dev → materialize → snapshot → port → consumer. 역참조 없음 |
| 7 | **Catalog = "세상", Dynamic = "나"** | `Oxicode::resolve_model` 이 dynamic 우선 → catalog fallback. 자동 pre-populate 함정 회피 |

> 결정적 트레이드오프: `&'static` 반환 + `String api` 둘 다 포기. lookup 비용은
> `Arc<dyn Trait>` vtable 한 번 + `RwLock` read guard. microbenchmark 기준 기존보다
> 한 자릿수 느리지만 절대값이 수십 ns 라 무관. `CatalogProtocol` enum 매칭은
> 컴파일 타임에 검증. trade-off 정당화됨.

## 2. 왜 port 가 구조적으로 맞는가 (근거)

기존 11 개 port 의 패턴:

```rust
// StateStore, ConfigStore, AuthProvider, EventBus, SkillLoader,
// PersonaProvider, AccessGate, CapabilityResolver, MemoryStore,
// CronScheduler, ResourceMonitor — 모두 동일한 모양:
#[async_trait_or_pin_box]
pub trait XxxPort: Send + Sync + 'static {
    async fn lookup(...) -> Result<...>;
    async fn list(...) -> Result<Vec<...>>;
}

pub struct NoopXxxPort;          // 빈 기본값
pub struct InMemoryXxxPort;      // 메모리 reference impl (inmem/)
pub struct FileXxxPort;          // 파일 reference impl (fs/)

pub struct PortRegistry { pub xxx: Arc<dyn XxxPort>, ... }

impl OxicodeBuilder {
    pub fn with_xxx(self, x: Arc<dyn XxxPort>) -> Self;
}
```

카탈로그는 이 11 개 중 어느 것보다 **더 명백히 port 다**:

- **Lookup**: "이 provider 의 모델 목록 줘" — 인프라 어댑터의 전형
- **Cache**: SNAP + LIVE + user overrides + LOCAL — products 마다 다름 (oxicode-cli 는
  파일 캐시, oxios 는 DB 캐시 가능)
- **Refresh 전략**: oxicode-cli 는 models.dev ETag conditional GET, oxios 는 자체 미러
- **LOCAL discovery**: oxicode-cli 는 env var 기반, oxios 는 설정 UI 기반
- **Subscription**: UI 가 새 모델 도착을 알아야 함 — port 가 push 채널을 가져야 함

기존 `LazyLock<HashMap<String, Model>>` 박힌 50 개 모델은 "port 가 없는 시절의
우회" 였고, dynamic-catalog 이 데이터 흐름을 단일화했지만 **계약은 여전히 우회**.
본 설계가 그걸 닫는다.

## 3. 아키텍처

```
                        ┌─────────────────────────────┐
                        │   models.dev / override /    │
                        │   local discovery / etc.     │
                        └─────────────┬───────────────┘
                                      │ (impl-specific)
                                      ▼
                        ┌─────────────────────────────┐
                        │   ModelCatalog (port trait)  │
                        │   ┌───────────────────────┐ │
                        │   │ snapshot: Arc<RwLock< │ │
                        │   │   Snapshot           │ │
                        │   │ >>                    │ │
                        │   │ tx: broadcast::Sender │ │
                        │   │ <CatalogEvent>        │ │
                        │   └───────────────────────┘ │
                        └─────────────┬───────────────┘
                                      │ impl = dyn
              ┌───────────────────────┼───────────────────────┐
              ▼                       ▼                       ▼
      FileModelCatalog         SqlCatalog            MockCatalog
      (oxicode-sdk ports/fs/)      (oxios impl)          (tests)
              ▲
              │ oxicode-cli registers via
              │   OxicodeBuilder::with_catalog()
              │
   ┌──────────┴───────────────────────────────────────┐
   │                Oxicode (composition root)            │
   │                                                  │
   │   oxicode.catalog() -> &Arc<dyn ModelCatalog>       │
   │   oxicode.catalog().list_providers().await           │
   │   oxicode.catalog().get_model(p, m).await            │
   │   oxicode.catalog().subscribe() -> Receiver<Event>   │
   │   oxicode.catalog().refresh().await                  │
   │   oxicode.resolve_model(id).await -> Model           │
   └──────────────┬───────────────────────────────────┘
                  │ delegates
                  ▼
        SDK consumer code (TUI, RPC, agent, ...)
```

핵심: **`Oxicode` 가 port 의 단일 클라이언트**이고, SDK consumer 는 `Oxicode` 만 본다.
내부 구현(`FileModelCatalog`)은 oxicode-cli 의 composition root 가 등록한다.

## 4. Port 정의

### 4.1 트레이트와 타입

위치: `oxicode-sdk/src/ports/catalog.rs` (신규 모듈).

```rust
//! Port 12 — ModelCatalog: source of truth for provider/model metadata.
//!
//! See `docs/designs/2026-06-17-catalog-port-design.md` for rationale.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use crate::error::SdkResult;
use super::AuthMethod;

// ─── 프로토콜 enum (SDK 의 자기 타입) ────────────────────────────

/// Protocol used to talk to a provider/model.
///
/// 이 enum 은 oxicode-ai 가 아니라 SDK 가 정의/소유. oxicode-sdk → oxicode-ai 정방향
/// 의존은 이미 존재 (v0.x 부터 `oxicode_ai::*` 재노출). 따라서 `as_oxicode_api()`
/// 는 **항상 컴파일** — feature flag 불필요. 핵심은 역방향: oxicode-ai 는 SDK
/// 타입을 보지 않으므로 (oxicode-sdk 에 의존하지 않음), port 트레이트가
/// oxicode-ai 내부 타입(`Api`)을 직접 쓰지 않는다. 변환은 SDK bridge layer
/// (§7.5) 에서만 일어난다.
///
/// 새 프로토콜 추가 = SDK 에 variant 추가 + `protocol_for` 매핑 1줄
/// + `as_oxicode_api()` 매핑 1줄 + bridge dispatch 1줄.
/// `OpenAiCompatible` 는 OpenAI Chat Completions 호환 폴백 (대부분의 npm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogProtocol {
    AnthropicMessages,
    OpenAiCompletions,
    OpenAiResponses,
    AzureOpenAiResponses,
    GoogleGenerativeAi,
    GoogleVertex,
    MistralConversations,
    BedrockConverseStream,
    /// OpenAI 호환 폴백 — custom npm, gateway, aggregator.
    OpenAiCompatible,
}

impl CatalogProtocol {
    /// Default authentication header scheme for this protocol.
    ///
    /// Per-model override (`model.npm`) 가 적용된 후 호출되므로, `default_auth`
    /// 결과가 곧 그 모델의 auth 다. 별도 `auth_method` 필드 불필요.
    pub fn default_auth(&self) -> AuthMethod {
        match self {
            CatalogProtocol::AnthropicMessages => AuthMethod::XApiKey,
            CatalogProtocol::AzureOpenAiResponses => AuthMethod::ApiKey,
            CatalogProtocol::GoogleGenerativeAi
            | CatalogProtocol::GoogleVertex
            | CatalogProtocol::BedrockConverseStream => AuthMethod::None,
            // OpenAI 호환 + Mistral + Responses
            CatalogProtocol::OpenAiCompletions
            | CatalogProtocol::OpenAiResponses
            | CatalogProtocol::OpenAiCompatible
            | CatalogProtocol::MistralConversations => AuthMethod::Bearer,
        }
    }

    /// Convert to oxicode-ai's internal `Api` enum.
    ///
    /// **항상 컴파일** — oxicode-sdk 는 v0.x 부터 oxicode-ai 를 재노출하므로 정방향
    /// 의존이 이미 있다. `#[cfg(feature = ...)]` gate 불필요 (v3 정정).
    ///
    /// 호출처: 오직 SDK 의 bridge layer (`oxicode_sdk::bridge::create_provider_from_entry`,
    /// §7.5). port 구현자(`FileModelCatalog` 등)는 이 메서드를 부르지 않는다.
    pub fn as_oxicode_api(&self) -> oxicode_ai::Api {
        use CatalogProtocol::*;
        match self {
            AnthropicMessages => oxicode_ai::Api::AnthropicMessages,
            OpenAiCompletions => oxicode_ai::Api::OpenAiCompletions,
            OpenAiResponses => oxicode_ai::Api::OpenAiResponses,
            AzureOpenAiResponses => oxicode_ai::Api::AzureOpenAiResponses,
            GoogleGenerativeAi => oxicode_ai::Api::GoogleGenerativeAi,
            GoogleVertex => oxicode_ai::Api::GoogleVertex,
            MistralConversations => oxicode_ai::Api::MistralConversations,
            BedrockConverseStream => oxicode_ai::Api::BedrockConverseStream,
            // OpenAI 호환은 oxicode-ai 에서 OpenAiCompletions 로 처리
            OpenAiCompatible => oxicode_ai::Api::OpenAiCompletions,
        }
    }
}

// ─── 데이터 타입 ──────────────────────────────────────────────────

/// Snapshot of a single model entry.
///
/// `Clone` cheap. `protocol` 과 `source` 가 typed — SDK consumer 는
/// 런타임 string 매칭 없이 dispatch 가능.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModelEntry {
    pub provider: String,
    pub model_id: String,
    pub name: String,

    /// Protocol for this specific model. May differ from the parent
    /// provider's protocol (per-model override; dynamic-catalog §2.2).
    pub protocol: CatalogProtocol,

    /// Where this entry came from. SNAP entries are `Embedded`, fresh
    /// fetches are `Live`, ollama probes are `Local`, etc.
    pub source: CatalogSource,

    /// Model-level base URL override. `None` = inherit from provider.
    /// See dynamic-catalog §2.2 (v3 결함 D — 55 models).
    pub base_url: Option<String>,

    pub reasoning: bool,
    pub supports_vision: bool,

    /// USD per million tokens. `0.0` = free or undisclosed by upstream.
    pub cost_input: f64,
    pub cost_output: f64,
    pub cost_cache_read: f64,
    pub cost_cache_write: f64,

    pub context_window: u32,
    pub max_tokens: u32,

    pub input_modalities: Vec<String>,
    pub release_date: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProviderEntry {
    pub id: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub protocol: CatalogProtocol,
    pub env_key: Option<String>,
    pub extra_env_keys: Vec<String>,
    pub base_url: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub category: String,
    pub description: String,
    pub default_enabled: bool,
}

/// Outcome of a single refresh attempt.
///
/// **왜 `Result`가 아닌가** (v3 정정): `RefreshOutcome::Failed`는 `Ok(Failed)`
/// 로 반환된다. 의도적 — lazy on-call 원칙에서 refresh 실패는 "SNAP으로 작동
/// 중"을 의미하며, 호출자의 `.await?` 성공 패스를 막지 않는다. 실패를
/// 특별 처리하고 싶은 호출자(예: CLI 명시적 갱신)는 `match`로 풀면 된다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RefreshOutcome {
    /// Snapshot unchanged (HTTP 304, mtime fresh, etc.).
    Unchanged,
    /// Snapshot replaced with newer data.
    Updated {
        provider_count: usize,
        model_count: usize,
    },
    /// No network attempted; served from stale cache or SNAP.
    /// (e.g. `OXICODE_MODELS_DEV_DISABLE_FETCH=1`, mtime 창 내.)
    Offline { reason: &'static str },
    /// Refresh attempted and failed. Previous snapshot still in effect.
    /// NOT an `Err` — see type-level doc.
    Failed { reason: String },
}

/// Per-entry origin. UI uses this for "local" badges, debugging uses it
/// for "where did this come from".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogSource {
    /// Compile-time embedded SNAP (models.dev gzip).
    Embedded,
    /// Runtime cache from a previous successful live fetch.
    Cache,
    /// Fresh fetch from upstream (e.g. models.dev).
    Live,
    /// Local /v1/models discovery (ollama, lmstudio, vllm, sglang).
    Local,
    /// User override file (highest precedence).
    Override,
}

/// Lifecycle event delivered to all subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CatalogEvent {
    /// Snapshot changed. New state available via read methods.
    Updated {
        provider_count: usize,
        model_count: usize,
    },
    /// Refresh failed; previous snapshot still in effect.
    RefreshFailed {
        reason: String,
        provider_count: usize,
        model_count: usize,
    },
    /// User override file applied or modified.
    OverrideApplied {
        path: PathBuf,
        provider_overrides: usize,
        model_overrides: usize,
    },
    /// Local discovery added new models.
    LocalDiscovered {
        base_url: String,
        model_count: usize,
    },
}

// ─── Port trait ──────────────────────────────────────────────────

/// Port trait.
///
/// # Threading & lifecycle
///
/// All read methods are async and return owned values. Implementations
/// typically hold a snapshot behind an `Arc<RwLock<_>>`; the trait does
/// not require any particular storage. The snapshot is replaced atomically
/// on refresh.
///
/// # Subscription
///
/// [`subscribe`](Self::subscribe) returns a `broadcast::Receiver` (capacity
/// 16). Slow consumers may miss intermediate updates — the latest state is
/// always available via the read methods, so this is not a correctness
/// issue.
pub trait ModelCatalog: Send + Sync + 'static {
    fn list_providers(
        &self,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<String>>> + Send + '_>>;

    fn get_provider(
        &self,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogProviderEntry>>> + Send + '_>>;

    fn list_models(
        &self,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>>;

    fn get_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogModelEntry>>> + Send + '_>>;

    fn search(
        &self,
        pattern: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>>;

    fn model_count(
        &self,
    ) -> Pin<Box<dyn Future<Output = SdkResult<usize>> + Send + '_>>;

    /// Force a refresh. The implementation decides what "refresh" means
    /// (HTTP fetch, file re-read, etc.).
    fn refresh(
        &self,
    ) -> Pin<Box<dyn Future<Output = SdkResult<RefreshOutcome>> + Send + '_>>;

    /// Subscribe to catalog lifecycle events. Multiple consumers supported.
    fn subscribe(&self) -> broadcast::Receiver<CatalogEvent>;
}
```

### 4.2 Noop 기본값

```rust
/// Empty catalog — for products that don't need any model metadata
/// (e.g. a single-provider app with hardcoded IDs).
///
/// `Default` 만 derive (빈 채널). `Debug` 는 manual impl — sender 를 제외한
/// 간소화 출력 (v3 정정: derive 와 manual 동시 구현은 컴파일 에러).
#[derive(Default)]
pub struct NoopModelCatalog {
    tx: broadcast::Sender<CatalogEvent>,
}

impl NoopModelCatalog {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(16);
        Arc::new(Self { tx })
    }
}

impl ModelCatalog for NoopModelCatalog {
    fn list_providers(&self) -> Pin<Box<dyn Future<Output = SdkResult<Vec<String>>> + Send + '_>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn get_provider(&self, _: &str) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogProviderEntry>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
    fn list_models(&self, _: &str) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn get_model(&self, _: &str, _: &str) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogModelEntry>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
    fn search(&self, _: &str) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn model_count(&self) -> Pin<Box<dyn Future<Output = SdkResult<usize>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }
    fn refresh(&self) -> Pin<Box<dyn Future<Output = SdkResult<RefreshOutcome>> + Send + '_>> {
        Box::pin(async { Ok(RefreshOutcome::Unchanged) })
    }
    fn subscribe(&self) -> broadcast::Receiver<CatalogEvent> {
        self.tx.subscribe()
    }
}

impl std::fmt::Debug for NoopModelCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoopModelCatalog").finish_non_exhaustive()
    }
}
```

(`current_source()` 제거. 진단 정보는 impl 의 `Debug` impl 에서 — §6.6 참고.)

### 4.3 PortRegistry 통합

`oxicode-sdk/src/ports/mod.rs` 의 `PortRegistry` 에 12번째 필드 추가:

```rust
#[derive(Clone)]
pub struct PortRegistry {
    // ... 기존 11개 필드 ...
    /// Model catalog — provider and model metadata source of truth.
    pub catalog: Arc<dyn ModelCatalog>,
}

impl PortRegistry {
    pub fn noop() -> Self {
        Self {
            // ... 기존 noop ...
            catalog: NoopModelCatalog::new(),
        }
    }
}
```

> `Debug` impl 에 `<dyn ModelCatalog>` 표시 추가 (기존 11개와 동일).

### 4.4 `OxicodeBuilder::with_catalog()` + `Oxicode::catalog()` + `Oxicode::resolve_model()`

```rust
impl OxicodeBuilder {
    /// Register a model catalog port.
    ///
    /// The catalog is the source of truth for provider/model metadata.
    /// If not called, the SDK uses `NoopModelCatalog` (empty results).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oxicode_sdk::{OxicodeBuilder, ModelCatalog};
    /// use std::sync::Arc;
    ///
    /// let catalog: Arc<dyn ModelCatalog> = /* ... */;
    /// let oxicode = OxicodeBuilder::new()
    ///     .with_catalog(catalog)
    ///     .build();
    /// ```
    pub fn with_catalog(mut self, catalog: Arc<dyn ModelCatalog>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.catalog = catalog;
        self.ports = Some(ports);
        self
    }
}

impl Oxicode {
    /// Catalog port accessor. Use this for all catalog queries.
    ///
    /// ```no_run
    /// # use oxicode_sdk::OxicodeBuilder;
    /// # async fn doc(oxicode: oxicode_sdk::Oxicode) -> Result<(), oxicode_sdk::SdkError> {
    /// let providers = oxicode.catalog().list_providers().await?;
    /// let model = oxicode.catalog().get_model("anthropic", "claude-sonnet-4-20250514").await?;
    /// # Ok(()) }
    /// ```
    pub fn catalog(&self) -> &Arc<dyn ModelCatalog> {
        &self.ports.catalog
    }

    /// Resolve a model identifier (`"provider/model"` or bare `"model"`)
    /// to a concrete `Model`. Checks the dynamic registry first, then
    /// falls back to the catalog port.
    ///
    /// Lookup order:
    /// 1. **Dynamic registry** — `OxicodeBuilder::model()` 로 등록한 모델.
    ///    sync, in-memory. 항상 우선.
    /// 2. **Catalog port** — port 의 `get_model()`. async. 없으면 NotFound.
    ///
    /// Returns `SdkError::ModelNotFound` if neither has it.
    ///
    /// # Why async
    ///
    /// Built-in models live in the catalog port (not pre-populated into
    /// the dynamic map — that was a UX trap with 5,277 models). So most
    /// resolutions touch the port, which is async.
    ///
    /// # Performance
    ///
    /// Per-lookup cost: dynamic HashMap (O(1), sync) + on miss, Arc<dyn>
    /// vtable dispatch + RwLock read + HashMap lookup. Sub-microsecond in
    /// practice. Cache if you call this in a hot path.
    pub async fn resolve_model(&self, model_id: &str) -> SdkResult<Model> {
        let (provider, model) = parse_model_id(model_id);
        // 1. Dynamic registry (sync, in-memory)
        if let Some(m) = self.models.lookup_dynamic(provider, model) {
            return Ok(m);
        }
        // 2. Catalog port (async)
        if let Some(entry) = self.ports.catalog.get_model(provider, model).await? {
            return Ok(Model::from_catalog_entry(&entry));
        }
        Err(SdkError::ModelNotFound { model_id: model_id.into() })
    }
}

fn parse_model_id(model_id: &str) -> (&str, &str) {
    match model_id.split_once('/') {
        Some((p, m)) => (p, m),
        None => ("anthropic", model_id),  // 기존 호환: bare id → anthropic
    }
}
```

> **노트**: `Oxicode` 는 `catalog()` 단일 accessor + `resolve_model()` 단일
> resolution entry point 만 노출. 모든 catalog 메서드는 `oxicode.catalog().*` 로
> 직접 호출. pass-through 메서드 6개를 두지 않음 — 표면 비대칭 방지 (기존
> `Oxicode::providers`/`models`/`tools`/`ports` 는 sync getter 한 줄 짜리).

## 5. 데이터 타입: SDK 가 노출하는 entry 모양

### 5.1 `CatalogProtocol` — SDK 의 자기 enum

`CatalogProtocol` 은 SDK 가 정의·소유하는 프로토콜 분류 enum. oxicode-ai 는 consumer
로서 `as_oxicode_api()` 로 변환. 양방향 의존이 단방향(oxicode-ai → SDK) 으로 정리됨.

```rust
let entry: CatalogModelEntry = ...;
match entry.protocol {
    CatalogProtocol::AnthropicMessages => { /* Anthropic 호출 경로 */ }
    CatalogProtocol::OpenAiCompletions | CatalogProtocol::OpenAiCompatible => { /* OpenAI 경로 */ }
    // ...
}

// Auth 는 protocol 에서 파생 — 별도 필드 불필요:
let auth = entry.protocol.default_auth();
```

### 5.2 `CatalogModelEntry` vs 기존 `ModelEntry`

| | `ModelEntry` (기존) | `CatalogModelEntry` (v2) |
|---|---|---|
| Lifetime | `&'static` (OnceLock) | owned (Clone) |
| Protocol | `oxicode_ai::Api` enum | `CatalogProtocol` enum (SDK 소유) |
| `auth_method` | provider 에서만 가져옴 (모델 수준 없음) | **삭제**. `entry.protocol.default_auth()` 로 파생 |
| `base_url` | 없음 | `Option<String>` (모델 수준 override, 55 모델) |
| `source` | 없음 | `CatalogSource` per-entry (UI 배지, 디버깅) |
| `input_modalities` | 없음 | 있음 (image/audio 등 필터링) |
| `status` | 없음 | 있음 (alpha/beta/deprecated UI 표시) |
| `id` 필드명 | `id` | `model_id` (사용처 명확) |

### 5.3 `CatalogProviderEntry`

| | `BuiltinProviderEntry` (기존) | `CatalogProviderEntry` (v2) |
|---|---|---|
| Protocol | `String` (`api` 필드) | `CatalogProtocol` enum |
| `auth_method` | 필드로 보관 | **삭제**. `entry.protocol.default_auth()` |
| `extra_headers` | 있음 (OpenRouter HTTP-Referer 등 보존) | 그대로 |
| `category` | 항상 빈 값 (dynamic-catalog §4.4) | 그대로 |
| `description` | 항상 빈 값 | 그대로 |

### 5.4 `CatalogSource` — per-entry

전체 catalog 의 source 가 아니라 **각 entry 의 source** 를 표현. UI 가 "Local"
배지 가능, 디버깅 시 "이 모델 어디서 왔지" 명확.

혼합 catalog (예: SNAP 의 모델 + Local ollama 모델) 에서 자연스럽게 작동:
각 entry 가 자기 source 를 들고 있으니 전체 source 를 정의할 필요 없음.

`CatalogEvent::Updated` 는 source 필드를 더이상 포함하지 않음 (개별 entry 가
자기 source 를 표현하므로 이벤트 차원에서는 aggregate count 만).

### 5.5 `CatalogEvent`

`broadcast::Sender<CatalogEvent>` 채널 **capacity 16**. slow consumer 는 update 를
놓칠 수 있지만, **read 메서드는 항상 최신**을 반환하므로 정합성 깨지지 않음.

Event 변종:

- `Updated { provider_count, model_count }` — snapshot 변경 알림. entry 별 diff
  가 필요하면 v2 에서 추가 (현재는 total count 만)
- `RefreshFailed { reason, ... }` — 직전 snapshot 유지됨
- `OverrideApplied { path, ... }` — overrides.toml 적용 또는 변경
- `LocalDiscovered { base_url, model_count }` — LOCAL probe 결과

### 5.6 Debug 진단 (§4.5)

trait 에는 diagnostic getter 를 두지 않음 (`current_source()` 제거됨). 대신
각 impl 이 자체 `Debug` impl 에 상태 요약을 노출 (예: §6.6 `FileModelCatalog`).

```rust
impl std::fmt::Debug for FileModelCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.state.read();
        f.debug_struct("FileModelCatalog")
            .field("providers", &snap.providers.len())
            .field("models", &snap.models.values().map(|v| v.len()).sum::<usize>())
            .field("default_source", &snap.source)  // snapshot 차원 source
            .finish_non_exhaustive()
    }
}
```

`tracing::debug!(?catalog)` 로 시작 시점 카탈로그 상태 확인 가능.

## 6. FileModelCatalog — Reference Impl

위치: `oxicode-sdk/src/ports/fs/catalog.rs` (신규). 기존 `oxicode-ai/src/catalog/*` 의
로직을 여기로 **이관** (dynamic-catalog §4 의 모든 코드).

### 6.1 구조

```rust
pub struct FileModelCatalog {
    state: Arc<RwLock<Snapshot>>,
    tx: broadcast::Sender<CatalogEvent>,
    config: CatalogConfig,
}

struct Snapshot {
    providers: BTreeMap<String, CatalogProviderEntry>,
    /// provider_id → (model_id → entry). nested map 은 O(1) lookup 을 위해.
    models: BTreeMap<String, BTreeMap<String, CatalogModelEntry>>,
    /// snapshot 차원의 source (entry 마다 source 있음, 이건 "기본값" 개념).
    /// load 시점이 Embedded, refresh 후 Live, override 적용 후 Override 등.
    default_source: CatalogSource,
}

#[derive(Clone, Debug)]
pub struct CatalogConfig {
    pub cache_path: PathBuf,           // ~/.oxicode/cache/models-dev.json
    pub etag_path: PathBuf,            // ~/.oxicode/cache/models-dev.json.etag
    pub override_path: PathBuf,        // ~/.oxicode/catalog/overrides.toml
    pub mtime_window: Duration,        // 기본 1h
    pub fetch_enabled: bool,           // OXICODE_MODELS_DEV_DISABLE_FETCH
    pub models_dev_url: String,        // OXICODE_MODELS_DEV_URL
    pub user_agent: String,
    pub local_discovery_urls: Vec<String>,  // 비어있으면 skip
}
```

### 6.2 초기화 — 단일 `init()`

```rust
impl FileModelCatalog {
    /// Build the catalog by loading SNAP + cache + overrides in order.
    /// If the cache is stale, attempts one refresh (failure is silent —
    /// SNAP serves as fallback).
    ///
    /// For explicit control, call `init(...).await?.refresh().await?`
    /// separately.
    pub async fn init(config: CatalogConfig) -> Result<Arc<Self>> {
        let (tx, _) = broadcast::channel(16);
        let cat = Arc::new(Self {
            state: Arc::new(RwLock::new(Snapshot::empty())),
            tx, config,
        });

        // 1. SNAP 임베드 (build-time include_bytes!)
        cat.load_embedded_snapshot().await;

        // 2. Runtime cache (mtime 신선하면 그대로, 아니면 stale 표시)
        if cat.try_load_fresh_cache().await.is_none() {
            tracing::debug!("catalog: cache stale or missing");
        }

        // 3. User overrides (Layer 2 — 최우선)
        cat.apply_user_overrides().await;

        // 4. LOCAL discovery (옵션, 비어있으면 skip)
        cat.discover_local_all().await;

        // 5. 캐시 stale 시 한 번 refresh 시도 (실패는 silent)
        if !cat.is_cache_fresh() && cat.config.fetch_enabled {
            let _ = cat.refresh().await;  // 결과 무시, SNAP 으로 작동 보장
        }

        Ok(cat)
    }
}
```

### 6.3 `protocol_for(npm) → CatalogProtocol`

dynamic-catalog §4.3 의 매핑 함수를 SDK 안으로 이관. 출력은 `Api` enum 이 아닌
`CatalogProtocol` enum (SDK 자기 타입).

```rust
// oxicode-sdk/src/ports/fs/catalog.rs (private)
fn protocol_for(npm: &str) -> CatalogProtocol {
    match npm {
        "@ai-sdk/anthropic" => CatalogProtocol::AnthropicMessages,
        "@ai-sdk/google" => CatalogProtocol::GoogleGenerativeAi,
        "@ai-sdk/google-vertex" | "@ai-sdk/google-vertex/anthropic" => {
            CatalogProtocol::GoogleVertex
        }
        "@ai-sdk/mistral" => CatalogProtocol::MistralConversations,
        "@ai-sdk/azure" => CatalogProtocol::AzureOpenAiResponses,
        "@ai-sdk/amazon-bedrock" => CatalogProtocol::BedrockConverseStream,
        "@ai-sdk/openai" | "@ai-sdk/openai-compatible" => CatalogProtocol::OpenAiCompletions,
        // unknown npm → OpenAI 호환 폴백 (대부분 gateway/aggregator)
        _ => CatalogProtocol::OpenAiCompatible,
    }
}
```

`auth_method` 는 별도 매핑 없음 — `entry.protocol.default_auth()` 로 파생.

### 6.4 refresh 흐름 (lazy on-call)

```rust
impl ModelCatalog for FileModelCatalog {
    fn refresh(&self) -> Pin<Box<dyn Future<Output = SdkResult<RefreshOutcome>> + Send + '_>> {
        let cat = self.clone_arc();
        Box::pin(async move {
            if !cat.config.fetch_enabled {
                return Ok(RefreshOutcome::Offline { reason: "fetch_disabled" });
            }
            // 1. mtime 신선 → HTTP skip
            if cat.is_cache_fresh() {
                return Ok(RefreshOutcome::Unchanged);
            }
            // 2. Conditional GET (ETag)
            match cat.fetch_conditional().await {
                Some(FetchResult::Updated(c)) => {
                    cat.replace_snapshot(c, CatalogSource::Live).await;
                    let stats = cat.snapshot_stats();
                    let _ = cat.tx.send(CatalogEvent::Updated {
                        provider_count: stats.providers,
                        model_count: stats.models,
                    });
                    Ok(RefreshOutcome::Updated { ... })
                }
                Some(FetchResult::NotModified) => {  // HTTP 304. port trait 의 RefreshOutcome::Unchanged 와 구분
                    cat.touch_cache_mtime().await;
                    Ok(RefreshOutcome::Unchanged)
                }
                None => {
                    let stats = cat.snapshot_stats();
                    let _ = cat.tx.send(CatalogEvent::RefreshFailed {
                        reason: "network".into(),
                        provider_count: stats.providers,
                        model_count: stats.models,
                    });
                    Ok(RefreshOutcome::Failed { reason: "network".into() })
                }
            }
        })
    }
}
```

> 백그라운드 task 없음. `oxicode refresh` CLI 명령은 `oxicode.catalog().refresh().await`
> 호출. SDK consumer 도 같은 메서드.

### 6.5 LOCAL discovery

`init()` 시점에 `local_discovery_urls` 가 있으면 `/v1/models` 를 1회 probe.
결과는 snapshot 에 merge. 각 entry 의 `source = CatalogSource::Local` 로
표시. 이후 갱신은 안 함 — LOCAL 은 보통 정적.

필요 시 `refresh_local(base_url: &str)` 메서드 (port trait 외부, impl 확장)
로 개별 갱신 가능. v2 에서 추가 검토.

### 6.6 Debug impl

```rust
impl std::fmt::Debug for FileModelCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.state.read();
        f.debug_struct("FileModelCatalog")
            .field("providers", &snap.providers.len())
            .field("models", &snap.models.values().map(|m| m.len()).sum::<usize>())
            .field("default_source", &snap.default_source)
            .finish_non_exhaustive()
    }
}
```

`tracing::debug!(?catalog)` 로 시작 시점 카탈로그 상태 확인.

## 7. 마이그레이션 — 무엇이 제거되는가

본 설계는 **breaking change** (사용자 합의). 다음이 전부 사라진다:

### 7.1 제거 대상

| 위치 | 심볼 | 이유 |
|---|---|---|
| `oxicode-ai/src/model_db.rs` | `ALL_PROVIDER_MODELS` (OnceLock), `try_materialize_from_snapshot`, `try_materialize_all`, `all_provider_models()`, `model_index()`, `provider_index()`, `get_all_models()`, `get_model_entry()`, `get_provider_models()`, `model_count()`, `builtin_model_count_sentinel()`, `get_providers()`, `search_models()`, `get_reasoning_models()`, `get_vision_models()`, `get_cheapest_models()`, `ModelEntry` | port 로 대체 |
| `oxicode-ai/src/model_registry.rs` | `STATIC_MODELS` (LazyLock), `add_openai_models()` 등 12개 함수, `ModelRegistry::from_static()`, `ModelRegistry::get()`, `ModelRegistry::get_by_provider()`, `ModelRegistry::all()`, `ModelRegistry::search()`, `ModelRegistry::model_ids()` (해당 부분만), **`ModelRegistry::lookup()`** (§7.3 에서 deprecated → 제거) | port 로 대체. 단 `ModelRegistry` 자체는 dynamic 등록용으로 보존 (register/lookup_dynamic/dynamic_models) |
| `oxicode-ai/src/catalog/{models_dev,materialize,provider,override_,model}.rs` | 전체 (`init_models_dev`, `MdCatalog`, `protocol_for`, `materialize`, `apply_*_overrides`, `load_builtin_providers`, `load_builtin_models`, `load_overrides`, ...) | `oxicode-sdk/src/ports/fs/catalog.rs` (FileModelCatalog) 으로 이관 |
| `oxicode-ai/src/providers/register_builtins.rs` | `get_builtin_providers()`, `get_builtin_provider()`, `get_provider_env_key()`, `get_provider_env_keys()`, `is_builtin_provider()`, `get_all_provider_names()`, `get_all_provider_aliases()`, `get_api_mappings()`, `get_provider_api()`, `get_provider_base_url()`, `resolve_provider_name()`, `BuiltinProvider` (struct 자체는 register factory 용으로 부분 보존 가능) | port 의 `get_provider()` 로 대체 |
| `oxicode-ai/src/lib.rs` | 위 re-export 전부 | |
| `oxicode-sdk/src/lib.rs` | `load_builtin_providers`, `builtin_providers_count`, `builtin_model_count`, `builtin_model_count_sentinel`, `BuiltinProviderEntry`, `BuiltinModelEntry`, `AuthMethod`, `OverrideFile`, `apply_*_overrides`, `find_override_files`, `load_overrides`, `discover_all*`, `discover_models`, `model_db::*` re-export 전부 | port 로 대체 |

### 7.2 보존 대상

| 위치 | 심볼 | 이유 |
|---|---|---|
| `oxicode-ai/src/model_registry.rs` | `ModelRegistry::register()`, `unregister()`, **`lookup_dynamic()`** (신규 분리), `dynamic_models()` | runtime 에 추가된 모델은 그대로 (예: oxicode-cli 의 `Oxicode::model()` builder method) |
| `oxicode-ai/src/types.rs` | `Api` enum, `Model` struct | SDK consumer 도 `Model` 직접 받음 (multi_provider 결과 등) |
| `oxicode-sdk/src/ports/catalog.rs` | `CatalogProtocol` enum, `CatalogModelEntry`, `CatalogProviderEntry` | SDK 의 port 표현. 신규 |

### 7.3 `ModelRegistry::lookup_dynamic()` 분리

현재 `ModelRegistry::lookup()` 은 `static_models` + `dynamic_models` 둘 다 봅니다.
v2 에서는 dynamic 만 분리:

```rust
impl ModelRegistry {
    /// Look up only in the dynamic registry (runtime-added models).
    ///
    /// The catalog port handles built-in / upstream models. Used by
    /// `Oxicode::resolve_model()` as the first-tier lookup before falling
    /// back to the catalog.
    pub fn lookup_dynamic(&self, provider: &str, model_id: &str) -> Option<Model> {
        let key = format!("{}/{}", provider, model_id);
        self.dynamic_models.read().get(&key).cloned()
    }

    // lookup() 자체는 deprecated 후 제거. dynamic + static 결합 뷰는
    // Oxicode::resolve_model() 로 이전 (dynamic 우선, catalog fallback).
}
```

`register()` / `unregister()` / `dynamic_models()` 는 그대로 (dynamic 만 다루므로).
`static_models` 필드 자체가 제거됨 — port 가 그 역할.

### 7.4 oxicode-cli 호출처 일괄 이관

oxicode-cli 는 `oxicode_sdk::get_all_models()` 등을 7+ 군데에서 사용 (setup_wizard,
tui/handlers, tui/slash, tui/overlay/settings, main, tui/handlers 등). 모두
다음 패턴으로 교체:

```rust
// before
let models: Vec<&ModelEntry> = oxicode_sdk::get_all_models().collect();

// after
let all = oxicode.catalog().search("").await?;          // 전체
let per = oxicode.catalog().list_models(provider_id).await?;  // 특정 provider
```

내부 oxicode-ai 코드 (`fallback_chain`, `multi_provider::model_from_entry` 등) 는:

```rust
// before
fn try_model(provider: &str, model: &str) -> Option<Model> {
    let entry = get_model_entry(provider, model)?;
    // entry 는 &'static ModelEntry
    Some(model_from_entry(entry))
}

// after
async fn try_model(
    catalog: &Arc<dyn ModelCatalog>,
    provider: &str, model: &str,
) -> Result<Option<Model>> {
    let entry = catalog.get_model(provider, model).await?;
    Ok(entry.map(model_from_catalog_entry))
}
```

`Arc<dyn ModelCatalog>` 를 전달하기 위해 `MultiProvider`, `FallbackChain`, `Agent` 등의
constructor signature 가 바뀐다. **`MultiProvider::new`/`Agent::new_with_resolver`
등에 catalog 파라미터 추가**. 변경 범위:

- `oxicode-ai/src/multi_provider.rs`: `MultiProviderBuilder` 에 `with_catalog()`
- `oxicode-ai/src/fallback_chain.rs`: `FallbackChain::new` 에 `Arc<dyn ModelCatalog>`
- `oxicode-agent/src/agent_loop/*`: `Agent` 가 `Arc<dyn ModelCatalog>` 보유
- `oxicode-sdk/src/builder.rs`: `Oxicode` 가 catalog 를 보유하고 agent 에 전달

### 7.5 `Oxicode::create_provider` async 화 — bridge layer 는 `oxicode-sdk`

기존 `create_provider(&self, name: &str) -> Result<Arc<dyn Provider>>` 는 sync.
내부에서 `oxicode_ai::create_builtin_provider(name)` 를 호출하는데, 이 함수는
`register_builtins::get_builtin_provider()` 로 `&'static BuiltinProvider` 를
받아 provider 인스턴스를 만든다. 본 설계에서 `&'static BuiltinProvider` 가
사라지므로, **provider 인스턴스 생성이 catalog 조회에 의존**하게 되고 catalog
조회는 async.

→ **`create_provider` 자체를 async 로 변경**:

```rust
// before
pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>>;

// after
pub async fn create_provider(&self, name: &str) -> SdkResult<Arc<dyn Provider>>;
```

#### 의존 방향 주의 (v3 정정)

**bridge layer 의 위치: `oxicode-sdk/src/bridge.rs` (NOT `oxicode-ai`)**.

`oxicode-ai` 는 `oxicode-sdk` 에 의존하지 않으므로, `create_provider_from_entry`
가 `oxicode-ai` 에 있으면 `CatalogProviderEntry` (oxicode-sdk 타입) 을 볼 수 없어
**컴파일 에러**. oxicode-sdk → oxicode-ai 정방향 의존은 이미 존재하므로, bridge 를
oxicode-sdk 에 두면 자연스럽게 모든 타입을 볼 수 있다.

```rust
// oxicode-sdk/src/bridge.rs (신규)
//! SDK port types → oxicode-ai provider instances.
//! 이 모듈이 SDK → oxicode-ai 정방향 의존의 단일 소스다.

use std::sync::Arc;
use crate::ports::catalog::{CatalogProviderEntry, CatalogProtocol};

/// Build a concrete `oxicode_ai::Provider` from a catalog entry.
///
/// Dispatch 는 `entry.protocol.as_oxicode_api()` 로 `oxicode_ai::Api` 변환 후
/// 해당 provider impl 생성자로 위임. auth 는 `protocol.default_auth()`
/// 에서 파생, base_url 은 entry 의 것 우선.
///
/// Returns `None` for unknown protocols (e.g. 향후 추가된 variant 로
/// bridge 가 아직 미구현). 호출처는 `SdkError::Internal` 로 래핑.
pub fn create_provider_from_entry(
    entry: &CatalogProviderEntry,
    api_key: Option<&str>,
) -> Option<Arc<dyn oxicode_ai::Provider>> {
    use oxicode_ai::Api;
    let base = entry.base_url.as_deref();
    let auth = entry.protocol.default_auth();
    let api = entry.protocol.as_oxicode_api();
    match api {
        Api::AnthropicMessages => Some(Arc::new(
            oxicode_ai::AnthropicProvider::with_base_url_and_key(
                base.unwrap_or("https://api.anthropic.com"),
                api_key,
            ),
        )),
        Api::OpenAiCompletions | Api::OpenAiResponses => Some(Arc::new(
            oxicode_ai::OpenAiProvider::with_base_url_and_key(
                base.unwrap_or("https://api.openai.com/v1"),
                api_key,
            ),
        )),
        // ... Google, Vertex, Mistral, Azure, Bedrock ...
        _ => None,
    }
}
```

`Oxicode::create_provider` 구현:

```rust
// oxicode-sdk/src/builder.rs
pub async fn create_provider(&self, name: &str) -> SdkResult<Arc<dyn oxicode_ai::Provider>> {
    // 1. 커스텀 프로바이더 우선 (OxicodeBuilder::provider() / provider_factory())
    if let Some(p) = self.providers.get_custom(name) {
        return Ok(p);
    }
    // 2. catalog 에서 메타데이터 조회
    let entry = self.ports.catalog.get_provider(name).await?
        .ok_or_else(|| SdkError::ProviderNotFound { provider: name.into() })?;
    // 3. SDK bridge dispatch
    let api_key = self.api_keys.get(name).map(|s| s.as_str());
    crate::bridge::create_provider_from_entry(&entry, api_key)
        .ok_or_else(|| SdkError::Internal(anyhow::anyhow!(
            "no bridge dispatch for protocol={:?}", entry.protocol
        )))
}
```

**변경 정리**:

- `as_oxicode_api()` 의 feature flag 제거 (항상 컴파일).
- `create_provider_from_entry` 는 `oxicode_sdk::bridge` 에 위치 (oxicode-ai 아님).
- `oxicode-ai` crate 는 catalog port 타입을 import 하지 않음. 역방향 의존 0 유지.

호출처 (oxicode-sdk 테스트 6+ 개, oxicode-cli 의 provider resolution 등) 모두
`.await` 추가. PR 3 의 일부.

### 7.6 `Oxicode::resolve_model` async 화 (§4.4)

이미 §4.4 에서 정의. **여기서 추가로**: `AgentBuilder::build()` 가 `resolve_model`
을 부르므로, 그것도 async 가 됨. 시그니처 변경:

```rust
// before
impl AgentBuilder<'_> {
    pub fn build(self) -> SdkResult<Agent>;
}

// after
impl AgentBuilder<'_> {
    pub async fn build(self) -> SdkResult<Agent>;
}
```

`Oxicode::agent(config)` 자체는 sync builder 반환. `.build()` 만 async.

호출처 (oxicode-cli 의 `app/agent_session.rs`, oxicode-sdk 의 integration tests 등) 모
두 `.await` 추가. PR 3 의 일부.

### 7.7 `OxicodeBuilder::with_builtins()` 처리

기존 `with_builtins()` 는 두 가지를 동시에 했다:

1. `ModelRegistry::from_static()` — 50 개 하드코딩 모델 등록
2. `include_builtins = true` — `create_provider()` 가 built-in factory 로 fallback

본 설계에서 (1) 은 port 가 대체 (catalog 가 데이터 소스), (2) 는 **항상 true**
가 자연스러움 (catalog 가 있으면 자동 사용). 따라서:

- `with_builtins()` 메서드 자체를 **제거** (deprecated 후).
- `Oxicode::has_builtins()` 도 제거.
- `create_provider()` 가 catalog 미등록 provider 에 대해 `NoopModelCatalog`
  를 감지하면 `ProviderNotFound` 반환 (기존과 동일).
- 마이그레이션 경고: `with_builtins()` 호출 → `with_catalog(FileModelCatalog::init(default_config()).await?)` 로 대체.

> ⚠️ **PR 2 composition root 주의**: `bootstrap.rs` 가 반드시
> `with_catalog(FileModelCatalog::init(...).await?)` 를 호출해야 함. 빠뜨리면
> 모든 `create_provider()` / `resolve_model()` 이 `ProviderNotFound` /
> `ModelNotFound` 를 반환. `NoopModelCatalog` 는 진짜 빈 카탈로그라 빈 결과만
> 준다. CI 에 smoke test (`oxicode.catalog().model_count().await? > 0`) 추가 권장.

### 7.8 `ProviderResolver` trait async 화 (v3 정정 — Critical ripple)

`Oxicode::create_provider` (§7.5) 와 `Oxicode::resolve_model` (§4.4) 이 async 가
되면서, `oxicode_agent::ProviderResolver` trait 도 async 가 되어야 한다.
현재 다음과 같이 sync 다:

```rust
// oxicode-agent/src/agent.rs (현재)
pub trait ProviderResolver: Send + Sync {
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn Provider>>;
    fn resolve_model(&self, model_id: &str) -> Option<Model>;
}

impl ProviderResolver for Oxicode {
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.create_provider(name).ok()   // ← sync
    }
    fn resolve_model(&self, model_id: &str) -> Option<Model> {
        self.resolve_model(model_id).ok() // ← sync
    }
}
```

본 설계로 두 메서드가 async 가 되므로, trait 전체가 async 서명으로 바뀐다.

```rust
// after
pub trait ProviderResolver: Send + Sync {
    fn resolve_provider<'a>(
        &'a self, name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<Arc<dyn Provider>>> + Send + 'a>>;

    fn resolve_model<'a>(
        &'a self, model_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<Model>> + Send + 'a>>;
}

impl ProviderResolver for Oxicode {
    fn resolve_provider<'a>(
        &'a self, name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<Arc<dyn oxicode_ai::Provider>>> + Send + 'a>> {
        Box::pin(async move { self.create_provider(name).await.ok() })
    }

    fn resolve_model<'a>(
        &'a self, model_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<oxicode_ai::Model>> + Send + 'a>> {
        Box::pin(async move { self.resolve_model(model_id).await.ok() })
    }
}
```

**Ripple 효과** (PR 3 의 일부):

- `oxicode-agent/src/agent.rs::Agent::new_with_resolver` → resolver 는 저장만
  하고 (sync 유지), 실제 resolve_* 호출은 `agent_loop` 안에서.
- `oxicode-agent/src/agent_loop/{stream,tool_exec,...}.rs` → provider/model 해상
  시점마다 `.await` 추가.
- `oxicode-ai/src/multi_provider.rs::MultiProvider` → 동일.
- `oxicode-ai/src/fallback_chain.rs::FallbackChain` → 동일.
- 테스트: `MockProviderResolver` 도 async signature 로 갱신. 모든 test helper
  에 `.await` 추가.

> ⚠️ **PR 3 의 대공사 영역**. §7.4 에서 "`MultiProvider::new`/
> `Agent::new_with_resolver` 등에 catalog 파라미터 추가"라고만 적었지만,
> 실제로는 trait 자체가 async 화 되면서 **모든 resolve_* 호출부가 async**
> 가 된다. 이 sub-section 이 그 총량을 명시한다. PR 3 은 §7.3 ~ §7.8 전부를
> 담은 가장 큰 PR 이다.

> **대안 고려 (v3에서 기각)**: `Oxicode` 가 두 가지 resolver trait 를 구현하는
> 방법 — `SyncProviderResolver` (dynamic map 만 본다, sync) 와
> `AsyncProviderResolver` (catalog fallback, async). Agent 가 하나를 택하도록.
> 기각 이유: agent 의 resolve 는 built-in 모델도 필요 (catalog fallback),
> 따라 두 trait 중 하나로 통일하는 것이 호출처 코드를 더 단순하게 만든다.

### 7.9 v4 정정 — sync read API 로 async화 ripple 전면 회피

> **구현 중 발견된 단순화.** 이 정정은 §7.3, §7.5, §7.6, §7.8 전부를
> **무효화**한다. PR 3 은 "대공사" 가 아니라 작은 PR 이 되었다.

**핵심 통찰**: catalog 의 데이터는 이미 메모리에 `Arc<RwLock<Snapshot>>`
으로 존재한다. 따라서 sync read 는 I/O 가 아닌 단순 락 획득 + clone 이다.
이는 async 가 필요한 근거 (network/file I/O) 를 제거한다.

**추가된 API** (`ModelCatalog` trait, 기본 구현은 noop):

```rust
fn list_providers_sync(&self) -> Vec<String> { Vec::new() }
fn get_provider_sync(&self, _id: &str) -> Option<CatalogProviderEntry> { None }
fn list_models_sync(&self, _provider: &str) -> Vec<CatalogModelEntry> { Vec::new() }
fn get_model_sync(&self, _provider: &str, _model: &str)
    -> Option<CatalogModelEntry> { None }
fn search_sync(&self, _pattern: &str) -> Vec<CatalogModelEntry> { Vec::new() }
fn model_count_sync(&self) -> usize { 0 }
```

`FileModelCatalog` 는 async 버전과 동일한 RwLock read 패턴으로 구현한다
(락 → clone → 즉시 반환, Future 생성 없음).

**결과적으로 무효화된 작업들** (모두 PR 3 에서 제거됨):

| 설계 v3 항목 | v4 상태 | 이유 |
|---|---|---|
| §7.3 `ModelRegistry::lookup_dynamic()` 분리 | ❌ 불필요 | `Oxicode::resolve_model` 이 catalog sync API 로 직접 조회 |
| §7.5 `Oxicode::create_provider` async 화 | ❌ 불필요 | provider 객체 생성은 factory 기반이라 catalog 와 무관 |
| §7.6 `Oxicode::resolve_model` async 화 | ❌ 불필요 | catalog sync read 로 충분 |
| §7.7 `AgentBuilder::build()` async 화 | ❌ 불필요 | resolve_model 이 sync 면 build 도 sync |
| §7.8 `ProviderResolver` trait async 화 | ❌ 불필요 | resolve_* 가 sync 면 trait 도 sync |
| §7.8 ripple (agent_loop, multi_provider, fallback_chain) | ❌ 불필요 | trait 이 sync 면 `.await` 추가 불필요 |
| §7.4 모든 호출처 `.await` 추가 | ❌ 불필요 | sync API 로 sync 컨텍스트에서 호출 가능 |

**유지되는 작업** (PR 3 의 실제 내용):

- `bridge.rs` (`crate::bridge`): `catalog_entry_to_model()` + `provider_base_url()`
  + modality 변환. SDK 소유 (oxicode-ai 역방향 의존 방지).
- `Oxicode::resolve_model` 의 catalog fallback 통합: catalog sync API → bridge →
  `oxicode_ai::Model` (sync 유지).

**TUI 통합 패턴**: `AppState` 에 `catalog: Option<Arc<dyn ModelCatalog>>`
필드 추가. TUI 이벤트 핸들러 (sync 컨텍스트) 는 `cat.search_sync(...)` 등으로
조회. `Option` 인 이유는 단위 테스트/비-TUI 모드 호환성.

**Open Q (v3 §10) #3 정정**: "`refresh()` blocking 버전 필요?" → 여전히
async only 가 맞다 (refresh 는 진짜 network I/O). 단 **read** API 는 sync 가
충분하다. read/refresh 분리가 핵심 통찰이다.

## 8. Subscription 사용 패턴

### 8.1 SDK consumer 기본

```rust
let oxicode = OxicodeBuilder::new().with_catalog(catalog).build();

// Spawn a background task that reacts to updates
let mut rx = oxicode.catalog().subscribe();
tokio::spawn(async move {
    while let Ok(event) = rx.recv().await {
        match event {
            CatalogEvent::Updated { model_count, .. } => {
                tracing::info!("catalog updated: {model_count} models");
                // invalidate UI caches, re-pick default, etc.
            }
            CatalogEvent::RefreshFailed { reason, .. } => {
                tracing::warn!("catalog refresh failed: {reason}");
            }
            CatalogEvent::LocalDiscovered { base_url, model_count } => {
                tracing::info!("+{model_count} models from {base_url}");
            }
            _ => {}
        }
    }
});
```

### 8.2 oxicode-cli 통합

`oxicode-cli/src/tui/app.rs` 가 `Oxicode` 를 보유하므로, app 시작 시 subscription 한 개
spawn:

```rust
impl App {
    pub async fn handle_catalog_events(&mut self, mut rx: broadcast::Receiver<CatalogEvent>) {
        while let Ok(event) = rx.recv().await {
            match event {
                CatalogEvent::Updated { .. } => {
                    self.model_picker.invalidate();
                    self.status_bar.flash("Catalog updated");
                }
                CatalogEvent::LocalDiscovered { base_url, .. } => {
                    self.status_bar.flash(format!("+local models: {base_url}"));
                }
                _ => {}
            }
        }
    }
}
```

### 8.3 수동 refresh

```rust
// CLI 서브커맨드
async fn cmd_refresh(oxicode: &Oxicode) -> Result<()> {
    match oxicode.catalog().refresh().await? {
        RefreshOutcome::Updated { provider_count, model_count } => {
            println!("✓ {provider_count} providers, {model_count} models (updated)");
        }
        RefreshOutcome::Unchanged => println!("✓ already up to date"),
        RefreshOutcome::Offline { reason } => println!("⚠ offline: {reason}"),
        RefreshOutcome::Failed { reason } => bail!("refresh failed: {reason}"),
    }
    Ok(())
}
```

## 9. Test Plan

### 9.1 Port contract

| 테스트 | 내용 |
|---|---|
| `noop_catalog_returns_empty` | `NoopModelCatalog::new().list_providers().await == Ok(vec![])` |
| `noop_subscribe_doesnt_panic` | subscribe 후 recv → 채널 닫힘 (Err) |
| `noop_refresh_is_unchanged` | refresh → `RefreshOutcome::Unchanged` |

### 9.2 FileModelCatalog (deterministic, fixture 기반)

| 테스트 | 내용 |
|---|---|
| `snap_only_no_cache` | fixture SNAP → 145 provider, 5277 model 카운트 단언 |
| `cache_overrides_snap_when_fresh` | mtime 5분, fixture cache → cache 의 last_updated 반영 |
| `cache_stale_triggers_fetch_in_refresh` | mtime 2h → refresh 가 conditional GET 발사 (mock server) |
| `conditional_get_etag_match_returns_unchanged` | mock server 가 304 → `Unchanged`, mtime 갱신 |
| `conditional_get_etag_mismatch_updates` | mock server 200 → Updated, 캐시 파일 atomically written |
| `override_wins_over_everything` | overrides.toml 이 model/provider 덮어쓰기 (Layer 2) |
| `override_invalid_toml_errors` | 손상된 overrides.toml → SdkError::Internal (계속 SNAP/LIVE 로 작동) |
| `local_discovery_merges_into_snapshot` | mock ollama `/v1/models` → discovered models 추가 |
| `event_emitted_on_update` | refresh 성공 시 broadcast::Receiver 가 `CatalogEvent::Updated` 받음 |
| `event_emitted_on_refresh_failed` | refresh 실패 (network down) → RefreshFailed |
| `event_emitted_on_override_applied` | overrides.toml 적용 시 OverrideApplied |
| `subscription_capacity_16` | 17번 연속 refresh → 수신자는 최소 1개 받음 (broadcast 한계) |

### 9.3 SDK integration

| 테스트 | 내용 |
|---|---|
| `oxicode_catalog_accessor` | `OxicodeBuilder::new().with_catalog(c).build().catalog()` 가 등록한 c 와 동일 Arc |
| `oxicode_catalog_delegates_to_registered` | `oxicode.catalog().list_providers()` 가 `Arc::clone(c).list_providers()` 와 동일 결과 |
| `oxicode_noop_when_catalog_unset` | `OxicodeBuilder::new().build()` 의 catalog 는 `NoopModelCatalog` |
| `oxicode_refresh_returns_outcome` | `oxicode.catalog().refresh()` 가 RefreshOutcome enum 반환 |
| `oxicode_subscribe_yields_receiver` | `oxicode.catalog().subscribe()` 가 broadcast::Receiver 반환, drop 시 unsubscribe |
| `oxicode_resolve_model_dynamic_wins` | `Oxicode::model(M)` 로 등록 후 `resolve_model("p/M")` 이 dynamic 반환 |
| `oxicode_resolve_model_catalog_fallback` | dynamic 에 없으면 catalog 의 `get_model` 호출 → 반환 |
| `oxicode_resolve_model_not_found` | 둘 다 없으면 `SdkError::ModelNotFound` |
| `oxicode_resolve_model_bare_id` | `"claude-..."` → `("anthropic", "claude-...")` 로 파싱 |
| `catalog_debug_includes_stats` | `FileModelCatalog` 의 `Debug` impl 에 providers/models 카운트 + source 포함 |

### 9.4 oxicode-cli 회귀

| 테스트 | 내용 |
|---|---|
| `setup_wizard_uses_catalog_port` | `oxicode.catalog().list_providers()` 결과로 wizard 모델 목록 채움 |
| `tui_model_picker_uses_catalog` | picker 가 `oxicode.catalog().search()` 호출 |
| `cli_models_list_command` | `oxicode models list` 가 port 결과 출력 |
| `cli_models_refresh_command` | `oxicode models refresh` 가 `oxicode.catalog().refresh().await` |
| `bootstrap_registers_file_catalog` | `bootstrap.rs` 가 반드시 `with_catalog(FileModelCatalog::init(...))` 호출 — smoke assertion |
| `noop_catalog_smoke_fails_loudly` | `with_catalog()` 빠뜨리면 `create_provider`/`resolve_model` 이 NotFound (CI gate) |

### 9.5 AGENTS.md 게이트

- `cargo fmt --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings`
- `cargo nextest run --workspace`
- `cargo audit`
- `cargo deny check`

## 10. Open Questions

| # | 질문 | 현재 안 | 결정 필요 시점 |
|---|---|---|---|
| 1 | `OxicodeBuilder::build()` 가 `FileModelCatalog` 를 **자동으로** 설치해야 하나? (이전 `init_models_dev()` 와 비슷한 역할) | **아니오**. 명시적 `with_catalog()` 강제. SDK 의 "no global state" 원칙 유지 | oxicode-cli composition root 가 항상 `with_catalog(FileModelCatalog::init(...).await?)` 호출 — PR 2 에서 검증 |
| 2 | `CatalogModelEntry` 가 `CatalogProtocol` enum 을 갖는 트레이드오프 — SDK 가 oxicode-ai `Api` 에 약결합? | `CatalogProtocol` 이 SDK 소유. 변환은 `as_oxicode_api()` (항상 컴파일, v3 정정). oxicode-ai → SDK 역방향 의존 0. SDK → oxicode-ai 정방향은 이미 존재 (bridge layer 가 단일 소스, §7.5) | OK (확정) |
| 3 | `oxicode.catalog().refresh()` 가 **blocking** 버전도 필요한가? | 첫 버전은 async only. blocking 은 `tokio::runtime::Handle::block_on` 으로 충분 | 향후 필요 시 추가 |
| 4 | LOCAL discovery 를 port 의 **core** 에 둘지, 별도 port 로 뺄지 | core 에 둠 (config 옵션). LOCAL 도 결국 "모델 메타데이터" 니 같은 카테고리 | 향후 다중 백엔드 필요 시 분리 |
| 5 | `CatalogEvent::Updated` 가 **delta** (변경된 모델 목록) 를 줘야 하나? | 첫 버전은 total count만. delta 는 비용 vs 가치 트레이드오프 — 필요 시 v4 에서 | UI 가 "new!" 뱃지 필요해지면 추가 |
| 6 | 기존 `Oxicode::model(model)` builder method (단일 모델 등록) 와 port 의 관계? | 보존. `ModelRegistry::register` 도 보존. 등록된 모델은 catalog 에 자동 노출? **아니오** — port 의 snapshot 과 dynamic registry 는 별개 lookup tier. `Oxicode::resolve_model` 이 dynamic 우선 + catalog fallback (§4.4) | OK (§4.4, §7.3) |
| 7 | `OxicodeBuilder::with_catalog()` 가 `FileModelCatalog::init(...)` 로딩 도중 **await** 해야 하나 (현재 builder 는 sync) | builder 는 sync 유지. `FileModelCatalog::init()` 는 사전에 await 해서 Arc 를 받음. 표준 async-before-sync 패턴 | OK |
| 8 | 백그라운드 자동 refresh task 는 정말 안 두나? (사용자 결정: lazy on-call) | 두지 않음. 단, `tokio::spawn(catalog.clone().refresh_loop(interval))` 패턴을 consumer 가 직접 쓸 수 있도록 `FileModelCatalog::refresh_loop(interval)` 메서드 옵션 — port trait 외부, impl 확장 | 구현 시 메서드 시그니처 결정 |
| 9 | `auth_method` 필드를 entry 에서 완전히 제거하는 게 안전한가? | 모든 known protocol 의 auth 가 `default_auth()` 로 결정됨 (dynamic-catalog §2.3 검증 완료). 예외 없음. 단 향후 models.dev 에 미지의 npm 추가되어 auth 가 다른 경우를 대비해 `provider.auth_override: Option<AuthMethod>` 한정적 override 필드 가능 | OK (현재는 제거 확정) |
| 10 | `CatalogModelEntry.model_id` 필드명 변경은? (기존 `BuiltinModelEntry::id`) | 필드명 변경. 사용처 명확성 > 기존 일관성. `BuiltinModelEntry` 자체가 제거되니 일관성 부담 없음 | OK (확정) |
| 11 | `RefreshOutcome::Failed` 가 `Ok(Failed)` 로 반환되는 의미론 — 호출자가 `.await?` 로 못 잡음 | **의도적** (v3 정정). lazy on-call 원칙: refresh 실패해도 SNAP 으로 작동. 호출자는 `match` 로 `Failed` 변종을 선택적 처리. doc 에 명시 (§4.1). CLI 는 `match` 로 로깅. | OK (확정) |
| 12 | `SdkError` 의 catalog 변종 추가? | v3 정정로 추가 권장: `CatalogUnavailable { reason }`, `CatalogOverrideParse { path, reason }`, `CatalogRefresh { reason }`. 현재 `SdkError::Internal` fallback 보다 구체 match 가능 | PR 1 에서 `error.rs` 갱신 |

## 11. 검증 체크리스트 (구현 완료 시)

- [ ] `cargo build` / `cargo clippy -D warnings` clean (모든 feature)
- [ ] `cargo nextest run --workspace` 통과
- [ ] `oxicode-cli` 의 기존 catalog 사용처 (setup_wizard, tui/*, main, models 커맨드) 전부 port 로 이관
- [ ] `OXICODE_MODELS_DEV=off` 에서도 SNAP 으로 작동
- [ ] `OXICODE_MODELS_DEV_DISABLE_FETCH=1` 에서 network 안 나감
- [ ] `oxicode refresh` 가 `oxicode.catalog().refresh().await` 호출, 결과 stdout 출력
- [ ] subscription: `oxicode.catalog().subscribe()` 로 `Updated` 이벤트 수신 확인 (CLI 데모)
- [ ] `data/catalog/_snapshot.json.gz` 가 그대로 임베드 (FileModelCatalog::init 시)
- [ ] `~/.oxicode/catalog/overrides.toml` 이 Layer 2 로 작동
- [ ] local discovery (ollama mock) 가 snapshot 에 merge, entry.source = `Local`
- [ ] `Oxicode::resolve_model("anthropic/claude-...")` 가 catalog fallback 으로 작동
- [ ] `Cargo.toml` 에서 `oxicode-sdk` 가 `oxicode-ai` 에 의존하지 않음 (port 트레이트만 oxicode-ai-agnostic)
- [ ] `cargo audit`, `cargo deny check` clean
- [ ] `docs/designs/2026-06-17-dynamic-catalog-design.md` 의 §4.3 `protocol_for` 출력 타입과 §4.4 materialize 가 본 설계와 동기화
- [ ] 외부 consumer (oxios, oxibrowser) changelog/README 에 `with_builtins()` → `with_catalog()` 마이그레이션 명시

## 12. 구현 단계 (PR 분할 가이드)

큰 PR 하나보다 작은 PR 5개가 리뷰/롤백에 유리:

### PR 1: Port 추가 (additive, breaking 없음)
- `oxicode-sdk/src/ports/catalog.rs` (ModelCatalog trait + noop + `CatalogProtocol` enum + types)
- `PortRegistry::catalog` 추가
- `OxicodeBuilder::with_catalog()`, `Oxicode::catalog()`
- `oxicode-sdk/src/ports/fs/catalog.rs` (FileModelCatalog::init, `protocol_for(npm) → CatalogProtocol`)
- 기존 API 는 **전부 유지** (deprecated 표시만)
- 테스트: §9.1, §9.2

### PR 2: oxicode-cli 이관
- oxicode-cli 의 7+ 호출처를 port 로 교체
- `bootstrap.rs` 가 `FileModelCatalog::init(...)` 호출하고 `with_catalog()` 로 등록
- `OxicodeBuilder::build()` 호출 직후 subscription 한 개 spawn → status bar / picker invalidate
- `oxicode refresh` 명령이 `oxicode.catalog().refresh().await` 호출
- 기존 API deprecated 경고 활성화
- 테스트: §9.4

### PR 2.5: TUI + setup wizard 호출처 (sync API)
> v4 정정 (§7.9) 로 추가된 PR. sync read API 로 TUI sync 컨텍스트에서
> catalog 조회 가능.

- `ModelCatalog` trait 에 **sync read API** 6 개 추가 (`*_sync`),
  기본 구현 noop. `FileModelCatalog` 구현.
- `AppState.catalog: Option<Arc<dyn ModelCatalog>>` 필드 추가, TUI 시작 시 주입
- `App::oxicode()` getter 추가
- `tui/slash`, `tui/handlers`, `tui/overlay/{settings,provider_select}` 호출처
  catalog sync API 로 이관 (legacy fallback 유지)
- `setup_wizard.rs` `load_providers`/`load_models` catalog-aware, `run()` async
- `ProviderEntry.env_key` 필드 추가 (catalog 에서 채움)
- 테스트: catalog_port sync API 2 개 + 회귀

### PR 3: bridge layer + `Oxicode::resolve_model` catalog 통합 (v4 축소)
> **v4 정정 (§7.9)**: 원래 "oxicode-ai 내부 이관 + ProviderResolver async ripple"
> 이었으나, sync read API 도입으로 **대폭 축소**. async화 ripple 전면 회피.

- **bridge layer 신설** (`oxicode-sdk/src/bridge.rs`): `catalog_entry_to_model()`,
  `provider_base_url()`, modality 변환. SDK 소유 (oxicode-ai 역방향 의존 방지).
  7 개 단위 테스트.
- `Oxicode::resolve_model()` 의 catalog fallback 통합: catalog sync read → bridge →
  `oxicode_ai::Model`. **sync 유지** (async화 없음).
- **무효화됨** (§7.9 표): `ModelRegistry::lookup_dynamic()`, `create_provider`
  async 화, `AgentBuilder::build()` async 화, `ProviderResolver` trait async 화,
  agent_loop/multi_provider/fallback_chain ripple — 전부 불필요.
- `CatalogProtocol::as_oxicode_api()` 는 이미 항상 컴파일 (feature flag 없음).
- 테스트: `oxicode_resolve_model_uses_catalog` + bridge 단위 7 개 + 회귀

### PR 4: 정리 (부분 완료)
> `SdkError` catalog 변종만 완료. 나머지는 위험/효용 트레이드오프로 연기.

- ✅ **`SdkError` catalog 변종 추가** (§10 Q12): `CatalogUnavailable`,
  `CatalogOverrideParse`, `CatalogRefresh`.
- ⏸️ `OxicodeBuilder::with_builtins()` 제거 — provider factory 역할도 하므로
  함부러 제거 불가. deprecated 후 다음 메이저로 연기.
- ⏸️ `oxicode-ai/src/catalog/` 모듈 제거 — `FileModelCatalog` 가 `include_bytes!`
  로 oxicode-ai 의 SNAP 파일을 가져오므로 제거 불가 (데이터 위치).
- ⏸️ deprecated 표시된 SDK API 제거 — custom provider 동적 등록
  (`fetch_models_blocking`/`register_model`) 은 models.dev 와 별개 로직으로
  유지. setup_wizard/agent_session_runtime 의 `get_provider` fallback 도 유지.
- ⏸️ 문서 동기화 — §7.9 v4 정정으로 대체 완료.

### PR 5: External consumer 알림
- oxios, oxibrowser 등 sister repo 의 사용처 일괄 이관 (또는 changelog)
- README/changelog 에 breaking change 명시
- 외부 SDK consumer 를 위한 마이그레이션 가이드 (간단한 코드 변환 예시)

> 각 PR 은 cargo nextest + clippy 통과 후 머지. PR 1 만으로는 사용자가 볼 변화 0
> (SDK 새 API 추가만). PR 2 부터 oxicode-cli UX 가 동적으로 변함. PR 4 완료 시점에
> 모든 free fn 제거. PR 5 는 외부 repo 와의 조율 필요.
>
> **v4 상태 (구현 완료)**: PR 1 → PR 2 → PR 2.5 → PR 3 (축소) → PR 4 (부분)
> 모두 완료. 2297/2297 테스트 통과. 남은 PR 4 정리 항목과 PR 5 는 별도.

---

## 부록 A: v1 → v3 변경 요약

### v2 리뷰에서 발견된 7개 결함 (v2 정정)

v1 리뷰에서 7개 결함 발견, v2 에서 정정:

| # | 결함 (v1) | 정정 (v2) |
|---|---|---|
| 1 | `CatalogModelEntry.api: String` — SDK consumer 가 런타임 string 매칭 | `protocol: CatalogProtocol` enum. 컴파일 타임 검증. §4.1 |
| 2 | `auth_method` 를 entry 에 별도 필드로 저장 | `entry.protocol.default_auth()` 로 파생. 필드 삭제. §4.1 |
| 3 | `Oxicode::resolve_model` 이 sync + dynamic 만 — catalog 자동 fallback 없음 | async 화 + dynamic 우선 + catalog fallback. §4.4 |
| 4 | `Oxicode` 에 6 개 async pass-through (`list_providers` 등) — 표면 비대칭 | `catalog()` accessor 한 개만. 나머지는 `oxicode.catalog().*` 로 직접. §4.4 |
| 5 | `CatalogEvent::Refreshed` 가 `source` 포함 — `current_source()` 와 의미 중복 | `Updated` 로 통일, source 제거. `current_source()` 는 trait 에서 제거, impl 의 `Debug` impl 로 진단 이동. §4.1, §5.6 |
| 6 | `RefreshOutcome::{NotModified, Stale}` — "안 바뀜" 과 "안 시도함" 의 구분이 모호 | `Unchanged / Offline / Failed`. 의도 명확. §4.1 |
| 7 | `FileModelCatalog::load` + `load_with_refresh` — 두 메서드가 하는 일이 거의 같음 | 단일 `init()` — load + stale cache 시 1회 refresh 시도. §6.2 |
| (보너스) | `CatalogSource` 가 catalog 단위 — multi-source catalog (SNAP + override + local) 에서 의미 모호 | per-entry `CatalogModelEntry.source` 로 이동. 전체 source 의미 사라짐. §5.4 |
| (보너스) | `id` 필드명 — 사용처 모호 | `model_id` 로 명확. §4.1 |

### v3 리뷰에서 발견된 3개 Critical + 6개 불일치 + 3개 개선 (v3 정정)

| # | 결함 (v2) | 정정 (v3) |
|---|---|---|
| 🔴 C1 | `create_provider_from_entry` 가 oxicode-ai 에 위치하면 `CatalogProviderEntry` (oxicode-sdk 타입) 을 못 봄 → **컴파일 에러**. 또한 "SDK → oxicode-ai 의존 0" 주장이 현실과 불일치 (이미 재노출 중) | bridge layer 를 `oxicode-sdk/src/bridge.rs` 에 위치. `as_oxicode_api()` feature flag 제거 (항상 컴파일). 주장을 "oxicode-ai → SDK 역방향 의존 0" 으로 정정. §4.1, §7.5 |
| 🔴 C2 | `NoopModelCatalog` 가 `#[derive(Debug)]` + manual `impl Debug` 동시持有 → **컴파일 에러** | `#[derive(Default)]` 만 남기고 `Debug` 는 manual impl 유지. §4.2 |
| 🔴 C3 | `Oxicode::create_provider` / `resolve_model` 이 async 가 되면 `ProviderResolver` trait 도 async — v2 에서 전혀 언급 안 됨 | §7.8 신설: trait async 서명 + ripple (`oxicode-agent`, `oxicode-ai` 전체 resolve_* 호출부). 기각된 대안 (`SyncProviderResolver` + `AsyncProviderResolver` 분리) 도 명시 |
| 🟡 U1 | §3 아키텍처 다이어그램이 v1 잔재 (`oxicode.list_providers()`, `oxicode.refresh_catalog()`) | `oxicode.catalog().*` 로 통일 + `oxicode.resolve_model()` 추가 |
| 🟡 U2 | §5.6 이 존재하지 않는 §4.5 참조 | §6.6 으로 정정 |
| 🟡 U3 | §9.3 테스트 `oxicode_convenience_methods_delegate` 가 v2 에서 제거된 API 테스트 | `oxicode_catalog_delegates_to_registered` 로 변경 + resolve_model 테스트 4종 추가 |
| 🟡 U4 | §9.4 테스트 `Oxicode::list_providers()` / `Oxicode::search_models()` 가 v2 제거 API | `oxicode.catalog().list_providers()` / `oxicode.catalog().search()` 로 갱신 + bootstrap smoke test 2종 추가 |
| 🟡 U5 | §10 Q5 "v2에서" → 지금이 v3 | v4 로 |
| 🟡 U6 | §7.1 제거 목록에 `ModelRegistry::lookup()` 누락 (§7.3 에서 deprecated 언급) | §7.1 표에 추가 |
| 🟢 I1 | `Ok(RefreshOutcome::Failed)` 의미론 이상함 (Ok인데 Failed) — 호출자가 `.await?` 로 못 잡음 | **의도적** 확정. `RefreshOutcome` type-level doc 에 명시 (lazy on-call 원칙). §10 Q11 추가 |
| 🟢 I2 | `SdkError::Internal` fallback 이 너무 generic | catalog 변종 3개 추가 권장: `CatalogUnavailable`, `CatalogOverrideParse`, `CatalogRefresh`. §10 Q12 + PR 4 |
| 🟢 I3 | `with_builtins()` 제거 시 "`with_catalog()` 안 부르면 모든 provider 생성 실패" 가 bury 됨 | §7.7 에 ⚠️ 경고 추가 + §9.4 bootstrap smoke test 2종 추가 |

### v4 정정 — 구현 중 sync API 도입으로 async화 ripple 전면 회피 (§7.9)

v3 설계는 `ProviderResolver` trait async화를 "대공사 ripple"로 명시했다.
구현 중 catalog 의 데이터가 이미 메모리에 존재한다는 점에서 sync read API
(`*_sync`) 를 추가했고, 이로 인해 **§7.3 ~ §7.8 의 async화 전부가 불필요**해졌다:

| v3 항목 | v4 상태 |
|---|---|
| `ModelRegistry::lookup_dynamic()` 분리 | ❌ 불필요 — `resolve_model` 이 catalog sync 직접 조회 |
| `Oxicode::create_provider` / `resolve_model` async 화 | ❌ 불필요 — catalog sync read 로 충분 |
| `AgentBuilder::build()` async 화 | ❌ 불필요 |
| `ProviderResolver` trait async 화 | ❌ 불필요 |
| agent_loop / multi_provider / fallback_chain ripple | ❌ 불필요 |

PR 3 은 "대공사" 에서 **bridge layer + resolve_model 통합만** 으로 축소되었다.
이것이 이 마이그레이션의 핵심 단순화이다. 상세는 §7.9.

## 부록 B: 기존 dynamic-catalog-design.md 와의 관계

| 본 설계 (catalog-port) | dynamic-catalog (선행) |
|---|---|
| **누가** 데이터를 받는가 | 데이터가 **어디서** 오고 **어떻게** 변환되는가 |
| Port, subscription, ownership | SNAP/LIVE/materialize/ETag, 환경 변수 |
| SDK consumer 갱신 경로 | 데이터 흐름 단방향성 |
| 본 설계는 dynamic-catalog §6.3 의 "`&'static` 계약 유지" 결정을 **폐기** | 본 설계는 dynamic-catalog §4 의 모든 코드 (materialize, ETag, ...) 를 **FileModelCatalog** 로 이관 |
| 양립: 본 설계의 port 가 dynamic-catalog 의 materialize 를 호출 | |

> **읽는 순서 권장**: 먼저 dynamic-catalog (데이터 소스/변환), 그 다음 본 설계 (접근 계약).

## 부록 B: 변경 영향 매트릭스

> **v4 갱신**: sync API 도입으로 영향 강도 하향. oxicode-ai/oxicode-agent 변경 최소화.

| crate | 변경 강도 | 비고 |
|---|---|---|
| `oxicode-ai` | 🟢 변경 없음 | v3 예상(🔴) 과 달리 catalog 모듈 유지. `FileModelCatalog` 가 `include_bytes!` 로 SNAP 가져옴. legacy API 도 유지 |
| `oxicode-sdk` | 🔴 큰 변경 | 새 port + `CatalogProtocol` enum + builder + `bridge.rs` + `resolve_model` catalog 통합 + `SdkError` 변종 |
| `oxicode-agent` | 🟢 변경 없음 | v3 예상(🟢) 과 동일. `ProviderResolver` async화 회피로 영향 제로 |
| `oxicode-cli` | 🔴 큰 변경 | composition root (services/bootstrap) + TUI plumbing + setup wizard + main 명령. catalog sync API 로 이관 |
| `oxicode-tui` | 🟢 없음 | port 무관, 변경 없음 |
| `oxios` (sister repo) | 🟢 호환 | 자체 catalog impl 을 port 로 등록 가능. 기존 free fn 의존이 있었다면 이관 |
| SDK consumer 외부 앱 | 🟢 호환 | `with_catalog(my_impl)` 한 줄로 기존 호환. free fn 의존했다면 이관 필요 |