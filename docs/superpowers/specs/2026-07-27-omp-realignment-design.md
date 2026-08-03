# oxicode omp-정렬 Rust-native 리팩토링 — 마스터 설계

- **날짜**: 2026-07-27
- **상태**: 설계 승인 대기 (전략적 선택 확정, 상세 검토 게이트 pending)
- **저자**: a7garden + oxicode agent
- **참조 소스**: omp (oh-my-pi) v17.1.5 @ `/tmp/omp`
- **분석 증거**: `2026-07-27-omp-realignment-analysis.md` (동일 디렉토리)

---

## 1. 문제 정의

oxicode는 현재 **서로 충돌하는 3개의 아키텍처 논제(thesis)가 누적됐지만 한 번도 조율되지 않은 하이브리드**다.

| 층 | 흔적 | 현재 상태 |
|---|---|---|
| **pi 포팅** (원래) | 직접 wiring, `Provider` trait, inline 시스템 프롬프트 | 일부 잔존 |
| **omp 흡수** (중간) | `oxicode-sdk` 15개 port traits, issue 시스템, package manager | omp에 없는 oxicode-original이 핵심 로직에 스며듦 |
| **grok-build TUI** (revert `c37b6a3f`) | clean-room으로 재작성한 `oxicode-tui` v2 | **omp 포팅이 아니라 grok 재해석** |

6개 도메인(Providers, Catalog, Agent, TUI, CLI, Supporting) 병렬 분석 결과, "이상해진" 근본 원인은 이 세 논제의 충돌이다. 상세 발견은 분석 appendix 참조.

### 1.1 사용자가 체감한 "이상함"의 구체적 원인

- **프로바이더**: OMP는 스트리밍 **transport**(per-API `StreamFunction`)와 provider **identity**(`ProviderDefinition` registry)를 분리한다. oxicode는 이 둘을 하나의 `Provider` trait로 융합해, `OpenAiProvider::name()`이 하드코딩 "openai"를 반환하므로 `create_builtin_provider("deepseek").name() == "openai"` 정체성 붕괴가 발생. (`register_builtins.rs:548-551` 단정으로 확인.) SSE 파싱 8중 복제, 미포팅 API dialect 5+(`ollama-chat`, `cursor-agent`, `devin-agent`, `gitlab-duo-agent`, `google-gemini-cli`, `openai-codex-responses`), omp 없는 ~2500줄 발명(MultiProvider/FallbackChain/CircuitBreaker).
- **TUI**: v2는 omp의 tape/scrollback 모델도, 전체 입력 시스템도 없는 grok-inspired 재해석. legacy(74K LOC)와 v2(9.7K LOC)가 교착.
- **프롬프트**: omp는 `.md` 파일, oxicode는 inline Rust 문자열 — 편집/리뷰/유지보수 악화.
- **Catalog**: omp는 별도 패키지(단일 소스), oxicode는 oxicode-ai에 임베디드(의존성 역전).

## 2. 전략적 결정 (사용자 확정)

| 결정 | 선택 | 의미 |
|---|---|---|
| **목표 정체성** | **B: omp-정렬 Rust-native** | 견고한 oxicode-original(port system, LSP)은 유지, 표류한 핵심 층을 omp 설계 의도에 맞게 idiomatic Rust로 재정렬. omp를 기준선, Rust 관용구로 번역. |
| **TUI 전략** | **T1: legacy→omp tape 진화, v2 폐기** | omp tape/native-scrollback을 Rust로. legacy(이미 glyph/mermaid/전체 위젯 보유)를 omp 방향으로 진화, v2는 통합 후 폐기. |
| **issue 시스템** | 유지하되 격리 | agent 루프/session 모델에서 분리, 명시적 boundary 뒤로 |
| **package manager** | omp 플러그인 모델로 재정렬 | omp `extensibility/plugins/`(git caching, install/uninstall)에 맞춤 |
| **language policy** | 제거/단순화 | TUI-only opt-in + 약한 시행 + omp 대응 없음 → 제거 |

## 3. 타겟 아키텍처 (End-state)

