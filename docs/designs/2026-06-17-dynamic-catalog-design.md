# 설계: oxi 완전 동적 카탈로그 (models.dev 진실 소스)

> 상태: 설계 v2 (구현 전). v1 리뷰에서 발견된 5개 결함 정정.
> 작성: 2026-06-17
> 선행: `docs/MODELS_DEV_SYNC.md` (1차 enrich 구현), 본 설계는 그 후속·통합

## 0. 핵심 (TL;DR)

opencode의 진짜 혁신은 effect/immer/plugin 아키텍처가 아니라
**"models.dev JSON이 카탈로그의 유일한 진실 소스"**라는 결정 하나입니다.
`plugin/provider/anthropic.ts`가 단 25줄(헤더 한 줄 추가)인 것이 증거입니다 —
데이터는 전부 models.dev에서 옵니다.

oxi는 이 통찰만 번역합니다. opencode의 TypeScript적 표현(effect 스트림, immer draft,
런타임 `import("@ai-sdk/anthropic")`)은 가져오지 않습니다. oxi의 정적 Rust +
`Api` enum + 직접 HTTP provider 구현체를 그대로 두고, **데이터 소스만 단일화**합니다.

**한 문장 요약**: 수작업 TOML 71파일(14K줄)을 지우고, models.dev JSON을
**opencode와 동일하게 빌드 시점 snapshot으로 임베드** + **런타임 캐시로 갱신**하여,
npm 필드 → `Api` enum 7줄 매핑으로 145개 provider·5,277개 모델을 자동 구성한다.

## 1. 설계 원칙

| # | 원칙 | 구현 |
|---|---|---|
| 1 | **단일 진실 소스** | models.dev `api.json` 하나. 두 번째 카피 없음 |
| 2 | **단일 진입점** | 기존 `model_db::all_provider_models()` OnceLock — downstream 무변경 |
| 3 | **단일 매핑 함수** | `protocol_for(npm) → (Api, AuthMethod)`, 7줄 match. provider id 매칭 아님 |
| 4 | **자동 추론 최대화** | api·auth·base_url·env·modalities·cost·limit·reasoning 전부 models.dev에서 |
| 5 | **점진적 향상** | SNAP(임베드) + LIVE(캐시) + LOCAL(ollama). 어느 게 없어도 작동 |
| 6 | **제거 > 추가** | TOML 71파일 + providers.toml + category 제거. 추가보다 제거가 많음 |

> ⚠️ v1에서 "우아하다"고 자찬했던 것을 삭제. 원칙은 원칙이지 우아함의 증명이 아님.

## 2. 결정적 데이터 증거 (본 설계를 지탱하는 사실)

본 설계는 2026-06-17 `models.dev/api.json`(2.3MB) 직접 분석에 기반합니다.

### 2.1 npm 필드는 신뢰할 수 있는 프로토콜 분류자다

```
@ai-sdk/openai-compatible  → 112개 provider, 전부 OpenAI Chat Completions
@ai-sdk/anthropic          →   7개 (anthropic, minimax, minimax-cn, kimi-for-coding, freemodel, …)
@ai-sdk/google | google-vertex | mistral | azure | amazon-bedrock → 각 전용 프로토콜
@ai-sdk/openai | xai | groq | togetherai | vercel | … → OpenAI 호환
```

같은 npm을 공유하는 provider는 같은 프로토콜을 씁니다 (검증: 빈 npm provider = 0개,
모두 npm 보유). oxi는 이 문자열 값을 `match`하기만 하면 됩니다 — Rust가 npm 패키지를
import하는 게 아닙니다. opencode가 npm을 "import 대상"으로도 쓰는 건 그들의 부가 기능이고,
**프로토콜 분류자로서의 npm 값은 언어 무관**합니다.

### 2.2 프로토콜은 provider가 아니라 **model**의 속성이다 (핵심)

**154개 모델**이 부모 provider와 **다른** `provider.npm`을 가집니다. 예:

```
opencode-go/minimax-m2.5   : provider=@ai-sdk/openai-compatible  model=@ai-sdk/anthropic
vivgrid/gpt-5.4            : provider=@ai-sdk/openai              model=@ai-sdk/openai-compatible
```

opencode-go는 OpenAI 호환 provider 안에서 특정 모델만 Anthropic 프로토콜로 서빙합니다.
→ **프로토콜 결정은 provider 수준이 아니라 model 수준에서 일어난다.** 이게 opencode
`plugin/models-dev.ts`가 provider.api와 model.api를 **둘 다** 설정하는 이유입니다.

**v3 정정 (결함 A+D, 데이터 검증)**: 프로토콜·auth·**base_url** 모두 model 수준 속성이다.
데이터 확인 (2026-06-17): 55개 모델이 부모 provider와 **다른 `model.provider.api`(base_url)**를 가진다.
예: `zenmux/anthropic/claude-sonnet-4.5` — provider=`https://zenmux.ai/api/v1`,
model=`https://zenmux.ai/api/anthropic/v1` (같은 provider 안에서 모델별로 다른 경로).
즉 model은 프로토콜(npm)뿐 아니라 endpoint(base_url)까지 별도로 가질 수 있다.

`opencode-go/minimax-m2.5` 예: `model.npm=@ai-sdk/anthropic` + `model.api=null` →
부모 base_url 상속 + Anthropic 프로토콜 + **x-api-key**. provider 수준 bearer로 호출 시 401.

따라서 materialize는 model.provider에 **세 가지**를 모두 반영한다:

- `(model_api, model_auth)` ← `model.provider.npm` 있으면 override, 없으면 부모 것
- `model_base_url` ← `model.provider.api` 있으면 override, 없으면 부모 것 (None=상속)
- `BuiltinModelEntry`에 **`auth_method: AuthMethod`** 및 **`base_url: Option<String>`** 필드 추가.
- oxi provider 구현체는 이미 base_url override 지원(검증: `AnthropicProvider::with_base_url`,
  `OpenAiProvider::with_base_url`). `model_from_entry`가 model entry의 base_url/auth를
  provider 것보다 우선 사용하도록 수정 (§7.3).

