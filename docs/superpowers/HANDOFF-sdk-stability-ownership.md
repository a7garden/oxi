# SDK Stability & Ownership Program — Handoff

> **Status:** R0–R8 구현 완료 (핸드오프 문서). 다음 세션은 아래 "남은 작업" 섹션부터 시작.
> **소스 요청서:** `oxios/docs/production-audit/2026-08-01-ideal-oxi-sdk-proposal.html`
> **설계:** `docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md`
> **계획:** `docs/superpowers/plans/2026-08-01-sdk-stability-ownership-program.md`
> **브랜치:** `main`
> **마지막 커밋:** `25fcd522` (fix: R4 zero-panic enforcement)
> **작성일:** 2026-08-01

---

## 1. 프로그램 상태

### 완료된 커밋 (R0–R8, 이번 세션 포함 12개)

| 커밋 | 요청 | 내용 |
|------|------|------|
| `de914ba8` | R0/R5 | ownership contract 문서 (`docs/oxi-sdk-ownership.md`) |
| `0d6d746f` | R5 | trailing newline 수정 |
| `cd3740b9` | R1/R2/R7/R8 | governance conventions (`docs/release-process.md`) |
| `2a877b03` | R1 | cargo-public-api CI gate + retrospective Breaking entry |
| `6261c3a7` | R8 | protobuf feature-gate (Devin/Cursor providers) |
| `b7da6728` | R3 | `oxi-api-stability` proc-macro crate |
| `784bd913` | R3 | proc-macro 오류 보존 + name-collision 경고 |
| `c7e9dafe` | R3 | public API surface stability tier annotations |
| `a3d0ac57` | R6/R7 | `CircuitBreaker` trait + `DefaultCircuitBreaker` + `#[non_exhaustive]` (SdkError/ProviderError) |
| `aa67cbf1` | R6/R1/R0 | `SpawnValidator` trait + R0 §6 doc correction + CHANGELOG retrospective |
| `e28e0477` | R6/R3 | oxi-sdk cfg-gated re-exports (`unstable` umbrella feature) |
| `1a0f172e` | R3/R1 | release-process.md tier/feature-gate convention + 정확한 CI gate 설명 |
| `c23aeccc` | R3 | 남은 17개 untiered re-export 블록 tier 지정 |
| `e060d1b2` | R6 | `AgentLoopConfig.circuit_breaker` + `stream_with_retry_core_with_breaker` wiring |
| `25fcd522` | R4 | deny lint 승격 + 56개 `.expect()` 사이트 분류 + 3개 `unreachable!` 제거 |

### 검증 게이트 (통과 확인됨)

```bash
cargo clippy --workspace --all-targets -- -D warnings                     # clean
cargo clippy -p oxi-sdk --features native-browser -- -D warnings           # clean
cargo nextest run --workspace --no-fail-fast   # 3050 tests: 3048 pass / 2 fail / 4 skip
cargo fmt --all -- --check                     # clean
```

> ⚠ **2개 PTY 실패는 이 프로그램과 무관** — 아래 §3 참조.

---

## 2. 다음 세션을 위한 핵심 설계 결정 (재조사 불필요)

1. **R3은 doc badge + cfg-gate 이중 구조로 scope-narrowed 됨.** proc-macro는
   `#[doc]` HTML 배지만 emit (compile signal 없음). 진짜 신호는 `#[cfg(feature = "...")]`.
   `docs/oxi-sdk-ownership.md` §6.1이 전체 근거 문서화. **proc-macro에 lint 등록 시도 금지** (불가능함).
2. **`stream_with_retry_core`는 breaking 없이 `_with_breaker` 변형 추가.** 기존 시그니처 유지,
   새 함수가 `breaker: Option<&dyn CircuitBreaker>` 받음. `BreakerError::Open`은 **non-retryable**
   (fail-fast — 재시도 소진 금지).
