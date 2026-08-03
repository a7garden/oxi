# omp-정렬 리팩토링 — 남은 구현 명세 v2

- **갱신**: 2026-07-28 (v2)
- **브랜치**: `main`
- **이번 세션 완료**: P1.4 (approval/tier), P1.5 (soft req + harmony leak), P1.3 (AppendOnlyContext loop wiring)
- **기준선**: 3617 tests passing, clippy clean, fmt clean
- **참조**: `handoff.md`(진입점), `design.md`(마스터 설계)

---

## 빠른 시작

```bash
cd /Volumes/MERCURY/PROJECTS/oxicode
git checkout main

# 회귀 게이트 (각 변경마다)
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check

# omp 소스 (포팅/참조용)
ls /tmp/omp 2>/dev/null || git clone https://github.com/can1357/oh-my-pi.git /tmp/omp
```

---

## 이번 세션 완료된 작업

|작업|설명|변경 파일|
|---|---|---|
|P1.4 Approval/tier|ToolTier enum, ApprovalConfig/ApprovalDecision/ApprovalHook, ApprovalRequired/ApprovalResult events, approval check in tool_exec (sequential + parallel), 4 integration tests|`tools.rs`, `events.rs`, `config.rs`, `tool_exec.rs`|
|P1.5 Soft req + Harmony leak|SoftRequirement/SoftRequirementState types, remind/escalate lifecycle, harmony leak regex detection (LazyLock), stream abort on detection, 3 new event variants|`events.rs`, `config.rs`, `mod.rs`, `streaming.rs`|
|P1.3 AppendOnlyContext wiring|AppendOnlyContext in run_loop, sync after maybe_compact, final sync before return|`mod.rs`|

---

## 남은 작업 우선순위

| 순위 | 작업 | 영향 | 예상 규모 | 바로 시작 가능? |
|------|------|------|-----------|:---:|
| 1 | P1.6b Meta 6개 도구 | 에이전트 기능 | ~2400 lines | ✅ |
| 2 | P3 프롬프트 & CLI | 사용자 경험 | ~2000 lines | ⚠️ P1 후 |
| 3 | P4 oxicode-original 정리 | 코드 품질 | ~1500 lines | ✅ (독립) |
| 4 | P1.6c 고급 도구 6개 | 에이전트 기능 | ~2400 lines | ✅ (P1.6b 후) |
| 5 | P1.6a debug 도구 재등록 | 에이전트 기능 | ~600 lines | DAP 프록시 구현 후 |
| 6 | P0.5 remote-AGENT providers | provider | ~2000 lines | ✅ (요청 시) |
| 7 | P2 TUI 재정렬 | UI | ~10000 lines | ❌ 가장 큼, 마지막 |

---

## Phase 1 — Agent 루프 재정렬 (계속)

### P1.6b — Meta 6개 도구 [NEXT]

**omp 참조**: `/tmp/omp/packages/coding-agent/src/tools/`

| 도구 | omp 파일 | 설명 |
|------|---------|------|
| `checkpoint` | `checkpoint.ts` | 작업 상태 저장/복원 |
| `rewind` | `rewind.ts` | 이전 상태로 되돌리기 |
| `hub` | `hub.ts` | 에이전트 간 통신 |
| `yield` | `yield.ts` | 에이전트 양보 |
| `goal` | `goal.ts` | 목표 관리 |
| `review` | `review.ts` | 코드 리뷰 요청 |

**작업**:
1. 각 도구마다 `oxicode-agent/src/tools/<name>.rs` 생성
2. `AgentTool` trait 구현 (name, label, description, parameters_schema, execute)
3. `oxicode-agent/src/tools.rs`에 module 선언 + `ToolRegistry::with_builtins_cwd()` 등록
4. intent tracing 추가
5. 각 도구 단위 테스트

**수락 기준**: 6개 도구 모두 등록, 단위 테스트 통과.

**예상 규모**: ~2400 lines, 6개 도구 파일 + 등록.

---

### P1.6c — 고급 도구 6개

**omp 참조**: `/tmp/omp/packages/coding-agent/src/tools/`

| 도구 | omp 파일 | 설명 |
|------|---------|------|
| `learn` | `learn.ts` | 장기 기억 등록 |
| `manage_skill` | `manage-skill.ts` | SKILL.md 관리 |
| `inspect_image` | `inspect-image.ts` | 이미지 분석 |
| `computer` | `computer.ts` | 컴퓨터 제어 (Vision) |
| `tts` | `tts.ts` | 음성 합성 |
| `vibe` | `vibe.ts` | 분위기/무드 설정 |

**작업**: P1.6b와 동일 패턴. `inspect_image`/`computer`는 Vision LLM 의존성 있음.

**예상 규모**: ~2400 lines.

---

### P1.6a — debug 도구 재등록

debug 도구는 `oxicode-agent/src/tools/debug_tool.rs`에 파일이 있지만 `ToolRegistry::with_builtins_cwd()`에서 주석 처리되어 있음.
재등록하려면 DAP (Debug Adapter Protocol) 프록시 구현이 선행되어야 함.

```rust
// oxicode-agent/src/tools.rs: 주석 해제 대상
all_tools.push(Box::new(debug_tool::DebugTool));
```

**작업**:
1. DAP 프록시 구현 (DebugTool이 외부 DAP 서버와 통신)
2. `oxicode-agent/tests/tools.rs`의 도구 카운트 업데이트 (25 → 26)
3. 등록 해제된 `all_tools.push(...)` 주석 해제

