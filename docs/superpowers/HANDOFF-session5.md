# oxi omp-정렬 — Session 5 Handoff

> **작성일**: 2026-07-28 (session 5 종료 시점)
> **작성 위치**: docs/superpowers/HANDOFF-session5.md
> **목적**: 다음 세션에서 P0.5 (remote-AGENT providers) + P2 (TUI 재정렬) + P4.2 후속 작업을 이어하기 위한 자세한 인수인계
> **현재 작업 위치**: main 브랜치, 미커밋 (git status는 아래 참조)

---

## 0. 현재 상태 (5초 요약)

| 항목 | 상태 |
|---|---|
| **소스 위치** | `/Volumes/MERCURY/PROJECTS/oxi` |
| **Git 브랜치** | `main` |
| **미커밋 변경** | 있음 (P3.3 + P4.1 + P4.2 + P1.6a + RESUMING.md update) |
| **테스트 기준선** | **1907 tests passing** (oxi-cli 763, oxi-agent 746, oxi-sdk 398) |
| **clippy** | clean (-D warnings) |
| **fmt** | clean |
| **마지막 커밋** | `6bda2f9f docs: update RESUMING.md for session 3` |

**남은 우선순위 (다음 세션이 시작할 곳)**:
1. **P0.5** remote-AGENT providers — Cursor / Devin / GitLab Duo (~2000 lines)
2. **P2** TUI 재정렬 — `oxi-tui-legacy` (22499 lines) → omp `tui.ts` (4270 lines) 모델 (~10000 lines 작업)
3. **P4.2 후속** — `RuntimeConfig` / `ProjectPluginOverrides` / `Doctor` 추가 (P0.5와 함께 또는 별도)

---

## 1. Session 5에서 완료한 작업

### 1.1 P3.3 — main.rs 핸들러 분리 (F-5) ✅

**문제**: `oxi-cli/src/main.rs` 1779줄에 handler 함수 ~1400 LOC가 inline 정의됨.

**변경**:
- `oxi-cli/src/main.rs` → **62줄** (main + handle_subcommand match arm만)
- `oxi-cli/src/cli/commands/` 디렉토리 신규 생성 (10개 파일):
  - `mod.rs` (24줄) — module declarations + re-exports
  - `sessions.rs` — `handle_sessions` / `handle_tree` / `handle_fork` / `handle_delete`
  - `issue.rs` — `handle_issue`
  - `pkg.rs` — `handle_pkg`
  - `ext.rs` — `handle_ext`
  - `config.rs` — `handle_config` + 9개 sub-handler + 2개 parser helper
  - `setup.rs` — `handle_setup`
  - `reset.rs` — `handle_reset` + `ResetTarget` struct + 6개 helper
  - `export.rs` — `handle_export` / `handle_import` / `handle_share`
  - `misc.rs` — `handle_completions` / `handle_install` / `handle_update` / `handle_commit` / `handle_refresh` / `handle_models` / `build_catalog_for_cli`
- `oxi-cli/src/cli.rs` — `pub mod commands;` 추가
- `oxi-cli/src/main.rs` — `use oxi::cli::commands::*;`

**함정이슈 (다음 세션이 같은 실수 안 하게)**:
- handler 파일이 **library crate 내부**에 있으므로 `oxi::` 경로 → `crate::` 로 일괄 변경 필요 (sed로 처리함)
- `commands` 모듈은 `pub mod` (binary main.rs에서 접근 가능), `pub use` re-export는 `pub(crate)` 함수도 widen 가능

### 1.2 P4.1 — Issue 시스템 격리 ✅

**문제**: `oxi-cli/src/store/issues.rs` 2020줄 단일 파일.

