# Design: models.dev 라이브 동기화 (옵션 B 확정)

> 상태: 본 PR(옵션 B 1차) 구현 완료. §12에 후속 작업 3건(신모델 자동 등장 / 신프로바이더 자동 등록 / TOML 자동 dump) 명시.
> 작성: 2026-06-17

## 0. 결정 요약

oxicode 카탈로그를 models.dev(MIT, opencode의 진실 소스)에서 **런타임에 라이브 페치**하여 보강한다.
Layer 1 정적 TOML은 **폴백 전용**(오프라인·첫 실행·페치 실패 시)으로 유지한다.

**왜 B인가** — opencode와 동일한 모델. 항상 최신 가격/limit/reasoning이 반영되며, oxicode 팀이 수동
으로 카탈로그를 유지보수할 필요가 없다. 출고 바이너리 자체는 여전 0.0 가격이지만, 런타임 보강이
성공하면 비용 리포트가 정상화된다(오프라인/에어갭 사용자는 0.0으로 표시 — 수용된 제약).

## 1. 전제 확인: oxicode도 models.dev를 쓸 수 있는가? → YES

| 항목 | 결과 |
|---|---|
| 라이선스 | **MIT** (`anomalyco/models.dev` = `sst/models.dev` 미러). README: "We also use it internally in opencode" |
| API | 공개: `GET https://models.dev/api.json` (provider+모델 서빙/가격), `/models.json` (모델 무관 메타), `/catalog.json` (결합) |
| 데이터 | TOML 원본 → JSON 빌드. 스키마 공개. GitHub Action 검증. 4,843+ commits (활발) |
| oxicode 호환 | 철학·라이선스 충돌 없음. "Data © models.dev (MIT)" 표기 권장 |

## 2. 현재 oxicode 카탈로그 아키텍처 (조사 결과)

```
Layer 1 (정적, 컴파일타임 include_str!)
  data/catalog/{providers.toml, models/*.toml, openclaw/*.toml}
  → CatalogRoot::get()  (OnceLock)

Layer 2 (사용자 오버라이드, 오프라인)
  OXICODE_CATALOG_OVERRIDE / ~/.oxicode/catalog/overrides.toml / .oxicode/catalog.local.toml

Layer 3 (런타임 디스커버리) ⚠️ 현재 DEAD CODE — 본 설계로 부활·확장
  catalog/runtime.rs: discover_all_local() + discover_all_authenticated()
  → 정의/export 됨, 그러나 부트스트랩 미호출. /v1/models 만 (가격/limit 없음)

★ 단일 수렴점 (본 설계의 핵심 통찰):
  model_db.rs::all_provider_models()  — OnceLock<Vec<(provider, &[ModelEntry])>>
    1. CatalogRoot::get() 읽기 (Layer 1)
    2. load_overrides() + apply_model_overrides (Layer 2)
    3. BuiltinModelEntry → ModelEntry 변환 + Box::leak (&'static 화)
  → MODEL_INDEX / PROVIDER_INDEX 가 이 결과 참조
  → get_model_entry / model_from_entry / fallback_chain / setup_wizard / TUI 슬래시 등
    모든 소비자가 결국 여기로 수렴
```

→ **이 OnceLock의 `get_or_init` 클로저에 models.dev 보강을 한 번 끼우면, downstream 전체가
   자동으로 보강된 값을 본다. 진입점이 단 하나.**

## 3. opencode 연동 패턴 (`packages/core/src/models-dev.ts`, `plugin/models-dev.ts`)

폴백 체인: 컴파일타임 스냅샷 → 디스크 캐시(5분 TTL, Flock) → 라이브 페치(10s, 지수 백오프 2회) → 상위 폴백.
`refresh()` 백그라운드 fork, 60분 `Schedule.spaced`. `cachedInvalidateWithTTL(infinity)` 메모리 캐시.
보강 필드: name, family, api(native/aisdk), capabilities(tools/input/output modalities), variants,
released, **cost**, status, **limit(context/input/output)**.

## 4. 최종 설계