### 2.3 auth_method도 protocol_for가 함께 반환한다 (그러나 한계 명시)

현재 oxi `providers.toml` 특수 auth (bearer 이외 11개)를 npm으로 역추적:

```
@ai-sdk/anthropic      → x-api-key  (anthropic, minimax, minimax-cn, synthetic)
@ai-sdk/azure          → api-key    (azure, microsoft-foundry)
@ai-sdk/google | google-vertex | amazon-bedrock → none (OAuth/SigV4)
나머지                  → bearer
```

→ `protocol_for()`가 api와 auth를 함께 반환. auth_method 전용 오버레이 불필요.

**그러나** §2.4의 extra_headers는 protocol_for로 처리 불가 — oxi 제품 식별 헤더이므로.

### 2.4 extra_headers는 oxi 제품 메타 — 보존 필요 (v1 정정)

v1 §4.4가 `extra_headers: Vec::new()`로 날렸지만, 이는 회귀를 만듭니다. 현재 9개 provider가
extra_headers에 의존 (데이터 검증 완료):

```
openrouter    [["HTTP-Referer", "https://oxi.dev/"], ["X-Title", "oxi"]]
cloudflare-ai-gateway, vercel-ai-gateway, nvidia, llmgateway, zenmux, kilo  (동일)
cerebras      [["X-Cerebras-3rd-Party-Integration", "opencode"]]
anthropic-vertex [["anthropic-version", "vertex-2023-10-16"]]
```

- OpenRouter는 HTTP-Referer가 없으면 랭킹/집계에서 불이익, 일부 provider는 거부.
- models.dev는 oxi **제품 식별 헤더**를 모름(그들은 opencode referer 사용).

→ **제품 메타 오버레이를 최소 형태로 유지**한다. 단, category/aliases/description은
제거(사용자 요청)하고, **extra_headers만** 남긴다. 따라서 오버레이 파일은 매우 작다
(~9 provider, 각 헤더 1-2줄).

## 3. 아키텍처 — 단방향 데이터 흐름

```
                    models.dev/api.json  (JSON, 2.3MB, 진실 소스)
                            │
              ┌─────────────┴─────────────┐
              ▼                           ▼
     [빌드 시점] SNAP              [런타임] LIVE
     build.rs가 snapshot 주입         init_models_dev() (1차 재사용 + 확장)
     → 임베드 (opencode define 패턴)   → ~/.oxi/cache/models-dev.json
     → 첫실행/오프라인 안전망          → 진실 소스 우선 (mtime 창 + ETag 조건부 GET, §4.2)
              │                           │
              └─────────────┬─────────────┘
                            ▼ (LIVE가 신선하면 LIVE, 아니면 SNAP)
                    Protocol Resolver
                    protocol_for(npm) → (Api, AuthMethod)   ← 7줄 match
                            │
                            ▼
                    Materialize + Merge
                    models.dev → Vec<BuiltinProviderEntry + BuiltinModelEntry>
                    + product-meta.toml에서 extra_headers 병합 (§2.4)
                    + ~/.oxi/catalog/overrides.toml 병합 (Layer 2, 최우선)
                            │
                            ▼
                    OnceLock  ★ 기존 진입점
                    model_db::all_provider_models()
                            │
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
        get_model_entry  fallback_chain  setup_wizard
        multi_provider   TUI overlay     RPC/print
              (downstream 무변경 — OnceLock 결과만 소비)

                            +
                    [런타임] LOCAL  (별도 경로, §4.6)
                    runtime.rs /v1/models → ollama/lmstudio/vllm
                    → OnceLock와 병렬 병합 (§4.6에서 상세)
```

데이터는 한 방향으로만 흐릅니다: models.dev → snapshot/cache → materialize → OnceLock → 소비자.
사이클 없음, 역참조 없음.

## 4. 컴포넌트

### 4.1 SNAP — 빌드 시점 snapshot 주입 (v1 정정: opencode 패턴 정확 차용)

v1은 build.rs에서 fetch하는 것을 제안했으나, 이는 모든 `cargo build`를 잠재적
네트워크 의존으로 만듭니다. **opencode의 정확한 패턴을 차용**합니다:

```typescript
// opencode generate.ts (참고)
export const modelsData = process.env.MODELS_DEV_API_JSON
  ? await Bun.file(process.env.MODELS_DEV_API_JSON).text()  // CI: 파일 주입 (결정론적)
  : await fetch(`${modelsUrl}/api.json`).text()             // 로컬: 라이브 fetch
```

```rust
// oxi-ai/build.rs (opencode 패턴 번역)
fn main() {
    let snapshot_bytes: Vec<u8> = match std::env::var("OXI_CATALOG_SNAPSHOT") {
        Ok(path) => std::fs::read(&path)              // CI: 파일 주입 (결정론적)
            .expect("OXI_CATALOG_SNAPSHOT 파일 읽기 실패"),
        Err(_) => {
            // 로컬 개발: 10s 타임아웃 fetch. 실패면 커밋된 fallback 사용.
            fetch_or_fallback().unwrap_or_else(|| FALLBACK_SNAPSHOT.to_vec())
        }
    };
    // gzip 압축 → OUT_DIR/models-dev-snapshot.json.gz
    // include_bytes!("../../OUT_DIR/...") 또는 env!("OXI_SNAPSHOT_PATH")
    // → 런타임에 decompress_snapshot() 으로 노출
}
```

- **CI/릴리스 빌드**: `OXI_CATALOG_SNAPSHOT=path`로 미리 받아둔 파일 주입 → **결정론적, 네트워크 없음, 재현성 보장**.
- **로컬 개발**: 빌드 시 fetch (또는 `data/catalog/_snapshot.json.gz` fallback).
- **바이너리 크기**: gzip 후 ~300-500KB. 허용 가능.
- `data/catalog/_snapshot.json.gz`를 리포에 **커밋** (fallback + CI 결정론성의 기준점).
- 데이터 출처 명시: `data/catalog/README.md`에 "© models.dev, MIT" + NOTICE 파일 (부록 B).

