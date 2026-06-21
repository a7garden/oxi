# Changelog

All notable changes to the oxi project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Added — selectable Unicode / ASCII / Nerd Font glyph set (default: Unicode)

Every UI symbol (status markers, list cursors, box drawing, spinners, icons)
now comes from a pluggable glyph-set table, so the whole UI can switch
rendering styles from one setting. Based on the omp (oh-my-pi) symbol-preset
design.

- **New `oxi_tui::symbols` module**: `GlyphSet` enum (`Unicode` / `Ascii` /
  `Nerd`) + `Symbols` table (`Copy`, all `&'static str`). Three preset
  constructors; `GlyphSet::default()` is `Unicode`. Symbol codepoints are
  standards-defined (Unicode box-drawing range + Nerd Fonts PUA).
- **`Theme` / `ThemeStyles` carry the active `Symbols`**: `with_glyph_set` /
  `set_glyph_set`; `to_styles()` propagates them, so every render fn that
  already takes `&ThemeStyles` gets the glyph set with no signature change.
- **Hardcoded glyphs migrated**: `tool_renderer` (✓/✗/⚠/→), the chat
  tool-call status icons (○/●/✓/✗ on every call + result box), list
  highlight cursors (`stateful_list`, `table_list`, `completion`, and 7
  overlays), health/status dots (`routing`, `dashboard`, todo panel), the
  chat spinner, todo status markers, and horizontal rules now read from the
  symbol table.
- **New `glyph_set` setting** (`settings.toml`, snake_case): `unicode` |
  `ascii` | `nerd`. Settings version bumped 7 → 8 with migration (defaults to
  Unicode). Selectable live in `/settings` → `glyph` (cycles the three
  presets) and **applied immediately** — the main loop rebuilds the live
  theme from freshly-loaded settings on the next draw, no restart needed.


## [0.39.0] - 2026-06-20
### Added — SDK consumers can now use the todo tool with observable state

`oxi-sdk` exposes the todo tool (`TodoProvider::default()`) for SDK consumers,
backed by observable state via `RegistrySnapshot`.

- **New `TodoProvider` trait** in `oxi_sdk::tool_providers` (with default impl
  that wraps the built-in `TodoState` + `InMemoryTodoStore`).
- **`RegistrySnapshot` observable state**: SDK consumers can subscribe to
  `AgentGroupState` registries via `.state()`, getting a diff-driven snapshot
  (`RegistrySnapshot`) that includes `todo_state: Vec<String>` for live TUI
  rendering without polling.
- **Todo tool wiring in `OxiBuilder`**: `with_todo_provider()` to register a
  custom impl; default is `TodoProvider::default()` noop-compatible.
- **Dependency**: the todo tool (always available, no `essential` flag), crate
  metadata, and `constants` module are now exported from `oxi-sdk`.


## [0.38.0] - 2026-06-20
### Changed — HTML export tool 렌더링 재설계 (C1)

`oxi-cli/src/storage/export.rs` 의 도구 호출 렌더링을 구조화된
세션 데이터 기반으로 전면 재작성했다.

* **데드 코드 제거**: 이모지 prefix (`🔧`/`📤`/`📄`/`📝`/`✏️`/`🔍`) 를
  파싱하는 `render_tool_blocks`, `extract_path_from_line`, `ToolOp` enum,
  5개 fused 렌더러 (`render_bash_tool` 등), `render_markdown_with_options`
  내 이모지 분기, 그리고 이들이 사용하던 ~80줄의 dead CSS 를 삭제했다.
  이 코드들은 실제 프로듀서가 없는 self-fulfilling 코드였다.
* **구조적 렌더링 추가**: `AssistantContentBlock::ToolCall` 을 도구 이름별로
  디스패치하는 `render_tool_call_block`, `AgentMessage::ToolResult` 를
  bare `.tool-result` div 로 렌더하는 `render_tool_result_block` 를 추가.
  `render_entry` 가 어시스턴트 블록을 순서대로 순회하도록 재구조화했다.
