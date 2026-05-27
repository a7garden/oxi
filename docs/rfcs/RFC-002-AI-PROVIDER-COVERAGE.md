# RFC-002: AI Provider 커버리지 확대 — 모델·프로바이더·프로토콜

**상태**: 재검토 완료  
**우선순위**: P1 — 모델 가용성이 사용자 채택의 핵심  
**현재 완성도**: ~78%  
**목표**: 기능 동등성 95%+  

---

## 1. 문제 정의

oxi-ai는 코딩 에이전트용 LLM 추상화 계층으로, 28개 프로바이더 / 544개 모델 / 8개 API 프로토콜을 지원한다.

```
현재 상태:
├── 모델 수: 544개 (28개 프로바이더)
├── API 프로토콜: 8개 variant
├── Compat 레이어: ✅ 이미 Model.compat 레벨로 구현
├── 메시지 변환: ✅ 2-pass pipeline 완료
├── 모델 DB 자동화: ❌ 스크립트 없음 (전체 수작업)
└── 이미지 생성: 🔶 코딩 에이전트 관점에서 우선순위 낮음

보류 항목 (실제 프로바이더 요구 시 재검토):
├── WebSocket 스트리밍 — SSE로 충분한데 추가해도 비용만 발생
├── Claude Code 스텔스 — API 약관 위반 소지, 법적 검토 필요
└── 이미지 생성 API — 코딩 에이전트 핵심 기능 아님
```

8개 API 프로토콜 (정확한 프로바이더 수):

| Enum variant | 구현 파일 | 프로바이더 수 |
|---|---|
| `OpenAiCompletions` | `openai.rs` | 35개 |
| `OpenAiResponses` | `openai_responses.rs` | 2개 (openai-responses, openai-codex) |
| `AnthropicMessages` | `anthropic.rs` | 4개 (anthropic, minimax, minimax-cn, google-vertex-anthropic) |
| `GoogleGenerativeAi` | `google.rs` | 1개 |
| `GoogleVertex` | `vertex.rs` | 1개 |
| `MistralConversations` | `mistral.rs` | 1개 |
| `AzureOpenAiResponses` | `azure.rs` | 1개 |
| `BedrockConverseStream` | `bedrock.rs` | 1개 |

**총**: 47개 BuiltinProvider → 8개 Api variant 매핑

---

## 2. 설계 원칙

1. **Data-driven provider factory**: 기존 `register_builtins.rs` 패턴 활용. 프로바이더 추가 시 새 `BuiltinProvider` 항목만 추가.
2. **Compat는 이미 구현됨**: `Model.compat: Option<CompatSettings>`이 프로바이더-agnostic하게 모델 단위로 동작. BuiltinProvider 레벨이 아닌 Model 레벨에서 호환성 설정 관리.
3. **모델 DB 자동화**: `scripts/generate-models.rs`로 `model_db.rs` 자동 갱신. 기존 544개는 전부 수작업.
4. **확장 가능한 프로토콜**: 새 API 프로토콜 추가 시 provider 파일 하나 + Api variant 추가.

---

## 3. 기존 구현 상태

### 3.1 CompatSettings — 이미 Model 레벨에 완전 구현

`oxi-ai/src/types.rs`의 `CompatSettings`는 9개 필드를 가진다:

```rust
pub struct CompatSettings {
    pub supports_store: bool,                    // default: true
    pub supports_developer_role: bool,          // default: true
    pub supports_reasoning_effort: bool,         // default: true
    pub supports_usage_in_streaming: bool,      // default: true
    pub max_tokens_field: Option<MaxTokensField>,// MaxCompletionTokens vs max_tokens
    pub requires_tool_result_name: bool,         // default: false
    pub requires_assistant_after_tool_result: bool, // default: false
    pub requires_thinking_as_text: bool,        // default: false
    pub thinking_format: Option<ThinkingFormat>,// OpenAI, OpenRouter, DeepSeek, Zai, Qwen...
}
```

`Model`에 이미 `compat: Option<CompatSettings>` 필드가 있고, `model_registry.rs`에서 모델별 기본값 자동 설정 가능.

