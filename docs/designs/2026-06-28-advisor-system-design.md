# 설계: Advisor 시스템 — omp → oxicode 포팅

> **상태:** 설계 (구현 전)
> **소스:** omp v16.2.2 — `packages/coding-agent/src/advisor/` (6 파일, ~1,270줄 + 테스트 1,577줄)
> **라이선스:** omp는 MIT. 파일 헤더에 attribution 명시 (Mnemopi/roles 포팅과 동일 패턴).

## 0. 핵심 (TL;DR)

Advisor는 **주 에이전트를 shadowing하는 두 번째 읽기 전용 LLM 에이전트**다. 주
에이전트의 트랜스크립트를 턴 단위로 지켜보며 `advise` 도구로 `nit`/`concern`/
`blocker` 심각도의 조언을 전달한다. omp의 철학은 *"weigh, don't blindly obey"* —
동료 프로그래머로서 전략을 다듬고, 검증 부족을 지적하며, 사용자 의도에서 벗어나는
순간을 알린다.

**핵심 통찰:** oxicode는 advisor의 인프라 대부분을 **이미 갖추고 있다.**
`ModelRole::Advisor`, `AgentEvent::TurnEnd`(트리거), `AgentLoop::steer()`(steer 채널),
steering/follow-up 큐가 모두 존재한다. 따라서 신규 작업은 **런타임 로직 + 통합
배선**에 집중되며, omp의 ~1.3K줄을 Rust로 옮기는 것이 본질이다.

**범위:** 4개 Phase. Phase 1-2가 엔진 코어(독립 테스트 가능), Phase 3-4가 호스트
통합(TUI/print에서 실제 동작). MVP는 Phase 1-3로 aside/steer 채널까지 확보한다.

## 1. 배경 및 동기

### 1.1 omp advisor가 해결하는 문제

단일 에이전트 루프의 고질적 약점: (a) 성급한 "done" 선언, (b) 얕은 검증,
(c) 사용자 의도에서의 drift, (d) rabbit-hole 갇힘. omp는 이를 **동시에 실행되는
저비용 백그라운드 검토자**로 해결한다 — 주 에이전트의 추론을 방해하지 않고, 토큰이
허락하는 한 1-2개의 읽기 도구 호출로 의심을 검증한 뒤, 필요한 순간에만 개입한다.

### 1.2 왜 oxicode에 도입하는가

- **이미 설계된 역할:** `ModelRole::Advisor`가 `oxicode-ai/src/roles.rs:78`에 정의되어
  있으나 소비자가 없다 — 역할만 있고 기능이 없는 상태.
- **이미 존재하는 인프라:** `AgentEvent::TurnEnd`, `AgentLoop::steer()`,
  steering 큐가 있어 omp 대비 오히려 통합이 더 깔끔하다.
- **Mnemopi와의 시너지:** 장기 기억 + 실시간 조언은 상호 보완적이다. 어드바이저가
  과거 패턴을 참조할 수 있고, 기억 시스템이 어드바이저가 제기한 이슈를 보존할 수 있다.

### 1.3 omp에서 배운 교훈 — prose가 아닌 코드로 강제

omp #3520 버그가 설계의 나침반이다: 어드바이저 모델이 92개 고유 노트에 대해 309회
`advise`를 호출(`Stop.` 114회, `No issue; continue.` 52회 등)해 주 트랜스크립트를
`<advisory severity="blocker">Stop.</advisory>`로 뒤덮었다. 시스템 프롬프트의
"at most one advise per update / NEVER repeat"는 **prose 규칙**이라 모델이 위반했다.

**결론:** 중복 제거, 속도 제한, 의미없는 문구 억제는 **`EmissionGuard` 코드가
강제**한다. 이것이 advisor가 신뢰할 수 있는 이유다 — 모델이 잘못 행동해도 호스트가
지킨다. oxicode 포팅에서도 이 경계를 절대 prose로 약화시키지 않는다.

## 2. omp 아키텍처 — 포팅 대상 분해

```
AdvisorAgent ──prompt(delta)──> [advisor LLM + read/grep/glob + advise tool]
                                      │ advise(note, severity)
                                      ▼
                                 AdviseTool (severity-rank dedupe)
                                      │ onAdvice callback
                                      ▼
                           AdvisorEmissionGuard (최종 관문)
                                      │ accept()
                          ┌───────────┼────────────┐
                      "aside"      "steer"      "preserve"
                   (카드 렌더)   (큐 주입)    (보이는 카드, 런 無)
```

### 2.1 6개 컴포넌트

| 컴포넌트 | omp 파일 (줄) | 책임 | oxicode 목적지 |
|---|---|---|---|
| `AdviseTool` | advise-tool.ts (199) | 에이전트 도구; 심각도 랭크 중복제거; `Recorded.` 반환 | `oxicode-agent/src/tools/advise.rs` (NEW) |
| `AdvisorRuntime` | runtime.ts (508) | 델타 렌더 → 어드바이저 prompt → 재시도/epoch 가드 | `oxicode-cli/src/app/advisor/runtime.rs` (NEW) |
| `AdvisorEmissionGuard` | emission-guard.ts (172) | dedupe + 1/update 속도제한 + 문구 억제 | `oxicode-cli/src/app/advisor/emission_guard.rs` (NEW) |
| `AdvisorTranscriptRecorder` | transcript-recorder.ts (136) | `<session>/__advisor.jsonl` append | `oxicode-cli/src/app/advisor/transcript_recorder.rs` (NEW) |
| `watchdog` | watchdog.ts (109) | WATCHDOG.md 발견 + 컨텍스트 파일 주입 | `oxicode-cli/src/app/advisor/watchdog.rs` (NEW) |
| `formatAdvisorBatchContent` 등 순수 함수 | advise-tool.ts | `<advisory>` 렌더링, 채널 결정 | 동일 모듈 내 함수 |

