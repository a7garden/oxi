# omp-정렬 리팩토링 — 남은 구현 명세

- **갱신**: 2026-07-28 (v2)
- **브랜치**: `main`
- **완료된 작업**: P0 (catalog + complexity 제거 + NamedProvider + KnownApi14), Step 2 (Provider::name() 제거), P1.1 (owned dialect), **P1.2** (intent tracing), **P1.3** (append-only context struct), **P1.6a** (eval, ast_grep, ast_edit 도구)
- **범위**: 완료된 작업 이후의 모든 미구현 작업. 총 4개 phase 잔여.
- **참조**: `handoff.md`(진입점), `design.md`(마스터 설계)
- **3613 tests passing**, clippy clean

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

## 완료된 작업 요약

|작업|커밋|변경 파일|설명|
|---|---|---|---|
|P0 catalog 분리|main|—|모델 카탈로그 models.dev 분리|
|P0 complexity 제거|main|—|multi_provider/complexity_router/provider_pool/circuit_breaker/fallback_chain 제거 (-4791 lines)|
|P0 NamedProvider + KnownApi14 + SSE|main|—|정체성 붕괴 수정, SSE 중앙화|
|P0 OllamaProvider|main|—|NDJSON 프로토콜|
|Provider::name() 제거|main|—|Step 2|
|P1.1 owned dialect|main|—|Dialect enum 11종, XML dialect renderer+parser, 24 tests|
|P1.2 Intent tracing|`87fb31fb`|7 files|AgentTool::intent(), ToolExecution{Start,End}.intent, AskTool intent, 통합 테스트|
|P1.3 Append-only context struct|`87fb31fb`|2 files|AppendOnlyContext (+replace_history, sync_from, queue_tool_result), 10 tests (루프 와이어링은 P1.4/1.5로 연기)|
|P1.6a eval 도구|`87fb31fb`|2 files|python3/bun/node 코드 실행, stdout/stderr 캡처, 종료 코드|
|P1.6a ast_grep 도구|`87fb31fb`|2 files|sg CLI 래퍼 — AST 패턴 검색, JSON-stream 파싱|
|P1.6a ast_edit 도구|`87fb31fb`|2 files|sg 기반 구조적 리라이트, dry-run + apply|
|P1.6a debug 도구 (infra only)|`87fb31fb`|2 files|ToolRegistry에서 등록 해제 — DAP 프록시 구현 후 재등록 필요|

---

## 우선순위 요약

| 순위 | 작업 | 영향 | 예상 규모 | 바로 시작 가능? |
|------|------|------|-----------|:---:|
| 1 | P1.4 Approval/tier 시스템 | 루프 보강 | ~300 lines | ✅ |
| 2 | P1.5 Soft req + Harmony leak | 루프 보강 | ~200 lines | ✅ |
| 3 | P1.3 루프 와이어링 (append-only → run_loop) | 루프 핵심 | ~300 lines | ⚠️ P1.4/1.5와 함께 |
| 4 | P1.6b Meta 6개 도구 | 에이전트 기능 | ~2400 lines | ✅ |
| 5 | P3 프롬프트 & CLI | 사용자 경험 | ~2000 lines | ⚠️ P1 후 |
| 6 | P4 oxi-original 정리 | 코드 품질 | ~1500 lines | ✅ (독립) |
| 7 | P1.6c Meta 6개 도구 | 에이전트 기능 | ~2400 lines | ✅ (P1.6b 후) |
| 8 | P1.6a debug 도구 재등록 | 에이전트 기능 | ~600 lines | DAP 프록시 구현 후 |
| 9 | P0.5 remote-AGENT providers | provider | ~2000 lines | ✅ (요청 시) |
| 10 | P2 TUI 재정렬 | UI | ~10000 lines | ❌ 가장 큼, 마지막 |

---

## Phase 1 — Agent 루프 재정렬

### P1.4 — Approval/tier 시스템 [NEXT]

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

### P1.5 — Soft tool requirements + Harmony leak [NEXT]

**무엇**: omp는 remind-then-escalate 패턴(soft requirements) + GPT-5 프로토콜 누수(Harmony leak) 감지.

**omp 참조**:
- `/tmp/omp/packages/agent/src/agent-loop.ts` — soft requirement 체크
- `/tmp/omp/packages/ai/src/utils/harmony-leak.ts` — Harmony leak 감지

**작업**:
1. Soft requirement 추적: 각 도구에 `required: bool | "soft"` 속성.
   - `soft` 도구 누락 시 첫 턴에 remind, 두 번째 턴에 escalate.
2. Harmony leak 감지: 정규식 기반 탐지, stream abort.

**수락 기준**: soft requirement 누락 → remind 에이전트 이벤트 발생.

**예상 규모**: ~200 lines, 1 commit.

---

### P1.3 후속 — Append-only context 루프 와이어링

**무엇**: `AppendOnlyContext` 구조체는 구현됨. `run_loop()`에 와이어링 필요.
**이유**: P1.4/1.5도 루프를 수정하므로 함께 작업하는 것이 효율적.

**작업**:
1. `stream_assistant_response()`가 메시지를 `&mut Vec<Message>`로 받는 대신 `AppendOnlyContext` 사용.
2. `maybe_compact()`에 `replace_history()` 통합.
3. `pending_tool_results` 큐를 통한 tool result 분리 (tool_exec.rs와 조정).
4. Dialect 활성 시 sync 생략.

---

### P1.6 — 누락 도구 나머지 포팅

