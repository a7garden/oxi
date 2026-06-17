# Changelog

All notable changes to the oxi project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — models.dev 라이브 보강 (catalog Layer 2.5)

opencode가 사용하는 동일 진실 소스인 **models.dev** (MIT)에서
`https://models.dev/api.json` 을 런타임에 페치하여 카탈로그를 보강한다.
이는 oxi-original TOML의 광범위한 `0.0` 가격 결손을 해소한다
(anthropic/openai/azure 등 유료 모델의 cost_input/cost_output이
대부분 `0.0`으로, 비용 리포트가 부정확했다).

- **신규 모듈** `oxi-ai/src/catalog/models_dev.rs`: models.dev 스키마
  파서, provider ID 매핑(oxi 지역 변형 collapse), reasoning 보존
  allowlist(TEE/tput/compound/FP8 변형), enrich 로직, fetch/캐시
  (5분 TTL, atomic temp→rename, 교차프로세스 Flock, 2회 재시도).
- **단일 진입점**: `model_db::all_provider_models()`의 OnceLock 클로저에
  enrich 3줄 삽입 — 모든 소비자(`get_model_entry`/
  `model_from_entry`/`fallback_chain`/TUI 슬래시)가 자동 보강.
  부트스트랩(`bootstrap.rs::build_app`)에서 `init_models_dev().await` 호출.
- **우선순위**: Layer 2 override > models.dev > Layer 1. 양수 가격/
  양수 limit만 덮어쓰며, verified-free/unknown은 보존. openclaw
  `-1.0` 센티넬은 models.dev 양수 도착 시 자동 정상화.
- **오프라인 안전**: init 미실행/페치 실패 시 `get()`=None →
  Layer 1로 graceful fallback. 기능은 항상 동작, 비용 정확도만 저하.
- **게이트**: `OXI_MODELS_DEV`(`on`/`auto`/`off`, 기본 `auto`),
  `OXI_MODELS_DEV_URL`, `OXI_MODELS_DEV_DISABLE_FETCH`(에어갑),
  `OXI_MODELS_DEV_TTL`, `OXI_MODELS_DEV_CACHE_PATH`.
- **테스트**: 단위 10개(스키마/enrich/매핑/allowlist) + end-to-end
  통합 1개(캐시 fixture → init → model_db 조회 검증).
- **문서**: `docs/MODELS_DEV_SYNC.md`(설계 청사진),
  `data/catalog/README.md`(Upstream sync / Price data quality 표 정정),
  `AGENTS.md`(catalog 4-tier 설명, 환경변수, Pitfalls 갱신).

### Added — TUI 출력 언어 정책 (TUI-only, per-channel)

`Settings::output_languages`를 신설하여 **TUI 세션**에서 출력
채널별 언어 정책을 구성할 수 있게 했다. `oxi --print` 및 RPC
모드는 정책이 있어도 **조용히 무시**된다 (의도적 격리 — TUI
하네스 전용).

- **데이터 모델** (`oxi-cli/src/store/settings.rs`):
  `output_languages: HashMap<String, String>` — 채널 키
  (`response`, `code_comment`, `documentation`,
  `commit_message`)에 ISO 639-1 코드(`en`, `ko`, `ja`, …) 또는
  `"auto"`를 매핑. 기본값 = 전 채널 `auto` (현재 동작 100% 보존).
  `settings.toml` v4→5 마이그레이션은 값 변환 없이 버전만 올림.
- **확장형 맵**: 핵심 4채널 외에 사용자가 임의 키를 추가할 수
  있다 (예: `pr_description = "en"`). `KNOWN_CHANNELS`는 이제
  prompt label 매핑 테이블로만 사용되며, 알 수 없는 채널은 raw
  키를 label fallback으로 directive에 포함된다.
- **3레이어 전파** (`oxi-cli/src/app/agent_session_runtime.rs`,
  `oxi-cli/src/prompt/system_prompt.rs`):
  1. 시스템 프롬프트의 **마지막 섹션**에 "Output Language
     Policy (enforced)"로 부착 — 모델이 가장 강하게 attend하는
     위치.
  2. `compaction_instruction`에도 같은 directive를 흘려 요약
     누출 차단. 단, summarizer는 이를 `"Focus areas: …"`로
     wrap하므로 강도가 약해짐 (문서화됨, 별도 cross-crate 변경
     필요).
  3. 서브에이전트는 부모 `system_prompt`를 `--append-system-prompt`
     플래그로 자식에게 전달하고 자식은 `set_system_prompt()`로
     통째 replace하므로, **부모 directive가 자식에게 자연
     전파**된다 (추가 코드 불필요).
- **TUI UX** (`oxi-cli/src/tui/overlay/settings.rs`):
  `/settings` 오버레이에 "Language (TUI)" 섹션 추가. 채널당
  Choice로 `auto → en → ko → ja → zh → es → fr → de → auto` 사이클.
  Esc로 디스크 persist + `OXI`-mode 알림.
- **Hot-apply** (`oxi-cli/src/app/agent_session.rs`,
  `oxi-cli/src/tui/slash.rs`): `AgentSession::rebuild_system_prompt()`
  신규, `/reload` 슬래시 명령에서 `set_thinking_level`과 함께
  호출. 변경 후 `/reload` 한 번이면 다음 턴부터 적용.
- **검증**: 알 수 없는 언어 코드는 `tracing::warn!` 후 유지
  (사용자가 새 언어 추가 가능). 알 수 없는 채널 키는 그대로
  통과 (확장형). 화이트리스트 검증 없음.
- **테스트 8개 신규** (settings 4, system_prompt 4,
  agent_session_runtime 4) — 핵심 invariant를 단위 테스트로
  잠금.
- **Strong default, NOT a hard guarantee.** 코드 docstring 4곳
  + AGENTS.md Pitfall에 한계 명시. 100% 보장이 필요하면 도구
  출력 wrapping 또는 응답 후처리가 필요 (현재 MVP 범위 외).

### 사용 예시 (`~/.oxi/settings.toml`)

```toml
[output_languages]
response = "ko"
code_comment = "en"
documentation = "en"
commit_message = "en"
```

## [0.35.0] - 2026-06-15