### 2.2 전달 채널 모델 (`resolveAdvisorDeliveryChannel`)

세 가지 경로는 주 에이전트의 **상태**에 따라 결정된다:

| 채널 | 조건 | 동작 |
|---|---|---|
| **`aside`** | `nit` 항상; 또는 immune-turn 창의 `concern`/`blocker` | 트랜스크립트에 `<advisory>` 카드 렌더, **런 중단 無** |
| **`steer`** | `concern`/`blocker` + 라이브 스트리밍 중 + auto-resume 허용됨 | 주 에이전트 **steering 큐에 주입** → 다음 턴 시작 시 처리 |
| **`preserve`** | `concern`/`blocker` + 사용자 중단 후(`autoResumeSuppressed`) + idle/aborting | 보이는 카드, **런 재개 無** (다음 사용자 프롬프트 대기) |

**Immune-turn 쿨다운:** steer 직후 `advisor.immuneTurns` 턴 동안 추가
`concern`/`blocker`를 `aside`로 강등 — 조언 폭주 방지.

### 2.3 드레인 루프 (`AdvisorRuntime#drain`) — 가장 까다로운 부분

omp의 핵심 동시성 로직. `onTurnEnd()`가 트랜스크립트 델타를 `#pending`에 push하면
`#drain()`이 이를 어드바이저 `prompt()`로 보낸다. 핵심 불변량:

- **epoch 가드:** `reset()`/`dispose()`마다 `#epoch++`. drain 루프는 `await` 전후로
  epoch를 검사해 — 리셋 중이던 배치가 재시도/재큐되어 리셋 후 대화로 새는 것을 막는다.
- **컨텍스트 유지보수:** `maintainContext(incomingTokens)`가 어드바이저 컨텍스트가
  창에 근접하면 더 큰 sibling 모델로 승격; 부족하면 `true` 반환 → **재프라임**
  (컨텍스트 리셋 + 현재 트랜스크립트 전체 재생).
- **3회 연속 실패:** `notifyFailure` + 백로그 드랍 + `seenContext` 클리어.
- **실패한 턴 롤백:** `messageSnapshot` 캡처 후 실패 시 `rollbackTo`로 사용자 배치 +
  합성 assistant-error 턴 제거 (재시도가 실패 배치 위에 쌓이는 것 방지).

Rust에서는 `Promise` 체인 대신 **`tokio` 태스크 + `Arc<AtomicU64>` epoch +
`Notify`** 로 옮긴다 (§9 참조).

## 3. 설계 원칙

1. **omp 정합 (MN-style):** omp가 검증한 동작을 최대한 그대로 옮긴다. "개선"은
   omp 동작을 먼저 1:1 재현한 후에만 고려한다 (Mnemopi 포팅 결정과 동일).
2. **prose가 아닌 코드 강제:** `EmissionGuard`의 모든 규칙(속도제한/dedupe/문구
   억제)은 Rust 코드가 보장한다. 시스템 프롬프트는 보조일 뿐.
3. **기존 인프라 재사용:** `AgentLoop::steer()`, `AgentEvent::TurnEnd`,
   `SessionManager` append 패턴, 슬래시 명령 레지스트리를 새로 만들지 않는다.
4. **점진적 활성화:** `advisor.enabled` 기본값 `false`. 핵심 도구는 essential 마크
   안 함 (비활성화 가능). Phase별로 독립 테스트 가능한 단위로 쪼갠다.
5. **오프라인 안전:** 어드바이저 실패가 주 세션을 망가뜨리지 않는다 — 3회 실패 후
   자동 백로그 드랍, 모든 에러는 `tracing::debug!`로만 기록.

## 4. oxicode 현재 상태 — 의존성 매핑

### 4.1 이미 충족된 의존성 (신규 작업 無)

| omp 필요 | oxicode 상태 | 위치 |
|---|---|---|
| `advisor` 모델 역할 | ✅ 정의됨 (tag ADVISOR, Accent 색상) | `oxicode-ai/src/roles.rs:78,222` |
| turn-end 트리거 | ✅ `AgentEvent::TurnEnd` 발생 | `oxicode-agent/src/events.rs:422` |
| `steer` 전달 채널 | ✅ `AgentLoop::steer(Message)` 내장 | `agent_loop/mod.rs:175` |
| steering/follow-up 큐 | ✅ `AgentSession` 필드 | `agent_session.rs:156-157` |
| streaming 상태 | ✅ `streaming: Arc<AtomicBool>` | `agent_session.rs:171` |
| 도구 훅 | ✅ `before/after_tool_call` | `agent_loop/config.rs` |
| 슬래시 명령 | ✅ 레지스트리 | `oxicode-cli/src/tui/slash/` |
| 역할 → 모델 해석 | ✅ `role_switcher.rs`, `RoleRegistry` | `oxicode-ai/src/` |

**omp 대비 이점:** omp는 `setOnTurnEnd` 훅을 별도로 장착해야 했으나, oxicode는
`AgentEvent::TurnEnd` 이벤트를 **구독만 하면** 트리거가 된다 — 통합이 더 깔끔하다.

### 4.2 신규 작업 (MISSING)

1. `advise` AgentTool — `oxicode-agent/src/tools/advise.rs`
2. Advisor 런타임 모듈 — `oxicode-cli/src/app/advisor/` (runtime, emission_guard,
   transcript_recorder, watchdog, mod)
3. 어드바이저 `Agent` 구성 — Advisor 역할 + 읽기 전용 도구 + 시스템 프롬프트
4. `/advisor` 슬래시 명령 + 설정 플래그
5. `SessionEvent` 확장 — `aside` 채널용 변형
6. `AgentLoop` 큐 peek API (preserve 채널용) — `peek_steering_queue()`
7. 4개 프롬프트 파일 `include_str!` 임베드