### 3.1 크레이트 맵 (재정렬)

```
Leaf 크레이트 (oxicode-* 의존 없음):
  oxicode-catalog   [신규 — oxicode-ai에서 분리]
  oxicode-hashline  [★유지 — 충실한 포팅]
  oxicode-mnemopi   [★유지 — 충실한 포팅]
  oxicode-snapcompact [★유지 — 충실한 포팅]
  oxicode-tui       [= 현 legacy를 omp tape로 진화, v2 폐기]
  oxicode-lsp       [★유지]

의존 흐름:
  oxicode-catalog (leaf)
    ↓
  oxicode-ai  (재정렬: transport/identity 분리, catalog 분리, SSE 중앙화,
           per-provider 에러 계층)
    ↓
  oxicode-agent  (재정렬: owned dialect, intent tracing, approval,
              append-only context, 누락 도구 16개 포팅)
    ← oxicode-hashline
    ↓
  oxicode-sdk  [★유지 — port system, oxios 자매 제품용]
    ← oxicode-snapcompact
    ↓
  oxicode-cli  (재정렬: .md 프롬프트, 명령어 추가, bootstrap/lib 경리,
            issue 격리, pkg→omp 플러그인, language policy 제거)
    ← oxicode-tui (제품에서 widget 도메인 타입 변환)
```

### 3.2 핵심 원칙 (5개)

#### 원칙 1 — Provider를 omp처럼 **세 우려로 분리** (transport / auth-login / metadata)

이것이 사용자의 핵심 pain이자 가장 중요한 재설계. omp 소스 직접 조사로 정제됨 (초기 "둘로 분리" 표현에서 "셋"으로 확정 — advisor 교차 검증 + `registry/types.ts`, `descriptor-types.ts`, `types.ts:8-22` 조사).

현재 oxicode는 하나의 `Provider` trait + `BuiltinProvider` struct가 세 우려를 전부 융합한다:
- **스트리밍 transport**: HTTP/SSE로 토큰을 스트리밍하는 방법 (API dialect별)
- **Auth/login wiring**: env key, OAuth login/refresh, callback port
- **Model/host 메타데이터 + discovery**: base URL, 기본 모델, 모델 발견

OMP는 이 셋을 **엄격히 분리**한다 (omp 소스 검증):
1. **Streaming transport** = per-API `StreamFunction<TApi>` (`streamAnthropic`, `streamOllama`, `streamCursor`, `streamDevin`…), `model.api`로 dispatch. **identity·auth·메타데이터를 전혀 갖지 않는다.** (`packages/ai/src/types.ts:631`)
2. **Auth/login wiring** = `ProviderDefinition` registry (`packages/ai/src/registry/`). 필드: `id`, `name`, `available`, `showInLoginList`, `envKeys`, `login`, `refreshToken`, `getApiKey`, `storeCredentialsAs`, `callbackPort`, `pasteCodeFlow`. **base_url·auth_method·category는 여기 없다.** ~65 엔트리, provider당 1 코드 모듈 → `ALL` 배열 집계 → `PROVIDER_REGISTRY` 단일 소스. **컴파일타임 완전성 검사**: 모든 catalog chat-model provider가 registry에 정의되어야 함 (TypeScript type-level; Rust에서는 test로 복제). (`registry/registry.ts:153-167`, `registry/types.ts:36-56`)
3. **Model/host 메타데이터 + discovery** = `ProviderDescriptor` (`packages/catalog/src/provider-models/`). `createModelManagerOptions(config)`, `catalogDiscovery`, 기본 base URL, 모델 발견. (`descriptor-types.ts`)

**정체성 붕괴의 근본 원인**: 현재 `Provider::name()`이 transport에 붙어 있어, `Api::OpenAiCompletions` arm이 항상 `OpenAiProvider`를 반환하고 그 `name()`은 하드코딩 "openai" → `create_builtin_provider("deepseek").name() == "openai"` (`register_builtins.rs:548-551` 단정).