#### 완료된 도구
- ✅ `eval` — 코드 실행 (python3/bun/node)
- ✅ `ast_grep` — AST 패턴 검색
- ✅ `ast_edit` — AST 구조적 리라이트
- 🟡 `debug` — 파일만 있고 등록 해제됨 (DAP 프록시 필요)

#### 미완료 (P1.6b) — 컨텍스트/메타 도구

| 도구 | omp 파일 | 설명 | 우선순위 |
|------|---------|------|:---:|
| `checkpoint` | `checkpoint.ts` | 작업 상태 저장/복원 | 중 |
| `rewind` | `rewind.ts` | 이전 상태로 되돌리기 | 중 |
| `hub` | `hub.ts` | 에이전트 간 통신 | 중 |
| `yield` | `yield.ts` | 에이전트 양보 | 중 |
| `goal` | `goal.ts` | 목표 관리 | 중 |
| `review` | `review.ts` | 코드 리뷰 요청 | 중 |

#### 미완료 (P1.6c) — 고급 도구

| 도구 | omp 파일 | 설명 | 우선순위 |
|------|---------|------|:---:|
| `learn` | `learn.ts` | 장기 기억 등록 | 하 |
| `manage_skill` | `manage-skill.ts` | SKILL.md 관리 | 하 |
| `inspect_image` | `inspect-image.ts` | 이미지 분석 | 하 |
| `computer` | `computer.ts` | 컴퓨터 제어 (Vision) | 하 |
| `tts` | `tts.ts` | 음성 합성 | 하 |
| `vibe` | `vibe.ts` | 분위기/무드 설정 | 하 |

**omp 참조**: `/tmp/omp/packages/coding-agent/src/tools/`

---

## Phase 3 — 프롬프트 & CLI 재정렬

**대상 크레이트**: `oxi-cli/`, `oxi-ai/`

### P3.1 — `.md` 기반 시스템 프롬프트

현재 inline Rust 문자열(`prompt/system_prompt.rs` 736줄) → `.md` 파일 `include_str!()`으로 전환.

**omp 참조**: `/tmp/omp/packages/coding-agent/src/prompts/`

### P3.2 — CLI 명령 포팅

**omp 참조**: `/tmp/omp/packages/cli/src/commands/`

주요 누락 명령: `completions`, `config` (CLI 접근), `install` (MCP 서버), `update`, `commit`.

### P3.3 — `main.rs` 핸들러 분리 (F-5)

`main.rs`의 `handle_subcommand` (~90 lines) + inline `handle_*` 함수 (~1400 LOC)를 `cli/commands/*.rs`로 분리.

**위험**: clap Subcommand-derived enum과 generic-bound 호환성 이슈.

---

## Phase 4 — oxi-original 처리

**대상 크레이트**: `oxi-cli/`

### P4.1 — Issue 시스템 격리

Issue 시스템(CAS + flock)을 agent 루프/session 모델에서 분리. 명시적 API boundary 뒤로 이동.

### P4.2 — Package manager → omp 플러그인 모델

`oxi-cli/src/storage/packages.rs`(106KB)를 omp `extensibility/plugins/` 모델에 맞춤.

### P4.3 — Language policy 제거

`Settings::output_languages`, `KNOWN_CHANNELS` 제거. (현재 opt-in, 기본 동작 무변경.)

### P4.4 — Dead config 필드 정리

`circuit_breaker_failure_threshold`, `enable_routing`, `prefer_cost_efficient`, `fallback_chain` 등 제거.

---

## P0.5 — remote-AGENT provider 포팅 (요청 시)

Cursor/Devin/GitLab Duo transport. `Api` enum에 variant 존재, transport만 `_ => None`.
각각 고유 프로토콜 (WebSocket + SSE, GitLab API). 높은 노력.

---

## Phase 2 — TUI 재정렬 (가장 큼, 마지막)

**omp 참조**: `/tmp/omp/packages/tui/src/tui.ts`(173KB)
**대상 크레이트**: `oxi-tui`, `oxi-tui-legacy → oxi-tui rename`

1. `oxi-tui-legacy` → `oxi-tui` rename
2. omp 3-전략 차등 렌더링 Rust 구현 (Component memoization, Native scrollback commit, ED3 replay)
3. Append-only "tape" 렌더 계약
4. 전체 입력 시스템 (Kitty keyboard, bracketed paste, keybinding, mouse SGR, kill ring, undo)
5. LaTeX, mermaid, image rendering
6. Glyph 시스템 단일화

---

## 변경 시 주의사항

### dialect `xml.rs` 코드 작성 금지 규칙
`oxi-ai/src/dialect/xml.rs`에 literal XML 태그(`<invoke`, `<parameter`, `</invoke>`)를 **절대 포함하지 마십시오**. harness wire framing과 충돌:
- 모든 태그는 `concat!("<", "invoke")` / `concat!("</", "parameter>")` 형태로 빌드
- 문서 작성 시 prose로 설명
- `edit` 도구로 기존 xml.rs 수정은 안전 (직접 파일을 읽고 쓰므로)

### 회귀 게이트
변경 후 항상 실행:
```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```

### Config 필드 추가 규칙
`AgentLoopConfig`에 새 필드 추가 시 `Default::default()`에도 반드시 `None`/`false` 기본값 추가.

### debug_tool 재등록
`oxi-agent/src/tools.rs`에서 주석 처리된 `all_tools.push(Box::new(debug_tool::DebugTool));`의 주석 해제 필요.
DAP 프록시 구현 후 `oxi-agent/tests/tools.rs`의 카운트도 `25` → `26`으로 업데이트.