> opencode `OPENCODE_MODELS_DEV` define과 동일한 역할. **빌드 자체는 네트워크 없이도 가능** (OXI_CATALOG_SNAPSHOT 또는 fallback).

### 4.2 LIVE — 런타임 캐시 + 조건부 GET (1차 `models_dev.rs` 재사용 + 확장)

**동기화 주기는 '시간'이 아니라 '데이터 변화'로 정의한다.** v1/v2 초안이
"60분 갱신", "24시간 TTL" 같은 임의 주기를 제안했으나, 데이터 분석(2026-06-17)에
의하면 models.dev 신규 모델은 일평균 ~10개지만 **주요 foundation 모델 출시는 주 1회
이하**이고, 일평균의 대부분은 aggregator(nano-gpt/openrouter)의 사소한 변종이다.
따라서 시간 기반 주기는 과잉 또는 부족이 된다.

대신 **mtime 로컬 판정 + HTTP 조건부 GET(ETag) 혼합**을 쓴다:

```rust
/// 동기화 결정 (models_dev.rs::source)
/// 2단계: 로컬 mtime으로 HTTP 비용을 0으로 만들고, 실제 변화는 ETag로 판정.
pub fn source() -> Source {
    // 1단계: mtime이 1시간 이내면 HTTP 자체를 안 함 (로컬만, 0비용)
    if let Some(c) = read_cache_if_fresh() {  // age < MTIME_WINDOW (1h)
        return Source::Live(c);
    }
    // 2단계: 1시간 넘었으면 조건부 GET. 캐시의 ETag를 보내서 실제 변화만 갱신.
    match conditional_fetch() {
        Some(FetchResult::NotModified) => {
            touch_cache_mtime();   // 304: 데이터 동일 → mtime만 갱신해 1h 창 리셋
            Source::Live(read_cache_any())  // 기존 캐시 사용
        }
        Some(FetchResult::Updated(c)) => Source::Live(c),  // 200: 새 데이터 → 캐시 갱신
        None => Source::Snap(decompress_snapshot(MODELS_DEV_SNAPSHOT)?),  // 폴백
    }
}
```

**측정 기준의 명확화** (이 섹션의 핵심):

- **mtime** = 로컬 캐시 파일(`~/.oxi/cache/models-dev.json`)의 수정 시각. oxi가
  마지막으로 *성공적으로 검증한* 시점(304든 200이든). `SystemTime::now() - mtime`.
- **MTIME_WINDOW** (기본 1시간) = 로컬만으로 fresh로 간주하는 창. 이 창 내에는
  HTTP를 아예 안 보내 0비용. oxi를 아무리 자주 켜도 1시간에 1회 HTTP가 상한.
- **ETag** = models.dev 응답의 강한 ETag(Cloudflare CDN, 검증됨).
  `If-None-Match`로 보냄. `304 Not Modified`면 데이터 동일 → 갱신 비용 0(수 ms 왕복).
- **ETag 저장 (v3 정정 결함 A)**: 사이드카 파일 `~/.oxi/cache/models-dev.json.etag`
  (1줄 짜리 ETag 문자열). 캐시 JSON 본문과 분리해 models.dev 원본 포맷 유지.
  - 쓰기: 200 수신 시 JSON(atomic write) + ETag 사이드카 동시 작성.
  - 읽기: 조건부 GET 직전에 사이드카 읽어 `If-None-Match` 헤더 구성.
  - 304 수신 시: 캐시 JSON은 그대로, 사이드카도 그대로 (mtime만 갱신해 1h 창 리셋).
  - 사이드카 손상/없음: 일반 GET(If-None-Match 없음)으로 폴백, 이후 사이드카 재생성.
- **TTL 환경변수는 폐지** (§8). mtime + ETag가 TTL의 역할을 대체한다.

**이 방식이 해결하는 것**:

| 문제 | 해결 |
|---|---|
| "너무 자주 동기화" (사용자 지적) | 데이터 안 변하면 304로 갱신 안 함. foundation 모델 주 1회면 주 1회 갱신 |
| "시간은 어떻게 재나" (사용자 질문) | mtime(로컬) + ETag(HTTP) — 임의 시간 주기 아님 |
| opencode 60분의 비합리성 | opencode는 60분 고정 대기. oxi는 '변화가 있을 때만' 갱신. 데이터 기반 |
| 자주 켜는 사용자 비효율 | MTIME_WINDOW 1h로 HTTP 자체를 억제. 1h 내엔 완전 로컬 |

**확장점** (1차 대비):
- 조건부 GET(ETag) — 위 설명의 핵심. opencode보다 oxi 우위.
- **스키마 보강** (1차가 놓친 필드): `reasoning_options`, `structured_output`,
  `cost.tiers`, `cost.context_over_200k`, `interleaved`, `knowledge`, `open_weights`.
  serde 스키마를 opencode `models-dev.ts`와 1:1로.
- 백그라운드 갱신은 도입 안 함 (§11.1: OnceLock 유지 + 1h MTIME_WINDOW로 충분).
- `OXI_MODELS_DEV_FORCE_REFRESH=1` 또는 `oxi models refresh`가 mtime 창을 무시하고
  강제 조건부 GET 수행 (사용자가 즉시 최신을 원할 때).

### 4.3 PROTOCOL RESOLVER — npm → (Api, AuthMethod), 7줄 (본 설계 핵심)

oxi가 models.dev 데이터를 소비하기 위해 가지는 **유일한 자체 지식**.

