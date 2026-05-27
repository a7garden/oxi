# RFC-002 코드베이스 검증 리뷰 & 개선안

**검토일**: 2026-05-27  
**대상**: RFC-002 (AI Provider 커버리지 확대)  
**방법**: RFC의 모든 주장을 실제 코드베이스와 교차 검증  

---

## 1. 정확한 정보 (RFC ≈ 코드)

| 항목 | RFC 주장 | 실제 코드 | 판정 |
|------|---------|----------|------|
| 모델 수 | 546개 | `ModelEntry {` = 546개. 단, 파일 주석은 "544 models" | ⚠️ 사소한 불일치 |
| 프로바이더 | 47개 | `BuiltinProvider {` = 47개 | ✅ 정확 |
| API 프로토콜 | 8개 | `Api` enum에 8 variant 존재 | ✅ 정확 |
| transform.rs | 1252줄 | 정확히 1252줄 | ✅ 정확 |
| 2-pass 파이프라인 | downgrade → normalize → synthetic | 코드에 모두 존재 | ✅ 정확 |
| Data-driven factory | `register_builtins.rs` 패턴 | 완전히 일치 | ✅ 정확 |
| `images.rs` 미구현 | 존재하지 않음 | 파일 없음 확인 | ✅ 정확 |
| WebSocket 미지원 | SSE만 지원 | 코드에 WebSocket 관련 코드 전무 | ✅ 정확 |
| Claude Code 스텔스 미구현 | OAuth + 동적 헤더 없음 | `sk-ant-oat` 등 관련 코드 전무 | ✅ 정확 |
| `generate-models.rs` | "이미 존재 가능성 — 확인 필요" | 파일 시스템에 존재하지 않음 | ✅ 확인 완료 |

---

## 2. 잘못된 정보 (RFC ≠ 코드)

### 2.1 ❌ `BuiltinProvider` 필드 설명이 부정확

RFC §3.1에 제시된 `BuiltinProvider` 구조체:

```rust
// RFC에 나온 버전 (불완전)
struct BuiltinProvider {
    pub name: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub api: Api,
    pub env_key: &'static str,
    pub base_url: &'static str,
    pub auth_method: AuthMethod,
    pub extra_headers: &'static [(&'static str, &'static str)],
    pub category: &'static str,
    // ...
}
```

**실제 구조체** (누락된 필드가 4개 있음):

```rust
pub struct BuiltinProvider {
    pub name: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub api: Api,
    pub env_key: &'static str,
    pub extra_env_keys: &'static [&'static str],     // ← RFC에 누락
    pub base_url: &'static str,
    pub default_enabled: bool,                        // ← RFC에 누락
    pub auth_method: AuthMethod,
    pub extra_headers: &'static [(&'static str, &'static str)],
    pub category: &'static str,
    pub description: &'static str,                    // ← RFC에 누락
}
```

**개선**: RFC의 `BuiltinProvider` 예시 코드를 실제 코드와 일치시킬 것.

### 2.2 ❌ CompatSettings — 이미 존재하며 RFC보다 훨씬 풍부

RFC §3.1 "enhancement 기회"에서 제안:

```rust
// RFC가 "새로 추가해야 한다"고 주장
- supports_store: bool
- supports_developer_role: bool
- max_tokens_field: MaxTokensField
- thinking_format: ThinkingFormat
- cache_control_format: CacheControlFormat
```

**실제 코드** (`types.rs` `CompatSettings`):

```rust
pub struct CompatSettings {
    pub supports_store: bool,                    // ✅ 이미 있음
    pub supports_developer_role: bool,           // ✅ 이미 있음
    pub supports_reasoning_effort: bool,         // ✅ RFC에 언급 안 됨
    pub supports_usage_in_streaming: bool,       // ✅ RFC에 언급 안 됨
    pub max_tokens_field: Option<MaxTokensField>,// ✅ 이미 있음
    pub requires_tool_result_name: bool,         // ✅ RFC에 언급 안 됨
    pub requires_assistant_after_tool_result: bool,// ✅ RFC에 언급 안 됨
    pub requires_thinking_as_text: bool,         // ✅ RFC에 언급 안 됨
    pub thinking_format: Option<ThinkingFormat>, // ✅ 이미 있음
}
```