### 4.1 인프라 — 이미 구비됨 (신규 의존성 0)

oxicode-ai `Cargo.toml` 확인 결과 모두 존재: `reqwest`(json/stream/blocking), `tokio`(full),
`serde`+`serde_json`, `toml`, `fs2`(flock) + `raii_flock`(RAII 가드), `dirs`(5), `parking_lot`,
`thiserror`, `tracing`. **새 크레이트 의존성 추가 불필요.**

### 4.2 모듈 레이아웃

```
oxicode-ai/src/catalog/
  models_dev.rs   ← 신규 (신규 코드의 90%)
    - 스키마 (serde)
    - fetch_models_dev()      async, 캐시+라이브+폴백
    - init_models_dev()       async, 전역 OnceLock 채움 (부트스트랩 호출)
    - enrich(entry, oxicode_pid)  sync, BuiltinModelEntry 보강
    - PROVIDER_MAP, REASONING_PRESERVE
  mod.rs          ← pub use 추가
  runtime.rs      ← (기존, 그대로 유지 — 별개 레이어)
  model_db.rs     ← all_provider_models()에 enrich 1줄 삽입 (유일한 기존 코드 변경)
oxicode-cli/src/bootstrap.rs  ← build_app() 맨 앞에 init_models_dev().await 추가
```

### 4.3 스키마 (Rust serde — models.dev `api.json`)

```rust
use std::collections::BTreeMap;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct MdCatalog(pub BTreeMap<String, MdProvider>);  // provider_id → provider

#[derive(Debug, Deserialize)]
pub struct MdProvider {
    pub name: String,
    pub env: Vec<String>,
    pub npm: Option<String>,
    pub api: Option<String>,
    pub models: BTreeMap<String, MdModel>,
}

#[derive(Debug, Deserialize)]
pub struct MdModel {
    pub name: String,
    pub family: Option<String>,
    pub reasoning: bool,
    pub tool_call: bool,
    pub attachment: bool,
    pub temperature: bool,
    pub limit: MdLimit,
    pub cost: Option<MdCost>,
    pub modalities: Option<MdModalities>,
    pub status: Option<String>,  // alpha|beta|deprecated|null
}

#[derive(Debug, Deserialize)]
pub struct MdLimit {
    pub context: f64,
    pub input: Option<f64>,
    pub output: f64,
}

#[derive(Debug, Deserialize)]
pub struct MdCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    // tiers / context_over_200k: oxicode는 단일 cost만 → base tier 사용, 무시 (주석 문서화)
}

#[derive(Debug, Deserialize)]
pub struct MdModalities {
    pub input: Vec<String>,
    pub output: Vec<String>,
}
```

### 4.4 fetch / 캐시 / 락

```rust
use std::{path::PathBuf, sync::Arc, time::{Duration, SystemTime}};
use parking_lot::RwLock;

const TTL: Duration = Duration::from_secs(5 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_URL: &str = "https://models.dev";
const UA: &str = concat!("oxicode/", env!("CARGO_PKG_VERSION"));

fn cache_path() -> PathBuf {
    // ~/.oxicode/cache/models-dev.json  (dirs::home_dir 기반)
    dirs::home_dir().unwrap_or_default().join(".oxicode/cache/models-dev.json")
}

/// 전역 보강 테이블. init_models_dev() 가 채운다.
/// all_provider_models() 의 sync 경로가 읽는다.
static MODELS_DEV: OnceLock<Option<Arc<MdCatalog>>> = OnceLock::new();

pub async fn init_models_dev() {
    MODELS_DEV.get_or_init(|| {
        Arc::new(()) /* placeholder */;
        fetch_with_fallback().await
    });
}

async fn fetch_with_fallback() -> Option<Arc<MdCatalog>> {
    // 1) 디스크 캐시 신선도 검사 (sync, 빠름)
    if let Some(cached) = read_cache_if_fresh() { return Some(Arc::new(cached)); }
    // 2) 라이브 페치 (교차프로세스 Flock 하에)
    if !fetch_disabled() {
        if let Some(live) = live_fetch_locked().await {
            write_cache_atomic(&live);   // temp + rename (AGENTS.md 규칙)
            return Some(Arc::new(live));
        }
    }
    // 3) 만료된 디스크 캐시라도 있으면 사용 (stale-but-better)
    read_cache_any()
}

async fn live_fetch_locked() -> Option<MdCatalog> {
    // raii_flock::ExclusiveFlock::lock(cache_path) → 다른 oxicode 프로세스와 동기화
    // client.get("{url}/api.json").header("User-Agent", UA).timeout(FETCH_TIMEOUT)
    // 실패 시 지수 백오프 2회 재시도 (reqwest retry 또는 수루프)
    // serde_json::from_str → MdCatalog. 파싱 실패 시 None.
}
```