* **`include_tool_calls` 의미 재정의**: 이모지 라인 필터링에서 구조적
  엔트리 스킵으로 변경. `false` 시 `ToolCall` 블록과 `ToolResult` 엔트리를
  완전히 제외한다.
* **`find` 도구 라벨 수정**: 검색어(`name`)가 아닌 디렉토리(`path`) 가
  표시되던 문제를 수정했다.
* **`extract_text` 헬퍼**: `ContentValue` → text 추출 로직을 공용 함수로
  통합하여 User/System/ToolResult arm 의 중복을 제거했다.
* `BashExecution` 변형은 생산자가 없으므로 렌더링을 유보했다
  (설계 문서 §3.7 참조).


### Fixed — release/publish 워크플로우 (v0.37.1 릴리스 중 발견)

v0.37.1 릴리스 파이프라인에서 두 가지 자동화 결함이 드러나 수정했다.

* **release.yml `trigger-publish` 잡에 checkout 누락**:
  `gh workflow run publish.yml` 이 로컬 git 체크아웃에서 대상 워크플로우
  파일을 식별하는데, checkout 단계가 없어 `fatal: not a git repository`
  로 실패하던 것을 `actions/checkout@v5` 추가로 수정. 결과적으로
  v0.37.1 은 GitHub Release 생성까지는 성공했지만 publish.yml 이
  자동 dispatch 되지 않아 수동 dispatch 로 이어졌다.
* **publish.yml 멱등성 부재**: 일부 크레이트만 게시된 뒤 일시적 네트워크
  에러(curl HTTP2 framing)로 매트릭스가 중단된 경우, 재실행하면 이미
  게시된 크레이트에서 `already exists` 에러로 실패해 남은 크레이트에
  도달하지 못하던 것을 수정했다. 각 게시 단계가
  `already exists/already uploaded/already been published` 메시지를
  감지하면 성공으로 처리하도록 했다. (이번 0.37.1 의 `oxi-cli` 가 이
  케이스로 실패했고, 로컬에서 직접 게시해 마무리했다.)

## [0.37.1] - 2026-06-19

### Fixed — CI 워크플로우 (v0.37.0 릴리스 직후 발견)

v0.37.0 릴리스 시도에서 세 가지 CI 인프라 문제가 드러났다.

* **SBOM** (`sbom.yml`): `cargo cyclonedx` 가 각 워크스페이스 멤버마다
  `oxi.json` 을 생성하는데 (not `target/`), 워크플로우는 단일
  `target/oxi.cdx.json` 을 기대해 `find: 'target': No such file` 로
  실패하던 것을 수정. `oxi-cli` (전체 의존성 트리 포함) 을 단일 SBOM 으로
  취합.
* **Sync Labels** (`labels.yml`): 존재하지 않는 `github/issue-labeler@v2`
  태그 + 잘못된 도구 (이슈 분류용) → `.github/labels.yml` 포맷과 정확히
  호환되는 `EndBug/label-sync@v2` 로 교체.
* **crates.io publish 자동화** (`release.yml` + `publish.yml`):
  GITHUB_TOKEN 으로 만든 GitHub Release 는 `release: published` 이벤트를
  다른 워크플로우로 전파하지 않아 `publish.yml` 이 자동 실행되지 않던
  것을, `release.yml` 에 `trigger-publish` 잡을 추가해 Release 생성 직후
  `gh workflow run` 으로 명시적 dispatch 하도록 수정.

### Fixed — published `oxi-sdk` 0.37.0 가 downstream 에서 컴파일되지 않던 결함 (crates.io 패키징 버그)

crates.io 에 게시된 `oxi-sdk` 0.37.0 를 의존하는 consumer(oxios 등)가
컴파일할 수 없었던 치명적 패키징 결함을 수정했다.

```
error: couldn't read '.../oxi-sdk-0.37.0/src/ports/fs/../../../../oxi-ai/data/catalog/_snapshot.json.gz':
       No such file or directory
  --> oxi-sdk-0.37.0/src/ports/fs/catalog.rs:247
    include_bytes!("../../../../oxi-ai/data/catalog/_snapshot.json.gz")
```