**변경**:
- `oxi-cli/src/store/issues.rs` 삭제
- `oxi-cli/src/store/issues/` 디렉토리 신규 생성 (7개 파일):
  - `mod.rs` — module declarations + public re-exports
  - `error.rs` — `IssueError` enum
  - `types.rs` — `Status` / `Priority` / `Assignment` / `GithubRef` / `IssueMeta` / `Issue` / `IssuePatch`
  - `serialize.rs` — `parse_issue` / `serialize_issue` / `content_hash` / `issue_filename` / `issues_dir` / `slugify` / `empty_meta` + tests
  - `liveness.rs` — `TUI_OWNERSHIP_ID` / `acquire` / `is_session_alive` / `reap_orphans` / `AliveGuard` + 5 tests
  - `filter.rs` — `IssueFilter` + `matches` impl
  - `store.rs` — `Cache` / `Inner` / `IssueSummary` / `FileIssueStore` + ~500 lines tests

**호환성**: 모든 `crate::store::issues::*` 경로 그대로 사용 가능. AGENTS.md에서 경고한 "Phase 0 / defect #13" liveness identity invariant 보존.

### 1.3 P1.6a — debug 도구 재등록 ✅

**문제**: `oxi-agent/src/tools.rs`의 `all_tools.push(Box::new(debug_tool::DebugTool));` 라인이 주석 처리되어 있어서 debug 도구가 등록 안 됨.

**변경**:
- `oxi-agent/src/tools.rs:1157-1159` — 주석 해제, debug 도구 등록
- `oxi-agent/tests/tools.rs:1100,1107` — 도구 카운트 `37` → `38`

**현재 tool count**: 38 (debug 도구 포함)

### 1.4 P4.2 — Package manager 모듈화 ✅

**문제**: `oxi-cli/src/storage/packages.rs` 3096줄 단일 파일.

**변경**:
- `oxi-cli/src/storage/packages.rs` 삭제
- `oxi-cli/src/storage/packages/` 디렉토리 신규 생성 (9개 파일, 총 3214줄):

| 파일 | 줄 | 내용 |
|------|------|------|
| `mod.rs` | 86 | 모듈 선언, 상수, public re-exports |
| `types.rs` | 225 | `ResourceKind`, `PackageManifest`, `DiscoveredResource`, `PathMetadata`, `ResourceOrigin`, `SourceScope`, `ResolvedResource`, `ResolvedPaths`, `ProgressEvent`/`ProgressEventType`/`ProgressAction`/`ProgressCallback`, `PackageUpdateInfo`, `ConfiguredPackage` |
| `source.rs` | 221 | `ParsedSource` enum + `parse_npm_spec` / `split_git_path_ref` / `parse_git_source` + `NPM_SPEC_RE` regex |
| `npm.rs` | 95 | `NpmPackageInfo` + `get_latest_npm_version` |
| `git_ops.rs` | 157 | `git_command` / `git_command_silent` / `git_clone` / `git_update` / `git_has_update` |
| `lockfile.rs` | 210 | `LockEntry` / `Lockfile` / `ResourceCounts` + `compute_dir_hash` / `verify_lockfile_integrity` / `collect_file_paths` |
| `discovery.rs` | 181 | `discover_extensions` / `discover_skills` / `discover_prompts` / `discover_themes` + recursive walkers |
| `fs.rs` | 62 | `copy_dir_recursive` / `find_single_subdir` / `prune_empty_parents` |
| `manager.rs` | 1977 | `PackageManager` struct + all impl methods + 36 tests |

**함정이슈**:
- constants `MANIFEST_NAME` / `NPM_MANIFEST_NAME` / `LOCKFILE_NAME` 은 mod.rs에 `pub(super) const`로 정의
- manager.rs에서 `use super::{LOCKFILE_NAME, MANIFEST_NAME, NPM_MANIFEST_NAME};` 로 가져와야 함
- (subagent 초기 PR에서 빠뜨려서 컴파일 실패 → fix함)

**호환성**: `crate::storage::packages::*` 모든 경로 그대로 사용 가능. `oxi::storage::packages::PackageManager`, `oxi::storage::packages::ResourceKind` re-export도 lib.rs에서 그대로 살아있음.

### 1.5 omp plugins 모델 정렬 상태