`ThinkingFormat` enum (6 variant): `OpenAI`, `OpenRouter`, `DeepSeek`, `Zai`, `Qwen`, `QwenChatTemplate`  
`MaxTokensField` enum (2 variant): `MaxCompletionTokens`, `MaxTokens`

### 3.2 BuiltinProvider — 실제 필드 목록

```rust
pub struct BuiltinProvider {
    pub name: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub api: Api,
    pub env_key: &'static str,
    pub extra_env_keys: &'static [&'static str],  // 추가 env var 폴백
    pub base_url: &'static str,
    pub default_enabled: bool,                     // 기본 활성화 여부
    pub auth_method: AuthMethod,                   // Bearer, XApiKey, ApiKey, None
    pub extra_headers: &'static [(&'static str, &'static str)],
    pub category: &'static str,
    pub description: &'static str,                 // UI용 짧은 설명
}
```

### 3.3 메시지 변환 (완전 구현)

`oxi-ai/src/transform.rs` (1252줄) — 2-pass 파이프라인:

```
Pass 1: transform_messages_for_model()
├── downgrade_unsupported_images()    → 비전 모델이 아니면 이미지 → 플레이스홀더
├── normalize_tool_call_id()          → 툴 콜 ID 정규화 (utils::normalize_tool_call_id 위임)
└── ContentBlock 변환                 → thinking → text, redacted → 동일 모델 유지

Pass 2: insert_synthetic_results()
├── error/aborted 메시지 건너뛰기
└── 고아 툴 콜 → "No result provided" synthetic 결과 삽입
    (API 에러 방지를 위한 핵심 로직)
```

### 3.4 모델 DB 현황

`oxi-ai/src/model_db.rs`: 544개 모델 / 28개 프로바이더 static 배열

```
AMAZON_BEDROCK_MODELS:     50개
ANTHROPIC_MODELS:          23개
AZURE_OPENAI_RESPONSES_MODELS: 42개
CEREBRAS_MODELS:            4개
CLOUDFLARE_AI_GATEWAY_MODELS: 20개
CLOUDFLARE_WORKERS_AI_MODELS: 8개
DEEPSEEK_MODELS:            2개
FIREWORKS_MODELS:          19개
GITHUB_COPILOT_MODELS:     26개
GOOGLE_MODELS:             27개
GOOGLE_VERTEX_MODELS:      13개
GROQ_MODELS:               18개
HUGGINGFACE_MODELS:       22개
KIMI_CODING_MODELS:        3개
MINIMAX_MODELS:             2개
MINIMAX_CN_MODELS:          2개
MISTRAL_MODELS:            28개
MOONSHOTAI_MODELS:          7개
MOONSHOTAI_CN_MODELS:       7개
OPENAI_MODELS:             42개
OPENAI_CODEX_MODELS:       10개
OPENCODE_MODELS:           30개
OPENCODE_GO_MODELS:        14개
OPENROUTER_MODELS:         60개
VERCEL_AI_GATEWAY_MODELS:  30개
XAI_MODELS:               25개
XIAOMI_MODELS:             5개
ZAI_MODELS:                5개
─────────────────────────────────
TOTAL:                   544개
```

**스크립트 부재**: `scripts/generate-models.rs`는 존재하지 않음. 544개 전부 수작업으로 관리.

### 3.5 프로바이더 팩토리

`create_builtin_provider()` + `create_builtin_provider_with_options()` 두 함수로:
- 이름/앨리어스 → BuiltinProvider 메타데이터 조회
- Api variant 매칭 → 해당 provider struct 인스턴스 생성
- base_url + extra_headers + api_key 동적 주입

---

## 4. 구현 계획

### Phase 1: 모델 DB 자동화 (2주) — 가장 높은 가치

| 작업 | 산출물 |
|------|--------|
| `scripts/generate-models.rs` 작성 | JSON/YAML → Rust ModelEntry[] codegen |
| 850+ 모델 목표 | pi의 872개 중 oxi에 적합한 것 선별 |
| 프로바이더별 CompatSettings 기본값 | model_registry.rs에 자동 설정 규칙 추가 |
| Dedup + 정렬 검증 | 중복 모델 제거, ID 정규화 |