## 5. Phase 1 — 핵심 도구 + 방어막 (독립 테스트 가능)

### 5.1 목표

`AdviseTool`과 `EmissionGuard`를 oxicode-agent/oxicode-cli에 추가. 이 둘은 순수 로직이므로
호스트 통합 없이 단위 테스트가 가능하다.

### 5.2 `AdviseTool` — `oxicode-agent/src/tools/advise.rs` (NEW)

omp `AdviseTool`(advise-tool.ts:154)의 Rust 역역. `AgentTool` 트레이트 구현.

```rust
//! Advisor의 `advise` 도구 — omp `advise-tool.ts` 포팅.
//!
//! Attribution: omp (oh-my-pi), MIT licensed.

use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// 어드바이저 노트 심각도. omp `AdvisorSeverity` 정합.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdvisorSeverity {
    Nit,
    Concern,
    Blocker,
}

/// `advise` 도구로 전달된 노트. `enqueue_advice` 콜백으로 호스트에 전달.
#[derive(Debug, Clone)]
pub struct AdvisorNote {
    pub note: String,
    pub severity: Option<AdvisorSeverity>,
}

/// `formatAdvisorBatchContent`용 텍스트 — omp 정합:
///   <advisory severity="blocker" guidance="weigh, don't blindly obey">...</advisory>
pub const ADVISOR_GUIDANCE: &str = "weigh, don't blindly obey";

/// 어드바이저 노트를 호스트에 전달하는 콜백.
/// `Option<AdvisorSeverity>`를 받아 accept/reject은 호스트 `EmissionGuard`가 결정.
pub type EnqueueAdviceFn = Arc<dyn Fn(AdvisorNote) + Send + Sync>;

pub struct AdviseTool {
    enqueue: EnqueueAdviceFn,
    /// 심각도 랭크 기반 중복제거: nit=1 < concern=2 < blocker=3.
    /// 새 호출이 이전 랭크를 *초과*할 때만 통과 (에스컬레이션만 허용).
    delivered: Mutex<std::collections::HashMap<String, u8>>,
}

impl AdviseTool {
    pub fn new(enqueue: EnqueueAdviceFn) -> Self {
        Self { enqueue, delivered: Mutex::new(Default::default()) },
    }
    /// 어드바이저가 새 대화를 시작할 때 호출 (omp `resetDeliveredNotes`).
    pub fn reset_delivered(&self) {
        self.delivered.lock().clear();
    }
}
```

`execute` 동작 (omp 정합):
1. `advisor_note_dedupe_key(note)` = trim + 공백 정규화.
2. `rank = severity_rank(severity)` (nit=1, concern=2, blocker=3).
3. `prev = delivered.get(key).unwrap_or(0)`.
4. `rank <= prev` → `Duplicate advice ignored.` 반환 (`useless: true`).
5. 아니면 `delivered.set(key, rank)`, `enqueue(note)` 호출, `Recorded.` 반환.

**스키마:** `{ note: String (non-empty), severity?: "nit"|"concern"|"blocker" }`.
`essential()` = `false` (비활성화 가능).

### 5.3 `AdvisorEmissionGuard` — `oxicode-cli/src/app/advisor/emission_guard.rs`

omp(emission-guard.ts:116) 정합. `AdviseTool`의 **2차 방어선**이자 호스트 소유의
최종 관문.

```rust
/// 어드바이저 `advise()` 호출의 세션별 게이트. omp #3520 폭주 방어.
///
/// 모델이 시스템 프롬프트 규칙을 위반해도 코드가 지킨다:
/// - 업데이트당 최대 1개 advise (`begin_update`로 매 프롬프트 사이클마다 리셋)
/// - 정규화된 노트의 FIFO 히스토리 dedupe (용량 4096)
/// - 의미없는 자기대화 문구 억제 (normalize → SUPPRESSED_PHRASES 조회)
pub struct AdvisorEmissionGuard {
    seen: std::collections::HashSet<String>,
    seen_order: std::collections::VecDeque<String>,
    consumed_this_update: bool,
    capacity: usize,
}
```

`accept(&mut self, note: &str) -> bool`: omp 정합 — 정규화 → 빈/억제문구/이미본/이번업데이트소진 시 `false`, 아니면 기록 후 `true`.

**`SUPPRESSED_NORMALIZED_PHRASES`** 테이블은 omp의 40+ 항목을 그대로 포팅 (`stop`,
`done`, `no issue continue`, `continue`, `ok`, `good`, `fine` 등). 이 테이블은
omp 이슈 #3520의 실제 폭주 데이터에서 추출되었으므로 **값을 임의 수정하지 않는다**.

### 5.4 순수 함수 (동일 모듈)

```rust
/// 심각도가 인터럽트를 유발하는가 (concern | blocker). omp `isInterruptingSeverity`.
pub fn is_interrupting_severity(s: Option<AdvisorSeverity>) -> bool { ... }

/// 인터럽트 후 immune-turn 창 활성 여부. 반개구간 [start, start+turns).
pub fn is_immune_turn_active(completed: u64, start: Option<u64>, turns: u64) -> bool { ... }

/// 채널 결정 — omp `resolveAdvisorDeliveryChannel` 정합.
pub fn resolve_delivery_channel(opts: DeliveryOpts) -> AdvisorDeliveryChannel { ... }
```

### 5.5 파일 변경 요약 (Phase 1)

| 파일 | 변경 | 줄 추정 |
|---|---|---|
| `oxicode-agent/src/tools/advise.rs` | NEW | ~140 |
| `oxicode-agent/src/tools.rs` | 모듈 선언 + `with_builtins_cwd` 등록 (비essential) | +5 |
| `oxicode-cli/src/app/advisor/mod.rs` | NEW (모듈 루트) | ~20 |
| `oxicode-cli/src/app/advisor/emission_guard.rs` | NEW | ~130 |
| `oxicode-cli/src/app/advisor/channels.rs` | NEW (순수 함수 + SUPPRESSED 테이블) | ~110 |