```rust
/// models.dev의 npm 필드 → oxi의 프로토콜 + 인증 방식.
/// 새 프로토콜이 models.dev에 추가되지 않는 한, 이 7줄은 영구 불변.
fn protocol_for(npm: &str) -> (Api, AuthMethod) {
    match npm {
        "@ai-sdk/anthropic"      => (Api::AnthropicMessages,    AuthMethod::XApiKey),
        "@ai-sdk/google"         => (Api::GoogleGenerativeAi,   AuthMethod::None),
        "@ai-sdk/google-vertex"
        | "@ai-sdk/google-vertex/anthropic"
                                  => (Api::GoogleVertex,         AuthMethod::None),
        "@ai-sdk/mistral"        => (Api::MistralConversations, AuthMethod::Bearer),
        "@ai-sdk/azure"          => (Api::AzureOpenAiResponses, AuthMethod::ApiKey),
        "@ai-sdk/amazon-bedrock" => (Api::BedrockConverseStream,AuthMethod::None),
        // @ai-sdk/openai, @ai-sdk/openai-compatible, groq, xai, togetherai, vercel,
        // 그리고 알 수 없는 모든 npm → OpenAI 호환
        _                         => (Api::OpenAiCompletions,    AuthMethod::Bearer),
    }
}
```

- **id 매칭 아닌 npm 매칭인 이유** (§2.1): 새 provider가 models.dev에 추가돼도 oxi 수정 없이 정확 분류. id 매칭은 매핑 테이블 수작업 부활.
- **모델 수준 오버라이드** (§2.2, v1 정정): `materialize_model`은 model.provider.npm(있으면)으로
  `(model_api, model_auth)` **둘 다** 부모 provider 값을 override. `BuiltinModelEntry`에
  `auth_method` 필드 추가 필요.

### 4.4 REGISTRY — materialize + merge (신규, `catalog/materialize.rs`)

models.dev → oxi 엔트리 변환. **신모델·신프로바이더 자동 등장**이 달성되는 곳.

```rust
pub fn materialize(
    catalog: &MdCatalog,
    headers_overlay: &ProductMeta,    // §2.4 extra_headers만
    user_overrides: &OverrideFile,    // Layer 2, 최우선
) -> (Vec<BuiltinProviderEntry>, Vec<BuiltinModelEntry>) {
    let mut providers = Vec::new();
    let mut models = Vec::new();

    for (pid, mdprov) in &catalog.0 {
        let (api, auth) = protocol_for(mdprov.npm.as_deref().unwrap_or(""));
        let pm = headers_overlay.get(pid);   // extra_headers (있으면)

        providers.push(BuiltinProviderEntry {
            id: pid.clone(),
            display_name: mdprov.name.clone(),        // ⬅ 보존 (v1 정정: 사용자 동의 범위)
            description: String::new(),               // models.dev엔 원라인 설명 없음
            aliases: vec![],                          // 제거: models.dev id를 그대로 사용 (v3 결정: 기존 호환성 버림)
            api: api.to_str().to_string(),
            env_key: mdprov.env.first().cloned().unwrap_or_default(),
            extra_env_keys: mdprov.env[1..].to_vec(),
            base_url: mdprov.api.clone().unwrap_or_default(),
            auth_method: auth,
            extra_headers: pm.map(|m| m.extra_headers.clone()).unwrap_or_default(),
            // category 필드: 제거됨 (§6.1)
            default_enabled: true,
        });

        for (mid, mdmodel) in &mdprov.models {
            // 모델 수준 override (v3): protocol+auth+base_url 세 가지
            let model_provider = mdmodel.provider.as_ref();
            let model_npm = model_provider.and_then(|p| p.npm.as_deref())
                .unwrap_or_else(|| mdprov.npm.as_deref().unwrap_or(""));
            let (model_api, model_auth) = protocol_for(model_npm);
            // base_url: model.provider.api 있으면 override, 없으면 None(부모 상속)
            let model_base_url = model_provider.and_then(|p| p.api.clone());

            models.push(BuiltinModelEntry {
                id: mid.clone(),
                name: mdmodel.name.clone(),
                api: model_api.to_str().to_string(),
                provider: pid.clone(),
                auth_method: model_auth,             // v1 정정
                base_url: model_base_url,            // v3 정정 (결함 D): None=부모 상속
                reasoning: mdmodel.reasoning,
                input: normalize_modalities(&mdmodel.modalities),
                cost_input:  mdmodel.cost.as_ref().map(|c| c.input).unwrap_or(0.0),
                cost_output: mdmodel.cost.as_ref().map(|c| c.output).unwrap_or(0.0),
                cost_cache_read:  mdmodel.cost.as_ref().and_then(|c| c.cache_read).unwrap_or(0.0),
                cost_cache_write: mdmodel.cost.as_ref().and_then(|c| c.cache_write).unwrap_or(0.0),
                context_window: mdmodel.limit.context as u32,
                max_tokens: mdmodel.limit.output as u32,
                ..Default::default()
            });
        }
    }

    // Layer 2 사용자 오버라이드 적용 (최우선) — v1 정정: 통합 지점 명시
    apply_provider_overrides(&mut providers, &user_overrides.provider);
    apply_model_overrides(&mut models, &user_overrides.model);

    (providers, models)
}
```

- `Box::leak` → `&'static` (기존 `all_provider_models` 패턴).
- **센티널 정책 변화** (§7.1): models.dev가 검증 소스 → "미검증" 상태 소멸.
- **Layer 2 병합 위치 명시** (v1 정정 결함 6): materialize 내부, Box::leak 직전.

### 4.5 LOCAL — 런타임 디스커버리 (기존 `runtime.rs`, 병합 메커니즘 명시 — v1 정정)

v1은 "별도 append"로만 적었으나 병합 시점이 모호. 명시:

- LOCAL(ollama 등)은 `/v1/models` 런타임 페치라 OnceLock init 시점엔 결과가 없을 수 있음.
- 해결: **LOCAL은 OnceLock과 별개 레지스트리**로 유지. `model_db` 소비자는
  `all_provider_models()`(SNAP/LIVE) + `discover_all_local()`(LOCAL)을 **순차 조회**.
  기존 `runtime.rs`가 이미 이 패턴(`discover_all_local` export)이므로, 본 설계는
  LOCAL을 OnceLock에 넣지 않고 그대로 둡니다.
