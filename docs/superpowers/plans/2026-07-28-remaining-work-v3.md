# omp-정렬 리팩토링 — 남은 구현 명세 v3

- **갱신**: 2026-07-28 (v3)
- **브랜치**: `main`
- **완료**: P0 (catalog + complexity + NamedProvider + KnownApi14 + Ollama), Step 2 (Provider::name() 제거), P1.1~P1.6c (agent loop 전항목 + 12개 도구 포팅)
- **기준선**: 3637 tests passing, clippy clean, fmt clean
- **참조**: `handoff.md`(진입점), `design.md`(마스터 설계)

---

## 완료된 작업

| Phase | 작업 | 설명 | 규모 |
|-------|------|------|:----:|
| P0 | catalog 분리 + complexity 제거 + 정체성 수정 | 모델 카탈로그 분리, -4791 lines dead code | 10 commits |
| Step 2 | Provider::name() trait 제거 | NamedProvider 래퍼 폐기 | 24 files, -174 lines |
| P1.1 | Owned dialect | Dialect enum 11종, XML renderer+parser | 24 tests |
| P1.2 | Intent tracing | AgentTool::intent(), ToolExecution 이벤트 | 7 files |
| P1.3 | Append-only context | AppendOnlyContext struct + loop wiring | 2 files + loop |
| P1.4 | Approval/tier 시스템 | ToolTier, ApprovalConfig, ApprovalHook | 4 files |
| P1.5 | Soft req + Harmony leak | SoftRequirement, remind/escalate, regex 감지 | 4 files |
| P1.6a | eval, ast_grep, ast_edit | 코드 실행 + AST 도구 3개 | 3 tool files |
| P1.6b | checkpoint, rewind, hub, yield, goal, review | 메타 도구 6개 | 5 tool files + 등록 |
| P1.6c | learn, manage_skill, inspect_image, computer, tts, vibe | 고급 도구 6개 | 6 tool files + 등록 |

---

## 남은 작업

| 순위 | 작업 | 영향 | 예상 규모 | 비고 |
|------|------|------|-----------|------|
| 1 | P3 프롬프트 & CLI | 사용자 경험 + 코드 품질 | ~2000 lines | **현재 진행중** |
| 2 | P4 oxicode-original 정리 | 코드 품질 | ~1500 lines | **현재 진행중** |
| 3 | P1.6a debug 재등록 | 도구 기능 | ~600 lines | DAP 프록시 필요 |
| 4 | P0.5 remote-AGENT | provider 3개 | ~2000 lines | 요청 시 |
| 5 | P2 TUI 재정렬 | UI | ~10000 lines | 마지막 |

---

## Phase 3 — 프롬프트 & CLI 재정렬

**대상 크레이트**: `oxicode-cli/`, `oxicode-ai/`

### P3.1 — `.md` 기반 시스템 프롬프트 [진행중]

현재 inline Rust 문자열(`oxicode-cli/src/prompt/system_prompt.rs` 736줄) → `.md` 파일 `include_str!()`으로 전환.

**omp 참조**: `/tmp/omp/packages/coding-agent/src/prompts/`

### P3.2 — CLI 명령 포팅

주요 누락 명령: `completions`, `config` (CLI 접근), `install` (MCP 서버), `update`, `commit`.

### P3.3 — `main.rs` 핸들러 분리 (F-5)

`main.rs`의 `handle_subcommand` (~90 lines) + inline `handle_*` 함수 (~1400 LOC)를 `cli/commands/*.rs`로 분리.

---

## Phase 4 — oxicode-original 처리

**대상 크레이트**: `oxicode-cli/`

### P4.1 — Issue 시스템 격리

Issue 관련 코드를 `oxicode-cli/src/store/issues/`로 이동, agent loop에서 port로 추상화.

### P4.2 — Package manager → omp 플러그인 모델

`oxicode-cli/src/storage/packages.rs`(3096 lines)를 omp `extensibility/plugins/` 모델에 맞춤.

### P4.3 — Language policy 제거 [진행중]

`Settings::output_languages`, `KNOWN_CHANNELS`, `language_directive` 제거.

### P4.4 — Dead config 필드 정리 [진행중]

`circuit_breaker_*`, `enable_routing`, `prefer_cost_efficient`, `fallback_chain` 등 제거.

---

## 변경 시 주의사항

- dialect `xml.rs`에 literal XML 태그 금지 — `concat!("<", "invoke")` 형태 사용
- `cargo clippy -p oxicode-sdk --features native-browser` 잊지 말 것
- P1.6 debug 재등록: `tools.rs`에서 주석 해제 + `tests/tools.rs` 카운트 37→38
- Config 필드 추가 시 `Default::default()`에도 기본값 추가