**예상 규모**: ~600 lines.

---

## Phase 3 — 프롬프트 & CLI 재정렬

**대상 크레이트**: `oxicode-cli/`, `oxicode-ai/`

### P3.1 — `.md` 기반 시스템 프롬프트

현재 inline Rust 문자열(`oxicode-cli/src/prompt/system_prompt.rs` 736줄)을 `.md` 파일(`include_str!()`)로 전환.

**omp 참조**: `/tmp/omp/packages/coding-agent/src/prompts/`

**작업**:
1. `oxicode-cli/src/prompts/` 디렉토리 생성
2. system prompt를 `.md` 파일로 분리
3. `include_str!()`으로 로드
4. 기존 inline 문자열 제거

**예상 규모**: ~800 lines.

### P3.2 — CLI 명령 포팅

**omp 참조**: `/tmp/omp/packages/cli/src/commands/`

주요 누락 명령: `completions`, `config` (CLI 접근), `install` (MCP 서버), `update`, `commit`.

**예상 규모**: ~600 lines.

### P3.3 — `main.rs` 핸들러 분리 (F-5)

`main.rs`의 `handle_subcommand` (~90 lines) + inline `handle_*` 함수 (~1400 LOC)를 `cli/commands/*.rs`로 분리.

**위험**: clap Subcommand-derived enum과 generic-bound 호환성 이슈. 분리 전에 각 subcommand 테스트 필요.

**예상 규모**: ~600 lines.

---

## Phase 4 — oxicode-original 처리

**대상 크레이트**: `oxicode-cli/`

### P4.1 — Issue 시스템 격리

Issue 시스템(CAS + flock)을 agent 루프/session 모델에서 분리. 명시적 API boundary 뒤로 이동.

**작업**:
1. Issue 관련 코드를 `oxicode-cli/src/store/issues/`로 이동
2. Agent loop에서 직접 참조하는 부분을 port로 추상화
3. 기존 `IssueStore` 의존성 정리

**예상 규모**: ~500 lines.

### P4.2 — Package manager → omp 플러그인 모델

`oxicode-cli/src/storage/packages.rs`(106KB)를 omp `extensibility/plugins/` 모델에 맞춤.

**예상 규모**: ~500 lines.

### P4.3 — Language policy 제거

`Settings::output_languages`, `KNOWN_CHANNELS` 제거. (현재 opt-in, 기본 동작 무변경.)

**예상 규모**: ~200 lines.

### P4.4 — Dead config 필드 정리

`circuit_breaker_failure_threshold`, `enable_routing`, `prefer_cost_efficient`, `fallback_chain` 등 제거.

**예상 규모**: ~300 lines.

---

## P0.5 — remote-AGENT provider 포팅 (요청 시)

Cursor / Devin / GitLab Duo transport. `Api` enum에 variant 존재, transport만 `_ => None`.
각각 고유 프로토콜 (WebSocket + SSE, GitLab API). 높은 노력.

**예상 규모**: ~2000 lines.

---

## Phase 2 — TUI 재정렬 (가장 큼, 마지막)

**omp 참조**: `/tmp/omp/packages/tui/src/tui.ts`(173KB)
**대상 크레이트**: `oxicode-tui`, `oxicode-tui-legacy → oxicode-tui rename`

1. `oxicode-tui-legacy` → `oxicode-tui` rename
2. omp 3-전략 차등 렌더링 Rust 구현 (Component memoization, Native scrollback commit, ED3 replay)
3. Append-only "tape" 렌더 계약
4. 전체 입력 시스템 (Kitty keyboard, bracketed paste, keybinding, mouse SGR, kill ring, undo)
5. LaTeX, mermaid, image rendering
6. Glyph 시스템 단일화

**예상 규모**: ~10000 lines.

---

## 변경 시 주의사항

### dialect `xml.rs` 코드 작성 금지 규칙
`oxicode-ai/src/dialect/xml.rs`에 literal XML 태그(`<invoke`, `<parameter`, `</invoke>`)를 **절대 포함하지 마십시오**. harness wire framing과 충돌:
- 모든 태그는 `concat!("<", "invoke")` / `concat!("</", "parameter>")` 형태로 빌드
- 문서 작성 시 prose로 설명
- `edit` 도구로 기존 xml.rs 수정은 안전 (직접 파일을 읽고 쓰므로)

### Config 필드 추가 규칙
`AgentLoopConfig`에 새 필드 추가 시 `Default::default()`에도 반드시 `None`/`false`/기본값 추가.

### P1.6b/c 도구 제작 순서
1. `oxicode-agent/src/tools/<name>.rs` 생성
2. `AgentTool` trait 구현
3. `oxicode-agent/src/tools.rs`에 `mod <name>;` 선언
4. `oxicode-agent/src/tools.rs`의 `with_builtins_cwd()`에 `Box::new(<name>::<Name>Tool::new(...))` 등록
5. `oxicode-agent/src/tools.rs`의 `names()` 배열에 추가

### debug_tool 재등록
`oxicode-agent/src/tools.rs`에서 주석 처리된 `all_tools.push(Box::new(debug_tool::DebugTool));`의 주석 해제 필요.
DAP 프록시 구현 후 `oxicode-agent/tests/tools.rs`의 카운트도 `25` → `26`으로 업데이트.

### 회귀 게이트
변경 후 항상 실행:
```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings
cargo fmt --all -- --check
cargo nextest run --workspace
```