### 5.6 테스트 (Phase 1)

```rust
#[test] fn advise_dedupes_lower_or_equal_severity()      { /* nit→nit 거부, nit→concern 통과 */ }
#[test] fn advise_normalizes_whitespace_in_dedupe_key()  { /* "a  b" == "a b" */ }
#[test] fn emission_guard_one_per_update()                { /* begin_update 후 첫 호출만 accept */ }
#[test] fn emission_guard_suppresses_content_free_phrases(){ /* "Stop." → false */ }
#[test] fn emission_guard_dedupes_across_updates()        { /* 같은 노트 두 번째 update에서 거부 */ }
#[test] fn emission_guard_fifo_evicts_at_capacity()       { /* 4097번째 → 가장 오래된 것 evict, 재등장 시 accept */ }
#[test] fn delivery_channel_nit_always_aside()            { /* nit → aside */ }
#[test] fn delivery_channel_concern_live_steers()         { /* streaming+concern → steer */ }
#[test] fn delivery_channel_post_interrupt_preserves()    { /* autoResumeSuppressed+idle → preserve */ }
#[test] fn delivery_channel_immune_downgrades()           { /* immune 창 + concern → aside */ }
```

## 6. Phase 2 — AdvisorRuntime + 드레인 루프

### 6.1 목표

omp `AdvisorRuntime`(runtime.ts:67)의 Rust 역역 — 어드바이저 에이전트를 구동하는
비동기 드레인 루프. 호스트 없이 `AdvisorAgent` 트레이트 페이크로 테스트 가능.

### 6.2 타입 설계

```rust
/// omp `AdvisorAgent` 인터페이스의 Rust 역역.
/// `oxicode_agent::Agent`가 만족하며, 테스트는 hand-rolled 페이크 사용.
#[async_trait]
pub trait AdvisorAgent: Send + Sync {
    async fn prompt(&self, input: String) -> Result<(), String>;
    fn abort(&self, reason: &str);
    fn reset(&self);
    /// 실패한 프롬프트 후 커서 이후 메시지 드랍. Agent에 추가 필요.
    async fn rollback_to(&self, count: usize);
    fn message_count(&self) -> usize;
    fn last_error(&self) -> Option<String>;
}

/// omp `AdvisorRuntimeHost` 역역 — 호스트가 제공하는 콜백 묶음.
pub trait AdvisorRuntimeHost: Send + Sync {
    /// 주 트랜스크립트 스냅샷.
    fn snapshot_messages(&self) -> Vec<AdvisorViewMessage>;
    /// accept된 노트를 주 에이전트로 라우팅 (채널은 호스트가 결정).
    fn enqueue_advice(&self, note: AdvisorNote);
    /// 어드바이저 컨텍스트 유지보수 — 승격/재프라임 필요 시 true.
    async fn maintain_context(&self, incoming_tokens: u32) -> bool { false }
    /// 매 프롬프트 사이클 전 호출 — `EmissionGuard.begin_update()`.
    fn begin_advisor_update(&self) {}
    /// 3회 연속 실패 시 UI 알림.
    fn notify_failure(&self, err: &str) {}
}

pub struct AdvisorRuntime {
    agent: Arc<dyn AdvisorAgent>,
    host: Arc<dyn AdvisorRuntimeHost>,
    last_count: AtomicU64,
    epoch: Arc<AtomicU64>,
    state: Mutex<DrainState>, // pending + draining 동시화 (§9.2)
    backlog: AtomicU64,
    consecutive_failures: AtomicU32,
    failure_notified: AtomicBool,
    seen_context: Mutex<HashMap<String, String>>,
    waiters: Mutex<Vec<CatchupWaiter>>,
    retry_delay: Duration,
    disposed: AtomicBool,
}
```

### 6.3 핵심 메서드 (omp 정합)

- `on_turn_end(&self, messages)` — 델타 렌더 → `pending` push → `backlog++` →
  `notify_waiters()` → `tokio::spawn(drain)`.
- `wait_for_catchup(max_ms, threshold, signal)` — `backlog < threshold`까지 대기.
  **등록 경쟁 주의 (§9.2 catchup 변형):** `backlog` 검사와 waiter 등록을 동일
  `waiters` 잠금 안에서 수행해야 drain 의 `notify_waiters` 와 원자적이다.
  그렇지 않으면 drain 이 백로그를 먼저 비우고 빈 waiters 에 notify 한 뒤
  등록된 waiter 가 타임아웃(30s)까지 잠잔다. 타임아웃/취소는 `tokio::select!`
  (oneshot 수신 / `sleep(max)` / cancel signal).
- `reset()` / `seed_to(count)` / `dispose()` — epoch 증가 + 컨텍스트 클리어.
- `render_delta()` — `last_count` 이후 메시지, advisor 자신의 custom 메시지 필터,
  재주입된 plan/goal 컨텍스트는 byte-동일 시 `(unchanged — still in effect)`로 축약.

### 6.4 드레인 루프 — Rust 변환의 핵심

omp의 `async #drain()`을 `tokio` 태스크로 옮긴다. 불변량 보존:

```rust
/// `pending` + `draining` 을 하나의 Mutex 로 묶어 check-and-release 를 원자화.
/// (omp 의 `#busy` + `#pending` 분리는 JS 싱글스레드에서만 안전 — §9.2 참조.)
struct DrainState { pending: Vec<PendingDelta>, draining: bool }

