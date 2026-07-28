# omp-정렬 리팩토링 — 남은 구현 명세

- **갱신**: 2026-07-28
- **브랜치**: `main` (P0 + Step 2 + P1.1 owned dialect 완료)
- **범위**: 완료된 작업 이후의 모든 미구현 작업. 총 5개 phase, ~30여개 작업.
- **참조**: `handoff.md`(진입점), `status.md`(진행 상황), `design.md`(마스터 설계)

---

## 빠른 시작

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git checkout main

# 회귀 게이트 (각 변경마다)
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check

# omp 소스 (포팅/참조용)
ls /tmp/omp 2>/dev/null || git clone https://github.com/can1357/oh-my-pi.git /tmp/omp
```

---

## 우선순위 요약

| 순위 | 작업 | 영향 | 예상 규모 | 바로 시작 가능? |
|------|------|------|-----------|:---:|
| 1 | P1.2 Intent tracing (`i` 필드) | 루프 핵심 | ~100 lines | ✅ |
| 2 | P1.3 Append-only context | 루프 핵심 | ~300 lines | ✅ |
| 3 | P1.6a 핵심 도구 4개 | 에이전트 기능 | ~800 lines ea | ✅ (병렬) |
| 4 | P3 프롬프트 & CLI | 사용자 경험 | ~2000 lines | ⚠️ P1 후 |
| 5 | P4 oxi-original 정리 | 코드 품질 | ~1500 lines | ✅ (독립) |
| 6 | P1.4-5 Approval + Soft req | 루프 보강 | ~500 lines | ⚠️ P1.2-3 후 |
| 7 | P1.6b Meta 6개 도구 | 에이전트 기능 | ~400 lines ea | ✅ (병렬, P1.6a 후) |
| 8 | P0.5 remote-AGENT providers | provider | ~2000 lines | ✅ (요청 시) |
| 9 | P1.6c Meta 6개 도구 | 에이전트 기능 | ~400 lines ea | ✅ (병렬, P1.6b 후) |
| 10 | P2 TUI 재정렬 | UI | ~10000 lines | ❌ 가장 큼, 마지막 |

---

## Phase 1 — Agent 루프 재정렬 [진행 중]

**완료된 P1 작업**: P1.1 owned dialect (엔진 + 루프 wiring + 수락 테스트).
**omp 참조**: `/tmp/omp/packages/agent/src/` (agent.ts 56KB, agent-loop.ts 102KB, types.ts 35KB)
**대상 크레이트**: `oxi-agent/`

---

### P1.2 — Intent tracing (`i` 필드) [HIGH]

**무엇**: omp `AgentTool`에 `i` 필드(intent trace)가 있어 루프가 도구 호출 의도를 추적. oxi trait은 약 10개 메서드, intent 필드 없음.

**omp 참조**:
- `/tmp/omp/packages/agent/src/types.ts` — `AgentTool` 인터페이스(18+ 필드)의 `i` 필드
- `/tmp/omp/packages/agent/src/agent-loop.ts` — 루프에서 `i` 주입/추출 패턴
- `/tmp/omp/packages/agent/src/utils/intent.ts` — intent 트레이싱 유틸리티

**작업**:
1. `oxi-agent/src/tools.rs` `AgentTool` trait에 `fn intent(&self) -> Option<&str>` 추가 (기본 `None`).
2. `AgentToolResult`에 `intent: Option<String>` 필드 추가.
3. 루프(`agent_loop/mod.rs`의 `run_loop`)에서 `i` 주입: `ToolContext`에 `intent` 필드, `AgentEvent::ToolExecutionStart`에 intent 포함.
4. `AgentEvent::ToolExecutionEnd`에도 intent 포함.
5. `ask` 도구 (인간에게 질문하는 도구)에서 intent 활용—사용자에게 왜 이 도구를 호출하는지 보여줌.

**수락 기준**: `ToolExecutionStart`/`ToolExecutionEnd` 이벤트에 intent 필드 포함.

**예상 규모**: ~100 lines, 1 commit.

**위험**: 낮음 — 기존 호출자에 `None` 기본값으로 호환성 유지.

---

### P1.3 — Append-only context (prefix caching) [HIGH]

**무엇**: omp는 컨텍스트를 append-only로 유지해 안정적인 prefix caching. oxi는 메시지를 재구성할 수 있어 캐싱이 깨짐.

**omp 참조**:
- `/tmp/omp/packages/agent/src/append-only-context.ts` — 핵심 구현 (~500 lines)
- `/tmp/omp/packages/agent/src/agent-loop.ts` — 사용 패턴
- `/tmp/omp/packages/ai/src/context.ts` — Context 타입

**작업**:
1. `oxi-agent/src/agent_loop/`에 `AppendOnlyContext` 구조체 생성:
   - `messages: Vec<Message>` — 불변 이력
   - `tool_results: Vec<Message>` — 현재 턴의 tool result 큐 (다음 턴에 이력으로 이동)
   - `tool_choice: Option<ToolChoiceDirective>` — 툴 초이스 큐
   - `sync_messages()` — 외부 메시지와 동기화 (새 메시지만 추가)
2. agent loop에서 context 빌드 시 append-only계약 적용:
   - `messages`를 매번 재구성하지 않고 `all_past_messages + pending_tool_results`로 구성.
   - 이전 턴과 같은 prefix 유지 → provider prefix caching 최대 활용.
3. `Dialect` 활성 시 `syncMessages` 호출 생략 (in-band 텍스트가 이미 일관된 prefix 유지).

**수락 기준**: 연속 턴에서 prefix가 바뀌지 않음을 검증하는 테스트.

**예상 규모**: ~300 lines, 1–2 commits.

**위험**: 중간 — 루프 컨텍스트 빌드 변경, 기존 메시지 처리 경로와 조화 필요.

---

### P1.4 — Approval/tier 시스템 [MED]

**무엇**: omp는 사용자 확인 게이트(approval tiers). oxi는 `AccessGate` port (oxi-sdk)가 있지만 루프 통합이 다름.

**omp 참조**:
- `/tmp/omp/packages/agent/src/agent-loop.ts` — approval 분기
- omp의 `beforeToolCall`/`afterToolCall` 훅 + approval tiers

**작업**:
1. 위험도 분류: 읽기(read/grep/ls/find) / 쓰기(write/edit) / 실행(bash/computer).
2. `oxi-sdk` `AccessGate` port를 agent loop에 통합.
3. `AgentLoopConfig`에 `approval_tiers: ApprovalConfig` 필드 (opt-in, 기본 해제).
4. 실행 전 확인 프롬프트: 쓰기/실행 도구 앞에서 사용자에게 확인.
5. `AgentEvent::ApprovalRequired` / `ApprovalResult` 이벤트.

**수락 기준**: 쓰기/실행 도구 전 사용자 확인 프롬프트를 보내는 통합 테스트.

**예상 규모**: ~300 lines, 1 commit.

**위험**: 낮음 — opt-in, 기존 동작 무변경.

---

### P1.5 — Soft tool requirements + Harmony leak [MED]

**무엇**: omp는 remind-then-escalate 패턴(soft requirements) + GPT-5 프로토콜 누수(Harmony leak) 감지. oxi 둘 다 없음.

**omp 참조**:
- `/tmp/omp/packages/agent/src/agent-loop.ts` — soft requirement 체크
- `/tmp/omp/packages/ai/src/utils/harmony-leak.ts` — Harmony leak 감지

**작업**:
1. Soft requirement 추적: 각 도구에 `required: bool | "soft"` 속성.
   - `soft` 도구 누락 시 첫 턴에 remind, 두 번째 턴에 escalate.
   - oxi의 `AgentTool::essential()` (bool)과 통합.
2. Harmony leak 감지: OpenAI 계열 모델이 GPT-5 내부 프로토콜(Harmony control tokens)을 출력하는지 감지.
   - 정규식 기반 탐지.
   - 감지 시 stream abort + `ProviderEvent::Error`로 라우팅.

**수락 기준**: soft requirement 누락 → remind 에이전트 이벤트 발생.

**예상 규모**: ~200 lines, 1 commit.

**위험**: 낮음 — 독립적 추가.

---

### P1.6 — 누락 도구 16개 포팅 [LARGE]

**omp 참조**: `/tmp/omp/packages/coding-agent/src/tools/`

**우선순위 및 병렬화**:

#### 핵심 (P1.6a) — 바로 시작 가능, 병렬

| 도구 | omp 파일 | 설명 | 우선순위 |
|------|---------|------|:---:|
| `ast_grep` | `ast-grep.ts` | AST 패턴 검색 | 1 |
| `ast_edit` | `ast-edit.ts` | AST 인식 수정 | 1 |
| `debug` | `debug.ts` | 디버거 통합 (DAP) | 1 |
| `eval` | `eval.ts` | 코드 실행 (IPython/Bun) | 1 |

각각 `oxi-agent/src/tools/<name>.rs` 생성, `AgentTool` trait 구현, `ToolRegistry::with_builtins_cwd()`에 등록. omp 소스를 Rust 관용구로 번역 (Bun/N-API 특산물 미포팅).

**예상 규모**: 각 400–1200 lines. **dispatching-parallel-agents** skill로 병렬 가능.

**수락 기준**: 각 도구 단위 커밋 + 테스트.

#### 컨텍스트/메타 (P1.6b/c) — 핵심 완료 후

| 도구 | omp 파일 | 설명 | 배치 |
|------|---------|------|:---:|
| `checkpoint` | `checkpoint.ts` | 작업 상태 저장/복원 | P1.6b |
| `rewind` | `rewind.ts` | 이전 상태로 되돌리기 | P1.6b |
| `hub` | `hub.ts` | 에이전트 간 통신 | P1.6b |
| `yield` | `yield.ts` | 에이전트 양보 | P1.6b |
| `goal` | `goal.ts` | 목표 관리 | P1.6b |
| `review` | `review.ts` | 코드 리뷰 요청 | P1.6b |
| `learn` | `learn.ts` | 장기 기억 등록 | P1.6c |
| `manage_skill` | `manage-skill.ts` | SKILL.md 관리 | P1.6c |
| `inspect_image` | `inspect-image.ts` | 이미지 분석 | P1.6c |
| `computer` | `computer.ts` | 컴퓨터 제어 (Vision) | P1.6c |
| `tts` | `tts.ts` | 음성 합성 | P1.6c |
| `vibe` | `vibe.ts` | 분위기/무드 설정 | P1.6c |

---

### P1.7 — Streaming scanner (선택 강화)

**무엇**: 현재 배치 파서(턴 완료 시 parse) → 증분 스캐너(스트리밍 중 parse). omp `InbandStreamProjector` + fabrication abort.

**omp 참조**:
- `/tmp/omp/packages/ai/src/dialect/owned-stream.ts` — `InbandStreamProjector` (~300 lines)
- `/tmp/omp/packages/ai/src/dialect/anthropic.ts` — `AnthropicInbandScanner` (~600 lines)

**작업**:
1. `InbandScanner` trait의 `feed(text) -> Vec<InbandScanEvent>` Rust 구현.
2. `InbandStreamProjector`: streaming TextDelta 중간중간 tool call을 감지하고 `ToolCall*` 이벤트 발행.
3. Fabrication abort: 모델이 tool call을 위조하면 stream abort.

**예상 규모**: ~1000 lines.

**비고**: 필수 아님 — 배치 파서가 기능적으로 동일. 토큰 단위 UX 개선용.

---

## Phase 3 — 프롬프트 & CLI 재정렬

**의존**: P0 완료.
**대상 크레이트**: `oxi-cli/`, `oxi-ai/`

---

### P3.1 — `.md` 기반 시스템 프롬프트

**무엇**: 현재 inline Rust 문자열(`prompt/system_prompt.rs` 736줄). omp처럼 `.md` 파일 `include_str!()`으로 전환.

**omp 참조**:
- `/tmp/omp/packages/coding-agent/src/prompts/` — 시스템 프롬프트, personalities, tool prompts
- `/tmp/omp/packages/agent/src/prompts/` — agent 코어 프롬프트

**작업**:
1. `oxi-cli/prompts/system/system-prompt.md` 생성 (`include_str!()`).
2. 경량 템플릿 엔진: `{{date}}`, `{{cwd}}`, `{{git_branch}}`, `{{os}}`, `{{arch}}` 등 치환.
3. Personality 시스템: `prompts/system/personalities/default.md`, `friendly.md`, `pragmatic.md`.
4. Tool-specific prompt `.md`: `prompts/tools/read.md`, `write.md`, `bash.md`, ... ~45개.

**예상 규모**: ~1000 lines.

---

### P3.2 — CLI 명령 포팅

**무엇**: omp CLI 명령 중 누락된 것 포팅.

**omp 참조**: `/tmp/omp/packages/cli/src/commands/`

**작업** (omp ↔ oxi 명령 비교):

| omp 명령 | oxi 현 | 포팅 필요? |
|----------|:------:|:---:|
| `bench` | ✗ | 낮음 |
| `commit` | ✗ | 낮음 (omp는 코드 검토 + git commit 통합) |
| `completions` | ✗ | 중간 |
| `config` | ✗ (oxi는 `/settings`) | 중간 (설정 CLI 접근) |
| `gc` | ✗ | 낮음 (캐시 정리) |
| `grep` | ✗ | 낮음 (도구로만 존재) |
| `gallery` | ✗ | 낮음 |
| `install` | ✗ | 중간 (MCP 서버 설치) |
| `models` | ✓ `oxi models` | — |
| `plugin` | ✗ | 낮음 |
| `setup` | ✓ `oxi setup` | — |
| `shell` | ✗ | 낮음 (쉘 통합) |
| `stats` | ✗ | 낮음 |
| `update` | ✗ | 중간 (자체 업데이트) |
| `usage` | ✗ | 낮음 |
| `worktree` | ✗ | 낮음 |
| `search` | ✗ | 낮음 |

**예상 규모**: ~500 lines (주요 명령).

---

### P3.3 — `bootstrap.rs`/`lib.rs` 경계 정리 + F-5

**무엇**: `main.rs`의 `handle_subcommand` ~90 lines + inline `handle_*` 함수 ~1400 LOC를 `cli/commands/*.rs`로 분리.

**작업**:
1. `oxi-cli/src/cli/commands/` 디렉토리 생성.
2. 각 `handle_*` 함수를 별도 파일로 이동 (`config.rs`, `session.rs`, `export.rs`, `share.rs`, `setup.rs`, `models.rs`).
3. `main.rs`의 `handle_subcommand` match arm에서 위임 호출로 대체.

**위험**: clap Subcommand-derived enum과 generic-bound 호환성 이슈 — 각 핸들러 분리 후 빌드 검증 필요.

**예상 규모**: ~500 lines.

---

## Phase 4 — oxi-original 처리

**의존**: P1, P3 (느슨).

---

### P4.1 — Issue 시스템 격리

**무엇**: issue 시스템(CAS + flock)은 유지하되 agent 루프/session 모델에서 분리.

**현재 상태**:
- `oxi-cli`에 issue tool, store, overlay 존재.
- Phase 0 버그(#13: session_id None) 수정 완료. Phase 2(#2: CAS strict store) 완료.
- agent 루프와 session 모델에 스며들어 있음.

**작업**:
1. 명시적 API boundary 뒤로 이동 (예: `oxi-sdk` port 또는 독립 모듈).
2. agent loop / session model과의 결합 제거.
3. issue 관련 `AgentEvent` variant 정리.

**예상 규모**: ~300 lines.

---

### P4.2 — Package manager → omp 플러그인 모델 재정렬

**무엇**: `oxi-cli/src/storage/packages.rs`(106KB)를 omp `extensibility/plugins/` 모델에 맞춤.

**omp 참조**:
- `/tmp/omp/extensibility/plugins/` — git caching, install/uninstall

**작업**:
1. `storage/packages.rs`의 106KB 재설계: git 기반 확장 설치/관리.
2. omp plugin 모델의 install/uninstall/update lifecycle Rust 구현.
3. WASM/native extension 분리 유지 (omp는 js만, oxi는 WASM+native).

**예상 규모**: ~1000 lines.

---

### P4.3 — Language policy 제거/단순화

**무엇**: `Settings::output_languages`, `KNOWN_CHANNELS` (`response`, `code_comment`, `documentation`, `commit_message`), TUI-only 주입.

**현재 상태**: 구현되어 있고 TUI에서 opt-in. AGENTS.md에 ~200줄 방어 문서 있음.

**작업**:
1. `Settings::output_languages` 필드 제거.
2. `KNOWN_CHANNELS` 상수 제거.
3. `build_system_prompt`의 language directive 제거.
4. 관련 AGENTS.md 문서 정리.

**예상 규모**: ~200 lines.

**위험**: 낮음 — 사용자가 명시적으로 opt-in한 경우에만 활성화되므로 제거해도 기본 동작 무변경.

---

### P4.4 — Dead config 필드 정리

**무엇**: `oxi-cli/settings.rs`에 남아있는 dead config 필드 제거.

**필드 목록**:
- `circuit_breaker_failure_threshold`
- `circuit_breaker_open_duration_secs`
- `enable_routing`
- `prefer_cost_efficient`
- `fallback_chain`
- `disable_fallback`

**참고**: serde가 unknown key를 무시하므로 기존 settings.toml은 안전. P4에서 정리 권장.

**예상 규모**: ~100 lines.

---

## P0.5 — remote-AGENT provider 포팅

**Api enum에 variant 존재(`CursorAgent`, `DevinAgent`, `GitLabDuoAgent`), transport만 `_ => None`.**

**omp 참조**:
- `/tmp/omp/packages/ai/src/providers/cursor.ts`
- `/tmp/omp/packages/ai/src/providers/devin.ts`
- `/tmp/omp/packages/ai/src/providers/gitlab-duo.ts`

**우선순위**: 낮음 — 사용자가 직접 요청할 때 진행.

**작업**:
1. `oxi-ai/src/providers/cursor.rs` — 고유 프로토콜 (WebSocket + SSE).
2. `oxi-ai/src/providers/devin.rs` — 고유 프로토콜.
3. `oxi-ai/src/providers/gitlab-duo.rs` — 고유 프로토콜 (GitLab API 기반).
4. `build_builtin_transport()` 각 arm 추가.
5. 각각 9+ tests.

**예상 규모**: 각 600–1000 lines.

**비고**: Cursor/Devin/GitLab Duo는 OpenAI-compatible endpoint가 아님 — 각각 고유 stream function + 고유 프로토콜. 높은 노력.

---

## Phase 2 — TUI 재정렬 (가장 큼, 다-월간)

**omp 참조**: `/tmp/omp/packages/tui/src/tui.ts`(173KB)
**대상 크레이트**: `oxi-tui` (현 legacy → rename), `oxi-tui-legacy` 폐기.

**방침 (사용자 확정)**: T1 — legacy→omp tape 진화, v2 폐기.

**작업**:
1. `oxi-tui-legacy` → `oxi-tui` rename (`Cargo.toml`, `lib.rs`, 모든 참조).
2. omp 3-전략 차등 렌더링 Rust 구현:
   - Component memoization (현재 RetainedTree).
   - Native scrollback commit (새 렌더 전략).
   - ED3 replay (비상 복구 전략).
3. Append-only "tape" 렌더 계약:
   - `append()` → `render()` ↔ `diff()` → `flush()`.
   - 기존 `draw_frame()` 교체.
4. 전체 입력 시스템:
   - Kitty keyboard protocol.
   - Bracketed paste.
   - Keybinding system (사용자 정의 가능).
   - Mouse SGR 1006.
   - Kill ring (복사/잘라내기 ring).
   - Undo.
5. LaTeX, mermaid(legacy 85KB 이관), image rendering(Kitty/iTerm2/Sixel).
6. Glyph 시스템 단일화 (현재 legacy + v2에 중복).
7. v2 crate 삭제.

**예상 규모**: ~10000 lines (가장 큰 작업).

**비고**: 단계적 접근 — 1) rename → 2) 입력 시스템 → 3) tape 계약 → 4) 렌더링 → 5) v2 삭제.

---

## 작업별 omp 파일 매핑 (빠른 참조)

| 작업 | omp 참조 파일 |
|------|--------------|
| P1.2 Intent tracing | `/tmp/omp/packages/agent/src/types.ts`, `agent-loop.ts` |
| P1.3 Append-only context | `/tmp/omp/packages/agent/src/append-only-context.ts` |
| P1.4 Approval/tier | `/tmp/omp/packages/agent/src/agent-loop.ts` (approval 분기) |
| P1.5 Soft req / Harmony leak | `/tmp/omp/packages/agent/src/agent-loop.ts` + `/tmp/omp/packages/ai/src/utils/harmony-leak.ts` |
| P1.6a ast_grep | `/tmp/omp/packages/coding-agent/src/tools/ast-grep.ts` |
| P1.6a ast_edit | `/tmp/omp/packages/coding-agent/src/tools/ast-edit.ts` |
| P1.6a debug | `/tmp/omp/packages/coding-agent/src/tools/debug.ts` |
| P1.6a eval | `/tmp/omp/packages/coding-agent/src/tools/eval.ts` |
| P1.6b checkpoint | `/tmp/omp/packages/coding-agent/src/tools/checkpoint.ts` |
| P1.6b rewind | `/tmp/omp/packages/coding-agent/src/tools/rewind.ts` |
| P1.6b hub | `/tmp/omp/packages/coding-agent/src/tools/hub.ts` |
| P1.6b yield | `/tmp/omp/packages/coding-agent/src/tools/yield.ts` |
| P1.6b goal | `/tmp/omp/packages/coding-agent/src/tools/goal.ts` |
| P1.6b review | `/tmp/omp/packages/coding-agent/src/tools/request-review.ts` |
| P1.6c learn | `/tmp/omp/packages/coding-agent/src/tools/learn.ts` |
| P1.6c manage_skill | `/tmp/omp/packages/coding-agent/src/tools/manage-skill.ts` |
| P1.6c inspect_image | `/tmp/omp/packages/coding-agent/src/tools/inspect-image.ts` |
| P1.6c computer | `/tmp/omp/packages/coding-agent/src/tools/computer.ts` |
| P1.6c tts | `/tmp/omp/packages/coding-agent/src/tools/tts.ts` |
| P1.6c vibe | `/tmp/omp/packages/coding-agent/src/tools/vibe.ts` |
| P1.7 Streaming scanner | `/tmp/omp/packages/ai/src/dialect/owned-stream.ts` |
| P3.1 .md prompts | `/tmp/omp/packages/coding-agent/src/prompts/` |
| P3.2 CLI commands | `/tmp/omp/packages/cli/src/commands/` |
| P4.2 Plugin model | `/tmp/omp/extensibility/plugins/` |
| P0.5 cursor | `/tmp/omp/packages/ai/src/providers/cursor.ts` |
| P0.5 devin | `/tmp/omp/packages/ai/src/providers/devin.ts` |
| P0.5 gitlab-duo | `/tmp/omp/packages/ai/src/providers/gitlab-duo.ts` |
| P2 TUI | `/tmp/omp/packages/tui/src/tui.ts`(173KB) |

---

## 변경 시 주의사항

### dialect `xml.rs` 코드 작성 금지 규칙

`oxi-ai/src/dialect/xml.rs` 소스에 literal XML 태그(`<invoke`, `<parameter`, `</invoke>`, `</parameter>`, `<tool_response>`, `</thinking>`, `<thinking>`)를 **절대 포함하지 마십시오**. harness wire framing과 충돌합니다.

모든 태그는 `concat!("<", "invoke")` / `concat!("</", "parameter>")` 형태로 빌드하세요. 문서 작성 시에는 `invoke 요소` / `parameter 요소` 등 prose로 설명하세요.

파일을 처음 작성할 때는 Python/JS eval 셸에서 `chr(60)` + `'invoke'` + `chr(62)` 등으로 문자 코드를 조립하는 것이 안전합니다.

이미 존재하는 xml.rs를 수정할 때는 `edit` 도구가 안전합니다 (파일을 직접 읽고 쓰므로 harness parser가 중간에 가로채지 않습니다).

### 회귀 게이트
변경 후 항상 실행: `cargo build --workspace` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` → `cargo fmt --all -- --check` → `cargo nextest run --workspace` (3581+ tests).

### Config 필드 추가 규칙
`AgentLoopConfig`에 새 필드를 추가할 때는 `Default::default()`에도 반드시 `None`/`false` 기본값을 추가하세요. 구조체 필드 순서와 `Default` 초기화 순서는 같을 필요가 없습니다.

### 패키지 구조
`oxi-agent/src/agent_loop/config.rs` = `AgentLoopConfig` struct. `streaming.rs` = provider streaming entry point. `mod.rs` = `run_loop()` (메인 루프). `tool_exec.rs` = 도구 실행. `helpers.rs` = 공유 유틸리티.
