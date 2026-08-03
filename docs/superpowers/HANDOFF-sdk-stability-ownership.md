# SDK Stability & Ownership Program — Handoff

> **Status:** R0–R8 + 후속 A–E 전부 완료 (2026-08-02). 후속 해결 커밋: `58659744` (B/C/D), `0aa72529` (A). §3 참조.
> **소스 요청서:** `oxios/docs/production-audit/2026-08-01-ideal-oxicode-sdk-proposal.html`
> **설계:** `docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md`
> **계획:** `docs/superpowers/plans/2026-08-01-sdk-stability-ownership-program.md`
> **브랜치:** `main`
> **마지막 커밋:** `0aa72529` (feat(tui_vt): sync-output tape + /agents Agent Hub overlay + two-press quit)
> **작성일:** 2026-08-01

---

## 1. 프로그램 상태

### 완료된 커밋 (R0–R8, 이번 세션 포함 12개)

| 커밋 | 요청 | 내용 |
|------|------|------|
| `de914ba8` | R0/R5 | ownership contract 문서 (`docs/oxicode-sdk-ownership.md`) |
| `0d6d746f` | R5 | trailing newline 수정 |
| `cd3740b9` | R1/R2/R7/R8 | governance conventions (`docs/release-process.md`) |
| `2a877b03` | R1 | cargo-public-api CI gate + retrospective Breaking entry |
| `6261c3a7` | R8 | protobuf feature-gate (Devin/Cursor providers) |
| `b7da6728` | R3 | `oxicode-api-stability` proc-macro crate |
| `784bd913` | R3 | proc-macro 오류 보존 + name-collision 경고 |
| `c7e9dafe` | R3 | public API surface stability tier annotations |
| `a3d0ac57` | R6/R7 | `CircuitBreaker` trait + `DefaultCircuitBreaker` + `#[non_exhaustive]` (SdkError/ProviderError) |
| `aa67cbf1` | R6/R1/R0 | `SpawnValidator` trait + R0 §6 doc correction + CHANGELOG retrospective |
| `e28e0477` | R6/R3 | oxicode-sdk cfg-gated re-exports (`unstable` umbrella feature) |
| `1a0f172e` | R3/R1 | release-process.md tier/feature-gate convention + 정확한 CI gate 설명 |
| `c23aeccc` | R3 | 남은 17개 untiered re-export 블록 tier 지정 |
| `e060d1b2` | R6 | `AgentLoopConfig.circuit_breaker` + `stream_with_retry_core_with_breaker` wiring |
| `25fcd522` | R4 | deny lint 승격 + 56개 `.expect()` 사이트 분류 + 3개 `unreachable!` 제거 |

### 검증 게이트 (통과 확인됨)

```bash
cargo clippy --workspace --all-targets -- -D warnings                     # clean
cargo clippy -p oxicode-sdk --features native-browser -- -D warnings           # clean
cargo nextest run --workspace --no-fail-fast   # 3050 tests: 3048 pass / 2 fail / 4 skip
cargo fmt --all -- --check                     # clean
```

> ⚠ **2개 PTY 실패는 이 프로그램과 무관** — 아래 §3 참조.

---

## 2. 다음 세션을 위한 핵심 설계 결정 (재조사 불필요)

1. **R3은 doc badge + cfg-gate 이중 구조로 scope-narrowed 됨.** proc-macro는
   `#[doc]` HTML 배지만 emit (compile signal 없음). 진짜 신호는 `#[cfg(feature = "...")]`.
   `docs/oxicode-sdk-ownership.md` §6.1이 전체 근거 문서화. **proc-macro에 lint 등록 시도 금지** (불가능함).
2. **`stream_with_retry_core`는 breaking 없이 `_with_breaker` 변형 추가.** 기존 시그니처 유지,
   새 함수가 `breaker: Option<&dyn CircuitBreaker>` 받음. `BreakerError::Open`은 **non-retryable**
   (fail-fast — 재시도 소진 금지).
3. **`SpawnValidator`는 `StdioTransport::spawn`의 마지막 파라미터로 wiring** (`None` = 기존 동작).
   SDK 하드코딩 `BLOCKED_ENV_VARS` (loader-injection)는 consumer 정책과 무관하게 항상 적용.
4. **oxicode-sdk의 unstable surface:** `circuit-breaker`, `mcp-spawn-validator`, `mcp-transport`
   (+ umbrella `unstable`). `oxios`는 `oxicode-sdk = { features = ["unstable"] }`로 접근.

---

## 3. 후속 작업 — 전부 해결됨 (2026-08-02)

핸드오프 당시 "남은 작업" 후보 A–E가 다음 세션에서 모두 해결됐다.