**산출 파일**:
- `scripts/generate-models.rs` — 빌드 시 model_db.rs 자동 생성
- `oxi-ai/src/model_db.rs` 갱신

### Phase 2: CompatSettings per-model 기본값 자동화 (1주)

| 작업 | 산출물 |
|------|--------|
| 프로바이더별 기본 CompatSettings | model_registry.rs에 패턴 매칭 규칙 |
| e.g., OpenAI 호환 → `thinking_format: Some(ThinkingFormat::OpenAI)` |
| e.g., Vertex → `max_tokens_field: Some(MaxTokensField::MaxCompletionTokens)` |

### Phase 3: 정합성 확보 및 문서 (0.5주)

| 작업 | 산출물 |
|------|--------|
| model_db.rs 주석 "544 models" 갱신 | 실제 카운트와 일치 |
| BuiltinProvider 목록 정렬 검증 | 47개 모두 name 정렬 확인 |
| API 프로바이더 수 표 갱신 | 실제 숫자로 동기화 |

---

## 5. 보류 항목 (재검토 필요)

아래는 현재 코드베이스에 없으나, 실제 사용자 요구 또는 프로바이더 API 변경이 발생할 때 재검토한다.

| 항목 | 보류 이유 | 재검토 트리거 |
|------|---------|------------|
| **이미지 생성 API** | 코딩 에이전트 핵심 기능 아님. DALL-E 3 호출이 코딩 워크플로우에서 거의 필요 없음. Provider 트레이트 확장은 모든 구현체에 변경 필요. | 사용자 요청 5건 이상 or 코딩 관련 이미지 생성 유스케이스 발견 |
| **WebSocket 스트리밍** | 주요 프로바이더(OpenAI, Anthropic, Google) 모두 SSE 지원. SSE로 충분한데 WebSocket 추가 시 이중 유지보수만 발생. | 주요 프로바이더가 SSE废弃宣告 시 |
| **Claude Code 스텔스** | `sk-ant-oat*` 토큰은 Anthropic API 약관 위반 소지. OAuth + 동적 헤더 패턴도 법적 검토 필요. | Anthropic 공식 지원 또는 법적 검토 완료 시 |

이미지 생성이 정말 필요하다면, `Provider` 트레이트 확장이 아닌 **독립 AgentTool**(`oxi-agent/src/tools/image_gen.rs`)로 구현할 것을 권장. 내부적으로 OpenAI 이미지 API를 직접 호출하고, 코딩 에이전트 워크플로우에 통합.

---

## 6. 성공 기준

| 기준 | 현재 | 목표 | 상태 |
|------|------|------|------|
| 모델 수 | 544개 | 850개+ | 🔴 Phase 1 필요 |
| 프로바이더 | 47개 | 50개+ | ✅ |
| API 프로토콜 | 8개 | 8개 (충분) | ✅ |
| CompatSettings | ✅ 완전 구현 | 유지 | ✅ |
| 메시지 변환 | ✅ 2-pass 완료 | 유지 | ✅ |
| 모델 DB 자동화 | ❌ 없음 | 스크립트 동작 | 🔴 Phase 1 필요 |
| CompatSettings 자동 기본값 | ❌ 없음 | model_registry 규칙 | 🔴 Phase 2 필요 |

---

## 부록 A: BuiltinProvider 목록 (47개)

```
primary:       openai, openai-responses, openai-completions, anthropic,
               google, vertex, mistral, azure, bedrock

open:          groq, cerebras, fireworks, togetherai, deepinfra,
               huggingface, baseten

chinese:       deepseek, zai, zai-coding-global, zai-coding-cn,
               zai-global, zai-cn, xiaomi, minimax, minimax-cn,
               moonshotai, moonshotai-cn

cloud:         azure-cognitive-services, cloudflare,
               cloudflare-ai-gateway, cloudflare-workers-ai,
               google-vertex-anthropic, vercel-ai-gateway

specialized:   codex, copilot, openai-codex, opencode-go,
               kimi-coding, moonshotai, moonshotai-cn, xai

enterprise:    nvidia, llmgateway, gitlab, sap-ai-core, zenmux, kilo

open:          openrouter
```