그리고 `Model` 타입에 이미 `compat: Option<CompatSettings>` 필드가 존재.

**판정**: RFC가 "enhancement 기회"라고 한 것은 이미 **완전히 구현**되어 있음. `cache_control_format`만 실제로 없다. RFC는 이미 구현된 것을 구현 예정인 것처럼 서술하고 있음.

### 2.3 ❌ 프로바이더별 API 매핑 숫자 오류

RFC 부록 B:
```
openai.rs          ← 38 providers
anthropic.rs       ← 2 providers
```

**실제 계산** (API variant별):

| API variant | 프로바이더 수 | RFC 주장 |
|---|---|---|
| `OpenAiCompletions` | 37개 | 38개 |
| `OpenAiResponses` | 2개 (openai-responses, openai-codex) | 2개 |
| `AnthropicMessages` | 3개 (anthropic, minimax, minimax-cn, google-vertex-anthropic) | 2개 |
| `GoogleGenerativeAi` | 1개 | 1개 |
| `GoogleVertex` | 1개 (vertex만) | 3개 |
| `MistralConversations` | 1개 | 1개 |
| `AzureOpenAiResponses` | 1개 (azure만) | 2개 |
| `BedrockConverseStream` | 1개 | 1개 |

상세 검산:
- **OpenAiCompletions**: openai, openai-completions, deepseek, groq, cerebras, xai, openrouter, fireworks, togetherai, deepinfra, cloudflare, copilot, codex, zai, zai-coding-global, zai-coding-cn, zai-global, zai-cn, cloudflare-ai-gateway, cloudflare-workers-ai, huggingface, moonshotai, moonshotai-cn, vercel-ai-gateway, xiaomi, kimi-coding, opencode-go, azure-cognitive-services, baseten, nvidia, llmgateway, gitlab, sap-ai-core, zenmux, kilo = **35개** (not 38)
- **AnthropicMessages**: anthropic, minimax, minimax-cn, google-vertex-anthropic = **4개** (not 2개)
- **GoogleVertex**: vertex = **1개** (not 3개)
- **AzureOpenAiResponses**: azure = **1개** (not 2개)

RFC의 숫자는 대부분 틀렸다.

### 2.4 ❌ 부록 A 프로바이더 목록 불일치

RFC 부록 A가 47개 목록을 제시했으나, 실제 `BUILTIN_PROVIDERS` 정렬 결과와 비교:

RFC에 있는데 코드에 없는 항목: **없음** (모두 존재)
코드에 있는데 RFC에 빠진 항목: **없음**

→ 목록 자체는 정확하나, 카테고리 분류가 RFC 부록에는 없어서 검증 불가.

---

## 3. 과한 설계 (Over-engineering)

### 3.1 🔶 이미지 생성 API — Phase 1 우선순위 재고

RFC는 `ImagesContext`, `ImageSize`, `ImageQuality`, `ImagesResult` 등 완전한 타입 체계와 `Provider` 트레이트 확장(`generate_images`)을 제안한다.

**문제점**:
1. **oxi는 코딩 에이전트**. 이미지 생성은 핵심 기능이 아니다. pi에 있다고 해서 oxi에도 있어야 하는 것은 아님.
2. `Provider` 트레이트에 `generate_images`를 추가하면 **모든 8개 프로토콜 구현체**에 이 메서드를 구현하거나 기본 구현을 제공해야 함. 트레이트 파편화 위험.
3. 실제 사용 사례: DALL-E 3 호출이 코딩 워크플로우에서 얼마나 필요한가?