**현재 정렬됨**:
- `ParsedSource` enum 4종 (Npm/Git/Local/Url) — omp `parsePluginSpec` 1:1 매핑
- `Lockfile` + `LockEntry` 패턴 (per-source metadata + integrity hash) — omp `omp-plugins.lock.json`과 동일 패턴
- `ResourceKind` enum (Extension/Skill/Prompt/Theme)

**아직 정렬 안 됨 (P4.2 후속 작업)**:
- `RuntimeConfig` (per-plugin enabled state) — 현재는 `Settings`의 extensions/skills/prompts/themes 배열로 우회
- `ProjectPluginOverrides` (per-project plugin disable) — 현재는 없음
- `Doctor` (health checks) — 현재는 `validate_package` warning list로 부분 구현
- shell metachar 가드 (`validatePackageName` / `validateGitSpec`) — 현재 미적용

---

## 2. 다음 세션: P0.5 — remote-AGENT providers (~2000 lines)

### 2.1 목표

`Api` enum에 이미 존재하는 다음 variant들의 transport를 구현:
- `Api::CursorAgent` — Cursor의 WebSocket + SSE 프로토콜
- `Api::DevinAgent` — Devin의 WebSocket + SSE 프로토콜
- `Api::GitLabDuoAgent` — GitLab Duo의 REST API

### 2.2 현재 상태 (탐색 가이드)

**Api enum 위치**: `oxi-catalog/src/api.rs:25-68`

```rust
pub enum Api {
    OpenAiCompletions, OpenAiResponses, OpenRouter,
    OpenAiCodexResponses, AzureOpenAiResponses,
    AnthropicMessages, BedrockConverseStream,
    GoogleGenerativeAi, GoogleGeminiCli, GoogleVertex,
    OllamaChat,
    CursorAgent,           // ← 구현 필요
    GitLabDuoAgent,        // ← 구현 필요
    DevinAgent,            // ← 구현 필요
}
```

**Transport dispatch 위치**: `oxi-ai/src/providers/register_builtins.rs`

- `build_builtin_transport(builtin) -> Option<Box<dyn Provider>>` — line 284-368
- `build_builtin_transport_with_options(...)` — line 392-507
- 두 함수 모두 마지막에 `_ => None` (line 366, 505) — 미구현 dialect는 None 반환

**기존 transport 참고 (복붙 + 수정 패턴)**:
- `oxi-ai/src/providers/ollama.rs` — NDJSON streaming의 좋은 예시 (가장 단순)
- `oxi-ai/src/providers/google_shared.rs` — Gemini 공유 SSE 파싱 로직
- `oxi-ai/src/providers/anthropic.rs` — SSE + thinking block 처리
- `oxi-ai/src/providers/openai.rs` — 가장 표준적인 OpenAI 호환 transport

**Api 매핑 table 위치**: `oxi-ai/src/providers/register_builtins.rs:155-165`
```rust
("google-generative-ai", Api::GoogleGenerativeAi),
("google-vertex", Api::GoogleVertex),
("bedrock-converse-stream", Api::BedrockConverseStream),
```
remote-AGENT entry를 여기에 추가해야 함.

### 2.3 omp 참고 자료

**위치**: `/tmp/omp/packages/coding-agent/src/providers/remote-agent/`

파일들 (omP 소스):
- `cursor.ts` — Cursor provider
- `devin.ts` — Devin provider
- `gitlab-duo.ts` — GitLab Duo provider

(아직 디렉토리 내용 검증 안 함. 다음 세션 첫 단계에서 `ls /tmp/omp/packages/coding-agent/src/providers/` 로 확인 권장)

### 2.4 구현 가이드 (단계별)

1. **Api 매핑 table 확장** (`register_builtins.rs:155-165`):
   ```rust
   ("cursor", Api::CursorAgent),
   ("cursor-agent", Api::CursorAgent),
   ("devin", Api::DevinAgent),
   ("devin-agent", Api::DevinAgent),
   ("gitlab", Api::GitLabDuoAgent),
   ("gitlab-duo", Api::GitLabDuoAgent),
   ```