fn on_turn_end(&self, messages: Vec<AdvisorViewMessage>) {
    if let Some(render) = self.render_delta(&messages) {
        let spawn = {
            let mut s = self.state.lock();
            s.pending.push(PendingDelta { text: render, turns: 1 });
            self.backlog.fetch_add(1, Ordering::SeqCst);
            !s.draining                       // 드레인 중이 아니면 spawn
        };
        if spawn { tokio::spawn(self.clone().drain()); }
        self.notify_waiters();
    }
}

async fn drain(self: Arc<Self>) {
    {   // "드레이너" 역할 획득 — 잠금 안에서 검사+설정 (CAS 대체).
        let mut s = self.state.lock();
        if s.draining || s.pending.is_empty() { return; }
        s.draining = true;
    }
    loop {
        let batch: Vec<_> = {
            let mut s = self.state.lock();
            if s.pending.is_empty() {
                s.draining = false;            // 역할 해제 + 빈 검사 = 동일 임계구역.
                return;                         // 잃어버린 웨이크업 無 (§9.2).
            }
            s.pending.drain(..).collect()
        };
        let epoch_start = self.epoch.load(Ordering::SeqCst);
        // ... maintain_context, epoch 검사, prompt, 실패 시 retry/unshift + sleep
        if self.epoch.load(Ordering::SeqCst) != epoch_start { continue; } // 리셋 무효화
        // ... 성공 시 backlog 감소, notify_waiters
    }
}
```

**omp와의 차이 (Rust 필연):**
- `Bun.sleep(ms)` → `tokio::time::sleep(Duration)`.
- `Promise.withResolvers` → `tokio::sync::oneshot` 또는 `Notify`.
- `#busy` JS 플래그 → `Mutex<DrainState>` 안의 `draining` 필드. `AtomicBool` 로는 check-then-release 경쟁(잃어버린 웨이크업)이 생긴다 — §9.2 참조.
- `#epoch` → `Arc<AtomicU64>` (드레인 태스크가 소유하는 snapshot과 비교).
- 재시도 시 `pending.unshift(batch)` → `pending.lock().insert(0, batch)`.

### 6.5 파일 변경 요약 (Phase 2)

| 파일 | 변경 | 줄 추정 |
|---|---|---|
| `oxicode-cli/src/app/advisor/runtime.rs` | NEW | ~320 |
| `oxicode-cli/src/app/advisor/mod.rs` | export 추가 | +3 |
| `oxicode-cli/src/app/advisor/message_view.rs` | NEW (델타 렌더, `AdvisorViewMessage`) | ~90 |

### 6.6 테스트 (Phase 2)

```rust
#[tokio::test] async fn drain_prompts_advisor_with_delta()         { /* on_turn_end → prompt 1회, 입력은 델타 */ }
#[tokio::test] async fn reset_aborts_inflight_and_drops_batch()    { /* drain 중 reset → epoch 불일치 → 배치 드랍 */ }
#[tokio::test] async fn three_failures_notify_and_drop_backlog()   { /* 3회 에러 → notify_failure 1회 + backlog 0 */ }
#[tokio::test] async fn rollback_on_failure_prevents_replay()      { /* 실패 후 rollback_to 호출 검증 */ }
#[tokio::test] async fn maintain_context_reprime_replays_full()     { /* maintain_context=true → 전체 재생 */ }
#[tokio::test] async fn dedup_context_collapses_unchanged_plan()    { /* 동일 plan 재주입 → "(unchanged)" 마커 */ }
#[tokio::test] async fn wait_for_catchup_resolves_below_threshold() { /* backlog < thr → 즉시 해결 */ }
#[tokio::test] async fn drain_exit_racing_turn_end_no_lost_wakeup() { /* on_turn_end 가 drain 종료 직후(빈 검사→busy 해제 창)에 다른 워커에서 push+spawn → 대기 없이 재드레인, pending 이 잔류/정지하지 않음 */ }
```

## 7. Phase 3 — 호스트 통합 (AgentSession + 전달 채널)

### 7.1 목표

`AgentSession`에 어드바이저를 장착: `TurnEnd` 구독, 어드바이저 `Agent` 구성,
3개 전달 채널 배선, `/advisor` 토글. 이 Phase 완료 시 TUI에서 실제 동작.

### 7.2 어드바이저 에이전트 구성

`AgentSession::build_advisor()` (omp `#buildAdvisorRuntime` 정합):

```rust
fn build_advisor(&self) -> Option<AdvisorHandle> {
    if !self.advisor_enabled.load(SeqCst) { return None; }
    // 1. Advisor 역할 모델 해석 — RoleRegistry + role_switcher로 Model 확정
    let model = self.resolve_role_model(ModelRole::Advisor)?;
    // 2. 읽기 전용 도구 서브셋: read/grep/find (oxicode 이름; omp는 glob)
    let tools = self.readonly_tools_subset(&["read", "grep", "find"]);
    // 3. 시스템 프롬프트 조립: system.md + context-files + watchdog?
    let system = self.build_advisor_system_prompt();
    // 4. advise 도구 + emission_guard 연결
    let advise = Arc::new(AdviseTool::new(self.advisor_enqueue.clone()));
    // 5. Agent 생성 (독립 SharedState, 자체 compaction)
    let agent = Agent::new(provider, advisor_config, tools, state);
    // 6. AdvisorRuntime + TranscriptRecorder 장착
    Some(AdvisorHandle { agent, runtime, advise, recorder, guard })
}
```

**역할 → 모델 해석:** `role_switcher.rs` + `RoleRegistry`가 이미 있으므로, 미설정 시
omp처럼 `slow` 체인으로 폴백한다 (`inherits_default`에 "advisor" 추가 — 현재는
`smol|slow|designist`만).

### 7.3 TurnEnd 트리거 — 이벤트 구독