### Fixed — native-browser 부활: edition 2024 lifetime 버그 전면 수정

`--features native-browser`로 컴파일하면 27~28개의 컴파일 에러가
발생하던 치명적 버그 수정. 근본 원인은 `BrowserTab`/
`BrowserEngine` trait가 수동 `Pin<Box<dyn Future + 'a>>` 패턴을
사용했는데, edition 2024의 정밀한 lifetime 캡처 규칙에 위배되었기
때문. 이 버그는 oxi CI가 `native-browser` feature를 단 한 번도
컴파일하지 않아 0.32.0~0.34.0까지 배포된 채 방치되었음.

- **`BrowserTab`/`BrowserEngine` trait를 `#[async_trait]`로 전환**
  (oxi-agent): 30개 메서드 시그니처를 `async fn`으로 단순화.
  `async-trait = "0.1"`은 이미 의존성이었고 sibling `AgentTool`
  trait 4개가 같은 패턴을 사용 중이었으므로 일관성 확보.
  `Pin<Box<...>>` 보일러플레이트 약 480줄 제거.
  `dyn BrowserTab`/`dyn BrowserEngine` object-safety 유지.
- **oxibrowser_backend.rs impl을 async_trait 기반으로 재작성**
  (oxi-agent): 27개 메서드 전부 `async fn`으로 변환.
  `tab_id(&'a self)`, `evaluate_await`의 선언되지 않은 lifetime
  버그(E0261), `new_tab`의 Box coercion 실패(E0271) 동시 해결.
- **Mock 구현체 3종 동기화** (oxi-agent): `tab_guard.rs::MockTab`,
  `browse_tool.rs::MockEngine`, `browse_session_tool.rs::MockTab`/
  `MockEngine` 전부 async_trait 기반으로 변환.

### Changed

- **`oxibrowser-core` 0.14.1 → 0.15 정렬** (oxi-sdk): oxi-agent은
  이미 0.15를 사용 중이었으나 oxi-sdk의 re-export만 0.14.1에
  고정되어 있던 버전 불일치 해결.

### CI — 재부팅 영구 차단

- **`clippy-native-browser` job 추가** (ci.yml): 매 PR마다
  `cargo clippy -p oxi-sdk --features native-browser -- -D warnings`
  + `cargo build -p oxi-agent --features native-browser` 실행.
  native-browser 코드 경로가 다시 부서지는 것을 영구 차단.
- AGENTS.md에 native-browser 컴파일 의무화 명시.

## [0.34.0] - 2026-06-15

### Added — MCP 서버 관리 TUI + 표준 config 호환성

`/mcp` 명령이 읽기전용 대시보드에서 **인터랙티브 관리 오버레이**로
승격. 서버 추가/편집/삭제를 TUI에서 직접 하고 디스크에 저장하면 런타임
`McpManager`에 핫 리로드. pi-mcp-adapter의 `/mcp` 패널 UX를 참고.

- **`McpConfigOverlay`** (oxi-cli): `/mcp`로 열리는 관리 오버레이.
  - **List 모드**: 라이브 연결 상태 표시(●/○/✗), scope 표시,
    unsaved 배지.
  - **Edit 모드**: 폼 편집기 — name, transport(stdio/http 토글),
    command+args 또는 url, lifecycle, idle timeout, direct-tools,
    env/headers.
  - **Confirm-remove 모드**: 삭제 가드.
  - **Discard-guard 패턴**: dirty 상태에서 첫 Esc/Tab은 경고만,
    두 번째가 실제 닫기/전환 — 조용한 데이터 손실 방지.
  - **명시적 Transport 토글**: 자동감지 대신 선택 필드로 URL 필드에
    접근 가능. 토글 시 관련 첫 입력 필드로 포커스 이동.
- **`/mcp` 자동완성**: `BUILTIN_SLASH_COMMANDS`에 추가되어 슬래시
  자동완성 목록에 표시.
- **Config 저장 헬퍼** (oxi-agent): `save_mcp_config()`(temp+rename
  atomic write), `load_or_default()`, `default_write_path_global/project()`.

### Changed

- **`/mcp` 라우팅 분리**: `/mcp` → 관리 오버레이, `/mcp dashboard` →
  기존 읽기전용 상태 대시보드, `/mcp status` → 상태 알림(변경 없음).
- **표준 MCP config 포맷 호환** (oxi-agent): serde가 canonical camelCase
  (`mcpServers`, `idleTimeout`, `directTools`, `toolPrefix`, …)로 직렬화.
  역직렬화는 camelCase와 legacy snake_case 모두 허용(`serde alias`)하여
  기존 파일이 그대로 동작.
- **`McpManager::replace_config()`** (oxi-agent): 런타임 config 핫 교체.
  새로 추가된 서버가 재시작 없이 proxy tool에 도달 가능.
  (direct-tool 등록은 여전히 부팅 시 1회만 수행하므로 재시작 필요.)
- **`OverlayAction::McpConfigApplied`** (oxi-cli): 오버레이가 디스크에
  쓴 merged config를 라이브 매니저에 반영하고 성공 알림.

### Fixed

- **"Save & Apply" 버튼 미작동**: Edit 모드 Save가 메모리만 고치고
  디스크 저장/live 적용을 안 하던 버그 수정 — `commit_and_save()`로
  stage + write + apply를 한 번에 수행.
- **scope 전환(Tab) 시 unsaved 변경사항 조용히 손실**: discard-guard로
  두 번 누르기 패턴 적용.
- **Esc로 닫을 때 unsaved 손실**: 동일한 discard-guard 패턴 적용.

## [0.33.0] - 2026-06-13

### Added — MCP 고도화 (Phase 1-3 + SDK + TUI)

pi-mcp-adapter 아키텍처 기반으로 MCP 기능 대폭 확장.

- **Disk-backed metadata cache** (`~/.oxi/mcp-cache.json`): 서버 연결 없이도
  `search`/`list`/`describe` 동작. 원본 툴 이름만 저장하여 `tool_prefix`
  설정 변경에도 무효화 불필요.
- **Channel-based lifecycle manager**: `mpsc` 채널로 idle disconnect 타이머와
  keep-alive health check를 `McpManagerInner` 뮤텍스 밖에서 실행 → 데드락 방지.