2. **Provider 구현** (각각):
   - `oxi-ai/src/providers/cursor.rs` — CursorProvider
   - `oxi-ai/src/providers/devin.rs` — DevinProvider
   - `oxi-ai/src/providers/gitlab_duo.rs` — GitLabDuoProvider

3. **mod.rs 등록**: `oxi-ai/src/providers/mod.rs`에 `pub mod cursor; pub mod devin; pub mod gitlab_duo;` 추가

4. **Transport dispatch 확장** (`build_builtin_transport` 와 `build_builtin_transport_with_options`):
   ```rust
   Api::CursorAgent => Some(Box::new(super::cursor::CursorProvider::new())),
   Api::DevinAgent => Some(Box::new(super::devin::DevinProvider::new())),
   Api::GitLabDuoAgent => Some(Box::new(super::gitlab_duo::GitLabDuoProvider::new())),
   ```

5. **테스트** (각 provider마다 최소 1개 unit test):
   - httptest 또는 mockito 로 SSE fixture
   - `parse_*_events()` 함수 테스트
   - 401/403/429 에러 처리 테스트

6. **KnownApi 직렬화** (필요 시): `oxi-ai/src/dialect/known_api.rs`에 새 dialect 추가

### 2.5 위험 요소

- **WebSocket** (Cursor/Devin): oxi는 현재 SSE/HTTP만 지원. WebSocket 추가가 필요할 수 있음 → `tokio-tungstenite` 이미 의존성에 있음 (Cargo.lock 확인)
- **인증**: Cursor/Devin는 OAuth 토큰 필요. `env_api_keys.rs`에 키 추출 함수 추가
- **스트리밍 형식 차이**: 각 provider의 SSE event 형식이 다름 → 각 provider마다 `parse_*_events()` 작성

