# oxi omp-정렬 — 남은 작업 명세

> **최종 갱신**: 2026-07-28
> **브랜치**: `main`
> **완료**: P0 전항목 + P1 전항목 (P1.1~P1.6c, 12개 도구) + P4.3 (language policy no-op)
> **기준선**: 3637 tests passing, clippy clean, fmt clean

---

## 1. 완료된 작업 요약

| Phase | 작업 | 커밋/상태 |
|-------|------|:---------:|
| P0 | catalog 분리 + complexity 제거(−4791 lines) + 정체성 수정 + KnownApi14 + Ollama | `main` |
| Step 2 | `Provider::name()` trait 제거 | `main` |
| P1.1 | Owned dialect (11종 enum, XML renderer/parser, 24 tests) | `main` |
| P1.2 | Intent tracing (AgentTool::intent(), ToolExecution 이벤트) | `main` |
| P1.3 | AppendOnlyContext struct + loop wiring | `main` |
| P1.4 | Approval/tier 시스템 (ToolTier, ApprovalConfig) | `main` |
| P1.5 | Soft req + Harmony leak (remind/escalate, regex 감지) | `main` |
| P1.6a | eval, ast_grep, ast_edit 도구 | `main` |
| P1.6b | checkpoint, rewind, hub, yield, goal, review (6 tools) | ✅ This session |
| P1.6c | learn, manage_skill, inspect_image, computer, tts, vibe (6 tools) | ✅ This session |
| P4.3 | Language policy → no-op (`language_directive()` → `None`) | ✅ This session |

**Tool count**: 37 built-in tools registered.

---

## 2. Phase 3 — 프롬프트 & CLI 재정렬 (~2000 lines)

> **대상 크레이트**: `oxi-cli/`, `oxi-ai/`
> **우선순위**: HIGH — 사용자 경험 + 코드 품질

### P3.1 — `.md` 기반 시스템 프롬프트 (~800 lines)

**현재**: `oxi-cli/src/prompt/system_prompt.rs` (640 lines) — 모든 프롬프트가 inline Rust 문자열.

**목표**: 대용량 정적 문자열을 `.md` 파일로 분리, `include_str!()`으로 로드.

**omp 참조**: `/tmp/omp/packages/coding-agent/src/prompts/`

**구현**:
1. `oxi-cli/src/prompts/` 디렉토리 생성
2. 큰 정적 블록 추출: identity, hashline format spec 등
3. `include_str!("../prompts/<name>.md")`로 교체
4. 동적 부분 (tools, skills, context files, cwd, date)은 Rust builder 유지

**검증**: `cargo nextest run -p oxi-cli`, prompt 테스트 통과.

---

### P3.2 — CLI 명령 포팅 (~600 lines)

**현재**: `oxi-cli/src/cli.rs`에 15개 subcommand enum. `oxi-cli/src/main.rs`에 handle_* 함수.

**누락 명령** (omp 기준):
- `completions` — shell completion 생성
- `config` — CLI에서 설정 접근 (현재는 `oxi config get/set` 만)
- `install` — MCP 서버 설치
- `update` — oxi 자체 업데이트
- `commit` — 단일 커밋 명령

**omp 참조**: `/tmp/omp/packages/cli/src/commands/`

---

### P3.3 — `main.rs` 핸들러 분리 (F-5) (~600 lines)

**현재**: `oxi-cli/src/main.rs`에 `handle_subcommand` (~90 lines match) + inline `handle_*` 함수 (~1400 LOC).

**목표**: 각 `handle_*` 함수를 `oxi-cli/src/cli/commands/*.rs`로 분리.

**위험**: clap `Subcommand`-derived enum이 sibling module에서 참조될 때 generic-bound 이슈 가능. 분리 전 각 subcommand 테스트 필요.

---

## 3. Phase 4 — oxi-original 정리 (진행중, ~1200 lines 남음)

> **대상 크레이트**: `oxi-cli/`

### P4.1 — Issue 시스템 격리 (~500 lines)

**현재**: Issue 시스템 (CAS + flock)이 agent 루프/session 모델에 직접 연결됨.

**목표**: Issue 관련 코드를 명시적 API boundary 뒤로 이동. `oxi-cli/src/store/issues/`로 모듈화.

**참조**: issue tool (`oxi-agent/src/tools/issue/`), `oxi-cli/src/main.rs`의 issue handler, TUI issue overlay.

---

### P4.2 — Package manager → omp 플러그인 모델 (~500 lines)

**현재**: `oxi-cli/src/storage/packages.rs` (3096 lines) — 자체 패키지 시스템.

**목표**: omp `extensibility/plugins/` 모델에 맞춤. 기존 packages.rs 기능을 omp 플러그인 시스템과 정렬.

**omp 참조**: `/tmp/omp/packages/coding-agent/src/extensibility/plugins/`

---

### P4.4 — Dead config 필드 정리 (~200 lines)

**현재**: `oxi-cli/src/store/settings.rs`에 복잡도 라우팅용 dead 필드 존재:
- `enable_routing`, `router_profile`, `prefer_cost_efficient`
- `fallback_chain`, `enable_fallback`, `disable_fallback`
- `circuit_breaker_failure_threshold`, `circuit_breaker_open_duration_secs`

**목표**: 필드 + 기본값 + `merge_cli()` 파라미터 + CLI arg + TUI overlay 참조 제거.

**영향 범위**:
- `oxi-cli/src/store/settings.rs` (struct, defaults, merge_cli, tests)
- `oxi-cli/src/cli.rs` (CLI args: `--enable-routing`, `--prefer-cost-efficient`, `--fallback-chain`)
- `oxi-cli/src/bootstrap.rs` (args → settings 전달)
- `oxi-cli/src/main.rs` (config get/set)
- `oxi-cli/src/tui/overlay/settings.rs` (routing toggle UI)

---

## 4. Phase 1 잔여

### P1.6a — debug 도구 재등록 (~600 lines)

**현재**: `oxi-agent/src/tools/debug_tool.rs` 파일은 있음. 도구 카운트 37, 등록 해제됨.

**목표**: DAP (Debug Adapter Protocol) 프록시 구현 후 재등록.
- `oxi-agent/src/tools.rs`에서 `all_tools.push(Box::new(debug_tool::DebugTool));` 주석 해제
- `oxi-agent/tests/tools.rs` 카운트 37→38

---

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
| 1 | **P3.1** `.md` 기반 시스템 프롬프트 | ~800 lines | ✅ 바로 시작 |
| 2 | **P4.4** Dead config 필드 정리 | ~200 lines | ✅ 독립적, 쉬움 |
| 3 | **P3.2** CLI 명령 포팅 | ~600 lines | P3.1 후 |
| 4 | **P3.3** main.rs 핸들러 분리 | ~600 lines | P3.2 후 (위험) |
| 5 | **P4.1** Issue 시스템 격리 | ~500 lines | 독립적 |
| 6 | **P4.2** Package manager 재정렬 | ~500 lines | P4.1 후 |
| 7 | **P1.6a** debug 도구 재등록 | ~600 lines | DAP proxy 구현 후 |
| 8 | **P0.5** remote-AGENT providers | ~2000 lines | 요청 시 |
| 9 | **P2** TUI 재정렬 | ~10000 lines | 마지막 |

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
- **debug 재등록**: tools.rs 주석 해제 + tests/tools.rs 카운트 37→38
- **P3.3 위험**: clap Subcommand generic-bound 이슈 — 각 subcommand 테스트 필요