## 부록 B: API 구현 파일 (8개)

```
oxi-ai/src/providers/openai.rs             ← 35개 (Api::OpenAiCompletions)
oxi-ai/src/providers/openai_responses.rs ←  2개 (Api::OpenAiResponses)
oxi-ai/src/providers/anthropic.rs         ←  4개 (Api::AnthropicMessages)
oxi-ai/src/providers/google.rs            ←  1개 (Api::GoogleGenerativeAi)
oxi-ai/src/providers/vertex.rs            ←  1개 (Api::GoogleVertex)
oxi-ai/src/providers/mistral.rs          ←  1개 (Api::MistralConversations)
oxi-ai/src/providers/azure.rs             ←  1개 (Api::AzureOpenAiResponses)
oxi-ai/src/providers/bedrock.rs           ←  1개 (Api::BedrockConverseStream)
```

## 부록 C: CompatSettings 필드 상세

```rust
pub enum ThinkingFormat {
    OpenAI,           // o1 native format
    OpenRouter,       // OpenRouter-specific
    DeepSeek,         // DeepSeek-R1 format
    Zai,              // Z.AI GLM format
    Qwen,             // Qwen API
    QwenChatTemplate, // Qwen chat template
}

pub enum MaxTokensField {
    MaxCompletionTokens, // OpenAI / Anthropic / Google
    MaxTokens,           // Legacy / 일부 호환 서버
}
```

## 부록 D: 모델 DB 자동화 스크립트 설계

```rust
// scripts/generate-models.rs
//
// 입력: providers.json (provider, base_url, models[{id, name, pricing...}])
// 출력: oxi-ai/src/model_db.rs (Rust source)
//
// Usage: cargo run --bin generate-models

fn main() {
    let providers = load_providers("providers.json");
    let mut rust_code = String::new();

    rust_code.push_str("//! Auto-generated — run `cargo generate-models`\n\n");
    rust_code.push_str("use crate::{Api, InputModality, model_db::ModelEntry};\n\n");

    for provider in providers {
        let snake = to_snake_case(&provider.name);
        rust_code.push_str(&format!(
            "static {}_MODELS: &[ModelEntry] = &[\n",
            snake.to_uppercase()
        ));
        for model in provider.models {
            rust_code.push_str(&format!(
                "    ModelEntry {{ id: \"{id}\", name: \"{name}\", ... }},\n",
            ));
        }
        rust_code.push_str("];\n\n");
    }
}
```

## 부록 E: CompatSettings 자동 설정 규칙

```rust
// model_registry.rs — 프로바이더별 CompatSettings 기본값

fn default_compat_for(provider: &str) -> CompatSettings {
    match provider {
        // OpenAI 계열: thinking_format = OpenAI
        "openai" | "openai-responses" | "openai-completions" => {
            CompatSettings {
                thinking_format: Some(ThinkingFormat::OpenAI),
                max_tokens_field: Some(MaxTokensField::MaxCompletionTokens),
                ..Default::default()
            }
        }
        // OpenRouter: thinking_format = OpenRouter
        "openrouter" => {
            CompatSettings {
                thinking_format: Some(ThinkingFormat::OpenRouter),
                requires_tool_result_name: true,
                ..Default::default()
            }
        }
        // DeepSeek: thinking_format = DeepSeek
        "deepseek" => {
            CompatSettings {
                thinking_format: Some(ThinkingFormat::DeepSeek),
                max_tokens_field: Some(MaxTokensField::MaxTokens),
                ..Default::default()
            }
        }
        // ZAI: thinking_format = Zai
        "zai" | "zai-coding-global" | "zai-coding-cn" |
        "zai-global" | "zai-cn" => {
            CompatSettings {
                thinking_format: Some(ThinkingFormat::Zai),
                ..Default::default()
            }
        }
        // Vertex: max_tokens_field 차이
        "vertex" | "google-vertex-anthropic" => {
            CompatSettings {
                max_tokens_field: Some(MaxTokensField::MaxCompletionTokens),
                ..Default::default()
            }
        }
        _ => CompatSettings::default(),
    }
}
```