### 2.6 회귀 게이트 (변경 후)

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run -p oxi-ai
```

---

## 3. 다음 세션: P2 — TUI 재정렬 (~10000 lines, 마지막)

### 3.1 목표

`oxi-tui-legacy` (22499 lines) → omp `tui.ts` (4270 lines) 렌더링 모델로 정렬. 핵심은 **append-only "tape"** 렌더링과 3-전략 차등 렌더링 (memoization / native scrollback / ED3 replay).

### 3.2 현재 구조 (탐색 가이드)

**oxi-tui-legacy 총량**: 22499 lines, 27+ 파일

```
oxi-tui-legacy/src/
├── lib.rs (45)                          # 모듈 진입점
├── theme.rs (1906)                      # 28-slot ColorScheme
├── symbols.rs (905)                     # Unicode/Ascii/Nerd glyph set
├── fuzzy.rs (294)                       # fuzzy 매칭
├── cell.rs (7)
├── table_renderer.rs (597)
├── overlay_anchor.rs (263)
├── markdown_styles.rs (93)
├── text.rs (54)
├── render/                              # ~2700 lines, 9 files
│   ├── mod.rs (743)
│   ├── terminal.rs (476)
│   ├── mermaid.rs, latex.rs, diff.rs, image.rs, ansi.rs, deccara.rs, color_level.rs
├── widgets/                             # ~7100 lines, 13 files
│   ├── mod.rs (31)
│   ├── completion.rs, dashboard.rs, footer.rs, input.rs (475)
│   ├── list_selector.rs (920), routing.rs (386), slash_dropdown.rs (415)
│   ├── stateful_list.rs (332), table_list.rs (457), todo_panel.rs (419)
│   └── tool_renderer.rs (1725)          # 가장 큰 단일 파일
├── widgets/chat/                        # ~1700 lines, 8 files
│   ├── mod.rs, mouse.rs, render.rs, state.rs, sticky.rs
│   └── terminal_support.rs, types.rs, dashboard.rs, highlight.rs, layout.rs, markdown.rs
└── keybindings/                         # 4 files
```

**oxi-tui v2 (신규, 4608 lines, 이미 작동 중)**: `oxi-tui/src/`
```
├── pipeline/         # draw_frame(), CursorState, DiffBackend
├── widget/           # Renderable trait, RetainedTree, RetainedChild
├── content/          # ChatLog (O(1) hash)
├── text/             # StreamingMarkdown, CJK wrap, syntax
├── theme/            # palette, capability, serializer
└── input/            # textarea wrapper
```

**oxi-cli에서 tui 사용 위치**:
- `oxi-cli/src/tui/app.rs` (2128 lines) — 메인 App struct
- `oxi-cli/src/tui/handlers.rs` (1637 lines) — 키/이벤트 핸들러
- `oxi-cli/src/tui/render.rs` (779 lines) — 메인 렌더링 함수
- `oxi-cli/src/tui/v2_bridge.rs` (57) — v2 파이프라인 진입점
- `oxi-cli/src/tui/v2_overlay_adapter.rs` (341) — legacy overlay를 v2로 어댑트
- `oxi-cli/src/tui/v2_render.rs` (35) — v2 렌더
- `oxi-cli/src/tui/welcome.rs` (144)

**현재 상태** (AGENTS.md 2026-07-22 메모):
- v2 파이프라인은 **이미 작동** (oxi-cli가 `draw_frame_closure`로 cutover 완료)
- `LegacyOverlayAdapter` + `ClosureRoot` 가 legacy 렌더링을 v2 파이프라인으로 bridge
- "Remaining: full rendering migration (Phase 5), legacy removal (Plan D)"

### 3.3 omp 참고 자료

**위치**: `/tmp/omp/packages/tui/src/`
```
tui.ts (4270)                          # 메인
components/                            # 위젯들
autocomplete.ts
bracketed-paste.ts
deccara.ts
desktop-notify.ts
editor-component.ts
fuzzy.ts
index.ts
keybindings.ts
keys.ts
kill-ring.ts
kitty-graphics.ts
latex-block.ts
latex-to-unicode.ts
loop-watchdog.ts
mouse.ts
stdin-buffer.ts
symbols.ts
terminal-capabilities.ts
terminal.ts
tmux.ts
ttyid.ts
utils.ts
```

핵심 차이점 (omP vs oxi):
- omp: 4270 lines 단일 `tui.ts` + 23개 컴포넌트 파일
- oxi: 22499 lines (`oxi-tui-legacy` 27+ 파일) + 4608 lines (v2 신규)

→ oxi는 **합쳐서 ~27000 lines**, omp는 **~6500 lines** (components 합산). 약 4배 큰 차이. 이유: oxi가 많은 dead code / 디버그 빌드 / 과도한 추상화 보유.

### 3.4 P2 작업 분해 (예상)

**Phase 1 — 정렬 (의존성 없음)**:
1. `oxi-tui-legacy/widgets/tool_renderer.rs` (1725) 분해 → 다중 파일
2. `oxi-tui-legacy/widgets/list_selector.rs` (920) 분해
3. `oxi-tui-legacy/theme.rs` (1906) 분해 → palette / construct / cap-detect 3개로
4. `oxi-tui-legacy/render/mod.rs` (743) 분해

**Phase 2 — omp 정렬 (의존: Phase 1)**:
1. **Append-only "tape" 렌더링**: oxi의 `oxi-tui/src/content/chat_log.rs` (185) 를 omp 모델로 강화
2. **3-전략 차등 렌더링**: `Renderable` trait + `RetainedTree` 가 이미 있음 → component memoization 활성화, ED3 replay 추가
3. **입력 시스템**: omp `keys.ts` / `kitty-keyboard.ts` / `bracketed-paste.ts` / `kill-ring.ts` 정렬
4. **Glyph 시스템 단일화**: 이미 Unicode/Ascii/Nerd 분리됨 (oxi-tui-legacy/symbols.rs 905) → 그 외 dead symbols 통합
5. **LaTeX / mermaid / image**: 이미 oxi-tui-legacy/render/ 에 구현됨 (latex.rs, mermaid.rs, image.rs) → 통합

**Phase 3 — legacy 제거 (의존: Phase 1+2)**:
1. `LegacyOverlayAdapter` 가 더 이상 legacy를 안 쓰게 전환
2. `oxi-tui-legacy` crate 전체 삭제
3. `oxi-tui` 단일 v2 crate가 모든 TUI 책임
4. `Cargo.toml` / `lib.rs` / `tui/` 모듈에서 legacy 의존성 제거

### 3.5 위험 요소

- **UI 회귀**: TUI는 시각적 인터페이스 → 테스트 자동화가 어려움. 변경 후 smoke test 필수
- **Catastrophic regression**: `oxi-tui-legacy` 22499 lines 중 dead code가 많을 수 있음. 본격 작업 전 `cargo +nightly udeps` 또는 `cargo-machete` 로 dead import 확인
- **TUI 라이브러리 migration**: ratatui 0.x ↔ 0.y 호환성 (현재 oxi-tui v2가 어떤 버전 쓰는지 확인 필요)
- **테스트 인프라**: `oxi-tui/src/`에 222 tests 있다고 AGENTS.md 메모 있음. legacy의 테스트는 일부만 이전됨

### 3.6 회귀 게이트 (변경 후)

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run -p oxi-tui -p oxi-tui-legacy  # v2 + legacy 둘 다
# 그리고 visual regression test (PTY 기반):
cargo nextest run -p oxi-cli --test pty_e2e
```