oxicode는 omp의 `setOnTurnEnd` 훅 대신 **`AgentEvent::TurnEnd` 이벤트 스트림을 구독**한다.
`AgentSession`의 기존 이벤트 리스너 루프에 분기 추가:

```rust
// agent_session.rs 이벤트 처리 루프 내
match &event {
    AgentEvent::TurnEnd { messages, .. } => {
        self.advisor_primary_turns_completed.fetch_add(1, SeqCst);
        if let Some(rt) = &*self.advisor_runtime.read() {
            rt.on_turn_end(messages.clone());
            if self.settings.advisor.sync_backlog != "off" {
                let thr = self.settings.advisor.sync_backlog.parse().unwrap_or(0);
                let _ = rt.wait_for_catchup(Duration::from_millis(30_000), thr, cancel).await;
            }
        }
    }
    _ => {}
}
```

### 7.4 전달 채널 배선

`enqueue_advice` 콜백(호스트 소유)이 `EmissionGuard.accept()` 통과한 노트를
`resolve_delivery_channel`로 라우팅:

| 채널 | oxicode 구현 |
|---|---|
| `aside` | `SessionEvent::Advisor(AdvisorNote)` 발행 → TUI 렌더러가 `<advisory>` 카드 표시. 트랜스크립트에 custom 메시지로 persist. |
| `steer` | `agent_loop.steer(Message::User(...))` 호출 — **이미 존재**. 라이브 스트림 중이면 다음 턴에 처리. |
| `preserve` | idle/aborting 시 `SessionEvent::Advisor` 발행만 하고 `steer`는 호출 안 함. |

**`AgentLoop` 큐 peek API 추가** (preserve용 — omp `peekSteeringQueue`):

```rust
impl AgentLoop {
    /// steering 큐의 스냅샷 (preserve 채널이 어드바이저 카드 추출용).
    pub fn peek_steering_queue(&self) -> Vec<Message> { self.steering_queue.read().clone() }
    pub fn replace_queues(&self, steer: Vec<Message>, follow: Vec<Message>) { ... }
}
```

### 7.5 `/advisor` 슬래시 명령

omp(builtin-registry.ts:439) 정합 — `tui/slash/builtin/`에 추가:

| 인자 | 동작 |
|---|---|
| `on` / `off` | `set_advisor_enabled(bool)` → 런타임 시작/정지 |
| `toggle` | 상태 토글 |
| `status` | enabled, backlog, primary_turns, recent failures 출력 |
| `dump` | 어드바이저 트랜스크립트(`__advisor.jsonl`) 직렬화 출력 |

### 7.6 설정 스키마 — `store/settings.rs`

```rust
pub struct AdvisorSettings {
    pub enabled: bool,           // default: false
    pub sync_backlog: String,    // "off" | 임계값 숫자문자열, default: "off"
    pub immune_turns: u64,       // default: 0
}
```

`Settings`에 `advisor: AdvisorSettings` 필드 추가. `settings.toml`의
`[advisor]` 섹션에서 읽음.

### 7.7 파일 변경 요약 (Phase 3)

| 파일 | 변경 | 줄 추정 |
|---|---|---|
| `oxicode-cli/src/app/agent_session.rs` | 어드바이저 필드 + build_advisor + TurnEnd 분기 + enqueue 콜백 | +180 |
| `oxicode-cli/src/app/agent_session.rs` | `SessionEvent::Advisor` 변형 추가 | +8 |
| `oxicode-agent/src/agent_loop/mod.rs` | `peek_steering_queue` / `replace_queues` | +25 |
| `oxicode-cli/src/tui/slash/builtin/advisor_command.rs` | NEW (`/advisor`) | ~90 |
| `oxicode-cli/src/store/settings.rs` | `AdvisorSettings` | +35 |
| `oxicode-ai/src/roles.rs` | `inherits_default`에 "advisor" 추가 (slow 폴백) | +1 |

## 8. Phase 4 — 트랜스크립트 레코더 + WATCHDOG + 프롬프트

### 8.1 `AdvisorTranscriptRecorder` — `transcript_recorder.rs`

omp(transcript-recorder.ts:38) 정합. 어드바이저 턴을 `<session>/__advisor.jsonl`에
append. 핵심: 파일 경로를 **세션 파일** 기반으로 동기 해결 (artifacts dir 아님 —
서브에이전트 충돌 방지).

oxicode의 `SessionManager`가 이미 append-only JSONL 패턴을 가지므로 재사용:

```rust
pub struct AdvisorTranscriptRecorder {
    resolve_session_file: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    resolve_cwd: Box<dyn Fn() -> String + Send + Sync>,
    queue: Mutex<()>, // 직렬화 — 순서 보장
    manager: Mutex<Option<SessionManager>>,
}
```

`record(&self, msg)`는 omp처럼 큐에 append 작업을 예약; `flush()`/`close()`로 배리어.
세션 전환(`/new`/resume) 시 이전 writer close 후 새 파일 오픈.

### 8.2 WATCHDOG 발견 — `watchdog.rs`

omp(watchdog.ts) 정합:
- `discover_watchdog_files(cwd, agent_dir)` — cwd→repoRoot→`~/.oxicode/` 워크업하며
  `WATCHDOG.md` 발견. user-level 우선, project-level은 depth 역순.
- `format_advisor_context_prompt(context_files)` — `AGENTS.md` 등을
  `<project-context>` 블록으로 렌더 (oxicode의 기존 컨텍스트 파일 발견 재사용).
- `format_active_repo_watchdog_prompt(repo_ctx)` — cwd가 git 밖이고 자식 repo 1개일 때.

### 8.3 프롬프트 임베드

4개 파일을 omp에서 복사해 `oxicode-cli/src/app/advisor/prompts/`에 두고 `include_str!`:

| 파일 | 변수 |
|---|---|
| `system.md` (~550단어) | 없음 |
| `advise-tool.md` | 없음 |
| `active-repo-watchdog.md` | `{{relativeRepoRoot}}` |
| `context-files.md` | `{{#each contextFiles}}` |

템플릿 렌더링은 omp의 Handlebars 대신 단순 문자열 치환(oxicode에 Handlebars 의존성
추가 회피 — 변수 패턴이 단순하므로 `replace`로 충분).

### 8.4 파일 변경 요약 (Phase 4)

| 파일 | 변경 | 줄 추정 |
|---|---|---|
| `oxicode-cli/src/app/advisor/transcript_recorder.rs` | NEW | ~110 |
| `oxicode-cli/src/app/advisor/watchdog.rs` | NEW | ~120 |
| `oxicode-cli/src/app/advisor/prompts/*.md` | NEW (4개, omp에서 복사) | — |
| `oxicode-cli/src/app/advisor/mod.rs` | export | +5 |

## 9. 동시성 모델 (Rust 특화)

omp는 JS 싱글스레드 + `Promise` 체인이지만, oxicode는 멀티스레드 `tokio`다. 핵심 안전 장치:

### 9.1 epoch 가드 (드레인 무결성)

```
reset()/dispose()  ──>  epoch.fetch_add(1, SeqCst)
drain 태스크       ──>  시작 시 epoch_snapshot = epoch.load()
                        await (maintain_context / prompt) 후:
                        if epoch.load() != epoch_snapshot { continue; } // 배치 폐기
```

`reset` 중이던 배치가 재큐되어 리셋 후 대화로 새는 것을 막는다 — omp의 핵심 불변량.

### 9.2 잃어버린 웨이크업 방지 (lost-wakeup race) — 핵심

omp의 `#drain()`이 안전한 것은 **JS 이벤트 루프가 동기 세그먼트를 직렬화**하기
때문이다: `while (#pending.length)` 가 실패하고 `finally` 의 `#busy = false` 가
실행되는 사이에는 다른 코드가 끼어들 수 없다 (오직 `await` 지점만 양보).

`tokio` 멀티스레드 런타임에서는 이 가정이 깨진다. 순진한 변환은 경쟁을 만든다:

```rust
    if self.pending.lock().is_empty() { break; }   // (A) 잠금 해제 후
    //   ↑↓ 다른 워커가 on_turn_end 로 push + spawn(drain) 할 수 있는 창
self.busy.store(false, Ordering::SeqCst);           // (B) busy 클리어
```

(A) 와 (B) 사이에 `on_turn_end` 가 다른 스레드에서 push + `spawn(drain)` 하면,
스폰된 드레인은 `busy==true` 를 보고 즉시 반환하고, 이어 (B) 가 busy 를 지운다.
결과: `pending` 은 비어있지 않은데 드레인은 아무도 안 돈다 — **잃어버린
웨이크업, 간헐적 "advisor가 조용히 멈춤" 하이젠버그 버그.**

**해결:** `pending` 과 `draining` 플래그를 **하나의 `Mutex<DrainState>`** 로
묶어, "그만둘지 결정"과 "역할 해제"를 하나의 원자적 임계구역으로 만든다
(§6.4 스케치). JS 가 공짜로 주던 불변량을 잠금으로 재구성하는 것이다. 핵심
불변량: `on_turn_end` 의 "push + `draining` 검사 + spawn" 도 **동일 잠금** 안에서
일어나므로, 두 임계구역의 선후관계가 무엇이든 일관적이다.

> `tokio::sync::Notify` 의 permit 의미론으로도 해결 가능하나, 단일
> `Mutex<DrainState>` 가 가장 단순하고 omp 불변량에 가장 가깝다. 회귀 테스트
> `drain_exit_racing_turn_end_no_lost_wakeup` (§6.6) 가 이 창을 잡는다.

**같은 부류의 경쟁 — catchup waiter 등록.** `wait_for_catchup` 도 lost-wakeup
패턴에 취약하다: `backlog >= threshold` 검사 후 waiter 를 `waiters` 에 push
하기 전에 drain 이 `backlog` 를 감소시키고 `notify_waiters` (빈 waiters) 를
호출하면, 막 push 된 waiter 는 타임아웃(30s)까지 잠잔다. 해결은 동일 패턴 —
backlog 검사 + waiter push 를 **동일 `waiters` 잠금** 안에서, `notify_waiters`
도 같은 잠금으로 각 waiter 임계값을 검사·해결. 두 임계구역이 같은 잠금으로
직렬화되므로 누락 無. (드레인 정지와 달리 타임아웃이 있어 영구 정지까진
아니지만, 최대 30s 지연 버그로 충분히 실격이다.)

### 9.3 락 규율 (AGENTS.md 준수)

- `parking_lot::Mutex` 사용, 가드를 `.await` 넘기지 않음 (`!Send`).
- 어드바이저 `Agent`는 자체 `SharedState` — 주 에이전트 상태와 격리.
- `enqueue_advice` 콜백은 `Arc<dyn Fn>` — 잠금 없이 호스트 큐에 push.

### 9.4 백프레셔

- 어드바이저가 주 에이전트보다 느리면 `backlog` 증가. `wait_for_catchup`이 임계값
  도달 시 주 루프를 잠시 멈춰 어드바이저가 따라잡게 함 (omp `syncBacklog` 정합).
- `pending` 무한 증가 방지: omp는 명시적 상한이 없으나, oxicode에서는 `MAX_PENDING`
  (예: 64) 도입 — 초과 시 가장 오래된 델타 병합 (드랍 아님).

## 10. 의존성 & 호환성

### 10.1 크레이트 의존 흐름

```
oxicode-ai (roles.rs: Advisor)  ── 이미 존재
oxicode-agent (tools/advise.rs) ── Phase 1 추가
oxicode-cli (app/advisor/)      ── Phase 2-4 추가
```