**핵심 함의**: "trait를 `Api` enum으로 keying"만으로는 이 버그가 **고쳐지지 않는다** — deepseek는 여전히 `OpenAiCompletions` impl로 라우팅되기 때문. 수정은 identity/auth/메타데이터를 streaming trait에서 완전히 빼서 (2)와 (3)으로 옮기는 것뿐이다. transport trait는 어떤 identity도 반환하지 않는다.

**Rust 번역**:
- `Api` enum (transport dialect) = omp `KnownApi` 14개 (아래 P0-C에서 full list + 매핑).
- `ProviderDefinition` registry (oxicode-ai): auth/login wiring만. code-as-data 정적 테이블 (provider당 1 모듈, `phf::Map` 또는 `const` slice 집계). compile-time completeness test로 catalog `KnownProvider` 전부 커버 검증.
- `ProviderDescriptor` (oxicode-catalog): model/host 메타데이터 + discovery. P0-A의 catalog 분리와 함께 이관.
- `stream()` dispatch: `model.api` → `Api` → transport 함수. identity는 `model.provider` → registry 조회.

#### 원칙 2 — `oxicode-catalog`은 별도 크레이트, 단일 소스
- 현: catalog가 oxicode-ai에 임베디드. oxicode-ai가 catalog materialization까지 담당.
- 타겟: omp `packages/catalog`처럼 별도 크레이트. 모델 데이터/identity/family/classify/descriptor가 모두 여기. `oxicode-ai`는 타입과 값을 소비만. 역방향 의존은 cargo로 컴파일 시 자동 금지.

#### 원칙 3 — 시스템 프롬프트는 `.md` 파일 + `include_str!()`
- 현: inline Rust 문자열 (`prompt/system_prompt.rs` 736줄). personality 시스템 없음, tool-specific prompt `.md` 없음, 환경 정보 주입 없음.
- 타겟: omp처럼 `prompts/system/system-prompt.md`, `prompts/system/personalities/*.md`, `prompts/tools/*.md`를 `include_str!()`으로 임베드, 경량 템플릿(`{{date}}`, `{{cwd}}`, `{{git_branch}}` 등)으로 렌더.

#### 원칙 4 — TUI는 omp tape 모델 기반, legacy에서 진화
- 현: `oxicode-tui` v2(grok-inspired, 9.7K LOC) + `oxicode-tui-legacy`(74K LOC) 교착. `LegacyOverlayAdapter` always-dirty.
- 타겟: legacy → `oxicode-tui`로 rename, omp의 3-전략 차등 렌더링(component memo → native scrollback commit → ED3 replay)과 append-only "tape" 계약을 Rust로 추가. v2는 단계적 폐기. glyph 시스템은 legacy 것으로 단일화(doc/code 모순 해소).

#### 원칙 5 — oxicode-original은 명시적 격리 boundary 뒤로
- port system(`oxicode-sdk`), LSP(`oxicode-lsp`), issue 시스템, package manager는 **omp 순수 핵심 로직(agent 루프, session 모델, provider)과 분리**. agent 루프와 session 모델은 omp 아키텍처에 일치.

### 3.3 의존성 방향 규칙 (엄격)
- `oxicode-catalog`은 새 leaf (어떤 oxicode-*에도 의존 안 함).
- `oxicode-ai` → `oxicode-catalog` 단방향. 역방향 의존 절대 금지 → cargo 컴파일로 자동 검증.
- `oxicode-tui`는 leaf (omp `packages/tui`와 동일).
- 순환 의존 절대 생성 금지 (기존 oxicode 규칙 유지).

## 4. 단계 분해 (각 단계 = 별도 spec → plan → 구현)

작업이 단일 구현 계획에 안 들어가 5단계로 분해. **각 단계는 독립 배포 가능**. Phase 0이 최우선 (사용자 pain + 기반 층).

### Phase 0 — Provider/AI 재설계 (최우선)

**범위**:

**A. catalog 분리**
- `oxicode-catalog` 신규 크레이트 추출 (oxicode-ai의 `catalog/`, `data/catalog/`, `model_db.rs` 이관).
- 의존성 방향: `oxicode-ai` → `oxicode-catalog` 단방향.