- **근본 원인**: `oxi-sdk/src/ports/fs/catalog.rs::load_snapshot()` 가
  크레이트 바깥(형제 크레이트 `oxi-ai/data/`)을 가리키는 경로로
  `include_bytes!` 를 썼다. oxi 워크스페이스 안(in-tree) 에서는 상대경로가
  해석되어 빌드/테스트가 통과하지만, crates.io 배포판은 `oxi-sdk` 자체
  파일만 포함하므로 게시된 tarball 안에서 해당 파일이 존재하지 않는다
  → consumer 컴파일 실패.
- **왜 게시까지 통과했나**: `publish.yml` 의 게시 단계와 사전 검사 단계가
  **모두** 컴파일 검증을 건너뛰었다 — 사전 검사는
  `cargo package --no-verify --list` (메타데이터/파일 조립만), 게시는
  `cargo publish --no-verify`. `include_bytes!` 경로 문제는 in-tree
  빌드로는 절대 잡히지 않고, 오직 **게시된 tarball 을 registry 의존성에
  대해 컴파일**(`cargo publish` 의 verify 단계)할 때만 드러난다.

**수정**:

- **`oxi-ai`**: `catalog::snapshot_gzip_bytes() -> &'static [u8]` 신규
  공개 접근자. `oxi-ai` 자기 트리 안의 자체 포함 `include_bytes!` 로
  snapshot 의 단일 진실 소스가 된다 (`catalog/materialize.rs` +
  `catalog/mod.rs` 재내보내기). 기존 `load_snapshot_catalog()` 도 이 접근자를
  사용하도록 통일.
- **`oxi-sdk`**: `load_snapshot()` 가 직접 `include_bytes!` 하던 것을
  `oxi_ai::catalog::snapshot_gzip_bytes()` 호출로 교체. 크레이트 바깥으로
  빠져나가는 경로를 완전히 제거했고, oxi-sdk 의 자체 `MdCatalog` 스키마로
  파싱하는 기존 동작은 그대로 유지.
- **`publish.yml`**: 게시 단계의 `cargo publish --no-verify` 에서
  `--no-verify` 제거. 이제 각 크레이트가 자기 registry 의존성에 대해
  컴파일 검증된다 (위상 순서 게시 + 의존성 가시성 폴링이 이미
  선행의존성의 신규 버전을 보장). 사전 검사(package-check) 단계는
  메타데이터 검사로 유지하되, 실제 컴파일 게이트는 게시 단계임을
  주석로 명시했다.

**검증**: in-workspace 빌드 + 단위테스트 회귀 없음 (oxi-ai 553 / oxi-sdk 314 /
catalog_port 19 전부 통과). `cargo package`(verify) 로 게시 시나리오 재현 —
`oxi-sdk` 패키지 검증이 `oxi-ai` 0.37.1 의 registry 가시성을 요구함을
확인(위상 순서 게시로 해결됨).

## [0.37.0] - 2026-06-18

### Added — Catalog Port (12번째 port, models.dev 동적 카탈로그)

SDK 에 **catalog port** (`ModelCatalog` trait) 를 추가하여, 모델/프로바이더
메타데이터를 동적으로 수신할 수 있게 했다. 이는 정적 TOML 카탈로그에서
동적 models.dev 기반 시스템으로의 전환을 완성하며, SDK consumer 가 모델
갱신을 런타임에 수신할 수 있도록 한다.

> **설계 문서**: `docs/designs/2026-06-17-catalog-port-design.md` (v4, §7.9
> sync API 회피 정정 포함). 데이터 흐름은
> `docs/designs/2026-06-17-dynamic-catalog-design.md`.