- **`McpTransport` trait**: stdio 전송을 추상화. 향후 HTTP/SSE 추가 용이.
- **`McpManager::spawn()`**: `Arc::new_cyclic`으로 lifecycle 태스크에
  `Weak<McpManager>` 전달. `Eager`/`KeepAlive` 서버는 백그라운드 자동 연결.
- **`McpDirectTool`**: 개별 MCP 툴을 `AgentTool`로 직접 등록.
  `directTools`/`excludeTools` 설정으로 제어. Consent system과 연동.
- **`ConsentManager`**: 툴 실행 전 Allow/Deny 사전 승인.
  `~/.oxi/mcp-consent.json`에 저장.
- **Generic `DashboardWidget`** (oxi-tui): MCP 독립적인 제네릭 대시보드.
  섹션/아이템/필터/뱃지 지원.
- **`McpDashboardOverlay`** (oxi-cli): `/mcp` 슬래시 명령으로 열리는
  인터랙티브 MCP 관리 대시보드. 서버 연결/해제, consent 관리, 필터 지원.
- **SDK 레이어**: `OxiBuilder::with_mcp_config()`, `Oxi::mcp()`,
  `mcp_tools()` factory. oxi-sdk re-export로 SDK 컨슈머(oxios 등)가
  MCP를 직접 사용 가능.
- **MCP 디스크 경로 커스터마이징** (SDK 컨슈머용): `McpManager::spawn_with_paths(config, cache, consent)`와
  `OxiBuilder::with_mcp_paths(cache, consent)` 추가. SDK 컨슈머(oxios 등)가
  자체 디렉토리(`~/.oxios/`) 아래에 MCP 캐시/consent 상태를 self-host할 수 있도록
  additive API. `oxi_sdk::MetadataCache` 재내보내기 포함. 기존 `spawn()`/
  `spawn_with_config()`는 `spawn_with_paths`의 thin wrapper가 됨 (관측 동작 불변).
  (참고: `docs/proposals/mcp-disk-path-customization.md`)

### Changed

- `McpManager::new()` → `Arc<Self>` 반환 (내부적으로 `spawn()` 호출).
  `ToolRegistry::with_builtins_cwd()`에서 `Arc` 한 겹 제거.
- `McpClient`가 `Box<dyn McpTransport>` 기반으로 리팩터링.
- `ToolRegistry`에 `mcp_manager` 필드 및 `set_mcp_manager()`/`mcp_manager()` getter 추가.
- `ServerEntry`, `McpSettings`에 `#[serde(default)]` 및 `Default` 추가.
- `ServerEntry`에 `direct_tools`, `exclude_tools` 필드 추가.
- `McpSettings`에 `direct_tools`, `disable_proxy_tool` 필드 추가.

### Fixed

- `McpManager::spawn()` / `spawn_with_paths()`가 Tokio runtime 밖에서
  호출되면 panic하던 회귀 수정 — `OxiBuilder::build()`를 runtime 없이 부르는
  단위 테스트(oxi-sdk 6개)가 `tokio::spawn` panic으로 실패했다. runtime
  가드(`Handle::try_current()`)를 추가해 runtime이 없으면 lifecycle/eager
  task를 생략 (`new_no_spawn()` 패턴 차용).
- `OxiBuilder::build()`에서 MCP paths-only 분기가 빈 `McpConfig`를 사용하던
  풋건 수정 — 이제 `with_mcp_config` 없이 `with_mcp_paths`만 호출해도
  표준 경로에서 config를 자동 발견한다.
- `McpClient`/`McpPrompt`/`McpLogLevel`/`McpSamplingRequest` 등 공개 API의
  missing-doc 누락 보충 및 clippy(clapsed-if, derive, map) 경고 해소.

## [0.32.0] - 2026-06-12

### Changed — RFC-008: Remove `max_iterations` loop guard

The agent loop no longer enforces a turn limit. This matches pi-agent's
behavior where the loop runs until the LLM naturally stops making tool calls.

- **`should_stop_after_turn()`** now only checks `external_stop` (Ctrl+C).
  The `max_iterations`, `turn_number`, `messages`, and `assistant_message`
  parameters were removed — the function signature is now
  `fn should_stop_after_turn(external_stop: &Arc<AtomicBool>) -> bool`.
- **`AgentConfig::max_iterations`** field removed. Existing code that sets
  this field will get a compile error — remove the field from struct literals.
- **`AgentLoopConfig::max_iterations`** field removed.
- **`AgentConfig::with_max_iterations()`** builder method removed.
- **`AgentEvent::ForcedSummary`** variant removed (was added during RFC-008
  development but is no longer needed without the max-iterations guard).
- **`LoopStopReason`** enum removed.

### Removed

- `max_iterations` field from `AgentConfig` and `AgentLoopConfig`.
- `with_max_iterations()` builder from `AgentConfig`.
- `LoopStopReason` enum from `agent_loop::helpers`.
- `ForcedSummary` variant from `AgentEvent`.

### Migration

Remove all `max_iterations` fields from `AgentConfig` and `AgentLoopConfig`
struct literals. The loop now runs indefinitely until the LLM produces a
text-only response (no tool calls) or the user cancels (Ctrl+C).

## [0.31.6] - 2026-06-12

### Fixed — Session persistence bug

- **`AgentMessage::User` and `AgentMessage::System` failed to serialize** due to
  `#[serde(flatten)]` on a `ContentValue` field. `ContentValue::String` serializes
  as a bare JSON string, but `flatten` can only merge structs/maps — causing
  `serde_json::to_string` to fail silently. User messages were never written to
  disk, making sessions invisible to `/resume`. Removed `#[serde(flatten)]` from
  both variants (`oxi-cli/src/store/session.rs`).
- **Silent serialization failures in `_persist()`** now emit `tracing::warn!`
  instead of being silently swallowed.
- Added regression tests: `test_session_roundtrip_preserves_user_content`,
  `test_session_list_finds_sessions_with_user_messages`.

## [0.31.0] - 2026-06-07

### Changed — Rust 2024 edition modernization