**B. 세 우려 분리 (정체성 붕괴 수정의 핵심 — 원칙 1 구현)**
- `ProviderDefinition` registry 추출 (oxicode-ai, **auth/login wiring만**): `id`, `name`, `env_keys`, `login`/`refresh_token`/`get_api_key`, `store_credentials_as`, `callback_port`, `paste_code_flow`. omp `registry/types.ts:36-56` 필드 매칭. **base_url/auth_method/category는 여기서 빼** catalog descriptor로.
- `ProviderDescriptor` (oxicode-catalog): model/host 메타데이터 + discovery (`create_model_manager_options`, `catalog_discovery`, 기본 base URL). P0-A와 함께 catalog로 이관.
- Streaming transport trait에서 identity/auth/메타데이터 **전부** 제거. `name()` 제거 (또는 protocol family 이름만, catalog id 아님).
- code-as-data 정적 registry: provider당 1 모듈, `phf::Map`/`const` slice 집계. **compile-time completeness test** 추가 (catalog `KnownProvider` 전부 registry에 정의됨을 검증 — omp의 TypeScript type-level 검사 `registry.ts:162-167`를 Rust test로 복제).
- 정체성 조회: `model.provider` → `ProviderDefinition` registry. **이것이 deepseek→openai 정체성 붕괴의 실제 수정** — trait를 Api로 keying만으로는 안 됨.

**C. API dialect 확장 (transport) — omp `KnownApi` 14개로 정렬**
- `Api` enum을 omp `KnownApi`(`catalog/src/types.ts:8-22`) 14개로 정렬:

| omp KnownApi | Rust variant | oxicode 현 |
|---|---|---|
| `openai-completions` | `OpenAiCompletions` | ✓ |
| `openai-responses` | `OpenAiResponses` | ✓ |
| `openrouter` | `OpenRouter` | ✗ 신규 |
| `openai-codex-responses` | `OpenAiCodexResponses` | ✗ 신규 |
| `azure-openai-responses` | `AzureOpenAiResponses` | (oxicode `Azure`→재명명) |
| `anthropic-messages` | `AnthropicMessages` | (oxicode `Anthropic`→재명명) |
| `bedrock-converse-stream` | `BedrockConverseStream` | (oxicode `Bedrock`→재명명) |
| `google-generative-ai` | `GoogleGenerativeAi` | (oxicode `Google`→재명명) |
| `google-gemini-cli` | `GoogleGeminiCli` | ✗ 신규 |
| `google-vertex` | `GoogleVertex` | (oxicode `Vertex`→재명명) |
| `ollama-chat` | `OllamaChat` | ✗ 신규 |
| `cursor-agent` | `CursorAgent` | ✗ 신규 (remote-AGENT) |
| `gitlab-duo-agent` | `GitLabDuoAgent` | ✗ 신규 (remote-AGENT) |
| `devin-agent` | `DevinAgent` | ✗ 신규 (remote-AGENT) |

- **oxicode `Mistral` Api는 제거** — omp는 Mistral을 `openai-completions` 호환으로 취급 (별도 dialect 아님). oxicode의 `Mistral` enum이 틀린 것.
- `Api = KnownApi | String` (omp의 open extension) → Rust는 `Api::Known(KnownApi)` + `Api::Custom(String)` 또는 별도 custom registry.
- **remote-AGENT 프로토콜 우선 포팅**: `cursor-agent`/`devin-agent`/`gitlab-duo-agent`는 OpenAI-compatible endpoint가 아님 — 각각 고유 stream function + 고유 프로토콜. `ollama-chat`도 production 필수.
- `parse_api()` silent coercion 제거 (알 수 없는 API → 명시적 에러).