- **신규 port** `oxi-sdk/src/ports/catalog.rs`: `ModelCatalog` trait (async
  read + refresh + subscribe) + **sync read API** (`*_sync`, §7.9). noop 기본값.
  `CatalogProtocol` enum (SDK 소유, oxi-ai `Api` 와 역방향 의존 없음).
  `CatalogModelEntry`, `CatalogProviderEntry`, `CatalogEvent`, `RefreshOutcome`.
- **신규 bridge layer** `oxi-sdk/src/bridge.rs`: `catalog_entry_to_model()`,
  `provider_base_url()`, modality 변환. SDK 소유 (oxi-ai 역방향 의존 방지).
- **참조 구현** `oxi-sdk/src/ports/fs/catalog.rs`: `FileModelCatalog` —
  embedded SNAP + runtime cache + ETag 조건부 GET + user overrides +
  LOCAL `/v1/models` discovery. lazy on-call refresh (백그라운드 작업 없음).
- **`OxiBuilder::with_catalog()`** / **`Oxi::catalog()`** 접근자.
- **`Oxi::resolve_model()`** catalog fallback 통합 (sync 유지, §7.9).
- **`SdkError` catalog 변종** 3개: `CatalogUnavailable`, `CatalogOverrideParse`,
  `CatalogRefresh`.

**sync read API (§7.9, 구현 중 단순화)**: catalog 데이터가 이미 메모리에
존재하므로, read-only 조회는 I/O 가 아닌 단순 락 획득 + clone 이다. 이로 인해
v3 설계가 명시했던 `ProviderResolver` trait async화 ripple (agent_loop /
multi_provider / fallback_chain 전체) 을 **전면 회피**했다. PR 3 이 "대공사"에서
bridge layer + resolve_model 통합으로 축소되었다.

**oxi-cli 이관**: composition root (`services::build_oxi`) 가
`FileModelCatalog::init()` + `with_catalog()` 로 catalog 를 등록한다. TUI
(`AppState.catalog` 필드 주입), setup wizard, `oxi models` / `oxi refresh`
명령이 모두 catalog port 기반으로 동작한다. legacy `init_models_dev()` 제거.

**하위 호환성**: legacy free fn (`get_all_models`, `get_provider`, 등) 은
**fallback path 로 유지** (catalog 가 `None` 일 때만). custom provider 동적 등록
(`fetch_models_blocking`/`register_model`) 과 `oxi-ai/src/catalog/` 모듈은
SNAP 데이터 위치 때문에 제거하지 않고 다음 메이저 버전으로 연기.

**테스트**: catalog port 19개 (sync API 2개, bridge 7개, resolve_model 통합 2개
포함) + 전체 회귀 **2297/2297 통과**.

### Fixed — issue 시스템 소유권 복원 (Phase 0 / 결함 #13)

에이전트가 `issue` 도구의 `start`/`close`를 호출할 때 소유권/liveness
검사가 **조용히 우회**되던 결함을 수정했다. 근본 원인: `agent.rs`가
`AgentLoopConfig.session_id`를 항상 `None`으로 하드코딩하여,
`ToolContext.session_id`가 `None` → 도구 caller id가 빈 문자열(`""`)이
되었고, 빈 문자열은 어떤 `flock` 홀더와도 매칭되지 않아 모든 할당이
즉시 reclaim 가능했다 (두 에이전트가 같은 이슈를 `start` 하면 마지막이
조용히 승리).

- **`oxi-agent`**: `AgentConfig.session_id: Option<String>` 신규 필드
  (additive, `#[serde(default)]`, `with_session_id` 빌더). `agent.rs`의
  두 `AgentLoopConfig` 생성지점이 이제 config에서 `session_id`를 주입.
- **`oxi-cli`**: `bootstrap.rs::build_app`가 run-mode별 ownership identity
  생성 — TUI 모드는 `liveness::TUI_OWNERSHIP_ID`("tui"), 그 외는
  `proc-<pid>-<uuid>`. `App::from_oxi(..., ownership_session_id)`가 프로세스
  수명 동안 `flock`을 잡고 `AgentConfig.session_id`에 동일 id 주입.
  `liveness::TUI_OWNERSHIP_ID` 상수가 단일 진실 소스 — 에이전트 도구·
  TUI 패널·`/issue` 슬래시 명령이 모두 같은 flock 홀더를 본다.