새 외부 의존성 **없음** — `tokio`, `parking_lot`, `serde`, `tracing` 모두 기존.
Handlebars 도입 회피 (§8.3).

### 10.2 호환성

- `advisor.enabled = false`(기본값)일 때 어드바이저 코드 경로 완전 우회 — 제로 오버헤드.
- `ModelRole::Advisor`에 모델 미할당 시 `build_advisor`가 `None` 반환 — 조용히 비활성.
- 기존 세션 resume 시 `__advisor.jsonl`이 없어도 정상 (선택적 아티팩트).

## 11. 리스크 & 트레이드오프

### 11.1 🟧 드레인 루프의 Rust 변환 정확성

omp의 `Promise` 체인을 `tokio`로 옮길 때 epoch 가드와 실패 롤백의 미묘한 순서가
깨질 수 있다. **완화:** Phase 2에서 omp `advisor.test.ts`(1,430줄)의 핵심 시나리오를
Rust로 1:1 포팅해 회귀 방어. 특히 "reset 중 배치 폐기"와 "3회 실패 후 백로그 드랍".

### 11.2 🟧 `Agent::rollback_to` 신규 API

omp는 `Agent.#runLoop`가 실패 시 합성 assistant-error 턴을 추가하므로 어드바이저가
`rollbackTo`로 제거한다. oxicode의 `Agent`는 이 패턴이 있는지 확인 필요 — 없다면
`AdvisorAgent` 트레이트의 `rollback_to`를 no-op으로 둬도록(최악엔 stale 턴 잔류)
기능은 동작하지만 컨텍스트 품질이 떨어진다. Phase 2에서 `Agent` 내부 상태 접근
가능성 먼저 조사.

### 11.3 🟨 aside 채널의 트랜스크립트 persist

`SessionEvent::Advisor`를 트랜스크립트에 영속화하려면 `SessionEntry` 변형이 필요할
수 있다. omp는 `CustomMessageEntry`를 쓴다. oxicode의 세션 스키마 확장 여부는 Phase 3에서
결정 — 임시로 런타임 전용(비persist)으로 시작하고, 사용자 반응 보고 영속화 결정.

### 11.4 🟨 도구 이름 매핑

omp는 `glob`, oxicode는 `find`. `ADVISOR_READONLY_TOOL_NAMES`은 oxicode 이름(`read`, `grep`,
`find`)로. 시스템 프롬프트의 `glob` 언급은 `find`로 수정 — 동작 동일.

### 11.5 🟩 비활성 시 제로 영향

`advisor.enabled = false`면 `build_advisor`가 `None`, TurnEnd 분기 스킵. 기존 동작
변경 없음. 이것이 가장 큰 안전망이다.

## 12. 테스트 계획 (통합)

Phase별 단위 테스트(§5.6, §6.6) 외에 통합 검증:

```rust
#[tokio::test] async fn advisor_steers_on_concern_during_stream() {
    // 주 에이전트가 잘못된 방향 → 어드바이저 concern → steer 큐 주입 → 주 에이전트 방향 전환 검증
}

#[tokio::test] async fn advisor_aside_does_not_interrupt() {
    // nit → 주 런 계속, 트랜스크립트에 카드만 추가
}

#[tokio::test] async fn emission_guard_stops_flood_after_disable() {
    // /advisor off → 진행 중 drain 중단, 큐 비움, 이벤트 누수 없음
}

#[tokio::test] async fn transcript_recorder_appends_to_advisor_jsonl() {
    // 어드바이저 턴 → <session>/__advisor.jsonl에 append, 주 세션 파일은 미변경
}

#[tokio::test] async fn reset_on_new_clears_interrupt_latches() {
    // steer 후 immune 창 활성 → /new → 창 리셋, 대기 카드 드랍
}
```

**수동 E2E (TUI):** 위험한 방향의 작업(예: 잘못된 파일 수정)을 주 에이전트에게
시키고 어드바이저가 `blocker`로 중단하는지, `/advisor status`가 올바른 통계를
보여주는지 확인.

## 13. 체크리스트 (구현 완료 기준)

- [ ] Phase 1: `advise.rs` + `emission_guard.rs` + 순수 함수, 단위 테스트 10개 통과
- [ ] Phase 2: `runtime.rs` 드레인 루프, omp 시나리오 7개 포팅 테스트 통과
- [ ] Phase 3: `/advisor` 명령 + 3개 전달 채널 + TurnEnd 구독, 통합 테스트 통과
- [ ] Phase 4: `__advisor.jsonl` 레코더 + WATCHDOG + 4개 프롬프트 임베드
- [ ] `cargo fmt` + `cargo clippy --workspace -- -D warnings` 클린
- [ ] `cargo nextest run --workspace` 통과
- [ ] `advisor.enabled = false` 시 기존 동작 100% 보존 (회귀 없음)
- [ ] 파일 헤더에 omp MIT attribution 명시 (roles.rs/Mnemopi 패턴)

## 14. 롤아웃 순서

```
Phase 1 (엔진 순수 로직)  ──>  Phase 2 (런타임 + 드레인)
        │                              │
        └──── 독립 단위 테스트 ──────────┘
                                       │
                                       ▼
                          Phase 3 (호스트 통합: TurnEnd + 채널 + /advisor)
                                       │
                                       ▼
                          Phase 4 (레코더 + WATCHDOG + 프롬프트)
                                       │
                                       ▼
                          통합 테스트 + clippy + 수동 E2E
```

Phase 1-2는 `oxicode-cli/src/app/advisor/`에 격리되어 호스트 없이 완성·검증 가능하다.
Phase 3가 처음으로 `AgentSession`을 건드리므로, Phase 1-2 머지 후 Phase 3를 별도
PR로 분리해 회귀 위험을 국소화한다.