**캐시 신선도**: `SystemTime::modified` vs `now`. `dirs::cache_dir`이 아닌 `~/.oxicode/cache/`
(oxicode 관례 — settings/sessions/auth 와 동일 루트, 사용자 단일 백업 지점).

**Flock**: opencode와 동일 목적(동시 oxicode CLI들이 같은 캐시 파일에 쓰기 경쟁 방지). `fs2` + `raii_flock`.

### 4.5 보강/병합 로직 + 센티널 규칙

```rust
/// Layer 1 entry를 models.dev 데이터로 보강. 원본을 변경하지 않고 새 entry 반환.
/// (all_provider_models 의 OnceLock init 클로저 내에서 호출 — &mut BuiltinModelEntry)
pub fn enrich(entry: &mut BuiltinModelEntry, catalog: &MdCatalog, oxicode_pid: &str) {
    let Some(md_pid) = provider_map(oxicode_pid) else { return };     // 매핑 없으면 스킵
    let Some(mdprov) = catalog.0.get(md_pid) else { return };
    let Some(mdm) = mdprov.models.get(&entry.id) else { return }; // ID 불일치 스킵

    // 가격: models.dev > 0 → 검증값으로 채움 (센티널 -1.0 도 양수로 정상화)
    //      models.dev 0/null → Layer 1 유지 (검증된 무료/알 수 없음 보존)
    if let Some(c) = &mdm.cost {
        if c.input > 0.0 { entry.cost_input = c.input; }
        if c.output > 0.0 { entry.cost_output = c.output; }
        if let Some(cr) = c.cache_read { if cr > 0.0 { entry.cost_cache_read = cr; } }
        if let Some(cw) = c.cache_write { if cw > 0.0 { entry.cost_cache_write = cw; } }
    }

    // limit: models.dev 가 더 최신. 0(알수없음)이거나 양수면 채움.
    if mdm.limit.context > 0.0 { entry.context_window = mdm.limit.context as u32; }
    if mdm.limit.output > 0.0 { entry.max_tokens = mdm.limit.output as u32; }

    // reasoning: 보존 allowlist 에 없을 때만 models.dev 값으로.
    if !reasoning_preserve(oxicode_pid, &entry.id) {
        entry.reasoning = mdm.reasoning;
    }

    // modalities: text/image 외 oxicode InputModality 미지원 → text/image 만 필터
    // (옵션: attachment=true 면 image 추가. Phase 2.)
}
```

**센티널 상호작용** (기존 `model_db.rs::From<&BuiltinModelEntry>` 와 일관성 유지):
- openclaw 소스의 `0.0` → 런타임 `-1.0`(미검증) 변환은 enrich **이후**에 일어남.
- enrich가 models.dev 양수 가격을 넣으면 변환 시 `-1.0`이 아니라 그 양수가 됨 → 미검증 경고 소멸. ✅
- enrich가 아무것도 안 넣으면(매핑/ID 불일치) 기존 센티널 동작 유지. ✅

### 4.6 Provider ID 매핑 (oxicode → models.dev)

oxicode의 지역/plan 변형은 단일 models.dev provider로 collapse. 비교 스크립트에서 검증된 매핑:

```rust
fn provider_map(oxicode_pid: &str) -> Option<&'static str> {
    Some(match oxicode_pid {
        "anthropic" | "anthropic-vertex" => "anthropic",
        "google" => "google",
        "google-vertex" => "google-vertex",
        "google-vertex-anthropic" => "google-vertex-anthropic",
        "openai" | "openai-responses" | "openai-completions" | "openai-codex" => "openai",
        "openrouter" => "openrouter",
        "deepseek" => "deepseek",
        "groq" => "groq",
        "xai" => "xai",
        "mistral" => "mistral",
        "azure" | "azure-cognitive-services" => "azure-cognitive-services",
        "bedrock" | "amazon-bedrock" | "amazon-bedrock-mantle" => "amazon-bedrock",
        "fireworks" => "fireworks-ai",
        "togetherai" | "together" => "togetherai",
        "cerebras" => "cerebras",
        "deepinfra" => "deepinfra",
        "cloudflare" | "cloudflare-workers-ai" => "cloudflare-workers-ai",
        "cloudflare-ai-gateway" => "cloudflare-ai-gateway",
        "huggingface" => "huggingface",
        "moonshotai" | "moonshot" => "moonshotai",
        "moonshotai-cn" => "moonshotai-cn",
        "kimi-coding" => "kimi-for-coding",
        "xiaomi" => "xiaomi",
        "xiaomi-token-plan" => "xiaomi-token-plan",
        "minimax" => "minimax",
        "minimax-cn" => "minimax-cn",
        "zai" | "zai-global" => "zai",
        "zai-cn" => "zai-cn",
        "zai-coding-global" | "zai-coding-cn" => "zai-coding-plan",
        "vercel-ai-gateway" => "vercel",
        "copilot" | "codex" | "github-copilot" => "github-copilot",
        "opencode" => "opencode",
        "opencode-go" => "opencode-go",
        "nvidia" => "nvidia",
        "novita" => "novita-ai",
        "venice" => "venice",
        "chutes" => "chutes",
        "gmi" => "gmicloud",
        "stepfun" => "stepfun-ai",
        "qwen-portal" | "alibaba" => "alibaba",
        "ollama-cloud" => "ollama-cloud",
        "synthetic" => "synthetic",
        // 매핑 없음(스킵): ollama, lmstudio, vllm, sglang, byteplus, qianfan, arcee,
        //   litellm, microsoft-foundry, copilot-proxy, kilocode 등 — 로컬/게이트웨이/미매핑
        _ => return None,
    })
}
```
→ 매핑 누락 시 `tracing::warn!` (CI 에서 누락 모델 추적). `/v1/models` 노출 프로바이더는
   Layer 3 기존 경로가 담당하므로 본 매핑에서 제외.

### 4.7 Reasoning 보존 allowlist

버킷 D(oxicode=True/md=False) 중 oxicode가 **의도적**으로 reasoning=true로 세팅한 변형. enrich가
덮어쓰지 않도록 보존. (oxicode=False/md=True 25종은 models.dev로 보강 — 의도적 아님 확인됨.)

```rust
fn reasoning_preserve(oxicode_pid: &str, id: &str) -> bool {
    let key = (oxicode_pid, id);
    // 패턴 기반 (변형 접미사) + 명시 목록
    matches!(key,
        // TEE(Trusted Execution Environment) 변형 — reasoning 비활성 버전
        ("chutes", _) if id.ends_with("-TEE") |
        ("together", _) if id.ends_with("-tput") |          // throughput 변형
        // groq compound — tool-augmented, reasoning 플래그 의미 다름
        ("groq", "groq/compound") |
        ("groq", "groq/compound-mini") |
        // open weight 양자화 변형 — 동작 다를 수 있음
        ("together", "Qwen/Qwen3-Coder-Next-FP8") |
        // mistral-medium-latest / together DeepSeek-V3 — oxicode 검증값, 수동 리뷰 예정
        ("mistral", "mistral-medium-latest") |
        ("together", "Qwen/Qwen3.7-Max") |
        ("together", "deepseek-ai/DeepSeek-V3")
    )
}
```
→ Phase 1에서는 명시 목록 운영. 누락/오분류는 CI 리포트로 추적 후 조정.

