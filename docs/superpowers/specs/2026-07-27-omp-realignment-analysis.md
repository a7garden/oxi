# oxi omp-정렬 분석 증거 (Scout Findings)

- **날짜**: 2026-07-27
- **출처**: omp v17.1.5 (`/tmp/omp`) vs oxi v0.60.0 (`/Volumes/MERCURY/PROJECTS/oxi`)의 6개 도메인 병렬 분석 (ScoutProviders, ScoutCatalog, ScoutAgent, ScoutCli, ScoutTui, ScoutSupport)
- **목적**: `2026-07-27-omp-realignment-design.md`의 설계 근거
- **검증**: P0 프로바이더 발견은 advisor가 oxi 소스(`register_builtins.rs:548-551`, omp `registry.ts:73-79`)에서 직접 교차 검증.

---

## 1. PROVIDERS / AI 층 (사용자 핵심 pain)

**평가**: Drifting — 핵심은 어느 정도 충실하나 싵각한 oxi-original 추가와 치명적 gap.

### 1.1 정체성 붕괴 (advisor 직접 검증 완료)

**증거**: `oxi-ai/src/providers/register_builtins.rs:548-551`이 단정:
```rust
// (oxi 자체 테스트)
create_builtin_provider("deepseek").name() == "openai"   // :548-551
create_builtin_provider("minimax").name()  == "anthropic" // :554-557
```

**근본 원인**: OMP는 스트리밍 **transport / auth-login / model-host-metadata** 세 우려를 분리하지만, OXI는 하나의 `Provider` trait + `BuiltinProvider` struct로 융합했다 (상세 §1.2).
- `Api::OpenAiCompletions` arm (`register_builtins.rs:320-349`)이 항상 `OpenAiProvider`를 반환.
- `OpenAiProvider::name()`은 하드코딩 `"openai"`.
- `builtin.name`(catalog id "deepseek")이 무시됨.

### 1.2 핵심 아키텍처 발견 — 세 우려 분리 (정정됨, primary spec 원칙 1과 일치)

> 초기 표현 "OMP has NO Provider interface"는 **과장**. OMP는 `Provider` trait가 없을 뿐, **세 우려(concern)**를 분리하는 명확한 추상이 있다 (omp 소스 검증: `registry/types.ts`, `descriptor-types.ts`, `types.ts:8-22`).

OMP는 세 우려를 **분리**한다:
1. **Streaming transport** = per-API `StreamFunction<TApi>` (`streamAnthropic`, `streamOllama`, `streamCursor`, `streamDevin`…), `model.api`로 dispatch. **identity·auth·메타데이터를 전혀 갖지 않음.** (`packages/ai/src/types.ts:631`)
2. **Auth/login wiring** = `ProviderDefinition` registry (`packages/ai/src/registry/`). 필드: id, name, envKeys, login, refreshToken, callbackPort, pasteCodeFlow 등. **base_url/auth_method/category는 여기 없다.** ~65 엔트리, code-as-data, compile-time completeness check (`registry.ts:153-167`).
3. **Model/host 메타데이터 + discovery** = `ProviderDescriptor` (`packages/catalog/src/provider-models/`). `createModelManagerOptions`, `catalogDiscovery`, 기본 base URL.

OXI는 이 셋을 하나의 `Provider` trait + `BuiltinProvider` struct로 융합해 `name()`이 identity 역할을 하게 됨 → 정체성 붕괴.

**수정의 함의**: "trait를 `Api` enum으로 keying"만으로는 deepseek→openai가 **안 고쳐짐** (deepseek는 여전히 `OpenAiCompletions` impl로 라우팅). 수정은 identity/auth/메타데이터를 streaming trait에서 완전히 빼서 (2)와 (3)으로 옮기는 것뿐.

### 1.3 API dialect gap (P0 scope 확장)

OMP는 `KnownApi` 14개 고유 API dialect (`catalog/src/types.ts:8-22`): `openai-completions`, `openai-responses`, `openrouter`, `openai-codex-responses`, `azure-openai-responses`, `anthropic-messages`, `bedrock-converse-stream`, `google-generative-ai`, `google-gemini-cli`, `google-vertex`, `ollama-chat`, `cursor-agent`, `gitlab-duo-agent`, `devin-agent`.

