# oxi omp-정렬 — 남은 작업 명세
> **다음 세션 인수인계**: [HANDOFF-session5.md](./HANDOFF-session5.md) — P0.5 (remote-AGENT) + P2 (TUI 재정렬) + P4.2 후속 작업의 자세한 가이드


> **최종 갱신**: 2026-07-28 (session 5)
> **브랜치**: `main`
> **완료**: P0 + P1 + P2 + P3.1 + P3.2 + **P3.3** + P4.3 + P4.4 + **P4.1** + **P4.2** + **P1.6a**
> **기준선**: 1907 tests passing (oxi-cli 763, oxi-agent 746, oxi-sdk 398), clippy clean, fmt clean

> **현재 상태 (2026-07-30):** 이 문서의 잔여 작업 설명은 superseded 되었습니다.
> 모든 dialect transport는 explicit dispatch arm이 있습니다. P2 rich-content는
> 부분 완료이며, Codex Responses는 OpenAI Responses transport를 재사용하고,
> Gemini CLI는 의도적으로 `ProviderError::NotImplemented`를 반환하는 stub입니다.
> backlog gap이 아닙니다.

---

## 1. 완료된 작업 요약

| Phase | 작업 | 커밋 |
|-------|------|:----:|
| P0 | catalog 분리 + complexity 제거(−4791 lines) + 정체성 수정 + KnownApi14 + Ollama | `main` |
| Step 2 | `Provider::name()` trait 제거 | `main` |
| P1.1 | Owned dialect (11종 enum, XML renderer/parser, 24 tests) | `main` |
| P1.2 | Intent tracing (AgentTool::intent(), ToolExecution 이벤트) | `main` |
| P1.3 | AppendOnlyContext struct + loop wiring | `main` |
| P1.4 | Approval/tier 시스템 (ToolTier, ApprovalConfig) | `main` |
| P1.5 | Soft req + Harmony leak (remind/escalate, regex 감지) | `main` |
| P1.6a | eval, ast_grep, ast_edit 도구 | `main` |
| P1.6b | checkpoint, rewind, hub, yield, goal, review (6 tools) | `dcdd0b27` |
| P1.6c | learn, manage_skill, inspect_image, computer, tts, vibe (6 tools) | `8432dd35` |
| **P3.1** | **`.md` 기반 시스템 프롬프트** — `include_str!("../prompts/identity.md")` + `oxi-hashline/src/prompt.md` 참조 | `138e5689` |
| **P3.2** | **CLI 명령 포팅** — `completions`, `install`, `update`, `commit`, `config path` | `1a6fa373`, `fb129dc4` |
| P4.3 | Language policy → no-op (`language_directive()` → `None`) | 이전 session |
| **P3.3** | **main.rs 핸들러 분리** (F-5) — 10개 commands/*.rs 파일, main.rs 62줄로 축소 | session 4 |
| **P4.1** | **Issue 시스템 격리** — `oxi-cli/src/store/issues/` 디렉토리 모듈 (7개 하위모듈) | session 4 |
| **P1.6a** | **debug 도구 재등록** — `oxi-agent`에 DebugTool 다시 연결, 37→38 tool count | session 5 |
| **P4.2** | **Package manager 모듈화** — `oxi-cli/src/storage/packages.rs` (3096줄) → `packages/` 디렉토리 모듈 (9개 파일) | session 5 |

**Tool count**: 38 built-in tools registered (debug tool re-enabled).

---

## 2. Phase 3 — 프롬프트 & CLI 재정렬 (완료)

> **대상 크레이트**: `oxi-cli/`

### P3.3 — main.rs 핸들러 분리 (F-5) ✅ (2026-07-28)

**변경 내용**: `oxi-cli/src/main.rs`의 모든 handler 함수를 `oxi-cli/src/cli/commands/*.rs`로 분리.
- 10개 파일 생성: `sessions.rs`, `issue.rs`, `pkg.rs`, `ext.rs`, `config.rs`, `setup.rs`, `reset.rs`, `export.rs`, `misc.rs`, `mod.rs`
- `main.rs` 1779줄 → **62줄** (main() + handle_subcommand match arm만)
- `cli.rs`에 `pub mod commands;` 추가
- 주의사항 해결: handler 함수들은 `oxi::` 대신 `crate::` 사용 (library crate 내부)
- **회귀 위험 해소**: clap Subcommand generic-bound 이슈 없음 — `pub use`로 정상 re-export

---

## 3. Phase 4 — oxi-original 정리 (P4.1 + P4.4 완료, P4.2 잔여)

> **대상 크레이트**: `oxi-cli/`

### P4.1 — Issue 시스템 격리 ✅ (2026-07-28)

**변경 내용**: `oxi-cli/src/store/issues.rs` (2020줄) → `oxi-cli/src/store/issues/` 디렉토리 모듈 (7개 파일)

| 파일 | 내용 |
|------|------|
| `mod.rs` | Re-exports + submodule declarations |
| `error.rs` | `IssueError` enum |
| `types.rs` | `Status`, `Priority`, `Assignment`, `GithubRef`, `IssueMeta`, `Issue`, `IssuePatch` |
| `serialize.rs` | `parse_issue`, `serialize_issue`, `content_hash`, `issues_dir`, `issue_filename` + slugify tests |
| `liveness.rs` | `TUI_OWNERSHIP_ID`, `acquire`, `is_session_alive`, `reap_orphans`, `AliveGuard` + 5 tests |
| `filter.rs` | `IssueFilter` + `matches` impl |
| `store.rs` | `Cache`, `Inner`, `IssueSummary`, `FileIssueStore` + ~500 lines tests |

**모든 public API 보존**: `crate::store::issues::*` 경로 변경 없음.

---

### P4.2 — Package manager → omp 플러그인 모델 (~500 lines)

**현재**: `oxi-cli/src/storage/packages.rs` (3096 lines) — 자체 패키지 시스템.

**목표**: omp `extensibility/plugins/` 모델에 맞춤. 기존 packages.rs 기능을 omp 플러그인 시스템과 정렬.

**omp 참조**: `/tmp/omp/packages/coding-agent/src/extensibility/plugins/`

---

## 4. Phase 1 잔여

_Phase 1 잔여 항목 모두 완료 (P1.6a debug 도구 재등록은 아래 참고)._

---


### P1.6a — debug 도구 재등록 ✅ (2026-07-28)

**변경 내용**:
- `oxi-agent/src/tools.rs`: `all_tools.push(Box::new(debug_tool::DebugTool));` 주석 해제
- `oxi-agent/tests/tools.rs`: 도구 카운트 `37` → `38` (두 곳)
- DAP 액션의 15/16은 여전히 scaffold (`xd://debug` device로 위임) — 핵심 launch/attach/breakpoint 액션은 wire됨

**Tool count**: 38 built-in tools registered.

### P4.2 — Package manager 모듈화 ✅ (2026-07-28)

**변경 내용**: `oxi-cli/src/storage/packages.rs` (3096줄 단일 파일) → `oxi-cli/src/storage/packages/` 디렉토리 모듈

| 파일 | 줄 수 | 내용 |
|------|-------|------|
| `mod.rs` | 86 | 모듈 선언, 상수, public re-exports |
| `types.rs` | 225 | ResourceKind, PackageManifest, DiscoveredResource, PathMetadata, ResourceOrigin, SourceScope, ResolvedResource, ResolvedPaths, ProgressEvent, ProgressAction, ProgressEventType, ProgressCallback, PackageUpdateInfo, ConfiguredPackage |
| `source.rs` | 221 | ParsedSource + parse_npm_spec/split_git_path_ref/parse_git_source + NPM_SPEC_RE |
| `npm.rs` | 95 | NpmPackageInfo + get_latest_npm_version |
| `git_ops.rs` | 157 | git_command/git_command_silent/git_clone/git_update/git_has_update |
| `lockfile.rs` | 210 | LockEntry/Lockfile/ResourceCounts + compute_dir_hash/verify_lockfile_integrity/collect_file_paths |
| `discovery.rs` | 181 | discover_extensions/skills/prompts/themes + recursive walkers |
| `fs.rs` | 62 | copy_dir_recursive/find_single_subdir/prune_empty_parents |
| `manager.rs` | 1977 | PackageManager struct + all impl methods + 36 tests |

**omp plugins 모델 정렬**:
- `ParsedSource` enum 4종 (Npm/Git/Local/Url) — omp `parsePluginSpec` 1:1 매핑
- `validatePackageName` / `validateGitSpec` 수준의 shell metachar 가드 추가 가능 (현재 미적용 — 후속 P0.5와 함께)
- `LockEntry` + `Lockfile` 구조는 omp `omp-plugins.lock.json`과 1:1은 아니지만 동일 패턴 (per-source metadata + integrity hash)
- `RuntimeConfig` / `ProjectPluginOverrides` / `Doctor` 기능은 **추가 필요** (P0.5 또는 후속 작업)

**모든 public API 보존**: `crate::storage::packages::*` 경로 변경 없음
## 5. P0.5 — remote-AGENT provider 포팅 (~2000 lines)

> **요청 시 진행**

| Provider | omp 파일 | 프로토콜 |
|----------|---------|---------|
| Cursor | `cursor.ts` | WebSocket + SSE |
| Devin | `devin.ts` | WebSocket + SSE |
| GitLab Duo | `gitlab-duo.ts` | GitLab API |

`Api` enum에 variant 이미 존재 (`CursorAgent`, `DevinAgent`, `GitLabDuoAgent`), transport만 `_ => None`.

---

## 6. Phase 2 — TUI 재정렬 (~10000 lines)

> **가장 큼, 마지막 순위**

**omp 참조**: `/tmp/omp/packages/tui/src/tui.ts` (173KB)
**대상 크레이트**: `oxi-tui`, `oxi-tui-legacy`

| 항목 | 설명 |
|------|------|
| `oxi-tui-legacy` → `oxi-tui` rename | v2 파이프라인 폐기, legacy를 주축으로 |
| 3-전략 차등 렌더링 | Component memoization, Native scrollback commit, ED3 replay |
| Append-only "tape" 렌더 계약 | omp 렌더링 모델로 전환 |
| 입력 시스템 | Kitty keyboard, bracketed paste, keybinding, mouse SGR, kill ring, undo |
| LaTeX / mermaid / image | Rich content 렌더링 |
| Glyph 시스템 단일화 | Unicode / Ascii / Nerd 통합 |

---

## 7. 작업 우선순위 총정리

| 순위 | 작업 | 예상 규모 | Recommended start |
|:----:|------|:---------:|:-----------------:|
| ~~1~~ | ~~**P3.3** main.rs 핸들러 분리~~ | ✅ 완료 | —
| ~~2~~ | ~~**P4.1** Issue 시스템 격리~~ | ✅ 완료 | —
| ~~3~~ | ~~**P4.2** Package manager 모듈화~~ | ✅ 완료 | —
| ~~4~~ | ~~**P1.6a** debug 도구 재등록~~ | ✅ 완료 | —
| **1** | **P0.5** remote-AGENT providers | ~2000 lines | 요청 시 (Cursor/Devin/GitLab Duo)
| **2** | **P2** TUI 재정렬 | ~10000 lines | 마지막

---

## 8. 회귀 게이트

변경 후 항상 실행:

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```

### 주의사항
- **dialect `xml.rs`**: literal XML 태그 금지 — `concat!("<", "invoke")` 형태 사용
- **Config 필드 추가**: `Default::default()`에도 기본값 반드시 추가
- **P3.1 참고**: `HASHLINE_FORMAT_SPEC` const 제거 — `include_str!("../../../oxi-hashline/src/prompt.md")`가 canonical source
- **P4.4 참고**: settings v10. 구버전 settings.toml의 dead 필드는 serde가 silent ignore. Router 기능 자체는 유지
- **P3.3 참고**: commands 모듈은 `oxi-cli/src/cli/commands/`에 위치. handler 함수는 `pub(crate)` + `pub use` re-export 조합으로 외부 노출
- **P4.2 참고**: packages 디렉토리 모듈에서 `MANIFEST_NAME`/`NPM_MANIFEST_NAME`/`LOCKFILE_NAME` 상수는 mod.rs에 `pub(super) const`로 정의, manager.rs에서 `use super::{...}`로 가져옴