### 4.8 부트스트랩 통합 (`oxicode-cli/src/bootstrap.rs`)

`build_app()` 맨 앞(설정 로드 전, 다른 모델 조회 전)에 한 줄:

```rust
pub async fn build_app(args: &CliArgs) -> Result<crate::App> {
    // ★ models.dev 라이브 보강 초기화 (비동기, 캐시 우선으로 빠름)
    oxicode_ai::catalog::models_dev::init_models_dev().await;

    let mut settings = Settings::load().unwrap_or_default();
    // ... 기존 로직
}
```

- `init_models_dev()`는 디스크 캐시가 신선하면 사실상 동기적(수 ms). 만료 시에만 10s 최대 대기.
- RPC 모드, print 모드 모두 `build_app` 경유하므로 자동 적용.
- 백그라운드 60분 갱신 태스크(opencode 패리티)는 Phase 2 — Phase 1은 시작 시 1회만.

### 4.9 단일 진입점 변경 (`model_db.rs::all_provider_models`)

기존 `get_or_init` 클로저의 Layer 2 적용 직후, ModelEntry 변환 직전에 enrich 삽입:

```rust
// (기존) Layer 2 apply_model_overrides(...) → all_builtins 갱신 완료
// ★ 신규: models.dev 보강
if let Some(md) = crate::catalog::models_dev::get() {  // sync 읽기, None 이면 스킵
    for bm in all_builtins.iter_mut() {
        crate::catalog::models_dev::enrich(bm, md, &bm.provider);
    }
}
// (기존) by_pid 그룹화 + ModelEntry 변환 + Box::leak
```

→ `get()`는 `MODELS_DEV.get().and_then(|o| o.as_deref())`. 부트스트랩이 init 전이거나
   라이브러리 단독 사용 시 `None` → Layer 1만으로 동작 (graceful degradation).

### 4.10 환경 변수 / 게이트

| 변수 | 기본 | 설명 |
|---|---|---|
| `OXICODE_MODELS_DEV` | `auto` | `auto`(캐시 있거나 네트워크 OK면 on)·`on`·`off` |
| `OXICODE_MODELS_DEV_URL` | `https://models.dev` | 엔터프라이즈 미러 (opencode `OPENCODE_MODELS_URL` 패리티) |
| `OXICODE_MODELS_DEV_DISABLE_FETCH` | (unset) | `1`이면 라이브 페치 금지, 디스크 캐시만 (에어갭) |
| `OXICODE_MODELS_DEV_TTL` | `300` | 캐시 신선도(초) |

## 5. 소비자 변경 — 최소화됨

단일 진입점 통합 덕분에 **기존 소비자는 변경 불필요**:

| 소비자 | 변경 | 이유 |
|---|---|---|
| `model_db::get_model_entry` / `get_provider_models` / `get_all_models` | 없음 | `all_provider_models()` 간접 참조, 자동 보강 |
| `multi_provider::model_from_entry` / `find_model_for_provider` | 없음 | 위와 동일 |
| `fallback_chain.rs` | 없음 | `get_model_entry` 사용 |
| `setup_wizard.rs`, TUI slash/overlay, `main.rs` (models cmd) | 없음 | SDK re-export 경유 |
| `oxicode-sdk::get_model_entry` 등 re-export | 없음 | `oxicode-ai` 전달만 |
| `oxicode-ai/src/catalog/models_dev.rs` | **신규** | 본 설계의 90% |
| `oxicode-ai/src/model_db.rs` | enrich 3줄 | §4.9 |
| `oxicode-cli/src/bootstrap.rs` | init 1줄 | §4.8 |

## 6. 에러/폴백 매트릭스