OXI는 8개만 (`Anthropic`, `OpenAiCompletions`, `OpenAiResponses`, `Google`, `Bedrock`, `Mistral`, `Azure`, `Vertex`). **OXI의 `Mistral` enum은 틀림** — omp는 Mistral을 `openai-completions` 호환으로 취급 (별도 dialect 아님). omp의 `mistral`/`kimi`/`synthetic`은 provider id이지 API dialect가 아님.

**주의**: `cursor-agent`/`devin-agent`/`gitlab-duo-agent`는 OpenAI-compatible endpoint가 아니라 **remote-AGENT 프로토콜** — 각각 고유 stream function과 고유 프로토콜 필요. Ollama/Cursor는 production 필수.

### 1.4 기타 발견

- **`ImageEnd` 이벤트 누락 (HIGH)**: OXI `ProviderEvent`에 없음. OMP `AssistantMessageEvent`에는 있음. Gemini 등 증분 이미지 스트리밍 표현 불가.
- **OXI 발명 ~2500줄**: `MultiProvider`(complexity router), `FallbackChain`, `CircuitBreaker`, `ProviderPool` — OMP 대응 없음.
- **StreamOptions 얇음**: OMP 25+ 필드(signal, watchdog timeout, middleware hook, per-provider options) → OXI 9 필드.
- **SSE 파싱 8중 복제**: 모든 프로바이더(`openai.rs`, `anthropic.rs`, `google.rs`, `bedrock.rs`, `mistral.rs`, `openai_responses.rs`, `vertex.rs`, `azure.rs`)가 private `parse_sse_events()` 보유. OMP는 `readSseEvents()`로 중앙화.
- **에러 평면화**: `ProviderError::HttpError(u16, String)` vs OMP 깊은 계층(`AnthropicApiError` request-id 파싱, `OpenAIHttpError` body envelope, `BedrockApiError`, `GoogleApiError`, `OllamaApiError`, `DevinApiError`, `CodexProviderStreamError`).
- **Catalog coupling 역전**: OMP는 `pi-catalog`/`pi-ai` 엄격 분리. OXI는 catalog materialization을 oxi-ai에 임베드.
- **Model 타입 gap**: `request_model_id`(alias용), `supports_tools`, `transport`(pi-native) 누락. `context_window`/`max_tokens`이 non-nullable `usize` vs OMP `number | null`.
- **`parse_api()` silent coercion**: 알 수 없는 API가 조용히 `Api::OpenAiCompletions`로 fallthrough.

### 1.5 dispatch 흐름 비교

**OMP**:
```
streamSimple() → ApiKeyResolver → pi-native check → custom API registry
  → GitLabDuo/Kimi/Synthetic routing → mapOptionsForApi()
  → stream() → register-builtins (API별 lazy import)
  → per-API 함수 (streamAnthropic, streamOpenAICompletions, streamCursor, streamDevin, ...)
  → AssistantMessageEventStream
```
Provider identity = **`ProviderDefinition` registry** (registry.ts). transport는 `model.api`로 dispatch, identity 없음.

**OXI**:
```
providers::stream() → get_provider(model.provider)
  → CUSTOM_PROVIDERS global registry (이름별)
  → create_builtin_provider() (BuiltinProvider 메타데이터 data-driven)
  → Provider trait 구현체 구조체 (name()이 identity 역할, 하지만 실제 protocol 이름만 반환)
  → Pin<Box<dyn Stream<Item=ProviderEvent> + Send>>
```
Provider identity = **name string** (`model.provider`), but trait의 `name()`이 protocol 패밀리 이름을 반환해 정체성 붕괴.

### 1.6 키 파일

**OMP**:
- `packages/ai/src/types.ts` — `AssistantMessage`, `AssistantMessageEvent`(13 변형), `StreamOptions`(25+ 필드), `StreamFunction<TApi>`, `Context`, `ToolChoice`, `ServiceTier`
- `packages/ai/src/stream.ts` — main dispatch
- `packages/ai/src/providers/register-builtins.ts` — API→함수 매핑 lazy import
- `packages/ai/src/registry/registry.ts` — ~70+ `ProviderDefinition` (단일 소스)
- `packages/ai/src/providers/transform-messages.ts` — 1076줄, 크로스 프로바이더 변환
- `packages/ai/src/providers/anthropic.ts` — 4473줄 (Claude Code fingerprint, betas, caching, thinking budget)
- `packages/ai/src/error/classes.ts` — 에러 계층