- **`tui/app.rs`**: `run_tui_interactive_impl`의 중복 flock 획득 제거
  (`App`가 이제 보유). `debug_assert!`로 identity 일치 검증.
- **회귀 테스트**: `session_id_wiring_tests`(oxi-agent,
  `build_tool_context` 레벨), `start_with_distinct_live_owners_collides` +
  `empty_session_assignment_is_immediately_reclaimable_documentation`(oxi-cli).

> 설계 문서: `docs/designs/2026-06-17-issue-system-hardening.md` (P0–P4 전부).

### Fixed — `atomic_write` temp 이름 UUID 접미 (Phase 1 / 결함 #1)

`store::issues`·`store::session`의 `atomic_write`가 temp 파일을
`tmp.<pid>`로 명명했다. PID-namespace 재활용(컨테이너)이나 fork+exec에서
두 프로세스가 같은 PID로 같은 temp 를 덮어 한 쪽 write 가 손실될 수 있었다.

- **신규** `oxi-cli/src/store/fs_util.rs`: `atomic_write`/`atomic_write_bytes`.
  temp 이름을 `<base>.tmp.<pid>.<uuid-simple>`로 변경 (PID는 디버깅용,
  UUID 가 유일성 보장). rename 실패 시 best-effort orphan 제거.
- issues/session 양쪽 로컬 복사본 제거 → 공유 헬퍼로 마이그레이션.
- 테스트: 16스레드 동시 동일경로 쓰기, temp 이름 형식, rename 실패 시 orphan 미누출.

### Changed — CAS 재시도 + `IssuePatch` + `reopen` + no-op 감지 (Phase 2 / #2 #3 #4 #9 #12)

저장소는 **엄격**을 유지(원시 `Conflict` 반환, 재시도 없음)하고, **도구만**
회복을 담당한다. 소유권 정책(assignee 한정)은 **유지**.

