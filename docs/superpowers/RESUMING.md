# oxi omp-정렬 — 남은 작업 명세

> **최종 갱신**: 2026-07-28 (session 3)
> **브랜치**: `main`
> **완료**: P0 전항목 + P1 전항목 (P1.1~P1.6c, 12개 도구) + P3.1 + P3.2 + P4.3 + P4.4
> **기준선**: 3635 tests passing, clippy clean, fmt clean

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
| **P4.4** | **Dead config 필드 정리** — 8개 routing/fallback/circuit-breaker 필드 제거, version bump v10 | `237448a6` |

**Tool count**: 37 built-in tools registered.

---

## 2. Phase 3 — 프롬프트 & CLI 재정렬 (완료 — P3.1 + P3.2 완료, P3.3 잔여)

> **대상 크레이트**: `oxi-cli/`

### P3.3 — `main.rs` 핸들러 분리 (F-5) (~600 lines)

**현재**: `oxi-cli/src/main.rs`에 `handle_subcommand` (~90 lines match) + inline `handle_*` 함수 (~1400 LOC).

**목표**: 각 `handle_*` 함수를 `oxi-cli/src/cli/commands/*.rs`로 분리.

**위험**: clap `Subcommand`-derived enum이 sibling module에서 참조될 때 generic-bound 이슈 가능. 분리 전 각 subcommand 테스트 필요.

---

## 3. Phase 4 — oxi-original 정리 (P4.4 완료, P4.1 + P4.2 잔여)

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
| 1 | **P3.3** main.rs 핸들러 분리 | ~600 lines | 다음 권장 (clap generic-bound 위험) |
| 2 | **P4.1** Issue 시스템 격리 | ~500 lines | 독립적, 언제든 가능 |
| 3 | **P4.2** Package manager 재정렬 | ~500 lines | P4.1 후 권장 |
| 4 | **P1.6a** debug 도구 재등록 | ~600 lines | DAP proxy 구현 후 |
| 5 | **P0.5** remote-AGENT providers | ~2000 lines | 요청 시 |
| 6 | **P2** TUI 재정렬 | ~10000 lines | 마지막 |

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
- **P3.1 참고**: `HASHLINE_FORMAT_SPEC` const 제거됨 — `include_str!("../../../oxi-hashline/src/prompt.md")`가 canonical source
- **P4.4 참고**: settings v10. 구버전 settings.toml의 dead 필드는 serde가 silent ignore. Router 기능 자체는 유지 (`/router` slash command, router config)
- **P3.2 CLI**: `clap_complete` 4.6.8 추가. Cargo.lock 업데이트 필요 시 함께 커밋