| 상황 | 동작 |
|---|---|
| 오프라인 + 캐시 없음 | `MODELS_DEV=None` → Layer 1만 (0.0 가격, 기존 동작) |
| 오프라인 + 캐시 만료 | stale 캐시 사용(opencode 패턴) → 보강은 적용, 약간 낙후 가능 |
| 페치 타임아웃/4xx/5xx | 재시도 2회 후 폴백. stale 캐시 → Layer 1 순 |
| JSON 파싱 실패 | 캐시 무시(삭제) → Layer 1. `tracing::error!` |
| Flock 획득 실패 | 락 없이 페치 진행(경쟁 쓰기 감수) → 캐시 일관성은 다음 시작시 정상화 |
| provider 매핑 누락 | 해당 모델 스킵(Layer 1 유지) + `tracing::warn!` |
| models.dev 가격 0/null | Layer 1 가격 유지 (검증 무료/알수없음 보존) |

모든 실패는 **비치명적**: 기능은 Layer 1으로 동작, 가격 정확도만 저하.

## 7. 테스트 계획

| 테스트 | 방식 |
|---|---|
| 스키마 파싱 | `api.json` 스냅샷 fixture → `MdCatalog` 디코딩 |
| 오프라인 폴백 | mock fetch 실패 → `get()`=None → Layer 1 유지 (DeepSeek V4 컨텍스트 1M 보존) |
| 보강 정확성 | mock `MdCatalog`에서 `deepseek-chat` 131072→1000000, 가격 0→0.14 검증 |
| 센티널 정상화 | openclaw `-1.0` entry가 enrich 후 양수 → `pricing_unverified()=false` |
| 우선순위 | Layer 2 오버라이드 가격 > models.dev (사용자가 이기면) |
| reasoning allowlist | `chutes/*-TEE` 보존, `vercel-ai-gateway/deepseek/deepseek-v3.2` 보강 |
| TTL/캐시 | 신선→동기, 만료→페치, atomic write(temp+rename) |
| 매핑 누락 경고 | 매핑 없는 provider 스킵 + warn |
| 기존 카탈로그 테스트 | 전부 통과 유지 (보강이 None 일 때 동일 결과) |

`OXICODE_MODELS_DEV=off` 환경에서 기존 `cargo nextest run -p oxicode-ai catalog` 가 동일 통과해야 함.

## 8. 문서 갱신

| 파일 | 변경 |
|---|---|
| `oxicode-ai/data/catalog/README.md` | "Upstream sync" 표: models.dev(MIT) 행 추가 + "live enrichment" 설명. opencode 행 정정("no data" → "fetches models.dev live"). "Price data quality" 표의 "✅ All verified" → 현재 실상 정정(0.0 누락 인정) + 런타임 보강으로 해소 명시 |
| `AGENTS.md` | Layer 3 dead code 해소 + 신규 "models.dev enrichment layer" 명시. Common Commands에 `OXICODE_MODELS_DEV` 환경변수 추가 |
| `oxicode-ai/src/catalog/models_dev.rs` (모듈 doc) | 본 설계 요약 + 라이선스 귀속("Data © models.dev, MIT") |

## 9. 리스크 & 완화

| 리스크 | 완화 |
|---|---|
| 오프라인/에어갭 사용자는 비용 0.0 표시 | Layer 1 정확화(옵션 A)를 **후속 PR**으로 검토 — 본 결정에서는 수용 |
| 첫 실행 시 10s 대기 가능 | 디스크 캐시 우선(신선하면 수 ms). TUI에 "loading models..." 인디케이터 (Phase 2) |
| 빌드 재현성 약화 (런타임 비결정론) | 게이트(`OXICODE_MODELS_DEV=off`) + 캐시로 통제 가능. CI는 `off` 고정 |
| reasoning allowlist 누락/오분류 | 명시 목록 + CI 리포트로 추적, 점진 정정 |
| provider 매핑 누락 → 신규 모델 미보강 | warn 로그 + 분기 리뷰. 매핑은 정적 테이블이라 PR로 보강 용이 |
| models.dev 자체 부정확 (커뮤니티 데이터) | README 면책 유지("공식 문서가 최종 기준"). 청구 정확성 보장 X |
| 캐시 파일 손상 | atomic write + 파싱 실패 시 삭제 폴백 |
| OnceLock init 전 동시 조회 | `get()`가 None 반환 → Layer 1 폴백 (데드락/패닉 없음) |
| 백그라운드 갱신 태스크 생명주기 | Phase 2에서만 도입, shutdown 정리 포함 |