- **`store::update`**: no-op 감지(#12) — 직렬화된 before/after 를 `updated_at` 를
  정규화해 비교; 의미 변화가 없으면 쓰기/타임스탬프/cache invalidate 스킵.
- **`IssuePatch`**(#3): 모든 필드 `Option` (None=유지, Some=교체). `labels`만
  의미적 공란 — `Some([])`=전체 삭제, `None`=유지. 도구 스키마로 표현 못 하던
  absent vs `[]` 구분 해소.
- **`apply_patch`**: 정밀 patch 를 엄격 CAS 로 적용, 소유권 **강제**(다른
  assignee → `NotAssigned`). `status=Open` 시 `closed_at` 도 클리어(#4 잠재 버그 수정).
- **`reopen`** 액션(#4): `status=Open` + `closed_at=None`.
- **도구 `cas_retry`**(#2): bounded CAS 회복(4회). 첫 시도는 에이전트 hash(빠른
  경로), conflict 시 fresh hash 재독 후 재시도 — stale hash 가 advisory 로 작동.
  모든 변형 액션이 이를 경유. `update` 는 `apply_patch`+`cas_retry` 조합.
- 테스트: stale hash 회복·바운드 후 포기; reopen/apply_patch 의 closed_at 클리어;
  no-op 미갱신; labels keep/clear/replace; 소유권 강제.

### Changed — 스키마 정밀화 + 크기 상한 + github readOnly (Phase 3 / #5 #6 #7)

- **`validate_size`**(#5): `create`/`update` 의 과대 페이로드 조기 거부 —
  title≤512자, body≤256KiB, labels≤32, 라벨당≤64자. 디스크 채우기 방지.
- **스키마 description**(#6, #7): `status` 이중의미 정리(list 필터 vs update 값,
  close/reopen 권장), `labels` REPLACES 시맨틱, `content_hash` ADVISORY 표기,
  `github` `readOnly` 속성 추가(Phase 6 동기화 전용). 도구 최상위 설명에
  update 필드 관례·reopen 워크플로우·자동재조정 정책 명시.
- 테스트: 소형 통과·비텍스트 액션 스킵·body/title/라벨수/긴라벨 거부.

### Changed — orphan 수거 + `top_free_priority` + flock 헬퍼 (Phase 4 / #8 #10 #11)

- **`liveness::reap_orphans`**(#8): `.alive/` dead 파일 수거(best-effort,
  멱등, TOCTOU 안전). (1) 홀더 체크(is_session_alive)로 live 락 미건드림,
  (2) `ORPHAN_AGE_SECS`(1h) age gate 로 최근 파일 보존. `FileIssueStore::open` 에서
  lazy 호출(실패는 warn 만, 시작 차단 없음).
- **flock 헬퍼**(#11): 산재하던 2곳의 `unsafe libc::flock` 호출을
  `try_flock_exclusive`/`probe_flock_shared` 명명 함수로 중앙화(SAFETY 주석).
- **`top_free_priority`**(#10): open + 미할당 이슈 중 최대 우선순위 —
  "지금 당장 손댈 것" 신호. Cache 필드로 계산, 접근자 노출.
- 테스트: reap 멱등/최근 dead 보존(age gate)/오래된 dead 제거+live 보존;
  top_free_priority 가 할당·닫힌 이슈 무시.

### Added — `oxi issue` CLI `reopen`·`reap` 서브커맨드

설계 §11 권장 항목. store와 에이전트 도구에 이미 노출된 `reopen`/
`reap_orphans`의 CLI 래퍼.

- `oxi issue reopen <id> [--hash]`: 닫힌 이슈 재개(status=Open, closed_at 클리어).
  close 와 달리 소유권 락 불필요(닫힌 후엔 owner 없음).
- `oxi issue reap`: `.oxi/issues/.alive/` dead 파일 수거(age-gated, 멱등) 후
  제거 개수 출력. 주의: store 생성자가 자체 lazy reap 을 돌리므로, 이 명령은
  store 를 열지 않고 디렉토리를 직접 reap 해 **정확한 카운트**를 보고한다.
  (없으면 double-reap 으로 0이 된다.)

### Fixed — `oxi issue close` CAS 경쟁 (기존 버그)

`close` 핸들러가 `start`→`close` 호출에 **같은 content_hash** 를 재사용했다.
`start` 가 assignment 를 쓰면서 파일(와 해시)을 바꾸기 때문에, 이어지는
`close` 가 거의 항상 `Conflict`("was modified since last read")로 실패했다
(미할당 이슈를 close 할 때 특히). `start` 직후 파일을 다시 읽어 fresh 해시를
`close` 에 넘기도록 수정.

### Fixed — Production readiness (crates.io publish)

main 의 두 CI 잡이 실패 상태였고, 그대로는 crates.io publish 가 막히는
상황이었다.

* **doc** (ci.yml, `RUSTDOCFLAGS=-D warnings`): oxi-ai 와 oxi-sdk 의
  깨진 intra-doc 링크 6곳, oxi-cli 의 모호한 링크 1곳 수정.
* **test-doc** (test.yml, `cargo test --doc`): 컴파일조차 안 되는 doctest
  2건 수정 — `build_oxi_engine` 이 `async` 가 된 뒤 `.await` 가 누락된 것,
  `OxiBuilder::with_catalog` 예시의 빈 `/* ... */;` RHS.

### Changed — clippy `--all-targets` 게이트 강화

`cargo clippy --workspace --all-targets -- -D warnings` 가 **완전히 clean**
하도록 정리했다 (기존 ~1448 warning, 전부 test/bench/example 코드).
shipped 라이브러리는 엄격함을 유지하고, test 코드는 정확히 두 가지
test-idiom lint (`clippy::unwrap_used`, `clippy::field_reassign_with_default`)
만 `#![cfg_attr(test, allow(...))]` 로 완화했다. 나머지 모든 lint
(correctness/suspicious/style/complexity) 는 test 코드에서도 그 자리에서
수정했다.

* ci.yml 의 `clippy` 잡과 `.pre-commit-config.yaml` 의 hook 을 `--workspace`
  에서 `--workspace --all-targets` 로 강화 (AGENTS.md 의 "Pre-existing TODO"
  후속 작업 완료).
* oxi-ai 의 `benches/` exclude 를 제거해 패키지된 크레이트가 벤치마크 소스를
  포함하도록 수정 (`cargo package` 경고 제거).

## [0.36.0]

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

### Changed — TUI 언어 정책 default OFF + 자동 적용

기존 TUI 언어 정책(`Settings::output_languages`)은 **사용자 설정이 있어도
기본적으로 활성화되지 않도록** 변경되었으며, 오버레이 변경이 라이브
세션에 즉시 반영되도록 개선되었다. `oxi --print` 및 RPC 모드의 비대칭은
**의도된 설계**이므로 변경되지 않았다 (AGENTS.md pitfalls 참조).

- **Master toggle 신설** (`Settings::language_policy_enabled: bool`,
  `oxi-cli/src/store/settings.rs`): default `false`. `output_languages`
  맵에 값이 있어도 이 플래그가 `false`이면 정책이 주입되지 않는다.
  신규/기존(v5) 사용자 모두 OFF로 시작한다. `/settings` 오버레이에서
  명시적으로 ON 해야 동작.
- **자동 적용** (`oxi-cli/src/tui/overlay/settings.rs`,
  `oxi-cli/src/app/agent_session.rs`): `/settings` 오버레이 Esc 시
  `changed=true`이면 `persist_changes()` + `AgentSession::rebuild_system_prompt()`
  를 자동 호출한다. 디스크 저장이 `set_system_prompt()`까지 단일 흐름으로
  연결된다. `/reload` 슬래시 명령은 백업 경로로 유지.
- **in-memory 캐시 동기화** (`rebuild_system_prompt`): 호출 직전에
  디스크에서 fresh load하여 `AgentSession::settings` `Arc<RwLock<Settings>>`
  를 교체한다. overlay가 `AgentSession` mutable API를 알 필요 없이
  결정적으로 동기화됨.
- **OFF 시 채널 설정 보존**: `language_policy_enabled`를 false로 두어도
  `output_languages` 맵은 디스크에 보존된다. 다시 ON 하면 이전 채널
  매핑이 그대로 적용됨.
- **Disabled UI** (`SettingsItem::Choice::disabled: bool`): OFF일 때
  채널 항목 4개는 회색으로 표시되고 `Enter`/`Space`로 순환되지 않는다.
  시도 시 "Enable language_policy first." notification 표시.
- **시그니처 변경** (`oxi-cli/src/prompt/system_prompt.rs`,
  `oxi-cli/src/app/agent_session_runtime.rs`):
  `language_directive(enabled: bool, channels: &HashMap<...>)` —
  마스터 게이트 신규. `build_system_prompt(thinking, enabled, languages)`,
  `build_compaction_instruction(enabled, languages)` — `enabled` 인자 추가.
  기존 8개 테스트 시그니처 반영 + 신규 8개 테스트 추가.
- **마이그레이션** (`Settings::SETTINGS_VERSION: 5 → 6`): 누락 시
  `#[serde(default = "default_false")]`로 안전하게 false로 떨어진다.
  별도 데이터 변환 없음 (필드 추가 only).
- **문서**: `AGENTS.md` pitfalls에 "TUI-only — by design, not oversight"
  및 채널이 분류기가 아님을 명시. `/settings` 슬래시 description 정정.
  `docs/designs/2026-06-17-tui-language-policy.md` 신규.

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
- **`publish.yml`** (new) — publishes all 6 workspace crates to
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

[0.39.0]: https://github.com/a7garden/oxi/compare/v0.38.0...v0.39.0
[Unreleased]: https://github.com/a7garden/oxi/compare/v0.39.0...HEAD

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