| 항목 | 내용 | 해결 | 커밋 |
|------|------|------|------|
| 🟢 E | R4 `unreachable!` 3개(`mcp/mod.rs`, `debug_tool.rs`, `eval_tool.rs`) 회귀 확인 | `cargo nextest run -p oxicode-agent --lib` = 572/572 통과 확인 | (검증만) |
| 🟡 D | `#[oxicode_unstable]` 블록 전수(15개)에 `#[cfg(feature)]` 게이트 + 12 feature 추가; oxicode-cli는 소비 4개(`router`,`role-routing`,`role-switching`,`url-resolver`) 활성화 | 완료 | `58659744` |
| 🟡 B | api-diff.yml PR enforcement — `origin/main` 대비 심볼 제거 시 `### Breaking`/`### Removed` 없으면 fail (fail-safe: infra 실패는 warning skip) | 완료 | `58659744` |
| 🟡 C | `build_authorization_url_result` additive + `OAuthError` `#[non_exhaustive]` + `InvalidAuthorizationEndpoint` variant + legacy 함수 deprecated(0.64→0.66) | 완료 | `58659744` |
| 🔴 A | PTY 2건 = VT-TUI cutover 미구현. **스태시가 아니라 직접 구현으로 해결** — synchronized output(`\x1b[?2026l`), `/agents` Agent Hub 오버레이, 두-누르기 종료, alt-screen 제거. 스태시 4개는 무관 사용자 WIP이라 건드리지 않음 | 완료 | `0aa72529` |

**최종 검증 (2026-08-02):** `cargo nextest run --workspace` = **3051/3051 통과** (4 skip), `cargo fmt --check`·`cargo clippy --workspace --all-targets -D warnings` clean.

> ⚠ **PTY 검증 주의:** harness가 `oxicode`를 PATH로만 찾는다 (`Command::new("oxicode")`). 로컬 검증 시 `PATH=target/debug:$PATH` 후 빌드 필요. CI(ci.yml/test.yml)는 `oxicode`를 PATH에 올리지 않아 PTY 테스트가 **skip**됨 (실패 아님) — TUI 회귀는 로컬에서만 검출.

---

## 4. (참고) 검증 명령

```bash
cd /Volumes/MERCURY/PROJECTS/oxicode
git log --oneline -5          # 25fcd522 확인
cargo clippy --workspace --all-targets -- -D warnings   # clean 확인
cargo nextest run -p oxicode-agent --lib                    # R4 회귀 확인

# oxios 쪽 통합 확인 (선택, oxios 저장소에서):
#   oxicode-sdk = { features = ["unstable"] } 로 빌드
```

**권장 우선순위:**
1. 사용자가 VT TUI cutover를 진행 중이면 그 WIP와 PTY 테스트 먼저 (Stream B)
2. 그 다음 D (cfg-gating — oxicode-cli 호출부 확인 포함)
3. B (CI enforcement) / C (oauth additive Result)는 저위험, 언제든

---

## 5. 관련 파일 맵

```
docs/oxicode-sdk-ownership.md              # R0 ownership contract (canonical)
docs/release-process.md                # R1/R2/R7/R8 + tier/feature-gate 규약
docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md   # 설계
docs/superpowers/plans/2026-08-01-sdk-stability-ownership-program.md          # 실행 계획
oxicode-api-stability/src/lib.rs           # R3 proc-macro (#[stable]/[unstable]/[internal]/[deprecated])
oxicode-ai/src/circuit_breaker.rs          # R6 CircuitBreaker trait + DefaultCircuitBreaker
oxicode-agent/src/mcp/spawn.rs             # R6 SpawnValidator trait + NoopSpawnValidator
oxicode-agent/src/stream_retry.rs          # R6 stream_with_retry_core[_with_breaker]
oxicode-agent/src/agent_loop/config.rs     # R6 AgentLoopConfig.circuit_breaker
oxicode-sdk/src/lib.rs                     # re-exports + tier annotations + cfg gates
oxicode-sdk/Cargo.toml                     # features: unstable, circuit-breaker, mcp-*
oxicode-ai/src/error.rs, oxicode-sdk/src/error.rs  # R7 #[non_exhaustive]
```

## 6. 리스크 & 주의

| 리스크 | 상태 | 후속 |
|--------|------|------|
| D를 수행하면 oxicode-cli가 기본 의존하는 심볼이 게이트 뒤로 숨음 | 미수행 (의도적) | 호출부 grep 후 feature를 default에 포함하거나 호출부 수정 |
| PTY 테스트 2건이 계속 실패 → CI nextest gate 실패 | accepted (기존 상태) | VT cutover 완료 시 해결 |
| stash WIP 3개가 사용자 작업과 얽힘 | 손대지 않음 | 사용자 결정 필요 |

---

End of handoff. 프로그램(R0–R8) 및 후속(A–E) 전부 완료. §3은 해결 이력.