## 10. 구현 순서 (승인 시)

1. `models_dev.rs`: 스키마 + `provider_map` + `reasoning_preserve` (단위테스트 가능, 네트워크 X)
2. `models_dev.rs`: `fetch_with_fallback` + 캐시 + Flock (mock 테스트)
3. `models_dev.rs`: `enrich` + 센티널 상호작용 테스트
4. `model_db.rs`: `all_provider_models`에 enrich 3줄 + `get()` 추가
5. `bootstrap.rs`: `init_models_dev().await` 1줄
6. 환경변수 + 게이트
7. 문서 갱신 (README/AGENTS.md/모듈 doc)
8. CI: 기존 catalog 테스트 off 모드 통과 확인 + 신규 enrichment 테스트
9. (Phase 2) 백그라운드 60분 갱신 + TUI 인디케이터

## 11. 옵션 A(정적 동기화)와의 관계

본 결정(B)은 A를 **배제하지 않음**. A를 후속으로 추가하면 출고 바이너리 자체가 정확해져
오프라인/에어갭 제약이 해소된다(옵션 C로 수렴). A의 스크립트는 본 설계의 `enrich` 로직과
`provider_map`/`reasoning_preserve` 테이블을 그대로 재사용 가능 → 구현 중복 최소.

## 12. 본 PR이 남기는 미해결 과제 (후속 작업)

본 PR은 *기존 모델의 가격/limit/reasoning 자동 갱신*까지만 다룬다. 더 완전한
동적 카탈로그를 향해 다음 두 가지가 남아 있다.

### 12.1 사용자 페인포인트 (재진술)

> 모델이 주기적으로 계속 출시되는데, 그때마다 oxicode는 `data/catalog/models/*.toml`을
> 수정해서 새 버전을 배포해야 한다. 불편하다.

이전에는 "수동 TOML 수정 + 릴리즈"가 필요했다. **본 PR 적용 후**:

| 시나리오 | 동작 |
|---|---|
| 기존 모델의 가격/limit/reasoning 변동 | ✓ 다음 실행 시 자동 반영 (enrich) |
| **신모델 출시** (예: DeepSeek V5) | ⚠️ **여전히 불편함 잔존** — Layer 1 `data/catalog/models/*.toml`에 모델 ID가 *없으면* enrich 대상이 아니므로 `oxicode models` 목록에 안 뜨고, 자동완성/TUI 추천에 안 나온다. 가격만 보고 싶으면 모델 ID를 사용자가 알고 직접 지정해야 한다. |
| **신프로바이더 출시** | ❌ `data/catalog/providers.toml`에 추가하기 전까지 oxicode는 해당 프로바이더를 모름 |

즉 본 PR은 "기존 모델의 데이터 신선도"는 해결했지만, "신모델/신프로바이더의 자동 등장"은
아직 해결하지 못했다.

### 12.2 후속 PR 1: 신모델 자동 등장

**목표**: models.dev에 추가된 모델이 oxicode의 `oxicode models`/TUI 자동완성/선택지에
자동으로 나타난다. TOML 수정/릴리즈 사이클 제로.

**핵심 변경** (`catalog/models_dev.rs`):
- `enrich`가 "기존 entry 보강" 외에 "models.dev-only entry를 synthetic Layer 1 entry로
  추가"하는 모드도 지원. 즉 `(provider, id)`가 Layer 1에 없을 때:
  - `BuiltinModelEntry`를 구성해서 `all_provider_models()`의 결과에 append
  - `api`/`provider` 필드는 매핑 테이블에서 가져옴
  - 가격/limit/reasoning은 models.dev 값 그대로
- `OXICODE_MODELS_DEV_ADD_NEW=1` 게이트 (opt-in, 기본 off — 안전한 동작 보존)
- 단, **신프로바이더는 여전히 정적** — `provider_map`에 없는 프로바이더의 모델은
  추가 안 됨. 신모델 자동 등장의 범위는 "기존 oxicode가 아는 프로바이더 한정".