- 추후 병합이 필요하면 별도 Phase에서 검토. 본 설계 범위 아님.

### 4.6 PRODUCT-META 오버레이 — extra_headers만 (v1 정정 결함 3)

```toml
# data/catalog/product-meta.toml — extra_headers만 담은 작은 파일 (~9 provider)
[[provider]]
id = "openrouter"
extra_headers = [["HTTP-Referer", "https://oxi.dev/"], ["X-Title", "oxi"]]

[[provider]]
id = "anthropic-vertex"
extra_headers = [["anthropic-version", "vertex-2023-10-16"]]

[[provider]]
id = "cerebras"
extra_headers = [["X-Cerebras-3rd-Party-Integration", "opencode"]]
# ... nvidia, vercel-ai-gateway, cloudflare-ai-gateway, llmgateway, zenmux, kilo (HTTP-Referer 공통)
```

- category/aliases/description/auth_method는 **여기에 두지 않음** (모두 models.dev 또는 protocol_for로 해결).
- 오직 oxi 제품 식별 헤더만. 71개 → ~9개로 축소.

## 5. 데이터 스키마 (Rust serde — opencode `models-dev.ts`와 1:1)

### 5.1 MdCatalog / MdProvider / MdModel

```rust
use std::collections::BTreeMap;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct MdCatalog(pub BTreeMap<String, MdProvider>);

#[derive(Debug, Deserialize)]
pub struct MdProvider {
    pub id: String,
    pub name: String,
    pub env: Vec<String>,
    pub npm: Option<String>,
    pub api: Option<String>,     // base_url
    pub doc: Option<String>,
    pub models: BTreeMap<String, MdModel>,
}

#[derive(Debug, Deserialize)]
pub struct MdModel {
    pub id: String,
    pub name: String,
    pub family: Option<String>,
    pub reasoning: bool,
    pub tool_call: bool,
    pub attachment: bool,
    pub temperature: Option<bool>,
    pub structured_output: Option<bool>,
    pub knowledge: Option<String>,
    pub release_date: Option<String>,
    pub last_updated: Option<String>,
    pub open_weights: Option<bool>,
    pub interleaved: Option<serde_json::Value>,   // bool | struct
    pub reasoning_options: Option<Vec<ReasoningOption>>,
    pub limit: MdLimit,
    pub cost: Option<MdCost>,
    pub modalities: Option<MdModalities>,
    pub status: Option<Status>,
    pub provider: Option<MdModelProvider>,   // ★ 모델 수준 npm/api 오버라이드 (§2.2)
}

#[derive(Debug, Deserialize)]
pub struct MdModelProvider {
    pub npm: Option<String>,
    pub api: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MdCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub tiers: Option<Vec<MdCostTier>>,
    pub context_over_200k: Option<MdCostContext>,
    pub reasoning: Option<f64>,
}
// MdCostTier, MdCostContext, MdLimit, MdModalities, ReasoningOption: opencode와 동일
```

### 5.2 CostTier — schema와 materialize를 같은 Phase로 (v1 정정 결함 10)

v1은 schema에 넣고 materialize에선 무시 → dead field. 정정: **CostTier는 Phase 5로
완전 미루고, schema에서도 Phase 5까지 빼둔다**. Phase 1-4는 단일 cost만. 일관성 유지.

## 6. 삭제/추가 매트릭스

### 6.1 제거

| 대상 | 규모 | 이유 |
|---|---|---|
| `data/catalog/models/*.toml` | 30+ 파일, ~14K줄 | models.dev가 대체 |
| `data/catalog/openclaw/*.toml` | 일부 | 동일 |
| `data/catalog/providers.toml` | 71개, 2K줄 | models.dev + protocol_for가 대체 (extra_headers는 product-meta로 축소 이관) |
| `BuiltinProviderEntry.category` | 필드 1 | 사용자 요청. TUI는 알파벳 정렬로 전환 |
| `catalog/provider_map()` | ~42줄 | id 매칭 폐지 |
| `catalog/reasoning_preserve()` | ~30줄 | models.dev가 검증 소스 → 보존 예외 거의 불필요 (§7.2) |
| openclaw 센티널 `-1.0` 정책 | 변환 로직 | models.dev 0 = 진짜 값 |

### 6.2 추가

| 대상 | 규모 | 이유 |
|---|---|---|
| `catalog/materialize.rs` | ~180줄 | models.dev → oxi 엔트리 변환 + merge (§4.4) |
| `protocol_for()` | 7줄 | 본 설계 핵심 (§4.3) |
| `BuiltinModelEntry.auth_method` | 필드 1 | 모델 수준 auth override (§2.2) |
| `BuiltinModelEntry.base_url` | 필드 1 | 모델 수준 base_url override, 55개 모델 (§2.2 v3 정정 결함 D) |
| `build.rs` snapshot 주입 | ~50줄 | opencode define 패턴 번역 (§4.1) |
| `data/catalog/product-meta.toml` | ~30줄 | extra_headers만 (§4.6) |
| `data/catalog/_snapshot.json.gz` | 커밋 | fallback + CI 결정론성 기준점 |
| CI 스냅샷 갱신 워크플로 | ~40줄 YAML | 주간 `_snapshot.json.gz` 갱신 + `OXI_CATALOG_SNAPSHOT` 주입 |

### 6.3 무변경 (단일 진입점 덕분)

`model_db::get_model_entry`, `get_all_models`, `multi_provider::model_from_entry`,
`fallback_chain`, `setup_wizard`, TUI overlay, RPC/print, `oxi-sdk` re-export.
단, `multi_provider`가 model entry의 `auth_method`를 **사용**하도록 수정 필요
(현재는 provider에서만 읽음) — §7.3.

