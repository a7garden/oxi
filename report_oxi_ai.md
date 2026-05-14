# oxi-ai 크레이트 심층 분석 보고서

**분석 일자:** 2026-05-14  
**분석 대상:** `/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/src/` (총 28,183줄, 39개 .rs 파일)  
**분석자:** AI Code Review Agent

---

## 목차

1. [Provider 트레잇 설계 및 구현 품질](#1-provider-트레잇-설계-및-구현-품질)
2. [스트리밍 아키텍처](#2-스트리밍-아키텍처)
3. [에러 처리 패턴](#3-에러-처리-패턴)
4. [타입 시스템 설계](#4-타입-시스템-설계)
5. [모델 레지스트리 정확성](#5-모델-레지스트리-정확성)
6. [컨텍스트 관리 및 Compaction 로직](#6-컨텍스트-관리-및-compaction-로직)
7. [프로바이더 간 메시지 변환](#7-프로바이더-간-메시지-변환)
8. [프로바이더 간 코드 중복](#8-프로바이더-간-코드-중복)
9. [API 키 / 보안 처리](#9-api-키--보안-처리)
10. [토큰 추정 정확도 및 엣지 케이스](#10-토큰-추정-정확도-및-엣지-케이스)

---

## 1. Provider 트레잉 설계 및 구현 품질

### 1.1 트레잇 정의 (`providers/trait_def.rs`)

```rust
// trait_def.rs:14-20
pub trait Provider: Send + Sync + 'static {
    async fn stream(...) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;
    fn name(&self) -> &str;
}
```

**평가:** 트레잇이 매우 단순하여 구현이 쉽지만, 기능이 제한적입니다.

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Medium** | 트레잇에 `health_check()`, `supported_models()`, `capabilities()` 같은 메서드가 없어 런타임에 프로바이더 기능을 검색할 방법이 없음 | `trait_def.rs:14` | `Provider` 트레잇에 `fn capabilities(&self) -> ProviderCapabilities` 추가. `ProviderCapabilities`는 reasoning, vision, tool_calling, caching 등 플래그를 포함 |
| **Medium** | `name()`만 유일한 non-async 메서드. 버전, 레이트 리밋 등 메타데이터 접근 불가 | `trait_def.rs:20` | `fn metadata(&self) -> ProviderMetadata` 추가 |
| **Low** | `Send + Sync + 'static` 바운드가 명시적이지만, 트레잇에 `#[async_trait]`만 사용 | `trait_def.rs:14` | Rust 1.75+에서는 `async fn in trait` 고려 |

### 1.2 프로바이더 구현 패턴

13개 프로바이더가 구현되어 있으며, 크게 4가지 패턴으로 분류됩니다:

| 패턴 | 프로바이더 | 특징 |
|------|-----------|------|
| 독립 SSE 파싱 | Anthropic, OpenAI, OpenAI Responses, Bedrock | 자체 이벤트 구조체 + 파싱 로직 |
| OpenAI 호환 | DeepSeek, Mistral, Azure, Cloudflare, Copilot, Codex | OpenAI SSEChunk 구조체 재정의 |
| Google 공유 | Google, Vertex | `google_shared.rs` 모듈 공유 |
| 래핑 | OpenAI Completions (Legacy) | 별도 엔드포인트 |

---

## 2. 스트리밍 아키텍처

### 2.1 SSE 파싱

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Critical** | **UTF-8 청크 경계 처리 불일치** — OpenAI 프로바이더(`openai.rs:166-175`)는 `split_complete_lines()`로 UTF-8 안전 처리를 하지만, 다른 모든 프로바이더(DeepSeek, Mistral, Azure, Cloudflare, Copilot, Codex, Google, Vertex, Bedrock)는 `String::from_utf8_lossy()`를 사용하여 유효하지 않은 UTF-8 문자를 `�`로 대체합니다. 실제 멀티바이트 문자(한국어, CJK 등)가 HTTP 청크 경계에서 분할되면 문자 손상이 발생할 수 있습니다. | `deepseek.rs:258`, `mistral.rs:263`, `azure.rs:262`, `cloudflare.rs:258`, `copilot.rs:255`, `codex.rs:255`, `google.rs:100`, `vertex.rs:123`, `bedrock.rs:265` | OpenAI 프로바이더의 `split_complete_lines()` + `find_valid_utf8_prefix()` 패턴을 공유 유틸리티로 추출하여 모든 프로바이더가 사용하도록 통일 |
| **High** | **OpenAI Responses API 파서에 UTF-8 경계 처리 완전 누락** — `openai_responses.rs`는 `String::from_utf8_lossy()`만 사용하며, JSON 객체가 청크 경계에서 분할될 때 파싱 실패 가능성이 있습니다. | `openai_responses.rs:226` | `bytes_stream().scan()` 패턴을 사용하여 상태를 유지하며 청크를 버퍼링하는 방식으로 변경 (openai.rs 패턴 참조) |
| **High** | **Anthropic 프로바이저에도 UTF-8 경계 처리 누락** — `anthropic.rs:179`에서 `String::from_utf8_lossy(&bytes)` 사용. 한국어 thinking 블록이나 CJK 텍스트 처리 시 문제 가능. | `anthropic.rs:179` | 동일하게 `split_complete_lines()` 패턴 적용 |

### 2.2 백프레셔 (Backpressure)

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **모든 프로바이더에 백프레셔 메커니즘 부재** — `response.bytes_stream().flat_map(...)` 패턴은 데이터가 들어오는 대로 즉시 `ProviderEvent` 벡터를 생성합니다. 소비자(예: UI 렌더링)가 느린 경우 메모리에 무제한 이벤트가 버퍼링됩니다. `futures::stream::iter()`는 동기적이므로 내부 버퍼링 제어가 불가합니다. | 모든 프로바이더의 `stream()` 구현 | `flat_map` 대신 `scan` + async 채널(`tokio::sync::mpsc`) 패턴 도입 고려. 또는 `stream::buffer_unordered()` 대신 `stream::ready_chunks(1)` 사용 |
| **Medium** | OpenAI 프로바이더의 `scan()` 패턴(`openai.rs:166`)은 상태를 `HashMap`에 저장하는데, 툴 콜이 많을 경우 메모리 선형 증가 | `openai.rs:166-235` | 툴 콜 완료 후 즉시 `pending_tc`에서 제거하는 로직 추가 (현재는 Done 이벤트에서만 `clear()` 호출) |

### 2.3 이벤트 타입 설계

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Medium** | `ProviderEvent`의 모든 이벤트 변형이 `partial: AssistantMessage`를 포함하여 크기가 매우 큼. `AssistantMessage`는 `Vec<ContentBlock>` 등을 포함하므로 각 이벤트마다 전체 메시지가 복제됩니다. 100개의 TextDelta 이벤트가 발생하면 100번의 전체 메시지 클론이 발생. | `event.rs:15-95` | `partial` 대신 `Rc<RefCell<AssistantMessage>>` 또는 `Arc<Mutex<AssistantMessage>>`를 사용한 공유 참조 고려. 또는 `partial`을 선택적(Optional)으로 변경 |
| **Low** | `TextEnd`, `ThinkingEnd` 이벤트에 `content: String` 필드가 있는데, 이미 이전 델타에서 누적했으므로 중복 데이터 | `event.rs:62,82` | `content` 필드를 `Option<String>`으로 변경하거나 제거하고, 소비자가 직접 누적하도록 유도 |

---

## 3. 에러 처리 패턴

### 3.1 에러 타입 계층

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **`ProviderError`에 재시도 관련 정보가 없음** — HTTP 429(Rate Limit), 503(Service Unavailable) 등 재시도 가능한 에러와 401(Unauthorized), 400(Bad Request) 등 재시도 불가능한 에러를 구분할 수 없습니다. | `error.rs:14-50` | `ProviderError::RateLimited { retry_after: Option<Duration> }`, `ProviderError::ServiceUnavailable` 변형 추가. 또는 `is_retryable(&self) -> bool` 메서드 추가 |
| **High** | **`CompactionError`가 `std::error::Error`를 구현하지만 `Error` 트레잇 계층에 통합되지 않음** — `Error` enum은 `Provider`, `Validation`, `Io`만 래핑하며, `CompactionError`는 별개의 타입으로 존재합니다. 이로 인해 `complete()` 함수에서 compaction 에러를 처리할 수 없습니다. | `compaction.rs:244-280`, `error.rs:55-63` | `Error` enum에 `Compaction(CompactionError)` 변형을 추가하거나, `CompactionError`를 `ProviderError`의 하위 변형으로 통합 |
| **Medium** | **에러 메시지에 한국어 주석과 영어 메시지가 혼재** — `error.rs`의 `#[error("...")]` 메시지는 영어이지만 doc comment(`///`)는 한국어입니다. 일관성 부족. | `error.rs:8-50` | 모든 사용자 대면 메시지를 영어로 통일 (이미 `#[error(...)]`는 영어이므로 doc comment도 영어로 변경) |
| **Medium** | **`ValidationError`가 두 곳에 정의됨** — `error.rs:40-50`와 `tools.rs:95-107`에 동일한 이름의 `ValidationError` enum이 존재합니다. | `error.rs:40`, `tools.rs:95` | `tools::ValidationError`를 제거하고 `error::ValidationError`만 사용하도록 통합. 또는 각각을 `ToolValidationError`, `DataValidationError`로 명확히 구분 |

### 3.2 에러 복구

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **스트림 중간에 에러 발생 시 부분 결과 손실** — 모든 프로바이더가 `bytes_stream().flat_map()` 패턴을 사용하는데, 에러 발생 시 `ProviderEvent::Error`만 반환하고 이전에 누적된 텍스트/툴콜이 손실됩니다. | 모든 프로바이더 | 에러 발생 시 이전까지 누적된 `partial` 메시지를 `ProviderEvent::Error`에 포함시키는 방식 고려. 이미 `error: AssistantMessage` 필드가 있으나 빈 메시지를 생성(`create_error_message()`) |
| **Medium** | Bedrock 프로바이더의 `get_token_from_service_account()`가 `sleep(55분)`을 호출(`vertex.rs:97-99`). 이는 테스트/초기 호출 시 불필요한 대기 시간을 유발합니다. | `vertex.rs:97-99` | 토큰 캐싱 메커니즘을 도입하고, `sleep`은 토큰 갱신 루프에서만 수행. 또한 이 로직이 `stream()` 호출 경로에 있으므로, 백그라운드 태스크로 분리 |

---

## 4. 타입 시스템 설계

### 4.1 메시지 타입

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **`ContentBlock`의 `#[serde(untagged)]` 역직렬화가 불안정** — `TextContent`(`type: "text"`), `ThinkingContent`(`type: "thinking"`), `ImageContent`(`type: "image"`), `ToolCall`(`type: "toolCall"`)이 모두 `type` 필드로 구분되는데, `Unknown(JsonValue)`이 마지막 catch-all이므로 예상치 못한 JSON 객체가 `Unknown`으로 처리됩니다. 또한 `text_signature`, `thinking_signature`, `thought_signature` 등 선택적 필드가 `untagged` 매칭에 영향을 줄 수 있습니다. | `messages.rs:146-155` | `#[serde(tag = "type")]` (internally tagged) 방식으로 변경. 또는 `ContentBlock`에 커스텀 `Deserialize` 구현 |
| **Medium** | **`MessageContent::Text(String)` vs `MessageContent::Blocks(Vec<ContentBlock>)`의 이원화** — 사용자 메시지가 `Text("hello")`인지 `Blocks([Text("hello")])`인지에 따라 동작이 달라집니다. 모든 프로바이더의 `build_messages()`에서 이 분기를 처리해야 합니다. | `messages.rs:576-584` | `MessageContent`를 항상 `Vec<ContentBlock>`으로 통일하고, `From<&str>`이 단일 텍스트 블록 벡터를 생성하도록 변경 |
| **Medium** | **`Message::assistant()` 컨벤언스 생성자가 하드코딩된 `Api::AnthropicMessages` 사용** — 이 생성자로 만든 메시지는 항상 Anthropic API로 태그되어 다른 프로바이더로 전송 시 잘못된 메타데이터를 가집니다. | `messages.rs:458-471` | `api` 파라미터를 받도록 변경하거나, `Api::Unknown` 같은 중립적 기본값 사용 |
| **Low** | `UserRole`, `AssistantRole`, `ToolResultRole` 등이 각각 단일 변형 enum으로 정의되어 있음. `Message` enum이 `#[serde(tag = "role")]`으로 이미 역할을 구분하므로 중복적 | `messages.rs` 여러 위치 | 역할 enum을 단순 문자열 상수로 대체하거나, `#[serde(tag = "role")]`에만 의존 |

### 4.2 `Usage::calculate_cost()`의 정확성

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **`Usage::calculate_cost()`가 모델별 가격이 아닌 고정 비율($1/1M 토큰)을 사용** — 이 메서드는 토큰 수를 단순히 1백만으로 나누어 비용을 계산합니다. 실제 모델 가격(예: GPT-4o $2.5/1M input, $10/1M output)을 전혀 반영하지 않습니다. | `types.rs:144-150` | `calculate_cost(&self, model: &Model)` 또는 `calculate_cost(&self, cost: &Cost)` 시그니처로 변경하여 모델별 가격을 적용 |
| **Medium** | `Usage.total_tokens`가 `input + output + cache_read + cache_write`로 계산되는데, `cache_read`와 `cache_write`는 이미 `input`에 포함된 경우(OpenAI)와 별개인 경우(Anthropic)가 혼재 | `types.rs:146` | `total_tokens`의 정의를 명확히 하고, 프로바이더별로 다른 계산 방식을 문서화 |

---

## 5. 모델 레지스트리 정확성

### 5.1 하드코딩된 모델 정보

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **정적 레지스트리(`model_registry.rs`)와 모델 DB(`model_db.rs`, 544개 모델)의 중복** — 두 시스템이 독립적으로 모델을 정의하므로 정보 불일치 가능성이 높습니다. `model_registry.rs`는 `HashMap` 기반 동적 레지스트리, `model_db.rs`는 `const` 배열 기반 정적 DB입니다. | `model_registry.rs`, `model_db.rs` | 단일 소스로 통합. `model_db.rs`를 기본으로 하고, `model_registry.rs`는 런타임 오버라이드만 담당 |
| **Medium** | **`model_registry.rs`의 가격 정보가 오래됨** — OpenAI o1/o3 모델의 `cache_write` 비용이 `input_cost * 7.5`로 단순 공식으로 계산됨. 실제 가격은 모델마다 다름. | `model_registry.rs:107-112` | 실제 가격을 하드코딩하거나, 가격 자동 업데이트 메커니즘 도입 |
| **Medium** | **`model_registry.rs`에서 reasoning 모델(GPT-4o 제외)이 vision을 지원하지 않는 것으로 설정** — `input: vec![InputModality::Text]`이지만, 실제로 o1/o3은 이미지 입력을 지원합니다. | `model_registry.rs:107-120` | 최신 모델 스펙에 맞게 `InputModality::Image` 추가 |
| **Medium** | **Anthropic 모델의 `max_tokens`이 8192로 하드코딩** — Claude Sonnet 4, Claude Opus 4는 16,384+ 토큰 출력을 지원합니다. | `model_registry.rs:155` | 모델별로 올바른 `max_tokens` 값 설정 |
| **Low** | Google Gemini 모델이 `reasoning: false`로 설정되어 있지만, Gemini 2.5 Pro/Flash는 thinking을 지원 | `model_registry.rs:196-230` | `reasoning: true` 설정 추가 |

### 5.2 모달리티 플래그

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Medium** | `InputModality`에 `Audio`, `Video` 등이 없음 — Gemini 모델은 오디오/비디오 입력을 지원하지만 현재 타입 시스템에서 표현 불가 | `types.rs:100-106` | `InputModality`에 `Audio`, `Video` 변형 추가 |

---

## 6. 컨텍스트 관리 및 Compaction 로직

### 6.1 Context 구조체

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Medium** | **`Context`에 토큰 카운트 추적 기능이 없음** — 메시지를 추가할 때마다 자동으로 토큰 수를 추적하는 기능이 없어, 매번 전체 컨텍스트를 재추정해야 합니다. | `context.rs` 전체 | `token_count: Option<usize>` 필드를 추가하고, `add_message()` 시 증분 계산 |
| **Medium** | **`Context::clone()`이 수동으로 구현됨** — `#[derive(Clone)]` 대신 `impl Clone`을 수동 구현했는데, `Message` 타입이 이미 `Clone`을 derive하므로 불필요합니다. 또한 `#[derive(Clone)]`과 수동 구현이 충돌할 수 있습니다. | `context.rs:109-115` | `#[derive(Clone)]`을 사용하고 수동 `clone()` 제거 |
| **Low** | `Context`에 `message_count_limit`, `max_tokens` 등 설정 필드가 없음 | `context.rs` | 컨텍스트 제한 설정 필드 추가 |

### 6.2 Compaction

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **`LlmCompactor`의 `_provider` 필드가 사용되지 않음** — `LlmCompactor` 구조체에 `_provider: Arc<dyn Provider>`가 있지만 실제로는 `crate::high_level::complete()` 함수(전역 프로바이더 조회)를 사용합니다. 이로 인해 커스텀 프로바이더를 주입해도 무시됩니다. | `compaction.rs:297-300`, `compaction.rs:355` | `_provider`를 사용하여 직접 `stream()`을 호출하도록 변경. 또는 `_provider` 필드 제거 |
| **Medium** | **`CompactionConfig.max_batch`가 사용되지 않음** — `max_batch: usize` 필드가 설정 가능하지만, `compact()` 구현에서 분할 없이 전체 메시지를 한 번에 요약합니다. | `compaction.rs:274-290` | `max_batch` 설정에 따라 메시지를 배치로 분할하여 순차적 요약 |
| **Medium** | **`build_summarize_prompt()`에서 500자 잘림이 임의적** — 메시지 내용을 500자로 자르는데(`compaction.rs:316`), 코드 스니펫이나 구조화된 데이터의 경우 중요한 내용이 잘릴 수 있습니다. | `compaction.rs:316` | 문장/줄바꿈 단위로 자르거나, 토큰 기반 잘림 사용 |
| **Low** | `summarize_branch()`에서 300자 잘림(`compaction.rs:400`), 일관성을 위해 500자와 통일 필요 | `compaction.rs:400` | 상수로 잘림 길이를 정의하여 재사용 |

---

## 7. 프로바이더 간 메시지 변환

### 7.1 변환 로직

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **`messages.rs:569`의 `transform_for_provider()`와 `transform.rs`의 `transform_messages()`가 동일 기능을 중복 구현** — 두 함수 모두 메시지를 프로바이더에 맞게 변환하지만 구현이 다릅니다. `messages.rs` 버전은 단순하고, `transform.rs` 버전은 중간 표현(Intermediate)을 거칩니다. | `messages.rs:569-580`, `transform.rs:70-85` | `messages.rs`의 `transform_for_provider()`를 제거하고 `transform.rs` 버전으로 통일 |
| **Medium** | **`transform_messages_for_model()`에서 고아 툴 콜 처리가 복잡** — 2-pass 변환(pending_tool_calls 추적 → synthetic tool result 삽입)이 정확하지만, 다중 어시스턴트 메시지가 연속될 때 엣지 케이스가 있을 수 있습니다. | `transform.rs:545-660` | 툴 콜/툴 결과 페어링을 위한 전용 유틸리티 함수로 추출 |
| **Medium** | **`normalize_tool_call_id()`가 두 곳에 정의됨** — `transform.rs:504`와 `google_shared.rs:94`에 서로 다른 구현이 존재합니다. | `transform.rs:504`, `google_shared.rs:94` | 단일 구현으로 통일 |

### 7.2 데이터 손실 가능성

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **OpenAI → Anthropic 변환 시 툴 콜 arguments가 JSON 문자열에서 파싱 실패하면 빈 JSON으로 변환** — `openai.rs`의 툴 콜 델타 누적 로직에서 `parse_streaming_json()`이 실패하면 `{}`을 반환하지만, 원본 문자열이 손실됩니다. | `openai_responses_shared.rs:394-433`, `openai.rs:222` | 파싱 실패 시 원본 문자열을 보존하는 fallback 메커니즘 추가 |
| **Medium** | **Anthropic → OpenAI 변환 시 `thinking_signature` 손실** — `transform.rs`에서 thinking을 `<thinking>` 태그로 감싸지만, `thinking_signature`는 버려집니다. 이후 Anthropic으로 다시 변환할 때 서명이 복원되지 않습니다. | `transform.rs:336-345` | `TextContent.text_signature`에 thinking_signature를 인코딩하여 보존 |
| **Medium** | **`blocks_to_content()` 호출 시 Image 블록이 data URL로 변환되지만, 프로바이더에 따라 이미지를 지원하지 않을 수 있음** — DeepSeek, Mistral, Azure 등의 `build_messages()`에서 이미지 블록을 처리하려 하지만, 이 프로바이더들은 실제로 이미지 입력을 지원하지 않을 수 있습니다. | `deepseek.rs:187`, `mistral.rs:197`, `azure.rs:236` | 프로바이더 생성자에서 이미지 지원 여부를 명시하고, `build_messages()`에서 이미지 블록을 건너뛰거나 텍스트로 대체 |

---

## 8. 프로바이더 간 코드 중복

이것은 전체 크레이트에서 가장 심각한 구조적 문제입니다.

### 8.1 SSE 파싱 코드 중복

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Critical** | **SSEChunk/Choice/Delta/ToolCallDelta/FunctionDelta/UsageInfo/PromptTokensDetails 구조체가 8개 프로바이더에 걸쳐 거의 동일하게 중복 정의됨** — DeepSeek, Mistral, Azure, Cloudflare, Copilot, Codex가 각각 자체 SSE 파싱 구조체를 정의합니다. | `deepseek.rs:300-365`, `mistral.rs:415-480`, `azure.rs:395-460`, `cloudflare.rs:415-475`, `copilot.rs:380-435`, `codex.rs:385-440` | **OpenAI 호환 SSE 파싱 모듈(`openai_compat_sse.rs`) 생성**. 모든 OpenAI 호환 프로바이더가 이 모듈을 사용하도록 리팩터링 |
| **Critical** | **`build_messages()`, `blocks_to_content()`, `build_tools()` 함수가 모든 OpenAI 호환 프로바이더에 중복** — 각 프로바이더가 동일한 로직을 약간의 변형만으로 재구현합니다. | `deepseek.rs:155-230`, `mistral.rs:175-255`, `azure.rs:185-260`, `cloudflare.rs:185-260`, `copilot.rs:185-260`, `codex.rs:155-210` | **공유 `openai_compat_messages.rs` 모듈 생성**. `build_messages()`, `blocks_to_content()`, `build_tools()`를 한 곳에서 정의 |
| **Critical** | **`create_error_message()` 함수가 모든 프로바이더에 중복 정의** — 11개 프로바이더에 각각 동일한 시그니처의 함수가 존재합니다. | 거의 모든 프로바이더 파일 | `ProviderEvent`에 `Error` 생성 메서드를 추가하거나, 공통 유틸리티 함수로 추출 |

### 8.2 중복 정도 정량화

| 코드 블록 | 중복 횟수 | 총 중복 줄 수 (추정) |
|-----------|----------|---------------------|
| SSEChunk 등 구조체 | 8회 | ~400줄 |
| build_messages() | 8회 | ~500줄 |
| blocks_to_content() | 8회 | ~300줄 |
| build_tools() | 8회 | ~100줄 |
| parse_sse_events() | 8회 | ~600줄 |
| create_error_message() | 11회 | ~80줄 |
| API 키 조회 + 헤더 빌드 | 11회 | ~300줄 |
| **총계** | | **~2,280줄** (전체 코드의 ~8%) |

**개선 제안:** 다음 구조로 리팩터링:

```
providers/
  openai_compat/
    mod.rs          — 공유 트레잇 + 기본 구현
    sse.rs          — 공유 SSE 파싱
    messages.rs     — 공유 메시지 빌딩
    tools.rs        — 공유 툴 빌딩
```

---

## 9. API 키 / 보안 처리

### 9.1 Secret<T> 래퍼

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **`Secret<String>`의 `Serialize` 구현이 실제 값을 노출** — `secret.rs:72-75`에서 `s.serialize_str(&self.inner)`로 평문을 JSON에 씁니다. `Debug`는 마스킹하지만, `Serialize`는 마스킹하지 않습니다. 디스크에 저장하거나 로그에 출력할 때 API 키가 노출될 수 있습니다. | `secret.rs:72-75` | `Serialize` 구현에서 `[REDACTED]`를 출력하거나, `#[serde(skip_serializing)]` 속성 사용. 실제 값은 `expose()`로만 접근 가능하게 유지 |
| **Medium** | **`StreamOptions`에 `api_key: Option<String>`이 일반 텍스트로 저장** — `Secret<String>` 래퍼를 사용하지 않아, `StreamOptions`의 `Debug` 출력에 API 키가 노출될 수 있습니다. | `options.rs:15` | `api_key: Option<Secret<String>>`으로 변경 |
| **Medium** | **모든 프로바이더가 API 키를 `Option<String>`으로 보관** — `AnthropicProvider.api_key: Option<String>`, `OpenAiProvider.api_key: Option<String>` 등. `Secret<T>` 타입이 이미 정의되어 있음에도 사용하지 않습니다. | 모든 프로바이더의 구조체 정의 | 모든 `api_key: Option<String>`을 `api_key: Option<Secret<String>>`으로 변경 |
| **Low** | `Secret<String>`의 `Display` 구현이 8자 미만 문자열을 `[REDACTED]`로 표시하지만, 8자 이상은 첫 4자/끝 4자를 노출 | `secret.rs:56-63` | 프로덕션에서는 `[REDACTED]`만 표시하도록 변경 (디버깅 시에만 expose 사용) |

### 9.2 Bedrock 인증

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Critical** | **Bedrock SigV4 서명에 취약한 `simple_hash()` 사용** — `openai_responses_shared.rs:89-98`의 `simple_hash()`는 XOR 기반의 비암호화적 해시입니다. 도구 호출 ID 생성에만 사용되므로 직접적인 보안 위험은 아니지만, 식별자 충돌 가능성이 있습니다. | `openai_responses_shared.rs:89-98` | 식별자 생성에 SipHash 또는 FxHash 사용. 또는 UUID 생성 |
| **High** | **Vertex 프로바이더의 서비스 계정 개인 키가 환경 변수로 전달되는 파일 경로에서 읽혀짐** — `vertex.rs:87`에서 `fs::read_to_string(credentials_path)`로 PEM 키를 읽습니다. 파일 권한 검사가 없습니다. | `vertex.rs:82-88` | 파일 읽기 전에 권한이 600인지 확인하는 검증 추가 |

---

## 10. 토큰 추정 정확도 및 엣지 케이스

### 10.1 하이브리드 토큰 추정기

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **High** | **토큰 추정이 한국어, 일본어 등 비CJK 비알파벳 문자를 제대로 처리하지 못함** — `is_cjk()`는 CJK 통합 한자, 히라가나, 가타카나, 한글을 감지하지만, 라틴 확장 문자(악센트 포함 문자), 아랍어, 힌디어, 태국어 등은 `ascii_or_latin_chars`로 분류되어 4자/토큰 비율이 적용됩니다. 실제로 이런 문자들은 BPE 토크나이저에서 더 짧은 토큰으로 분할됩니다. | `high_level.rs:140-175` | Unicode 블록 기반 분류를 확장하여 라틴 확장, 아랍어, 데바나가리 등 추가. 또는 `char.len_utf8()` 기반 휴리스틱 사용 |
| **Medium** | **`estimate_words()`가 `estimate()`와 다른 결과를 반환** — 두 함수가 서로 다른 알고리즘을 사용하며, API 사용자가 어느 것을 사용해야 할지 불명확합니다. | `high_level.rs:186-192` | `estimate_words()`를 deprecated로 표시하고 `estimate()`로 통일 |
| **Medium** | **구두점 토큰 비율이 1.5 토큰/문자로 과대 추정** — `(punct_chars * 3 + 1) / 2` 공식은 `{`, `}`, `:` 같은 JSON 문자에 대해 너무 높은 토큰 수를 반환합니다. 실제로 이런 문자들은 보통 다른 토큰과 병합됩니다. | `high_level.rs:167` | JSON/코드 컨텍스트에서 구두점 비율을 1.0 토큰/2문자로 조정 |
| **Low** | 공백 토큰이 `whitespace_words / 8`로 계산되는데, 이는 공백당 0.125 토큰으로 매우 낮은 기여도 | `high_level.rs:169` | BPE 오버헤드를 더 잘 반영하도록 `whitespace_words / 4` 또는 적절한 값으로 조정 |

### 10.2 컨텍스트 사용량 계산

| 심각도 | 이슈 | 파일:라인 | 개선 제안 |
|--------|------|-----------|-----------|
| **Medium** | **`context_usage()`가 텍스트만 추정하고 도구 정의, 시스템 프롬프트를 무시** — 실제 컨텍스트 사용량에는 도구 스키마, 시스템 프롬프트, 메시지 메타데이터가 포함되지만, 이 함수는 텍스트 토큰만 추정합니다. | `high_level.rs:196-201` | `context_usage(messages, tools, system_prompt, context_window)` 시그니처로 변경하여 전체 컨텍스트를 추정 |

---

## 부록: 심각도별 요약

### Critical (4개)
1. **UTF-8 청크 경계 처리 불일치** — 9개 프로바이더에서 멀티바이트 문자 손상 가능 (`deepseek.rs:258`, `mistral.rs:263`, `azure.rs:262`, `cloudflare.rs:258`, `copilot.rs:255`, `codex.rs:255`, `google.rs:100`, `vertex.rs:123`, `bedrock.rs:265`)
2. **SSE 파싱 코드 대규모 중복** — 8개 프로바이더에 ~400줄 중복 (모든 OpenAI 호환 프로바이더)
3. **메시지 빌딩/툴 빌딩 코드 대규모 중복** — 8개 프로바이더에 ~900줄 중복
4. **Bedrock SigV4 관련 취약 해시** — `openai_responses_shared.rs:89-98`

### High (10개)
1. 백프레셔 메커니즘 부재 (모든 프로바이더)
2. `ProviderError`에 재시도 정보 부재 (`error.rs:14-50`)
3. `CompactionError`가 메인 에러 계층에 통합되지 않음 (`compaction.rs:244`, `error.rs:55`)
4. 스트림 에러 시 부분 결과 손실 (모든 프로바이더)
5. `Usage::calculate_cost()`가 모델별 가격을 반영하지 않음 (`types.rs:144-150`)
6. 정적/동적 모델 레지스트리 중복 (`model_registry.rs`, `model_db.rs`)
7. `LlmCompactor`의 `_provider` 필드 미사용 (`compaction.rs:297`)
8. `transform_for_provider()` 중복 구현 (`messages.rs:569`, `transform.rs:70`)
9. 툴 콜 arguments JSON 파싱 실패 시 데이터 손실 (`openai_responses_shared.rs:394`)
10. 토큰 추정기의 비CJK 비알파벳 문자 처리 부족 (`high_level.rs:140-175`)
11. `Secret<String>`의 Serialize가 평문 노출 (`secret.rs:72-75`)
12. Vertex 서비스 계정 키 파일 권한 검사 부재 (`vertex.rs:82-88`)
13. OpenAI Responses API 파서 UTF-8 경계 처리 누락 (`openai_responses.rs:226`)
14. Anthropic 프로바이더 UTF-8 경계 처리 누락 (`anthropic.rs:179`)

### Medium (20개)
1. `Provider` 트레잇에 capabilities 메서드 부재 (`trait_def.rs:14`)
2. `ProviderEvent`의 과도한 `partial` 클론 (`event.rs:15-95`)
3. `MessageContent` 이원화 (`messages.rs:576-584`)
4. `Message::assistant()` 하드코딩된 API 타입 (`messages.rs:458-471`)
5. `ValidationError` 중복 정의 (`error.rs:40`, `tools.rs:95`)
6. `Context`에 토큰 카운트 추적 부재 (`context.rs`)
7. `Context::clone()` 수동 구현 (`context.rs:109-115`)
8. `CompactionConfig.max_batch` 미사용 (`compaction.rs:274`)
9. `build_summarize_prompt()` 임의 500자 잘림 (`compaction.rs:316`)
10. 모델 레지스트리 가격 정보 오래됨 (`model_registry.rs:107-112`)
11. Anthropic 모델 max_tokens 부정확 (`model_registry.rs:155`)
12. `InputModality`에 Audio/Video 부재 (`types.rs:100-106`)
13. `normalize_tool_call_id()` 중복 정의 (`transform.rs:504`, `google_shared.rs:94`)
14. thinking_signature 변환 시 손실 (`transform.rs:336-345`)
15. 비전 미지원 프로바이더의 이미지 블록 처리 (`deepseek.rs:187`, `mistral.rs:197`)
16. `StreamOptions.api_key`가 일반 텍스트 (`options.rs:15`)
17. 모든 프로바이더의 API 키가 `Option<String>` (`모든 프로바이더`)
18. 토큰 추정 구두점 비율 과대 (`high_level.rs:167`)
19. `context_usage()`가 도구/시스템 프롬프트를 무시 (`high_level.rs:196`)
20. 에러 메시지에 한국어/영어 혼재 (`error.rs`)

### Low (7개)
1. `TextEnd`/`ThinkingEnd` 중복 `content` 필드 (`event.rs:62,82`)
2. 역할 enum 과도한 정의 (`messages.rs` 여러 위치)
3. `summarize_branch()` 잘림 길이 불일치 (`compaction.rs:400`)
4. Google 모델 reasoning 플래그 부정확 (`model_registry.rs:196-230`)
5. `estimate_words()`와 `estimate()` 혼재 (`high_level.rs:186`)
6. 공백 토큰 기여도 과소 (`high_level.rs:169`)
7. `Secret` Display가 8자 이상에서 일부 노출 (`secret.rs:56-63`)

---

## 결론 및 우선순위 제안

### 즉각 개선 (P0)
1. **UTF-8 청크 경계 처리 통일** — 모든 프로바이더에 `split_complete_lines()` 적용
2. **OpenAI 호환 프로바이더 코드 중복 제거** — 공유 모듈로 리팩터링

### 단기 개선 (P1)
3. `ProviderError`에 재시도 가능 여부 및 `RateLimited` 변형 추가
4. `Secret<T>`를 모든 API 키 필드에 적용
5. `Usage::calculate_cost()`에 모델별 가격 적용
6. `ProviderEvent`의 `partial` 클론 최적화

### 중기 개선 (P2)
7. 모델 레지스트리 단일 소스로 통합
8. 토큰 추정기 정확도 개선 (비알파벳 문자 지원)
9. 백프레셔 메커니즘 도입
10. `Context`에 증분 토큰 카운트 추가

---

*이 보고서는 oxi-ai 크레이트의 정적 코드 분석을 기반으로 작성되었습니다. 런타임 동작이나 성능 프로파일링은 포함하지 않습니다.*