**OXI**:
- `oxi-ai/src/providers/trait_def.rs` — Provider trait (stream, name) ← 융합의 원인
- `oxi-ai/src/providers/register_builtins.rs` — data-driven factory, `parse_api()` coercion, identity-collapse 단정(:548-551, :554-557)
- `oxi-ai/src/providers/event.rs` — ProviderEvent 15 변형 (ImageEnd 누락)
- `oxi-ai/src/providers/options.rs` — StreamOptions 9 필드
- `oxi-ai/src/providers/openai.rs` — 1460줄, manual HTTP
- `oxi-ai/src/providers/anthropic.rs` — 1668줄
- `oxi-ai/src/error.rs` — 평면화 에러
- `oxi-ai/src/transform.rs` — 1254줄
- `oxi-ai/src/messages.rs` — ContentBlock/AssistantMessage
- `oxi-ai/src/multi_provider.rs` — 1284줄 OXI-original

---

## 2. CATALOG

**평가**: Boundary 역전 — OMP는 별도 패키지(단일 소스), OXI는 oxi-ai에 임베디드.

### 핵심 발견
- OMP `@oh-my-pi/pi-catalog`는 자기完結적 npm 패키지. 모델 데이터/identity/descriptor의 단일 소스. sub-entrypoint: `identity`, `provider-models`, `models.json`, `discovery/*`.
- **의존성 방향 역전 확인**: catalog source에서 `@oh-my-pi/pi-ai`로의 import **제로** (devDependency의 `generate-models.ts` 스크립트용만). **pi-ai가 catalog 타입을 소비**, 역방향 아님.
- OMP `models.json`: ~1199 provider 엔트리, 번들된 스냅샷.
- OXI는 catalog를 oxi-ai에 임베드 (`catalog/`, `data/catalog/`, `model_db.rs`). 4-계층 모델: SNAP(임베디드) → LIVE(런타임 캐시) → Layer 2(override) → LOCAL(로컬 서버). `OXI_MODELS_DEV*` 환경 게이트.
- OMP의 catalog 분리가 주는 이점: 단일 소스, 재사용성, AI 패키지와의 명확한 계약. OXI는 이 boundary를 잃음.

### 키 파일
- OMP: `packages/catalog/src/{types.ts, models.ts, model-cache.ts, model-manager.ts, identity/*.ts, provider-models/descriptors.ts, variant-collapse.ts, model-thinking.ts, build.ts, effort.ts}`
- OXI: `oxi-ai/src/catalog/{mod.rs, model.rs, models_dev.rs, runtime.rs, materialize.rs, provider.rs, override_.rs}`, `oxi-ai/src/model_db.rs`, `oxi-ai/data/catalog/{_snapshot.json.gz, product-meta.toml}`

---

## 3. AGENT RUNTIME

**평가**: DRIFTING — 핵심 런타임 충실도 ~40-50%, 도구 parity ~30%.

### 핵심 발견
- **아키텍처**: OMP는 agent 런타임을 `packages/agent`(코어 루프)와 `packages/coding-agent`(도구, MCP)로 분리. OXI는 `oxi-agent` 하나로 통합 (48 파일, ~25K줄).
- **도구 parity**: OMP 23개 도구. 7개 정확히 매칭 (read, write, edit, bash, grep, todo, github). 6개 OXI-original (ls, get_search_results, github_search, generate_image, commit, context7). **16개 OMP 도구 누락**: ast_grep, ast_edit, debug, eval, computer, checkpoint, rewind, hub, learn, manage_skill, inspect_image, yield, goal, review, tts, vibe.
- **도구 인터페이스**: MODERATE DRIFT — OMP `AgentTool`은 18+ optional 필드; OXI trait은 ~10 메서드. 누락: intent tracing(`i` 필드), per-call concurrency/resolver, per-call interruptibility, approval 시스템, TTSR matcher hook, custom wire 이름, rich streaming `onUpdate` 콜백.
- **에이전트 루프**: SIGNIFICANT SIMPLIFICATION. 6개 high-severity gap:
  1. **owned dialect system** — non-native-tool 모델의 in-band tool calling 불가
  2. **intent tracing** — `i` 필드 주입/추출
  3. **Harmony leak 감지** — GPT-5 프로토콜 누수 처리
  4. **soft tool requirements** — remind-then-escalate
  5. **approval/tier 시스템** — 사용자 확인 게이트
  6. **append-only context** — 안정적 prefix caching
  - 추가 누락: Cursor exec bridge, `transformAssistantMessage` hook, `beforeModelCall` gate, pause gate.