## 7. 인바리언트 보존

### 7.1 센티널 가격 정책 변화

현재 openclaw 소스의 `0.0` → 런타임 `-1.0`(미검증) 변환 + `pricing_unverified()`.
본 설계에서 models.dev가 검증 소스이므로 **"미검증" 상태 소멸**.

- models.dev `cost.input > 0` → 검증값 (양수)
- models.dev `cost` 없음 또는 `0` → 진짜 무료/미공개
- 기존 `sentinel_pricing_counted` 테스트(`s == 34`)는 **단언 변경 필요** (회귀 아님).

> 리스크: 사용자가 "이 가격 확실한가?"를 구분 불가. 단, §11의 "커뮤니티 데이터 오류"
> 리스크를 수용하는 대신 README 면책으로 보완. 사용자는 `overrides.toml`로 정정 가능.

### 7.2 reasoning 보존 예외 재검토

현재 `reasoning_preserve`는 TEE/throughput/양자화 변형 보호. 본 설계에서 reasoning은
models.dev 검증값 → **대부분 불필요**. 단, §7.1과 마찬가지로 models.dev 오류 가능성을
수용하므로, 정말 oxi가 의도한 차이만 0~5줄로 축소 보존.

### 7.3 multi_provider auth 반영 (v1 정정)

`model_from_entry`가 model entry의 `auth_method`(신규 필드)를 provider 것보다 우선
사용하도록 수정. 이래야 §2.2의 `opencode-go/minimax-m2.5` (Anthropic 프로토콜)가
올바른 x-api-key로 호출됨.

### 7.4 빌드 재현성 (v1 정정)

런타임 보강은 비결정론적이나, **SNAP 임베드는 `OXI_CATALOG_SNAPSHOT` 파일 주입으로
완전 결정론적**. CI는 (1) snapshot fetch → (2) `OXI_CATALOG_SNAPSHOT=path`로 빌드.
로컬 개발은 fetch 또는 커밋된 fallback. **빌드 자체는 네트워크 없이 가능**.

## 8. 환경 변수 / 게이트

| 변수 | 기본 | 설명 |
|---|---|---|
| `OXI_MODELS_DEV` | `auto` | `auto`/`on`/`off` (1차 유지) |
| `OXI_MODELS_DEV_URL` | `https://models.dev` | 엔터프라이즈 미러 |
| `OXI_MODELS_DEV_DISABLE_FETCH` | (unset) | `1`이면 라이브 페치 금지 (에어갭) |
| `OXI_MODELS_DEV_CACHE_PATH` | `~/.oxi/cache/models-dev.json` | 캐시 + ETag 저장 위치 |
| **`OXI_MODELS_DEV_MTIME_WINDOW`** | `3600` (1시간) | **신규/변경** — mtime이 이 창 이내면 HTTP를 아예 안 보냄(0비용). 1시간 내 여러 번 실행해도 HTTP 1회가 상한 |
| **`OXI_MODELS_DEV_FORCE_REFRESH`** | (unset) | **신규** — `1`이면 mtime 창 무시하고 강제 조건부 GET (`oxi models refresh`가 사용) |
| **`OXI_CATALOG_SNAPSHOT`** | (unset) | **신규** — 빌드 시 snapshot 파일 주입 (결정론적, opencode `MODELS_DEV_API_JSON` 패리티) |

> **v2 정정 (TTL 폐지)**: 1차/초안의 `OXI_MODELS_DEV_TTL=300`(5분)은 제거한다.
> 임의 시간 주기 대신 mtime 창(로컬) + ETag(HTTP)로 '데이터 변화'를 측정한다 (§4.2).
> foundation 모델 출시가 주 단위인 현실(데이터 검증)에 가장 적합하다.
>
> v1의 `OXI_CATALOG_OFFLINE`도 제거 — `OXI_CATALOG_SNAPSHOT`이 항상 있으면
> 빌드는 네트워크 없으므로 offline 게이트 불필요. 로컬 fetch 원치 않으면
> `OXI_CATALOG_SNAPSHOT` 미리 설정.

## 9. 사용자 인터페이스 — 수동 갱신

```
oxi models refresh          # mtime 창 무시, 조건부 GET (ETag) 강제 수행.
                            # 304=이미 최신 / 200=갱신. 효과는 다음 실행에 반영 (§11.1)
oxi models list             # materialize된 카탈로그 (LIVE 또는 SNAP)
oxi models show <id>        # 단일 모델 상세 (출처: live/snap)
```

> 동기화 주기 정의 (§4.2): 임의 시간 주기가 아니다. 매 실행 시
> (1) mtime 1h 이내면 로컬만 사용, (2) 그 이상이면 조건부 GET으로 실제 변화만 갱신.
> foundation 모델 출시가 주 단위이므로, 실제 갱신도 주 단위로 자연 수렴한다.

## 10. 구현 단계 (Phase 분할) — v1 정정: SNAP 선행

v1은 Phase 2(TOML 삭제)가 Phase 3(SNAP)보다 먼저 → "TOML 없고 SNAP 없는" 깨진 구간 발생.
**정정: SNAP을 Phase 2와 같거나 먼저**.

### Phase 1: 스키마 + protocol_for + product-meta (위험 最저, 기반)
- `MdCatalog` 스키마를 opencode와 1:1로 확장 (§5.1). CostTier 제외 (§5.2).
- `protocol_for(npm)` 구현 + 단위테스트 (네트워크 X, 결정론적).
- `BuiltinModelEntry.auth_method` 필드 추가 (§2.2 정정).
- `data/catalog/product-meta.toml` 생성 (extra_headers만, §4.6).
- materialize() 골격 — **기존 provider_map 경로와 병행, 게이트로 전환**.
- 기존 catalog 테스트 전부 통과 유지.