**개선안**: 
- 이미지 생성을 `Provider` 트레이트 확장이 아닌 **독립 툴**(AgentTool)로 구현할 것.
- `oxi-agent/src/tools/image_gen.rs`로 분리. 내부적으로 OpenAI API를 직접 호출.
- 우선순위를 Phase 1 → **Phase 4 이하**로 강하. 코딩 에이전트 핵심 경로가 아님.

### 3.2 🔶 WebSocket 스트리밍 — Phase 5 회의적

RFC는 Phase 5에서 WebSocket 전송을 제안한다.

**문제점**:
1. **주요 프로바이더 중 WebSocket을 요구하는 곳이 없다.** OpenAI, Anthropic, Google 모두 SSE로 충분.
2. SSE가 이미 작동하는데 WebSocket을 추가하면 **이중 유지보수 비용**만 발생.
3. pi가 WebSocket을 지원한다는 것이 oxi에도 필요하다는 근거가 안 됨.

**개선안**: 
- Phase 5를 **전체 삭제** 또는 "Future consideration"로 강하.
- 실제 프로바이더가 WebSocket을 요구할 때 재검토.

### 3.3 🔶 Claude Code 스텔스 모드 — 가치 제한적

RFC는 OAuth 토큰 감지(`sk-ant-oat*`), GitHub Copilot 헤더, 툴명 매핑 등을 제안.

**문제점**:
1. Claude Code 스텔스 모드는 Anthropic의 **API 약관 위반 소지**가 있는 기능. 공식적으로 지원하는 것이 적절한지 법적 리스크 검토 필요.
2. `AnthropicAuthMode` 4-모드 enum을 도입하는 것은 `AuthMethod`와 중복.
3. 실제 사용자 중 `sk-ant-oat*` 토큰을 직접 사용하는 비율은 극히 낮을 것.

**개선안**: 
- Phase 2를 **보류**로 변경. 법적 검토 선행.
- 필요시 `BuiltinProvider.auth_method`에 `OAuthToken` variant를 추가하는 것만으로 충분.

---

## 4. 누락된 항목 (RFC에서 빠진 것)

### 4.1 🔴 `generate-models.rs`가 존재하지 않음

RFC §3.5에서 "이미 존재 가능성 있음 — 확인 필요"라고 했으나, 실제로는 **존재하지 않음**. 

모델 DB(546개 항목)가 전부 수작성이라는 의미. Phase 3에서 "850+ 모델 목표"를 달성하려면 먼저 codegen 스크립트를 작성해야 함.

**개선안**: Phase 3에 `scripts/generate-models.rs` 작성을 **선행 작업**으로 명시.

### 4.2 🔴 `CompatSettings`가 이미 `Model` 레벨에 존재

RFC §3.1에서 `BuiltinProvider`에 `supports_store`, `thinking_format` 등을 추가하자고 제안했으나, 이 기능들은 이미 `Model.compat: Option<CompatSettings>`에 구현되어 있다.

RFC는 `BuiltinProvider` 수준에서 이를 다시 추가하려고 하는데, 이는 **잘못된 레벨**이다. 호환성 설정은 프로바이더가 아니라 **모델 단위**로 달라야 한다. (예: 같은 OpenAI 호환 프로바이더라도 모델마다 thinking 지원 여부가 다름.)

**개선안**: Phase 4를 전면 수정:
- ~~BuiltinProvider에 필드 추가~~ → **이미 Model.compat으로 충분**
- Phase 4를 "CompatSettings per-model 기본값 자동 설정"으로 변경

### 4.3 🟡 모델 수 불일치 (544 vs 546)

`model_db.rs` 주석은 "544 models"인데, 실제 `ModelEntry {` 카운트는 546개.

**개선안**: 주석 갱신 또는 코드 생성 스크립트로 자동화.

### 4.4 🟡 `azure-cognitive-services`의 API 타입 불일치

코드에서 `azure-cognitive-services`는 `Api::OpenAiCompletions`를 사용하지만, 이름상 Azure 계열인데 `azure`는 `Api::AzureOpenAiResponses`를 사용. RFC에는 이 차이에 대한 설명이 없음.