- **OXI 발명**: `ProviderResolver` trait (주입형 provider/model 해결, OMP에 없음), `AgentPoolProvider`, `SubagentRunner`, `LspProvider` (SDK 격리용 — 정당한 drift); 런타임 토글 auto-retry; deferred model switching; `TokenSource` enum; tool-result truncation; 더 정교한 MCP(McpManager lifecycle task).
- **MCP**: OXI가 더 정교 (McpManager lifecycle task, McpDirectTool Phase 3 등록, consent 관리, credential provider). OMP는 coding-agent에서 `@agentclientprotocol/sdk` 사용.
- **이벤트**: 양쪽 모두 lifecycle/turn/message/tool 이벤트. OXI 추가: Compaction, AutoRetryEnd, TtsrInterrupt, SteeringMessage. OMP가 intent 필드로 더 풍부한 tool_execution 이벤트.

### 키 파일
- OMP: `packages/agent/src/{agent.ts (56KB), agent-loop.ts (102KB), types.ts (35KB)}`, `packages/coding-agent/src/tools/{builtin-names.ts, essential-tools.ts, index.ts (663줄)}`
- OXI: `oxi-agent/src/{agent.rs (1133줄), tools.rs (1095줄), agent_loop/mod.rs (1495줄), events.rs (480줄), state.rs (~250줄)}`

---

## 4. TUI

**평가**: omp 포팅이 아님 — grok-inspired clean-room 재해석.

### 핵심 발견
- **OMP `packages/tui` 설계**:
  - Component 모델: `render(width) => readonly string[]`. 참조 identity = memoization 증명.
  - **3-전략 차등 렌더링**: (1) component memoization; (2) native scrollback commit (`NativeScrollbackLiveRegion`); (3) ED3 replay (CSI 3 J).
  - **Append-only 렌더 계약**: scrollback에 commit된 row는 불변. "tape"가 터미널의 시각 기록.
  - 입력: Kitty keyboard protocol, bracketed paste(paste markers), keybinding system(conflict resolution), mouse(SGR 1006), kill ring, undo.
  - Theme: component별 chalk 함수. `SymbolTheme`: 최소 12 필드.
  - 14 위젯 (editor 117.7KB, markdown 98.1KB, input, select-list, settings-list, image, box, scroll-view, tab-bar, loader, cancellable-loader, Text, TruncatedText, Spacer).
  - LaTeX(42.6KB + 51.7KB), mermaid, deccara, kitty-graphics, fuzzy, autocomplete(37.3KB), stdin-buffer(27.4KB), keys(16.5KB), keybindings, mouse, terminal(62.7KB), terminal-capabilities(43.8KB).

- **OXI `oxi-tui` v2 상태**: greenfield 9.7K LOC, 222 테스트. ratatui + crossterm 기반. 3 기둥: (1) terminal-first pipeline (`draw_frame`); (2) RetainedTree + content_hash memoization; (3) capability detection + consumption 동일 모듈. 위젯: ChatView, Footer, Sticky, Overlay, Border, List(virtualized), Scrollbar, Text. Theme: 28 ColorScheme 슬롯 (legacy와 스키마 다름). **glyph 시스템 v2에 없음** (doc/code 모순).

- **OXI `oxi-tui-legacy`**: ~74K LOC. theme.rs(75KB, 26 슬롯), symbols.rs(34KB, GlyphSet Unicode/Ascii/Nerd, 50+ 필드), widgets/chat/(~167KB), tool_renderer(61.9KB), list_selector(30.6KB), render/mermaid(85.3KB), render/color_level(DEAD), keybindings/.