- **`async-trait` crate removed**: All 104 `#[async_trait]` annotations across
  59 files replaced with native `async fn` in trait (stable since Rust 1.75).
  Trait methods now return `Pin<Box<dyn Future + Send>>` explicitly, eliminating
  macro expansion overhead and improving debuggability.
- **`once_cell::sync::Lazy` → `std::sync::LazyLock`**: All 4 uses in `oxi-ai`
  replaced with the standard library equivalent (stable since Rust 1.80).
- **Rust 2024 let chains**: 16 nested `if let` patterns flattened to
  `if let A && let B` syntax across the workspace.
- **oxibrowser upgraded** from 0.14.1 to **0.15.0** (edition 2024 update).

### Removed dependencies

- `async-trait` — from all 4 crates (oxi-ai, oxi-agent, oxi-sdk, oxi-cli)
- `once_cell` — from oxi-ai (replaced by `std::sync::LazyLock`)
- `lazy_static` — from oxi-cli (unused)
- `tokio-test` — from oxi-ai, oxi-agent (unused)

## [Unreleased]

### Changed — Edition upgrade (2024 edition)

- **Rust edition**: upgraded from 2021 to **2024** across all workspace
  crates (`oxi-ai`, `oxi-agent`, `oxi-tui`, `oxi-sdk`, `oxi-cli`,
  `scripts`).
- **MSRV**: bumped from **1.82** to **1.96** (2024 edition requires
  Rust ≥ 1.85; 1.96 is the MSRV floor going forward).
- `rust-toolchain.toml` now pins to channel `1.96` (was `stable`).
- All workspace crates inherit `edition` and `rust-version` from
  `[workspace.package]` in the root `Cargo.toml`.
- **Match ergonomics (2024)**: removed redundant `ref`/`ref mut`
  bindings in patterns matching on references — the compiler now
  implicitly borrows in these positions.
- **`set_var`/`remove_var` → unsafe**: wrapped all calls to
  `std::env::set_var` and `std::env::remove_var` in `unsafe {}`
  blocks (these functions became `unsafe fn` in the 2024 edition).
  Affected files: `oxi-cli/src/store/settings.rs`,
  `oxi-ai/src/providers/vertex.rs`,
  `oxi-ai/src/providers/register_builtins.rs`,
  `oxi-ai/src/env_api_keys.rs`, `oxi-ai/src/provider_registry.rs`.
- **Clippy 1.96**: auto-fixed `collapsible_if` and `let_and_return`
  lints (new in Rust 1.96 clippy) across the workspace.
- **CI**: `RUST_VERSION_MSRV` in `.github/workflows/ci.yml` updated
  to `1.96`.
- **README**: Rust badge and install instructions updated to reflect
  the new MSRV (≥ 1.96).

### Scope decisions (2026-06-07)

- **Distribution channel:** crates.io only. No Homebrew tap, no Scoop
  bucket, no apt/yum repos.
- **Build target:** `aarch64-apple-darwin` (macOS Apple Silicon) only.
  The maintainer does not have access to Linux or Windows build
  environments, so cross-OS verification is not part of this pipeline.
- **Supply chain:** SHA256SUMS generated on every release (unsigned).
  No GPG signing, no Codecov coverage reporting.

### Added — CI/CD & Supply Chain

- **`release.yml` enhancements**:
  - New `tag-check` job rejects tags not reachable from `origin/main`
    (defense against force-pushed stale tags).
  - Release job now generates `SHA256SUMS` next to binaries.
  - CycloneDX 1.5 SBOM (`oxi.cdx.json`) attached to the GitHub release.
  - Matrix simplified to a single target (`aarch64-apple-darwin`).
- **`publish.yml`** (new) — publishes all 5 workspace crates to
  `crates.io` in topological order on `release: published`, with a
  dry-run `cargo package --no-verify` pre-flight. Requires `CARGO_TOKEN`
  secret. Run `workflow_dispatch` for a manual dry run.
- **`sbom.yml`** (new) — generates a CycloneDX SBOM on every push to
  `main`, submits it to GitHub's dependency-graph API (so Dependabot
  sees transitive crates), and uploads the JSON as a workflow artifact.
- **`labels.yml`** (new) — single source of truth for issue labels
  (priority, area, status, type, provider). 30+ labels, including
  `good first issue` and `help wanted`. Synced to the repo by
  `labels.yml` workflow (weekly + on labels.yml change).
- **`FUNDING.yml`** (new) — surfaces a "Sponsor" button on the repo
  page (GitHub Sponsors).
- **`.pre-commit-config.yaml`** (new) — local pre-commit hooks that
  mirror the ci.yml gate: trailing whitespace, EOF, YAML/TOML lint,
  merge-conflict, large files, private keys, no-commit-to-main,
  `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`.
- **`ci.yml` enhancements**:
  - `smoke-test` now has a 15-min timeout.
  - New `msrv` job verifies the workspace builds on Rust 1.82.
  - New `doc` job builds `cargo doc --no-deps` with
    `RUSTDOCFLAGS="-D warnings"`.
- **`test.yml` enhancements**:
  - Triggered on `pull_request` (was: main-only). Every PR now runs
    the full nextest matrix.
  - Matrix simplified to `macos-latest` only.
- **`build-binaries.yml`** matrix simplified to `aarch64-apple-darwin`
  only.

### Changed — Repository Hygiene

- **Dependabot groups** — `dependabot.yml` now groups all cargo
  patches into a single weekly PR (with separate major-bump group),
  and groups all GitHub Actions updates similarly. Reduces PR
  noise from 3-5/week to 1-2/week.
- **Removed `[patch.crates-io]` from `Cargo.toml`** — workspace
  members are auto-resolved via `members`, and the explicit patches
  blocked `cargo publish`. This is a prerequisite for `publish.yml`.

### Added — Issue/PR Workflow

- **30+ standardized issue labels** — `priority: critical/high/medium/low`,
  `area: ai/agent/tui/sdk/cli/ci/docs/extensions/security`,
  `status: needs-triage/in-progress/review/blocked`,
  `type: regression/performance/refactor/breaking-change`,
  `provider: anthropic/openai/google/other`, plus `good first issue`,
  `help wanted`, `dependencies`, `release`.