**D. AI 층 품질**
- SSE 파싱 중앙화 (`read_sse_events()` 단일 구현, omp `pi-utils` 대응). 8개 provider의 private `parse_sse_events()` 제거.
- per-provider 에러 계층 (`AnthropicApiError`, `OpenAiHttpError`, `BedrockApiError`, `GoogleApiError`, `OllamaApiError`, `DevinApiError`, `CodexProviderStreamError` … — omp `error/classes.ts` 대응).
- `ProviderEvent::ImageEnd` 변형 추가 (증분 이미지 스트리밍).
- **oxicode-original 제거 (확정)**: `MultiProvider`(complexity router), `FallbackChain`, `CircuitBreaker`, `ProviderPool` (~2500줄) — omp catalog/ai 어디에도 complexity router 대응 없음 (`classify.ts`는 모델 family/id 의미 분류만, 조사 완료). **P0에서 제거**. 자동 라우팅/폴백이 필요하면 추후 oxicode-specific opt-in 기능으로 별도 격리 boundary에서 재도입.
- `Model` 타입 보강: `request_model_id`(alias), `supports_tools`, nullable `context_window`/`max_tokens` (omp는 `number | null`).
- `StreamOptions` 보강: signal, watchdog timeout, middleware hook, per-provider options (omp 25+ 필드 수렴).

**산출**: `oxicode-catalog` 신규 크레이트, 재설계된 `oxicode-ai`. callsite 점진적 마이그레이션.
**의존**: 없음.
**회귀 게이트**: `cargo nextest run --workspace` + `cargo clippy --workspace --all-targets -D warnings` + `cargo clippy -p oxicode-sdk --features native-browser -D warnings` 통과 유지.
**정체성 붕괴 수정 검증**: `provider_definition("deepseek").id == "deepseek"` (registry에서), transport는 `Api::OpenAiCompletions`로 라우팅되지만 identity는 "deepseek" 유지.

### Phase 1 — Agent 루프 재정렬
**범위**: owned dialect system (non-native-tool 모델 in-band tool calling), intent tracing(`i` 필드), append-only context(prefix caching), approval/tier 시스템, soft tool requirements, Harmony leak 감지. 누락 도구 16개 포팅(`ast_grep`, `ast_edit`, `debug`, `eval`, `computer`, `checkpoint`, `rewind`, `hub`, `learn`, `manage_skill`, `inspect_image`, `yield`, `goal`, `review`, `tts`, `vibe`). oxicode-original(`ProviderResolver`, `AgentPoolProvider`, `SubagentRunner`, `LspProvider`)은 SDK 격리 층으로 **유지**.
**산출**: 재정렬된 `oxicode-agent`.
**의존**: P0.

### Phase 2 — TUI 재정렬
**범위**: `oxicode-tui-legacy` → `oxicode-tui` rename, 현 v2 단계적 폐기. omp 3-전략 차등 렌더링(component memoization → native scrollback commit → ED3 replay) Rust 구현. append-only "tape" 렌더 계약. 전체 입력 시스템(Kitty keyboard, bracketed paste, keybinding system, mouse SGR 1006, kill ring, undo). LaTeX, mermaid(legacy 85KB 이관), image rendering(Kitty/iTerm2/Sixel). glyph 시스템 단일화. autocomplete, fuzzy search.
**산출**: 통합된 `oxicode-tui` (omp tape 모델 기반).
**의존**: P1 (느슨).

### Phase 3 — 프롬프트 & CLI 재정렬
**범위**: `.md` 기반 시스템 프롬프트(`prompts/system/system-prompt.md` + `include_str!()` + 경량 템플릿). personality 시스템(default/friendly/pragmatic). tool-specific prompt `.md` (~45개). 환경 정보 주입(CPU/GPU/terminal/OS). 누락 CLI 명령 포팅(`bench, commit, completions, config, gc, grep, gallery, install, models, plugin, setup, shell, stats, update, usage, worktree, search` 중 우선순위). `bootstrap.rs`/`lib.rs` 경계 정리 + F-5(main.rs inline subcommand 핸들러 → `cli/commands/*.rs` 분리).
**산출**: 정렬된 `oxicode-cli`.
**의존**: P0.

### Phase 4 — oxicode-original 처리
**범위**: issue 시스템 격리(agent 루프/session 모델에서 분리, CAS/flock 설계 유지, 하나의 boundary 뒤로). package manager → omp 플러그인 모델 재정렬(`storage/packages.rs` 106KB 재설계). language policy 제거/단순화(`Settings::output_languages`, `KNOWN_CHANNELS`, TUI-only 주입, AGENTS.md ~200줄 방어 문서).
**산출**: 정돈된 oxicode-original 층.
**의존**: P1, P3.