### Phase 2: SNAP + REGISTRY 전환 (위험 中, **동시 적용** — v1 정정 결함 1)
- **SNAP 먼저**: `build.rs` snapshot 주입 + `_snapshot.json.gz` 커밋 + `OXI_CATALOG_SNAPSHOT` 게이트.
- **REGISTRY 전환**: `all_provider_models()` OnceLock init을 materialize() 기반으로.
  모델 수준 auth override 처리 (§2.2).
- **TOML 삭제**: `data/catalog/models/*.toml` + `providers.toml` 제거.
- 센티널 정책 변경 + 테스트 단언 갱신 (§7.1).
- multi_provider auth 반영 (§7.3).
- 이 Phase 종료 = **완전 동적 달성** (SNAP로 첫실행 안전 + LIVE로 갱신).

### Phase 3: LIVE 고도화 (위험 低)
- **조건부 GET (ETag)** — mtime 창 + `If-None-Match`. TTL 폐지 (§4.2).
- `oxi models refresh` 명령 (mtime 창 무시 강제 GET, 다음 실행 반영).
- `OXI_MODELS_DEV_MTIME_WINDOW` / `OXI_MODELS_DEV_FORCE_REFRESH` 게이트.
- 스키마 보강 검증 (Phase 1 스키마가 실제 데이터와 일치).

### Phase 4: 부가 (위험 低, 선택)
- CostTier 지원 (§5.2) — schema + materialize 동시.
- reasoning_options → oxi reasoning 제어 연동.
- structured_output → tool 스키마 강제.
- LOCAL과 OnceLock 병합 (별도 설계).

> v1의 Phase 4(60분 백그라운드 갱신)는 **삭제** — 동기화는 §4.2의 조건부 GET이 담당하므로 별도 백그라운드 갱신 불필요.

## 11. 리스크 & 완화

| 리스크 | 확률 | 완화 |
|---|---|---|
| models.dev 가용성 장애 | 중 | SNAP 임베드 + stale 캐시. LIVE→SNAP→(빈도 LOCAL 작동) |
| npm 매핑 누락 프로토콜 | 낮 | 새 npm은 OpenAI 호환 폴백. 7줄 match만 갱신 |
| 모델 수준 auth override 미반영 | 확실(구현 누락 시) | §7.3로 보장. 154개 케이스 fixture화 |
| extra_headers 회귀 | 확실(제거 시) | product-meta.toml로 9개 보존 (§4.6) |
| 빌드 시간 증가 | 낮 | gzip + `OXI_CATALOG_SNAPSHOT` 주입 시 fetch 스킵 |
| 센티널 단언 실패 | 확실 | Phase 2 의도적 단언 갱신 (회귀 아님) |
| models.dev 데이터 오류 (커뮤니티) | 중 | README 면책. `overrides.toml` 사용자 최종 상단 (Layer 2 유지) |
| 신프로바이더 특수 인증 미작동 | 중 | OAuth/SigV4 provider는 protocol_for 정확 매핑. 그 외 bearer 폴밸 정상 |

### 11.1 OnceLock 유지 + 조건부 GET (v1 정정 결함 4 + 동기화 주기 재정의)

v1은 OnceLock(A)/RwLock(B)를 열어두고 "60분 갱신"/"refresh 즉시 효과"와 동시에 약속 → 모순.
v2 초안은 (A)를 택하면서 임의 주기(24시간)를 붙였으나, 사용자 리뷰로 "주 단위 출시인데
왜 시간 주기인가"가 제기되어 **mtime 창 + ETag 조건부 GET**으로 재정의했다 (§4.2).

**OnceLock 유지 — 결정 회피 없이 명시적 택일:**

- ❌ 런타임 메모리 갱신 (RwLock) — **포기**. 캐시 파일 갱신은 다음 실행에 반영.
- ❌ 임의 시간 주기 (60분/24시간) — **포기**. 데이터 변화(ETag)로 대체.
- ✅ 기존 `&'static` 계약 유지 → downstream 시그니처 무변경 (§6.3 진실화).
- ✅ 조건부 GET으로 실제 변화만 갱신 — foundation 모델 주 1회면 주 1회 갱신.

**`oxi models refresh` 동작** (§9 정정): mtime 창을 무시하고 즉시 조건부 GET을 수행한다.
304면 "이미 최신", 200이면 갱신. **다음 실행에 OnceLock에 반영** (런타임 메모리는 고정).
이것이 가장 합리적인 타협 — 사용자가 명시적으로 최신을 원할 때 ETag 1회 왕복으로
확인하고, 결과는 다음 실행에 깔끔하게 적용된다.

**근거**: 사용자는 보통 세션마다 oxi를 새로 시작. 캐시 파일 갱신 → 다음 실행 반영이
UX에 큰 지장 없다. 런타임 메모리 갱신이 꼭 필요해지면 별도 Phase에서 RwLock 전환 검토.

## 12. 본 설계가 MODELS_DEV_SYNC.md §12와 다른 점

| | 문서 §12 | 본 설계 (v2) |
|---|---|---|
| Layer 1 TOML | 유지(폴백) | **삭제** → SNAP 임베드 |
| 신모델 자동 등장 | §12.2 (게이트) | **기본 동작** (Phase 2) |
| 신프로바이더 자동 등록 | §12.3 (게이트 off) | **기본 동작**, 게이트 없음 |
| TOML 자동 dump | §12.4 (CI) | **불필요** — TOML 자체 소멸 |
| 프로토콜 매핑 | id 기반 provider_map | **npm 기반 protocol_for** |
| 컴파일타임 스냅샷 | 미언급 | **opencode define 패턴 정확 차용** (파일 주입 결정론성) |
| 모델 수준 프로토콜+auth | 미언급 | **154개 오버라이드, auth 필드 추가** |
| extra_headers | PRODUCT-META 필요 | **product-meta.toml로 축소 보존** (~9개) |
| category | 유지 | **제거** |
| 빌드 재현성 | 게이트(OFFLINE) | **OXI_CATALOG_SNAPSHOT 파일 주입** (게이트 없이 결정론적) |
| 동기화 주기 | (N/A) | **mtime 1h 창 + ETag 조건부 GET** — 시간 주기 아닌 데이터 변화 기반 (§4.2) |