### 3.7 권장 작업 순서

1. **첫 단계**: `oxi-tui-legacy` dead code 제거 (`tool_renderer.rs` 1725 lines, `theme.rs` 1906 lines 부터)
2. **두번째**: `oxi-tui` v2의 `draw_frame_closure` 가 legacy를 거의 다 어댑트 했으므로, 미어댑트 영역만 마이그레이션
3. **세번째**: `oxi-tui-legacy` 제거
4. **네번째**: omp 정렬 (tape 렌더, 3-전략, 입력 시스템 통합)

---

## 4. P4.2 후속 (P0.5와 함께 또는 별도)

### 4.1 정렬 안 된 기능

`oxi-cli/src/storage/packages/` 가 omp `extensibility/plugins/` 모델과 아직 차이:

| omp 기능 | oxi 현재 상태 | 필요한 작업 |
|----------|-------------|-----------|
| `PluginRuntimeConfig` (per-plugin enabled state) | `Settings::extensions` 배열 (Vec<String>) | `RuntimeConfig` 타입 추가, `packages/runtime_config.rs` 생성 |
| `ProjectPluginOverrides` (per-project) | 없음 | `.oxi/plugin-overrides.json` 로딩 로직 |
| `Doctor` (health checks) | `PackageManager::validate_package` (warning list) | 분리된 `doctor.rs` 모듈 + `DoctorCheck` 타입 |
| `validatePackageName` (shell metachar) | 미적용 | `source.rs`에 `validate_package_name` + `validate_git_spec` 추가 |
| `plugin-overrides.json` 로딩 | 없음 | `manager.rs` 에서 cwd 스캔 |

### 4.2 구현 가이드

1. **`packages/runtime_config.rs`** (신규):
   - `RuntimeConfig` struct (mirrors `omp::PluginRuntimeConfig`)
   - `ProjectOverrides` struct (mirrors `omp::ProjectPluginOverrides`)
   - `oxi-plugins.lock.json` 로딩/저장 (현재는 `oxi-lock.json`)

2. **`packages/doctor.rs`** (신규):
   - `DoctorCheck` struct
   - `Doctor::run()` 메서드 — installed packages + lockfile + cwd 모두 진단

3. **`packages/source.rs`** 수정:
   - `validate_package_name(name: &str) -> Result<()>`
   - `validate_git_spec(spec: &str) -> Result<()>`
   - `ParsedSource::parse` 의 시작 부분에서 둘 다 호출

