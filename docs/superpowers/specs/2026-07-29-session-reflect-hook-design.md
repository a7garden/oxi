# P1.5: session_reflect() Lifecycle Hook — Design

> **Tier 1 — Architectural design.** 대상: AgentSessionRuntime dispose path + MemoryBackend.
> 선행: REMAINING.md §P1.5.

## Problem

`services.rs:387`에 `session_reflect()` 함수가 완전히 구현되어 있지만, **어디서도 호출되지 않는다**. 세션이 종료될 때 자동으로 memory backend에 요약을 저장하는 훅이 없다. omp에서는 세션 종료 시 mental-models가 자동으로 요약을 생성하고 memory에 저장한다.

현재 memory pipeline은 60초마다 JSONL 파일을 스캔하여 Stage 1/2를 실행하지만, 세션의 마지막 메시지가 누락될 수 있고 실시간성이 부족하다.

## Current Teardown Path

```
TUI quit / Ctrl+C
  → AgentSessionRuntime::dispose()     [runtime.rs:740]
    → teardown_current(Quit)           [runtime.rs:749]
      → self.session.reset()           [AgentSessionHandle::reset()]
```

`reset()`은 세션 상태를 초기화하지만 memory에 저장하는 로직은 없다.

## Constraints

1. **Non-blocking** — memory 저장이 세션 종료를 지연시키면 안 됨. fire-and-forget with `tokio::spawn`
2. **Memory disabled면 skip** — `memory_enabled == false` 또는 `settings.memory_backend == None`이면 아무것도 안 함
3. **요약의 질** — 전체 메시지를 저장하지 않고, compact된 context 또는 마지막 N 메시지만 요약
4. **중복 저장 방지** — 같은 session_id로 여러 번 저장되지 않도록 idempotent key 사용

## Design

### Hook Point: teardown_current()

`teardown_current()`가 불리는 모든 경로에서 memory reflect 실행:

- `dispose()` — TUI 종료 (가장 중요)
- `switch_session()` (if exists) — 세션 전환
- `Teardown` on Drop

### Data Flow

```
teardown_current()
  → if memory_enabled && memory backend exists:
       spawn(async {
           let messages = session.messages();     // 최종 메시지 목록
           let summary = summarize_messages(&messages);  // LLM call or simple
           session_reflect(&backend, session_id, &summary).await;
       })
```

### 요약 전략

omp는 별도 LLM 호출로 요약을 생성한다. oxicode에서도 같은 접근:

**Option A (recommended):** `memory_reflect` 도구와 동일한 로직 재사용
- `MemoryReflectTool`이 이미 요약 생성 로직을 가지고 있음
- 세션 종료 시: `memory_reflect` 도구 호출 → "session end summary" 저장
- 장점: 기존 코드 재사용, 일관된 요약 품질
- 단점: LLM 호출 필요 (비용, 지연)

**Option B:** 단순 메시지 카운트 + 마지막 메시지 저장
- 저장 내용: session_id, message_count, last_message_preview, duration
- 장점: LLM 불필요, 즉시 완료
- 단점: 요약 품질 낮음

**Decision: Option A (LLM summary).** 비용이 concerns면 `memory_pipeline`의 Stage 1/2가 결국 JSONL에서 추출하므로, 이 훅은 "실시간성 보강"에 불과하다. LLM 요약 실패 시 단순 메시지 카운트로 fallback.

### session_reflect() 시그니처 (기존 유지)

```rust
pub async fn session_reflect(
    backend: &dyn MemoryBackend,
    subject: &str,   // session_id
    summary: &str,   // LLM-generated or fallback text
) {
    if let Err(e) = backend.put(summary, "summary", subject).await {
        tracing::warn!("Failed to store session memory: {e}");
    }
}
```

### 호출 위치 상세

```rust
// oxicode-cli/src/app/agent_session_runtime.rs

fn teardown_current(&mut self, reason: SessionSwitchReason) {
    // Capture data before reset
    let session_id = self.session.session_id();
    let messages = self.session.messages();  // Vec<Message>
    let has_memory = self.services.memory_enabled();  // check setting
    
    self.session.reset();
    
    if has_memory && messages.len() >= 3 {  // meaningful session
        let backend = self.services.memory_backend().clone();
        tokio::spawn(async move {
            let summary = match generate_session_summary(&messages).await {
                Some(s) => s,
                None => format!("Session {}: {} messages", session_id, messages.len()),
            };
            session_reflect(&*backend, &session_id, &summary).await;
        });
    }
}
```

`generate_session_summary()`는 LLM을 호출하거나 (Option A) 단순 문자열을 반환한다 (Option B fallback).

### MemoryBackend 접근

`AgentSessionServices`는 이미 `MemoryBackend`를 구성한다 (services.rs의 `start_memory_pipeline`). `AgentSessionRuntime`이 `services` 필드를 통해 접근:

```rust
// AgentSessionRuntime에 추가
fn memory_backend(&self) -> Option<Arc<dyn MemoryBackend>> {
    // services에서 memory backend 추출
    self.services.memory_backend.clone()
}
```

### Feature Gate

- `memory_enabled: false` → skip (services에 backend가 None)
- `messages.len() < 3` → skip (의미 없는 세션)

### Files to Modify

| File | Change |
|---|---|
| `oxicode-cli/src/app/agent_session_runtime.rs` | `teardown_current()`에 memory reflect 로직 추가. `memory_backend()` accessor. |
| `oxicode-cli/src/services.rs` | `session_reflect()`는 이미 있음. (선택) `generate_session_summary()` helper 추가 |
| `oxicode-cli/src/app/agent_session_services.rs` (if exists) | MemoryBackend accessor 노출 |

### Acceptance Criteria

1. 세션 종료 시 memory backend에 session_id → summary 저장됨
2. `memory_enabled: false`일 때 skip (no-op)
3. 3개 미만 메시지 세션 skip (noise 방지)
4. `teardown_current()` blocking 안 됨 (fire-and-forget)
5. `/memory status`에 해당 세션의 summary가 나타남 (60초 이내 pipeline과 중복 가능)

### Test Strategy

- Mock MemoryBackend으로 `teardown_current()` 호출 시 `backend.put()`이 불리는지 확인
- messages 길이별 경계 테스트 (0, 2, 3, 100)
- `memory_enabled: false` 시 skip 검증
- 기존 `session_reflect` 테스트는 변경 없음