3. **`SpawnValidator`는 `StdioTransport::spawn`의 마지막 파라미터로 wiring** (`None` = 기존 동작).
   SDK 하드코딩 `BLOCKED_ENV_VARS` (loader-injection)는 consumer 정책과 무관하게 항상 적용.
4. **oxi-sdk의 unstable surface:** `circuit-breaker`, `mcp-spawn-validator`, `mcp-transport`
   (+ umbrella `unstable`). `oxios`는 `oxi-sdk = { features = ["unstable"] }`로 접근.

---

## 3. 남은 작업 (다음 세션 후보)

### 🔴 A. oxi-cli PTY 테스트 2건 실패 — VT TUI cutover 미완성 (blocked, 사용자 WIP와 충돌)

**진단 완료 (이번 세션).** 두 실패 모두 **oxios SDK 프로그램과 무관**하며, 사용자가 진행 중인
VT TUI cutover 작업(스태시 WIP)이 완료돼야 해결됨.

| 테스트 | 실패 | 근본 원인 |
|--------|------|-----------|
| `oxi-cli/tests/pty_e2e.rs::test_pty_tui_renders_and_exits` | `\x1b[?2026l` (tape sync end)가 5초 내 안 나옴 | TUI가 시작하지만 테이프 첫 프레임을 안 그림. 스태시의 render-loop watchdog WIP (`8d69fc37`)와 연결 |
| `oxi-cli/tests/pty_e2e.rs::test_pty_hub_opens_and_closes` | `/agents` → `Unknown command: /agents` | `/agents`가 **VT slash registry에 등록된 적이 없음** (`git log -S agents` 빈 결과). Agent Hub `OverlayRequest` 타입도 oxi-vtui에 없음 (`oxi-vtui/src/tui/core_tui/types/overlay.rs` — Modal/List/Wizard만 존재) |

**관련 stash WIP (사용자 소유, pop 금지 — 충돌 위험):**
- `stash@{0}` HubRegistry (d114da70) — `/agents` 커맨드 + Hub 오버레이 예정
- `stash@{1}` render-loop watchdog (8d69fc37)
- `stash@{3}` MiniMax-M3 model_registry (사용자 작업)

**해결 경로 (사용자 결정 필요):** VT cutover 완료 시 테스트가 자연히 통과. 이 프로그램에서
임의로 `/agents` + Hub 오버레이를 만들면 진행 중 WIP와 충돌.

### 🟡 B. cargo-public-api CI gate — observational → enforcing (R1 후속)

**현재:** `.github/workflows/api-diff.yml`이 public API 스냅샷을 capture + artifact 업로드만 함.
workflow preamble에 "does not yet ENFORCE removals against a baseline" 명시.

**할 일:** `main` baseline과 PR diff를 비교해, `## Breaking` CHANGELOG 항목 없이 심볼이
사라지면 실패시키는 enforcement 단계 추가. 설계는 이미 release-process.md §Stability Tier에 문서화됨.

### 🟡 C. oauth.rs PKCE URL parse — recoverable이지만 pub signature (R4 follow-up)

**위치:** `oxi-ai/src/oauth.rs:284` — `url::Url::parse(&config.authorization_endpoint).expect(...)`
`build_authorization_url(config) -> PkceState`가 `pub`이라 Result 반환은 breaking change.

**현재:** scoped `#[allow(clippy::expect_used)]` + SAFETY 주석으로 처리 (recoverable임을 명시).

**할 일:** additive `build_authorization_url_result(config) -> Result<PkceState, ProviderError>`
추가 → 기존 함수는 delegate → deprecation window (R2 규약) → 다음 breaking release에서 전환.

### 🟡 D. cfg-gating 확장 — 8개 `oxi_unstable` 블록에 실제 `#[cfg]` 게이트 (R3 follow-up)

**현재:** `c23aeccc`가 8개 블록에 `#[oxi_unstable(feature = ...)]` **doc badge만** 추가.
release-process.md 규약: "every `#[oxi_unstable]` MUST also carry a matching `#[cfg(feature = ...)]`."