## 5. 마이그레이션 안전성
- 각 phase는 **기존 테스트 통과 유지**가 gate (`cargo nextest run --workspace`).
- Provider 재설계(P0): transport trait는 보존하되 identity를 registry로 이동 → callsite 점진적 마이그레이션, 한 번에 big-bang 교체 금지.
- TUI(P2): legacy를 주축으로 유지하며 v2를 단계적 제거. v2 의존 callsite를 legacy로 하나씩 옮긴 뒤 v2 crate 삭제.
- 모든 phase에 **회귀 훅**: `cargo clippy --workspace --all-targets -D warnings` + `cargo clippy -p oxicode-sdk --features native-browser -D warnings` + `cargo audit` + `cargo deny check`.
- 각 phase별 별도 spec에서 회귀 테스트 전략 상세화.

## 6. 비목표 (Non-goals)
- omp의 TS 생태계 특산물(npm plugin, Bun API, N-API)의 문자적 포팅 — Rust 관용구로 번역.
- omp의 collab-web, swarm-extension, metaharness, wire protocol — 이번 리팩토링 범위 밖 (별도 평가).
- stats dashboard — 별도 sub-project.
- OS-integration natives(audio, desktop capture, WebRTC) — 별도 평가.
- 파일 포맷 변경(TOML→YAML 등) — Rust 생태계 표준 유지.

## 7. 공개 질문 (각 phase 상세 spec에서 해결)

**P0 — omp 소스 조사로 해결됨 (본 섹션에 기록)**:
- ~~`MultiProvider`/`FallbackChain`/`CircuitBreaker` 제거 vs catalog 이동?~~ → **제거 확정**. omp catalog/ai에 complexity router 대응 없음 (`classify.ts`는 의미 분류만).
- ~~`Api` dialect 목록?~~ → **omp `KnownApi` 14개 1:1 정렬** (P0-C 표 참조). oxicode `Mistral` enum은 제거.
- ~~`ProviderDefinition` registry 저장 형태?~~ → **code-as-data 정적 테이블** (provider당 1 모듈, 집계, compile-time completeness test).
- ~~omp가 transport/identity를 둘로 나눈다?~~ → 실제는 **3-way 분리** (transport / auth-login / model-host-metadata). 원칙 1에 반영.

**잔존 공개 질문**:
- P0: remote-AGENT 프로토콜(`cursor-agent`/`devin-agent`/`gitlab-duo-agent`)의 Rust 스트리밍 추상 형태 — omp는 각각 고유 stream function. oxicode에서 공통 trait로 뽑을지, per-protocol 독립 함수로 할지.
- P1: 16개 누락 도구 우선순위 — 전부 vs 핵심(`ast_grep`, `ast_edit`, `debug`, `eval`) 우선.
- P2: native scrollback Rust 구현체 — 자체 구현 vs 기존 crate 조사.
- P3: CLI 명령어 우선순위 — 전부 vs 자주 쓰는 것.

## 8. 성공 기준
- **정체성 붕괴 제거 (P0 핵심)**: `provider_definition("deepseek").id == "deepseek"`. streaming transport는 `Api::OpenAiCompletions`로 라우팅되지만, transport가 identity를 주장하지 않음. identity는 오직 `ProviderDefinition` registry에.
- `oxicode-catalog`이 별도 크레이트로 컴파일, oxicode-ai가 소비만.
- SSE 파싱 단일 구현.
- remote-AGENT 프로토콜(cursor/devin/gitlab-duo)이 각각 고유 stream function으로 동작.
- v2 crate 삭제, legacy가 `oxicode-tui`로 단일화.
- 시스템 프롬프트가 `.md` 파일에서 로드.
- issue 시스템이 agent 루프에서 격리.
- language policy 제거.
- 전체 `cargo nextest run --workspace` + clippy 게이트 통과.