4. **`packages/manager.rs`** 수정:
   - `RuntimeConfig` 통합 (load_installed 시 같이 로드)
   - `ProjectOverrides` 통합 (cwd 스캔)
   - `effective_enabled(name)` 메서드

### 4.3 위험 요소

- **Lockfile 포맷 변경**: `oxi-lock.json` → `oxi-plugins.lock.json` 이름 변경 시 사용자 데이터 마이그레이션 필요. **이름은 유지하고 구조만 확장** 권장
- **Public API 호환**: `PackageManager::install` / `uninstall` / `list` 등의 시그니처는 그대로 유지. 새 메서드만 추가

---

## 5. Git 상태 및 커밋 가이드

### 5.1 현재 미커밋 변경

```bash
$ git status --short
 M docs/superpowers/RESUMING.md
 M oxi-agent/src/tools.rs
 M oxi-agent/tests/tools.rs
 M oxi-cli/src/cli.rs
 M oxi-cli/src/main.rs
 D oxi-cli/src/storage/packages.rs
 D oxi-cli/src/store/issues.rs
?? oxi-cli/src/cli/
?? oxi-cli/src/storage/packages/
?? oxi-cli/src/store/issues/
```

**권장 커밋 분할** (논리적 단위):

```bash
# 1. P3.3 — main.rs 핸들러 분리
git add oxi-cli/src/main.rs oxi-cli/src/cli.rs oxi-cli/src/cli/
git commit -m "refactor(cli): P3.3 — extract main.rs handlers to cli/commands/"

# 2. P4.1 — Issue 시스템 격리
git add oxi-cli/src/store/issues.rs oxi-cli/src/store/issues/
git commit -m "refactor(store): P4.1 — split issues.rs into directory module"

# 3. P4.2 — Package manager 모듈화
git add oxi-cli/src/storage/packages.rs oxi-cli/src/storage/packages/
git commit -m "refactor(storage): P4.2 — split packages.rs into directory module"

# 4. P1.6a — debug 도구 재등록
git add oxi-agent/src/tools.rs oxi-agent/tests/tools.rs
git commit -m "feat(agent): P1.6a — re-enable debug tool (37→38 tools)"

# 5. Docs
git add docs/superpowers/RESUMING.md
git commit -m "docs: update RESUMING for sessions 4-5 (P3.3, P4.1, P4.2, P1.6a)"
```

### 5.2 다음 세션 시작 시 첫 단계

```bash
# 1. 상태 확인
cd /Volumes/MERCURY/PROJECTS/oxi
git status --short
cargo build --workspace  # 깨끗한지 확인
cargo nextest run -p oxi-cli -p oxi-agent -p oxi-sdk  # 1907 통과 확인

# 2. 미커밋 커밋 (위 가이드대로)
# 3. main 브랜치에 push (push 권한 있으면)
# 4. P0.5 시작
```

---

## 6. 빠른 참조 (다음 세션이 5분 안에 시작하도록)

### 6.1 빌드 & 테스트

```bash
# 풀 빌드
cargo build --workspace

# Clippy
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings

# 테스트
cargo nextest run -p oxi-cli      # 763 tests
cargo nextest run -p oxi-agent    # 746 tests
cargo nextest run -p oxi-sdk      # 398 tests
# 합계: 1907 tests

# 포맷
cargo fmt --all -- --check
```

### 6.2 핵심 파일 경로