- **Revert 이력 (`.git/logs/HEAD`로 확인)**: `c37b6a3f "revert: roll back grok-build port, restore oxi-cli + oxi-tui baseline"` 직전에 ~20개 grok-build 포팅 커밋. revert 직후 같은 세션에서 `e73b5cb5 "docs(tui-v2): spec + Plan A"` → `oxi-tui-v2-plan-a` 브랜치 → 37 커밋 → v0.58에 main으로 merge. **v2 crate 자체는 vendored 코드로 오염되지 않음** (clean-room). 하지만 설계는 grok에서 영감.

### Gap (HIGH)
1. **Theme 스키마 비호환** — v2 ColorScheme이 legacy와 다름 (다른 필드, "derived" 값). theme 파일 이식 불가.
2. **Glyph 시스템 부재** — AGENTS.md는 v2에 있다고 약속하나 legacy에만 존재. doc/code 모순.
3. **이중 크레이트 마이그레이션 구조적 교착** — `LegacyOverlayAdapter`가 항상 dirty (단조 증가 hash) → hash-skip 불가. v2는 widget code ~13%. `draw_frame_closure`가 여전히 주 경로.

### Gap (MEDIUM/LOW)
4. grok-build 설계 논제가 v2 아키텍처 지배.
5. 입력 처리 극단적 단순화 (stock ratatui-textarea).
7. Native scrollback 부재 — OMP의 가장 혁신적 특징의 v2 대응 없음.
8. Markdown 렌더링 기능 미완 (pulldown-cmark checkpoint vs full marked + LaTeX + OSC 66).
9. Capability detection 덜 세분화.
10. Editor 극단적 단순화.

---

## 5. CLI / COMPOSITION

**평가**: 부분 충실 포팅에서 creative reimagining으로 표류.

### 핵심 발견
- **Composition root**: OMP `main.ts`(1648줄) 단일 함수 + `sdk.ts`(3549줄). OXI는 `bootstrap.rs`(601) + `services.rs`(~539) + `lib.rs`(603) 3분할. OXI가 testability엔 더 나으나 `lib.rs::App::from_oxi`가 `bootstrap.rs::build_app`과 중복 wiring (tool 등록, MCP credential). **경계 혼란**.
- **CLI 명령**: OMP 33개 명령 lazy-loaded table. OXI clap derive 17개 (Issue, Pkg, Ext, Refresh, Share는 OXI-original). subcommand 핸들러가 main.rs에 inline (1623줄) — F-5 audit 미해결. **OXI가 ~20개 OMP 명령 누락**.
- **Run mode**: OMP는 TUI/print/RPC/ACP. OXI는 TUI/print/RPC (ACP 미포팅). dispatch는 OXI가 더 깔끔 (bootstrap.rs).
- **Session 모델**: 핵심 JSONL 트리 모델은 충실 포팅. OXI가 blob store, session stats, rewind/checkpoint 엔트리, agent lifecycle registry, session handoff, 확장 엔트리 타입(TTSR, service tier, goal mode) 누락.
- **Settings**: OMP는 schema 기반(`settings-schema.ts` 5724줄, 10 탭, UI 메타데이터), YAML. OXI는 struct 기반(`store/settings.rs` 2602줄), TOML. OMP 수십 설정 누락 (task isolation, bash interceptor, MCP config, STT/TTS, auto-thinking, advisor tuning, goal mode). OMP multi-profile 시스템, OXI 없음.
- **시스템 프롬프트 (중대 drift)**: OMP는 `.md` 파일 템플릿 (`system-prompt.md` 18.8KB, `prompts/system/personalities/*.md`, `prompts/tools/*.md` ~45개). OXI는 inline Rust 문자열 (`prompt/system_prompt.rs` 736줄 + 작은 `templates/system.md` ~15줄). OXI에 personality 시스템, tool-specific prompt `.md`, 환경 정보 주입 전부 없음.
- **Auth**: OXI가 더 발전 (`auth_storage.rs` 1953줄, typed `AuthCredential` enum, Debug redaction, keyring scaffolding). OMP의 secret 난독화 subsystem(115KB)은 누락.
- **Extensions**: OXI가 깔끔 (WASM-first via extism, native `.so`/`.dylib`, 권한 시스템, legacy shim 없음). OMP가 더 강력 (npm plugin, marketplace, hooks, custom commands, dual JS/WASM, 다수 legacy shim).
- **OXI-original 비대**:
  - Issue 시스템 (`store/issues.rs` 2020줄 + 통합 지점 합산 ~10K줄) — CAS/flock ownership, `.oxi/issues/` 마크다운, OMP 대응 없음. 잘 설계됐으나 bolted-on.
  - Package manager (`storage/packages.rs` 106.7KB) — npm 기반, 통합 깊이 불투명.
  - Language policy — TUI-only opt-in, AGENTS.md ~200줄 할애, OMP 대응 없음. "strong default, NOT hard guarantee" 자인.