## 13. 테스트 계획 (v1 정정 결함 14: 구체화)

| 테스트 | 방식 |
|---|---|
| `protocol_for` 분류 | 모든 npm 값(21종) → 예상 (Api, AuthMethod) 단언. 결정론적, 네트워크 X |
| 모델 수준 auth override | fixture: opencode-go/minimax-m2.5 → (Anthropic, XApiKey). vivgrid/gpt-5.4 → (OpenAi, Bearer) |
| extra_headers 병합 | product-meta.toml의 9개 provider가 헤더 보존. 미지정 provider는 빈 헤더 |
| materialize 전체 | `api.json` 스냅샷 fixture → 145 provider / 5,277 model 카운트 단언 |
| 센티널 소멸 | materialize 후 pricing_unverified() 카운트 = 0 |
| 오프라인 폴백 | SNAP만 있고 LIVE fetch 실패 → SNAP로 145 provider 표시 |
| LIVE 우선 | LIVE 캐시 신선 → SNAP보다 최신 last_updated 반영 |
| Layer 2 최우선 | overrides.toml이 materialize 결과 덮어쓰기 |
| 빌드 결정론성 | `OXI_CATALOG_SNAPSHOT` 주입 빌드 2회 → 동일 바이너리 해시 (허용오차 내) |
| 회귀 | `OXI_MODELS_DEV=off` 환경에서 기존 catalog 테스트 통과 (단언 갱신 후) |
| clippy | `cargo clippy -p oxi-sdk --features native-browser -D warnings` (AGENTS.md) |

## 14. 검증 체크리스트 (구현 완료 시)

- [ ] `OXI_CATALOG_SNAPSHOT` 주입 빌드가 결정론적 (재현성)
- [ ] 첫 실행/오프라인: SNAP 임베드로 145 provider·5,277 모델 전부 표시
- [ ] LIVE 캐시 신선 시 SNAP보다 최신 값 반영
- [ ] models.dev 신규 provider 추가 시 oxi 재빌드 없이 다음 실행에 자동 등장
- [ ] `opencode-go/minimax-m2.5`가 x-api-key 인증 + Anthropic 프로토콜로 호출 (§2.2 검증)
- [ ] openrouter 요청에 HTTP-Referer/X-Title 헤더 포함 (§2.4 회귀 없음)
- [ ] `oxi models refresh` 후 캐시 갱신, 다음 실행에 반영
- [ ] `~/.oxi/catalog/overrides.toml`이 최우선 적용
- [ ] LOCAL(ollama) 모델이 별도 레지스트리로 작동 (§4.5)
- [ ] `cargo nextest run -p oxi-ai catalog` 통과 (단언 갱신 후)
- [ ] `data/catalog/README.md`에 "© models.dev, MIT" + NOTICE 파일

---

## 부록 A: v1 → v2 정정 요약

| 결함 (v1 리뷰) | 정정 (v2) |
|---|---|
| 🔴 Phase 2/3 순서 모순 (TOML 삭제 후 SNAP 없는 깨진 구간) | §10: Phase 2에서 SNAP + REGISTRY 동시 적용 |
| 🔴 모델 수준 auth 무시 (핵심 통찰과 모순) | §2.2/§4.4: `BuiltinModelEntry.auth_method` 추가, `(model_api, model_auth)` 둘 다 override |
| 🔴 extra_headers 제거 → 9개 provider 회귀 | §2.4/§4.6: product-meta.toml로 extra_headers만 보존 (~9개) |
| 🔴 OnceLock(A)/"60분 갱신"/"refresh 즉시" 삼중 모순 | §11.1: OnceLock 유지 + 동기화 주기를 mtime 창+ETag로 재정의 (시간 주기 폐지) |
| 🟡 build.rs 네트워크 fetch 과소평가 | §4.1: `OXI_CATALOG_SNAPSHOT` 파일 주입 (opencode 패턴 정확 차용) |
| 🟡 Layer 2 통합 지점 누락 | §4.4: materialize 내부에서 apply_*_overrides 호출 명시 |
| 🟡 LOCAL 병합 메커니즘 누락 | §4.5: LOCAL은 OnceLock과 별개 레지스트리 유지 |
| 🟡 display_name/description 과잉 삭제 | §4.4: display_name 보존, description만 빈 값 (사용자 동의 범위 준수) |
| 🟡 센티널 소멸 vs 데이터 오류 충돌 | §7.1: 데이터 오류 수용 + README 면책로 보완 |
| 🟡 CostTier dead field | §5.2: schema와 materialize 모두 Phase 4로 이동 |
| 🟢 "우아하다" 자찬 | §1: 삭제. 원칙만 명시 |
| 🟢 테스트 계획 부족 | §13: 구체적 테스트 매트릭스 추가 |
| 🔴 **v3 결함 E**: 기존 oxi id 32개 ↔ models.dev id 불일치 (호환성) | **사용자 결정: 호환성 버림** — models.dev id로 전면 교체, alias 맵 도입 안 함 |
| 🔴 **v3 결함 D**: model 수준 base_url override 누락 (55개 모델) | §2.2: `BuiltinModelEntry.base_url: Option<String>` 추가, materialize 반영 |
| 🔴 **v3 결함 A**: ETag 저장 메커니즘 미정의 | §4.2: 사이드카 파일 `models-dev.json.etag` 명시 |

## 부록 B: 라이선스 / 재배포 (v1 보완)

- models.dev 데이터: **MIT** (`sst/models.dev`). 바이너리 임베드 재배포 허용.
- `data/catalog/README.md`: "Model catalog data © models.dev (MIT)" 표기.
- `NOTICE` 파일 (또는 LICENSES/): models.dev 저작권 + 출처 URL 명시.
- gzip 임베드: ~300-500KB 바이너리 증가 (허용 가능).