## [0.30.0] - 2026-06-06

### Changed — oxi-agent

- **Replace `a3s-search` with `oxibrowser` search module**: Web search (`web_search` tool) now uses `oxibrowser::search::dispatch()` instead of the `a3s-search` crate. This consolidates search functionality into the oxibrowser ecosystem and removes the `a3s-search` dependency.
- **Remove Brave engine**: The `brave` engine option is no longer available. Supported engines: `ddg`, `wiki`, `bing`.
- **`SearchResult` type migration**: `search_cache::SearchResult` replaced by `oxibrowser::SearchResult` (fields `engines`/`score` → `source`/`extra`).

### Removed

- `a3s-search` dependency from `oxi-agent`.
- `RUSTSEC-2025-0057` (fxhash) advisory exception — no longer a transitive dependency.

### Changed — oxi-sdk

- `oxibrowser-core` dependency updated to `0.14.1`.

## [0.29.1] - 2026-06-06

### Added — oxi-agent

- **`ScreenshotMeta` struct**: Screenshot metadata (bytes, width, duration_ms) attached to `ToolCallContext::PageVisit`.
- **`PageVisit.navigation_error`**: Navigation error message from `BrowseProgress::NavigationFailed`.
- **`PageVisit.screenshot`**: Screenshot metadata from `BrowseProgress::ScreenshotCaptured`.
- **Enrichment match arms**: `make_browse_enrichment_cb` now handles `NavigationFailed` and `ScreenshotCaptured` events (previously only `DocumentReady` was processed).
- **Unit tests**: `browse_enrichment_callback_fills_navigation_error`, `browse_enrichment_callback_fills_screenshot`, `browse_enrichment_callback_navigation_failed_ignores_non_page_visit`.

### Fixed — oxi-cli

- **Clippy `large_enum_variant`**: `SessionEvent::Agent` variant boxed to reduce enum size from 264 bytes.

## [0.29.0] - 2026-06-06

### Added — oxi-agent

- **`ToolCallContext` enum**: Semantic context for tool calls (`WebSearch`, `PageVisit`, `DataExtraction`, `SessionAction`, `ScriptStep`). The agent loop infers context from tool name + args via `infer_context()`; tools remain unaware of semantics.
- **`BrowseProgress` enum**: Structured progress events from browser tab lifecycle (`NavigationStarted`, `WaitingForSelector`, `DocumentReady`, `ScreenshotCaptured`, `NavigationFailed`). Converted from `oxibrowser_core::BrowserEvent` in the backend drain task.
- **`VisitReason` enum**: `DirectNavigation`, `SearchResult { position }`, `LinkFollow` — distinguishes *why* a page was visited.
- **`BrowseCallbacks` mixin** (`callback_mixin.rs`): Eliminates duplicated pending-callback boilerplate across 4 browse tools. Provides `store_progress()`, `store_browse()`, `register_on_registry()`, `register_on_tab()`.
- **`TabCallbacks` composite** in `TabCallbackRegistry`: Single `HashMap<Uuid, TabCallbacks>` replaces the dual-map pattern. One `clear()` removes both string and browse callbacks atomically — no key-set divergence possible.
- **`make_browse_enrichment_cb()`**: Shared closure factory that enriches `ToolCallContext::PageVisit` and `DataExtraction` with `DocumentReady` data (title, status, bytes, duration).
- **`enrich_context_from_metadata()`**: Post-execute enrichment that fills `DataExtraction.result_count` from `AgentToolResult.metadata`.
- **Parallel tool execution parity**: `execute_prepared_tool_call_static` (parallel path) now has full context_cell, tab_id_slot, progress callback, and browse callback wiring — identical observability to the sequential path.
- **`browse_session "goto" → PageVisit`**: Semantic upgrade — `goto` action now produces `PageVisit { reason: DirectNavigation }` instead of generic `SessionAction`.
- **`browse_script → ScriptStep`**: `infer_context` parses step count from YAML or JSON args, producing `ScriptStep { current: 0, total: N, step: "starting" }`.
- **`browse_extract result_count`**: Extraction results include `result_count` in metadata; context enrichment populates `DataExtraction.result_count` after execute.
- **Integration tests**: `engine_forwards_browse_progress_to_callback`, `engine_routes_browse_progress_by_tab_id` — end-to-end browse progress verification with real browser.
- **Unit tests**: `browse_progress_serde_roundtrip`, `browse_enrichment_callback_*`, `infer_context_browse_script_*` — 18 new tests total.
- **`AgentTool::on_browse_progress`**: Default trait method for structured browse progress callbacks.
- **`BrowserTab::set_browse_progress_callback`**: Default trait method; only backends with browse callback support override.

### Changed — oxi-agent

- **`TabCallbackRegistry` restructured**: Dual `callbacks` + `browse_callbacks` maps → single `entries: HashMap<Uuid, TabCallbacks>` with composite `TabCallbacks { progress, browse }`. `clear()` is now atomic for both callback types.
- **`BrowserTab::clear_browse_progress_callback` removed**: `TabCallbacks` clearing handles both; no separate method needed.
- **4 browse tools refactored**: `pending_callback` + `pending_browse_callback` fields replaced with single `callbacks: BrowseCallbacks` field. ~80 lines of duplicated boilerplate eliminated.
- **`BrowseScriptTool` YAML parser rewritten**: `parse_steps` now handles the `{ steps: [...] }` map format correctly, with per-step variant dispatch and shorthand support (`- goto: "url"` for single-field struct variants, `screenshot: {}` for unit variants). Fixes 10 previously-failing tests.
- **`browse_progress_from_event`**: `NavigationFailed` match arm gated behind `oxibrowser-core ≥ 0.14` (crates.io 0.13 compatibility).

### Removed — oxi-agent (Breaking Changes)

- **`ToolProgress` enum**: Unused structured progress type (replaced by `BrowseProgress`).
- **`FileOp` enum**: Unused file operation types (part of `ToolProgress`).
- **`StructuredProgressCallback` type**: Unused callback type (replaced by `BrowseProgressCallback`).
- **`AgentTool::on_structured_progress`**: Unused trait method (replaced by `on_browse_progress`).

### Changed — oxi-sdk