### 4.5 🟡 `codex` vs `openai-codex` 혼란

코드에 두 개의 codex 관련 프로바이더가 있음:
- `codex` → `Api::OpenAiCompletions` (GitHub Codex)
- `openai-codex` → `Api::OpenAiResponses` (OpenAI Codex)

RFC에서는 "OpenAI Codex Responses: pi에 있음"이라고만 언급하고 이 중복/차이를 설명하지 않음.

---

## 5. 개선안 요약

### Phase 재구성

| 기존 Phase | 개선안 |
|---|---|
| Phase 1: 이미지 생성 (2주) | **보류** → 코딩 에이전트 핵심 기능 아님. 필요시 독립 AgentTool로 구현 |
| Phase 2: Claude Code 스텔스 (1주) | **보류** → 법적 리스크 검토 선행 |
| Phase 3: 모델 DB 확대 (1주) | **Phase 1으로 승격** → 선행 작업: `generate-models.rs` 작성 |
| Phase 4: BuiltinProvider 확장 (1주) | **삭제** → 이미 `Model.compat: CompatSettings`로 구현됨. 대신 "모델별 CompatSettings 기본값 자동 설정"으로 축소 |
| Phase 5: WebSocket (2주) | **삭제** → SSE로 충분. 프로바이더 요구 시 재검토 |

### RFC 본문 수정 사항

1. **§1**: 모델 수 "546" → 파일 주석과 통일 ("544 또는 546, 카운트 방식에 따라")
2. **§3.1**: `BuiltinProvider` 코드 스니펫 → 실제 구조체와 일치시키기 (4개 누락 필드 보충)
3. **§3.1**: "enhancement 기회" → **이미 구현됨**으로 정정. `CompatSettings` 문서로 대체.
4. **§3.2**: "누락 기회" → 실제 코드 재검토. redacted thinking 등 이미 처리됨.
5. **§3.4**: 이미지 생성 → `Provider` 트레이트 확장이 아닌 독립 툴로 재설계
6. **§3.5**: "이미 존재 가능성" → **존재하지 않음**으로 정정
7. **부록 B**: 프로바이더-프로토콜 매핑 숫자 전면 수정
8. **§5 성공 기준**: `BuiltinProvider enhancement` 체크박스 → ✅로 변경 (이미 구현됨)

### 새로 제안하는 Phase

| Phase | 내용 | 기간 |
|---|---|---|
| **Phase 1** | 모델 DB 자동화 (`generate-models.rs` 작성 + 850개 확대) | 2주 |
| **Phase 2** | 프로바이더별 CompatSettings 기본값 자동 설정 (model_db에 compat 필드 자동 부여) | 1주 |
| **Phase 3** | 프로바이더 숫자 정합성 확보 (부록 B 수정, 주석 갱신) | 0.5주 |
| **(보류)** | 이미지 생성 AgentTool | 필요시 |
| **(보류)** | Claude Code 스텔스 | 법적 검토 후 |
| **(보류)** | WebSocket | 프로바이더 요구 시 |

---

## 6. 총평

RFC-002는 문제 정의와 방향성은 맞지만, **이미 구현된 기능을 미구현인 것처럼 서술**하는 치명적인 오류가 있다. 특히 `CompatSettings` / `Model.compat`은 Phase 4의 핵심 제안인데 이미 완전히 구현되어 있다.

반면 **정말 미구현인 것**들(이미지 생성, WebSocket, Claude 스텔스)은 코딩 에이전트 관점에서 우선순위가 낮다. 실제 가치가 높은 것은 **모델 DB 확대**(Phase 3)인데, 이것조차 자동화 스크립트가 없어 수작업에 의존하고 있다.

**핵심 권고**: "pi에 있는 것을 oxi에도 추가하자"가 아니라 **"oxi 사용자에게 실제 가치가 있는 것"**을 기준으로 우선순위를 재설정하라.