**미완성 블록** (`oxi-sdk/src/lib.rs`):
`router`, `advisor`, `memory` (PortMemoryBackend + MemoryBackend + memory tool structs),
`subagent` (SubagentRunner), `agent-hub`, `lsp`.

**할 일:** 각 feature를 `oxi-sdk/Cargo.toml` [features]에 추가 + `#[cfg(feature = "...")]` 래핑
(e28e0477의 circuit-breaker/mcp-* 패턴 그대로). **주의:** 이 re-exports 중 일부는 oxi-cli가
기본 의존하므로 (`Agent`, `MemoryBackend` 등), 게이트 추가 전에 호출부 확인 필수 — 기본 빌드에서
심볼이 사라지면 oxi-cli 빌드가 깨짐. **이 때문에 이번 세션에서 보류.**

### 🟢 E. 3개 unreachable! 수정 후 회귀 확인 (R4 테일)

이번 세션 `25fcd522`가 수정: `mcp/mod.rs:437`, `debug_tool.rs:392`, `eval_tool.rs:142`.
다음 세션 시작 시 `cargo nextest run -p oxi-agent`로 해당 모듈 테스트 정상 확인만 하면 됨.

---

## 4. 다음 세션 시작 방법

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git log --oneline -5          # 25fcd522 확인
cargo clippy --workspace --all-targets -- -D warnings   # clean 확인
cargo nextest run -p oxi-agent --lib                    # R4 회귀 확인

# oxios 쪽 통합 확인 (선택, oxios 저장소에서):
#   oxi-sdk = { features = ["unstable"] } 로 빌드
```

**권장 우선순위:**
1. 사용자가 VT TUI cutover를 진행 중이면 그 WIP와 PTY 테스트 먼저 (Stream B)
2. 그 다음 D (cfg-gating — oxi-cli 호출부 확인 포함)
3. B (CI enforcement) / C (oauth additive Result)는 저위험, 언제든

---

## 5. 관련 파일 맵

```
docs/oxi-sdk-ownership.md              # R0 ownership contract (canonical)
docs/release-process.md                # R1/R2/R7/R8 + tier/feature-gate 규약
docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md   # 설계
docs/superpowers/plans/2026-08-01-sdk-stability-ownership-program.md          # 실행 계획
oxi-api-stability/src/lib.rs           # R3 proc-macro (#[stable]/[unstable]/[internal]/[deprecated])
oxi-ai/src/circuit_breaker.rs          # R6 CircuitBreaker trait + DefaultCircuitBreaker
oxi-agent/src/mcp/spawn.rs             # R6 SpawnValidator trait + NoopSpawnValidator
oxi-agent/src/stream_retry.rs          # R6 stream_with_retry_core[_with_breaker]
oxi-agent/src/agent_loop/config.rs     # R6 AgentLoopConfig.circuit_breaker
oxi-sdk/src/lib.rs                     # re-exports + tier annotations + cfg gates
oxi-sdk/Cargo.toml                     # features: unstable, circuit-breaker, mcp-*
oxi-ai/src/error.rs, oxi-sdk/src/error.rs  # R7 #[non_exhaustive]
```

## 6. 리스크 & 주의

| 리스크 | 상태 | 후속 |
|--------|------|------|
| D를 수행하면 oxi-cli가 기본 의존하는 심볼이 게이트 뒤로 숨음 | 미수행 (의도적) | 호출부 grep 후 feature를 default에 포함하거나 호출부 수정 |
| PTY 테스트 2건이 계속 실패 → CI nextest gate 실패 | accepted (기존 상태) | VT cutover 완료 시 해결 |
| stash WIP 3개가 사용자 작업과 얽힘 | 손대지 않음 | 사용자 결정 필요 |

---

End of handoff. 다음 세션은 §3 (남은 작업) → §4 (시작 방법) 순서로 진행.