- Re-exports `BrowseProgress`, `BrowseProgressCallback`, `ToolCallContext`, `VisitReason`.

### Changed — oxi-cli

- `ToolExecutionStart` and `ToolExecutionUpdate` pattern matches updated with `..` for backward compatibility.

### Changed — workspace

- Bumped all crate versions to 0.29.0.
- Inter-crate dependency versions aligned to 0.29.0.

- Per-`tab_id` `TabCallbackRegistry` replaces the single-slot `ProgressForwarder`.
  Concurrent `BrowseTool` calls (each with their own tab) are now routed correctly.
  Each `BrowseTool::execute` registers its callback on the specific tab; the
  engine's background event-drain task routes events by `tab_id`.
- `AgentTool::set_tab_id_slot` and `AgentTool::current_tab_id` default methods
  on the tool trait, enabling the agent loop to read the active tab ID.
- `BrowserTab::tab_id`, `BrowserTab::as_any`, `BrowserTab::clear_progress_callback`
  default methods on the browser tab trait.
- `BrowseTool::pending_callback` pattern: `on_progress` stores the callback;
  `execute` registers it on the actual tab (tab_id not known until tab creation).
- Integration test `engine_routes_events_by_tab_id_concurrent`: opens two tabs,
  registers per-tab callbacks, and verifies event isolation.

### Changed — oxi-agent

- `oxibrowser-core` dependency bumped from 0.12 to **0.13**.
- `BrowseTool::execution_mode` remains `SequentialOnly` (per-tab routing makes
  parallel safe, but no concrete multi-tab use case yet).

### Fixed — oxi-agent

- `AgentEvent::ToolExecutionUpdate.tab_id` is now populated (no longer always `None`).
  The agent loop passes a shared `tab_id_slot` to the tool; `BrowseTool` writes
  the tab ID when it opens a tab, and the progress callback reads it.
- `TabGuard::close` now calls `clear_progress_callback()` to unregister the
  per-tab callback, preventing stale callbacks from accumulating in the registry.

### Fixed — workspace

- Resolved CI gate violations (12 errors total under `cargo clippy --workspace -- -D warnings` and `RUSTFLAGS="-D warnings" cargo build --workspace`):
  - **oxi-sdk** (3): removed unused `std::sync::Arc` import in `ports/fs/access.rs`; replaced `let _ = tokio::spawn(...)` with `drop(tokio::spawn(...))` in `ports/mod.rs`; collapsed nested `if` in `ports/fs/capability.rs` wildcard prefix resolution.
  - **oxi-cli** (9): removed unused `clap::Parser` / `std::sync::Arc` imports in `bootstrap.rs` and `setup_wizard.rs`; removed unused `oxi::extensions::ExtensionRegistry` / `std::path::PathBuf` imports in `main.rs`; silenced `unexpected_cfgs` on the `keyring` placeholder cfg in `store/auth_storage.rs::persist`; deleted dead `run_single_prompt` helper from `bootstrap.rs` (replaced by `crate::main_dispatch::run_single_prompt`); dropped needless `&` on `args` borrow in `register_builtin_tools` call; suppressed unused `Result` from `App::switch_model` call in `lib.rs`; added missing `///` doc comment on `init_logging`; split doc-comment/regular-comment collision before `build_system_prompt` in `lib.rs`.
  - **oxi-agent** (1): `cargo fmt` trailing blank line in `tools/browse/engine.rs` (auto-fixed by `cargo fmt --all`).

### Changed — workspace

- Bumped all crate versions to 0.27.1 (oxi-ai, oxi-cli, oxi-sdk, oxi-tui). oxi-agent was already at 0.27.1. Inter-crate dependency versions aligned to 0.27.1.

### Fixed — oxi-agent

- `BrowseTool::execution_mode` now returns `SequentialOnly` to prevent the OxiBrowserEngine progress forwarder race. (Future work: per-tool_call_id forwarder.)

### Changed — infrastructure

- **CI**: Added `smoke-test` job to `.github/workflows/ci.yml` so PRs run a lightweight test subset
- **CI**: Replaced `cargo install` with `taiki-e/install-action` for `cargo-audit` and `cargo-deny` (saves ~3 min/job)
- **CI**: Added macOS to `test.yml` matrix for cross-platform test coverage
- **CI**: Added `RUSTDOCFLAGS=-D warnings` to `test.yml` so doc-tests fail on warnings
- **Release**: Switched x86_64 macOS runner from `macos-13` (deprecated) to `macos-14` (cross-compiled)
- **Release**: Added tag-on-main verification step to prevent releases from stale branches
- **PR Gate**: Conventional commit title is now enforced (error, not warning); PR size hard cap at 4000 lines
- **PR Gate**: Added merge-commit detection and issue-linkage encouragement
- **Dependabot**: Added `github-actions` ecosystem alongside cargo
- **Cargo**: Removed conflicting `[profile.release]` from `.cargo/config.toml` (workspace `Cargo.toml` is now the single source of truth)
- **Cargo audit/deny**: Synced ignore lists across `.cargo/audit.toml` and `deny.toml`; added upgrade tracker comment for extism ≥ 1.22 (wasmtime ≥ 43)
- **Docs**: Added `CODEOWNERS` for per-area review assignment

[0.29.0]: https://github.com/a7garden/oxi/compare/v0.28.0...v0.29.0
[Unreleased]: https://github.com/a7garden/oxi/compare/v0.34.0...HEAD

## [0.24.0] - 2026-05-30

### Changed — workspace

- Bumped all crate versions to 0.24.0
- Fixed 18 doc warnings across all crates (unresolved links, bare URLs, HTML tags)
- Added `.cargo/audit.toml` with documented vulnerability ignore rationale (wasmtime 41.x via extism)
- Updated README version badge to 0.24.0
- Updated AGENTS.md version to 0.24.0

## [0.25.7] - 2026-05-31

### Changed — oxi-cli

- **Provider select overlay improvements**: Updated handler logic, factory enhancements, and slash command integration
- Bumped all crate versions to 0.25.7

## [0.25.4] - 2026-05-31

### Added — oxi-sdk

- `oxi-sdk/examples/builder_demo.rs` — end-to-end SDK usage example