| 항목 | 경로 |
|------|------|
| Api enum | `oxi-catalog/src/api.rs:25-68` |
| Transport dispatch | `oxi-ai/src/providers/register_builtins.rs:284-368` (no-options) + `392-507` (with-options) |
| Api 매핑 table | `oxi-ai/src/providers/register_builtins.rs:155-165` |
| Issue 시스템 | `oxi-cli/src/store/issues/` (P4.1 완료) |
| Package 시스템 | `oxi-cli/src/storage/packages/` (P4.2 완료) |
| CLI commands | `oxi-cli/src/cli/commands/` (P3.3 완료) |
| Debug tool | `oxi-agent/src/tools/debug_tool.rs` (P1.6a 재등록 완료) |
| TUI v2 | `oxi-tui/src/` (작동 중) |
| TUI legacy | `oxi-tui-legacy/src/` (P2 미착수) |
| omp plugins | `/tmp/omp/packages/coding-agent/src/extensibility/plugins/` |
| omp tui | `/tmp/omp/packages/tui/src/` |
| omp providers | `/tmp/omp/packages/coding-agent/src/providers/` (remote-agent/ 하위) |

### 6.3 AGENTS.md 핵심 invariant (절대 어기지 말 것)

- **Issue liveness identity**: `liveness::TUI_OWNERSHIP_ID = "tui"` 가 단일 진실. App / agent tool / TUI panel / `/issue` 슬래시 커맨드 모두 동일
- **Issue CAS**: store의 `update`는 raw `IssueError::Conflict` 반환, 재시도는 tool이 함 (4회)
- **`mut MutexGuard` over `.await` 금지**: `parking_lot::MutexGuard` is `!Send`
- **`#[non_exhaustive]` `Api` enum**: 새 variant 추가 시 `mod.rs`/`register_builtins.rs` 양쪽 모두 업데이트 필수
- **clap `Subcommand` enum**: `pub use` 로 re-export 시 `pub mod` 경로여야 binary main.rs에서 접근 가능

### 6.4 P0.5 착수 시 1단계 TODO (5분)

```bash
# 1. omp remote-agent 디렉토리 확인
ls /tmp/omp/packages/coding-agent/src/providers/

# 2. Cursor/Devin/GitLabDuo 파일 위치 확인
ls /tmp/omp/packages/coding-agent/src/providers/remote-agent/ 2>/dev/null || echo "directory missing"

# 3. Api 매핑 table 위치
grep -n "google-vertex" oxi-ai/src/providers/register_builtins.rs

# 4. 첫 번째 패치 위치
grep -n "_ => None" oxi-ai/src/providers/register_builtins.rs
```

---

## 7. 알려진 이슈 / 함정 메모

1. **WebSocket 의존성**: oxi는 현재 `tokio-tungstenite` 가 이미 있지만, Cursor/Devin의 WebSocket 프로토콜이 표준이 아닐 수 있음 → omp `cursor.ts` 코드 보고 따라가기

2. **dialect XML literal 태그**: `oxi-agent/src/dialect/xml.rs`에서 literal XML 태그 금지 (`concat!("<", "invoke")` 형태 사용). 새 dialect 추가 시 동일 패턴 유지

3. **`oxi-catalog` 과 `oxi-ai` 경계**: `Api` enum은 `oxi-catalog` 에 있고 provider impl은 `oxi-ai`에 있음. 새 Api variant 추가는 `oxi-catalog/src/api.rs`의 enum + `oxi-ai/src/providers/register_builtins.rs`의 dispatch 둘 다 수정

4. **Cargo.lock**: oxi-catalog 버전이 oxi-ai와 sync 되어야 함. 새 의존성 추가 시 둘 다 업데이트

5. **테스트 fixture**: SSE / WebSocket fixture는 `mockito` (HTTP) 또는 직접 tokio::sync::mpsc (WebSocket) 사용. 기존 `oxi-ai/src/providers/anthropic.rs::tests` 가 SSE fixture 패턴의 좋은 예시

---

## 8. 마무리

- **P0.5 착수 시**: 이 doc의 §2 + §6.3 + §7 을 한 번에 읽고 시작
- **P2 착수 시**: 이 doc의 §3 + §6.3 + §7 을 한 번에 읽고 시작
- **P4.2 후속 착수 시**: 이 doc의 §4 + §6.3 + §7 을 한 번에 읽고 시작
- **테스트 1개라도 깨지면**: §5.2 의 회귀 게이트 전체 다시 돌리기

다음 세션은 P0.5 부터.