**검토 사항**:
- synthetic entry는 `Box::leak`으로 `&'static`화 (기존 패턴과 동일)
- `apply_provider_overrides`와 상호작용 검토 (override 우선순위 유지)
- `sentinel_pricing_counted` 테스트의 단언(`s == 34`) 영향 — 신모델은 센티넬이 아니라
  검증값으로 추가되므로 단언 조정 필요

### 12.3 후속 PR 2: 신프로바이더 자동 등록

**목표**: 새로운 LLM 프로바이더가 등장해도 oxicode가 알지 못하는 일이 없도록.

**핵심 변경**:
- 부트스트랩 시 `OXICODE_MODELS_DEV_AUTOREGISTER=1`이면 models.dev의 프로바이더 메타
  (`env`, `api`, `npm`)를 읽어 `BuiltinProviderEntry`로 자동 등록.
- `data/catalog/providers.toml`은 최소 *카테고리/별칭/추가 헤더/기본 base_url* 같은
  제품 메타만 유지. 핵심 *접속 정보*(env_key, api endpoint)는 동적.
- 게이트 기본값: `off` (정적 우선). 사용자가 명시적으로 켤 때만 동적.

**큰 리팩토링 포인트**:
- `register_builtins.rs::create_builtin_provider()`의 정적 `match`/`BuiltinProvider`
  경로를 *id + env_key + api + base_url + auth_method만 받아 인스턴스화*하는
  단일 동적 경로로 통합.
- `multi_provider.rs::construct_model_from_id`의 provider→API 타입 정적 매핑을
  동적으로.
- `setup_wizard`가 의존하는 정적 메타(표시명/별칭/카테고리) 호환성 유지.

**AGENTS.md 철학과의 긴장**:
- "Progressive enhancement — core works with zero config" — 완전 동적은
  `OXICODE_MODELS_DEV_AUTOREGISTER=on`을 요구하므로 zero config가 아님. 이 게이트가
  "opt-in" 원칙을 지켜준다.
- "Sandboxed extensions" — 무관.
- "Port-based adapters — opt-in" — 본 PR의 게이트 패턴과 일관성 유지.

### 12.4 후속 PR 3: TOML 자동 dump (CI 동기화)

**목표**: 정적 카탈로그의 결정론성/오프라인 정확성을 유지하면서 자동 갱신.

- `OXICODE_MODELS_DEV_TOML_OUTPUT=path` 게이트. 부트스트랩 후 현재 enriched 카탈로그를
  `data/catalog/models/*.toml` 형식으로 dump. (opencode의 `OPENCODE_MODELS_PATH` 패턴.)
- CI에서 주간 실행 → `models-dev`와 차이가 있으면 PR 자동 생성. (이미 동기화된
  출고 바이너리 + 항상 최신의 보강 = 옵션 C 수렴.)
- 이 PR이 머지되면 본 PR의 "오프라인 사용자는 가격 $0" 제약이 해소된다.

### 12.5 추정

| PR | 추정 작업량 | 위험 |
|---|---|---|
| 12.2 신모델 자동 등장 | 0.5~1 PR (200~400줄) | 낮음 (게이트 + synthetic entry 추가는 잘 격리됨) |
| 12.3 신프로바이더 자동 등록 | 1~2 PR (대형 리팩토링) | 중간 (`create_builtin_provider` 통합 영향) |
| 12.4 TOML 자동 dump | 0.5 PR (200줄) | 낮음 (단순 직렬화) |

### 12.6 이 문서가 작성된 이유

이 §12가 존재하는 이유: 본 PR(옵션 B)이 "완전한 동적 카탈로그"로 보이는 착시가
생기지 않도록, 그리고 후속 작업자가 *본 PR이 어디서 멈췄는지* 정확히 알 수 있도록.
사용자 페인포인트("TOML 수정/배포 사이클")는 12.2 + 12.3이 완료되어야 완전히 해소된다.