---

## 6. SUPPORTING PACKAGES

**평가**: 직접 포팅 3개는 충실, major gap은 stats/collab/wire.

### 핵심 발견
- **충실한 포팅**: `oxi-mnemopi` ★★★★★, `oxi-snapcompact` ★★★★★, `oxi-hashline` ★★★★☆.
- **미포팅**: stats dashboard (HIGH, 사용자 노출), collab web client (MEDIUM), wire protocol types (MEDIUM).
- **pi-natives 분석 정정**: 순수 Rust 논리는 필요한 곳에 흡수됨 (snapcompact renderer → oxi-snapcompact; grep/find/ls → oxi-agent native 도구; syntect → workspace dep). OXI는 N-API가 필요 없음 (Rust 자체이므로).
- **진짜 미포팅**: OS-integration ops (clipboard는 shell-out pbcopy/xclip, desktop capture, audio, WebRTC, SIXEL, power, keyboard simulation 없음).
- `oxi-sdk`: net-new 제품 표면 (15 port traits, security, coordination, workflow engine).
- `oxi-lsp`: XAI grok에서 온 net-new.
- Utils: Rust 생태계로 흡수.
- **"이상한" UX의 원인 추정**: clipboard subprocess 취약성, TUI용 native 텍스트 측정 누락, 속도용 `grep-*` crate 미사용, stats dashboard 부재.

### 키 파일
- OMP: `packages/mnemopi/src/types.ts`, `crates/pi-natives/src/lib.rs` (30+ 모듈)
- OXI: `oxi-mnemopi/src/types.rs` (충실), `oxi-agent/src/tools/grep.rs`, `oxi-cli/src/media/clipboard_write.rs`, `oxi-cli/src/extensions/loading.rs`
- 설계 문서: `oxi/docs/designs/omp-adoption-2/00-design-revisions.md:300-318` (pi-natives snapcompact renderer 흡수 확인)

---

## 요약: "이상함" 진단

oxi는 pi 포팅으로 시작 → omp 기능 흡수 → grok-build TUI 포팅 시도(revert)의 3층이 누적됐으나 조율 안 됨. 결과: omp 포팅도, 깔끔한 Rust-native도 아닌, 충돌하는 아키텍처 논제들이 싸우는 하이브리드.

**프로바이더 "이상함"의 정확한 진단** (advisor 교차 검증): `Provider` trait + `BuiltinProvider` struct가 스트리밍 transport + auth/login wiring + model/host 메타데이터 **세 우려**를 동시에 담당하도록 융합된 결과. OMP는 이 셋을 `StreamFunction`(transport, identity 없음) / `ProviderDefinition` registry(auth/login) / `ProviderDescriptor`(catalog, 메타데이터+discovery)로 분리. "trait를 Api로 keying"만으로는 정체성 붕괴가 안 고쳐지고, identity/auth/메타데이터를 trait에서 빼서 (2)·(3)으로 올리는 것이 유일한 수정. 추가로 omp의 `KnownApi` 14개 dialect(그중 cursor/devin/gitlab-duo는 remote-AGENT 프로토콜)가 P0 scope.

**TUI "이상함"의 진단**: v2가 omp 포팅이 아니라 grok-inspired 재해석. legacy가 omp에 더 가깝다(전체 위젯, glyph, mermaid 보유).