### Changed — workspace

- Added proper attribution to original [pi](https://github.com/earendil-works/pi) project (MIT License, Copyright © 2025 Mario Zechner)
- Updated LICENSE.md with dual copyright notice (pi + oxi contributors)
- Added NOTICE.md with detailed attribution of derived architecture
- Updated README.md, AGENTS.md, CONTRIBUTING.md to reflect port provenance
- Root repository cleaned up: removed 75+ analysis/report markdown files and orphaned source files
- All Korean comments and doc strings translated to English across 15 source files
- `.gitignore` expanded with editor, OS, and profiling exclusions
- `rust-toolchain.toml` added to pin toolchain version
- `deny.toml` added for `cargo deny` dependency auditing
- `.editorconfig` added for cross-editor consistency
- `.cargo/config.toml` added for build configuration
- CI pipeline enhanced with `cargo doc`, `cargo test --doc`, and `cargo deny` jobs
- `docs.rs` metadata added to all library crate Cargo.toml files
- Bumped all crate versions to 0.25.4

### Fixed — oxi-agent

- `truncate.rs` test updated to use emoji-based multi-byte characters

### Fixed — oxi-tui

- `fuzzy.rs` Unicode match test updated for ASCII pattern
- `chat.rs` CJK wrapping tests updated with English text
- `input.rs` CJK input tests updated with ASCII equivalents
- `text.rs` CJK truncation tests updated with ASCII equivalents

## [0.24.0] - 2026-05-19

### Added — oxi-sdk

- Re-export `SearchCache`, `CompactionEvent`, `UserMessage` and all built-in tools (`EditTool`, `ReadTool`, `WriteTool`, `GrepTool`, `FindTool`, `LsTool`, `WebSearchTool`, `GetSearchResultsTool`) for single-dependency access via `oxi-sdk`

## [0.15.1] - 2026-05-16

### Fixed — oxi-agent

- **tool_exec.rs**: Add `+ Send` bound to `FinalizedToolCallEntry::Future` and `pending_futures` type alias, making `AgentLoop::run()` / `run_messages()` / `continue_loop()` futures `Send`-compatible for `tokio::spawn`

### Changed — oxi-sdk, oxi-cli

- Bump `oxi-agent` dependency to 0.15.1

## [0.15.0] - 2026-05-16

(No changelog entry recorded)

## [0.14.0] - 2026-05-16

### Added — oxi-sdk (oxios Agent OS Engine)

- **KernelToolProvider trait** (`oxi-sdk/src/kernel_bridge.rs`): Bridge interface for oxios kernel tools (exec, memory, browser, persona) to be plugged into the SDK agent builder
- **AgentGroup** (`oxi-sdk/src/agent_group.rs`): Multi-agent orchestration with Pipeline/Parallel/Orchestrated strategies
- **MessageBus** (`oxi-sdk/src/message_bus.rs`): Broadcast-based inter-agent communication for oxios environments
- **AgentMetrics** (`oxi-sdk/src/metrics.rs`): Atomic counters for tracking runs, tokens, durations with snapshot export

### Added — oxi-agent

- **Agent::export_state() / import_state()**: Session persistence via JSON serialization of AgentState
- **Agent::continue_with()**: Session continuation within same agent instance
- **Agent::run_tokio_stream()**: Tokio-native event streaming with tokio::sync::mpsc channels (WebSocket/SSE gateway friendly)
- **StructuredOutput** (`oxi-agent/src/structured_output.rs`): JSON extraction and schema validation from agent responses
- **AgentState Serialize/Deserialize**: Full state serialization including messages, tokens, iteration progress
- **AgentConfig::output_mode**: Optional structured output mode configuration

### Added — oxi-ai

- **ProviderPool** (`oxi-ai/src/provider_pool.rs`): Rate limiting and concurrency control with semaphore + sliding window RPM for multi-agent shared API key scenarios

### Added — oxi-sdk / oxi-agent

- **AgentBuilder::kernel_tools()**: Register kernel tools via KernelToolProvider during agent construction

### Fixed — oxi-agent

- **edit_diff.rs**: Detect and reject ambiguous matches (old_text appearing >1 time) with clear error message
- **edit.rs**: Add serde aliases for `old_text`/`new_text` to fix multi-edit JSON parsing
- **grep.rs**: Detect and skip broken symlinks before `read_dir` to prevent crashes

### Fixed — tests

- **edge_cases.rs**: Fix `test_read_large_file` offset (101 for 1-indexed), `test_grep_with_broken_symlink` error handling
- **tools.rs**: Fix `test_bash_working_dir` (handle workspace restriction errors), `test_find_path_not_found` (accept 'Cannot read' error)
- **provider_mock.rs**: Fix `test_empty_stream` expectation (1 Start event, not 0)

### Changed — oxi-agent

- **SharedState now Clone + Arc-based**: `SharedState` wraps `Arc<RwLock<AgentState>>` enabling state sharing across async boundaries
- **AgentInner now Clone**: Inner config/provider cloneable for tokio streaming paths

## [0.13.0] - 2026-05-15

### Added — oxi-cli / oxi-agent

- **Thinking level display in footer**: Model shown with thinking level indicator (e.g., `(minimax) MiniMax-M2.7 • high`)
- **Shift+Tab to cycle thinking level**: Press Shift+Tab to cycle through thinking levels: off → minimal → low → medium → high → xhigh → off
- **Thinking level in TUI footer**: Footer now shows thinking level as secondary info (muted color) next to model name

### Changed — oxi-store

- **ThinkingLevel enum aligned with pi-agent**: Changed from `none, minimal, standard, thorough` to `off, minimal, low, medium, high, xhigh` to match pi-agent naming conventions
- **Default thinking level is now `medium`**: Consistent with pi-agent behavior

### Changed — oxi-cli / oxi-ai

- **Thinking level system prompts updated**: All thinking levels (off, minimal, low, medium, high, xhigh) now have appropriate system prompts with distinct characteristics

### Fixed — oxi-store

- **Fixed failing tests**: Updated environment variable tests to reflect that `apply_env()` and `from_env()` are now no-op (env overrides disabled)
- **Fixed PoisonError in parallel tests**: Removed unnecessary ENV_LOCK usage from tests that don't modify env vars

## [0.8.0] - 2026-05-06

### Added — oxi-agent

- **2-level agentic loop** matching pi-mono architecture: outer loop (follow-up messages), inner loop (tool calls + steering)
- **turn_start / turn_end events** emitted each iteration for lifecycle tracking
- **Steering messages**: inject user messages mid-run via `session.steer()`, polled after each turn
- **Follow-up messages**: queue messages during agent execution, processed when agent would stop via `session.follow_up()`
- **beforeToolCall / afterToolCall hooks** for tool execution pipeline customization
- **shouldStopAfterTurn hook** for graceful early termination
- **ToolExecutionMode** (Sequential / Parallel) config on AgentHooks
- **Terminate flag propagation**: batch terminates only when every tool result sets `terminate: true`
- **Streaming message lifecycle events**: `MessageStart` → `MessageUpdate` (per delta) → `MessageEnd`
- **ThinkingDelta forwarding** to TUI for real-time reasoning display
- **AgentHooks** struct with all hook types (get_steering_messages, get_follow_up_messages, etc.)
- **ToolBatchResult** for batch tool execution results
- **Compaction per iteration**: context window check at each iteration, not just once

### Added — oxi-cli

- **Tool snippets in system prompt**: Available tools now show descriptions instead of "(none)"
- **AgentSession queue → Agent hooks connection**: steering/follow-up queues wired to agent loop
- **Input unlock during agent busy**: typing, paste, and Enter allowed while agent is streaming
- **Enter while busy → queue as steering message** instead of being ignored

### Fixed

- **TurnEnd event**: real assistant message instead of placeholder UserMessage
- **Fallback model logic restored** on stream error
- **turn_number**: incremented before use (was starting at 0)
- **web_search.rs** compilation error simplified
- **Removed dead code**: old `execute_tool()` method, unused imports, Korean comments → English
- **ToolExecutionMode default**: Sequential (parallel was fallback to sequential anyway)

### Changed

- System prompt tool descriptions now populated from `tool_snippets` HashMap
- Agent loop restructured from single loop to pi-mono 2-level loop architecture

## [0.5.0] - 2026-05-05

### Fixed — oxi-ai

- **TextDelta double-push bug** in `high_level.rs` `complete()` function. Text was being pushed to `text_buffer` twice at block boundaries, causing double-counting. Fixed by reordering logic to execute `text_buffer.push_str(&delta)` exactly once.
- **ToolCallStart synthetic ID generation** now uses the actual `tool_call_id` from provider events instead of always generating synthetic IDs.

- **SSE parsing edge cases** comprehensively tested for both OpenAI and Anthropic providers. Added 39 unit tests covering single/multiple events, finish reasons, tool call deltas, usage accumulation, thinking blocks, carriage return line endings, and malformed input handling.
- **Serialization roundtrip tests** added to `types.rs`, `messages.rs`, and `error.rs`. All core types now have comprehensive test coverage for JSON/MessagePack roundtrips.
- Fixed pre-existing `concat!` macro syntax errors in `providers/anthropic.rs` and `providers/openai.rs`.


### Changed — oxi-ai

- `ProviderEvent::ToolCallStart` now carries `tool_call_id: Option<String>` for real tool call IDs from providers.

- `ContentBlockStart` (Anthropic) now includes `id` field.
- `ContentBlockRef` (Bedrock) now includes `id` field.

### Added — oxi-agent

- **Parallel tool execution**: `execute_tool_calls_parallel` now uses `futures::future::join_all` for concurrent execution while preserving result order.
- **Circuit breaker integration**: `CircuitBreaker` from `recovery.rs` is now wired into `AgentLoop`. Configurable threshold and open duration with automatic recovery.
- **18 integration tests** covering multi-turn tool use loop, compaction flow, cross-provider model switching, error recovery scenarios, steering messages, and follow-up queue processing.

### Added — oxi-cli

- **48 AgentSession tests** covering model cycling, thinking level changes, steering/follow-up queues, compaction trigger logic, session persistence, and event subscriptions.

## [0.1.0-alpha] - 2025-05-03

Initial alpha release of the oxi workspace.

### Added — oxi-ai

- Unified LLM API with provider-agnostic `Context` and `Message` types
- Streaming response handling via async `ProviderEvent` streams
- Multi-provider support (OpenAI, Anthropic, Google, Ollama, OpenRouter)
- Tool/function calling with typed definitions and responses
- Token estimation with hybrid algorithm (character + token heuristic)
- Conversation context management and message compaction
- Cross-provider message transformation
- JSON Schema validation for structured outputs

### Added — oxi-agent

- Agent runtime with streaming event loop
- `AgentTool` trait for defining LLM-callable tools
- `ToolRegistry` for tool management and dispatch
- Built-in tools: read, write, edit, bash, web search, questionnaire, review loop
- Context compaction for long conversations
- Tool streaming and progress updates
- Agent event types (thinking, text, tool calls, completion)

### Added — oxi-tui

- Component-based terminal UI framework
- Differential rendering (line-level dirty tracking)
- Theme system with TOML/JSON hot-reload
- Built-in components: Text, Input, Editor, Markdown, Completion
- Overlay system for modals and popovers
- Image rendering with Kitty and iTerm2 protocol support
- Chat view with streaming display
- Unified keyboard, mouse, and resize event handling

### Added — oxi (CLI)

- Interactive REPL for chatting with LLMs
- Session system with persistence and branching
- CLI argument parsing via clap
- Skill/template system for reusable prompt patterns
- Extension loading system for dynamic plugins
- Error handling and recovery
- TUI integration for interactive mode

### Added — Skills

- Brainstorming skill for collaborative ideation
- Deep-research skill for investigation and design
- Scout skill for fast codebase reconnaissance
- Super-review skill for deep system analysis
- Design-farmer skill for design system construction
- Playwright CLI skill for browser automation
- Worktree skill for git worktree management
- Obsidian skill for vault operations

### Infrastructure

- Workspace with 4 crates: oxi, oxi-ai, oxi-agent, oxi-tui
- Comprehensive test suites for all built-in tools
- Project README files for each crate
- MIT license
