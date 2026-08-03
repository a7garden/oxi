# RFC-008: 에이전트 루프 정상 종료 보장

| 메타 | 값 |
|------|-----|
| **상태** | ✅ 구현 완료 (oxicode 0.32.0, oxios 1.2.0) |
| **작성일** | 2026-06-12 |
| **영역** | `oxicode-agent/src/agent_loop/`, `oxios-kernel/src/agent_runtime.rs` |
| **영향 크레이트** | `oxicode-agent`, `oxios-kernel` |
| **관련** | RFC-003 (Agent/Tool superiority) |

---

## 1. 문제

### 1.1 현상

Oxios Web UI에서 에이전트 실행 후 **도구 호출 아이콘만 보이고 최종 텍스트 응답이 없다.**
"Agent execution completed" 같은 무의미한 폴백 메시지만 표시된다.

### 1.2 근본 원인

oxicode-agent 0.31.x의 `should_stop_after_turn()`이 `turn_number >= max_iterations`일 때
에이전트 루프를 **즉시 종료**했다. 이때 마지막 LLM 응답이 `tool_use`였으면
텍스트 응답이 생성되지 않은 채로 루프가 끝났다.

```
iteration 6: LLM → tool_use(grep "hackernews")
              → 도구 실행 → 결과 반환
iteration 7: LLM → tool_use(find "hacker")
              → 도구 실행 → 결과 반환
iteration 8: max_iterations (8) 도달 → 강제 종료
              → final_content = ""  ← LLM이 답변할 기회 없음
```

### 1.3 pi-agent 비교

pi-agent(= pi-mono)의 `shouldStopAfterTurn`은 **아무도 설정하지 않는 optional hook**이다.
루프는 LLM이 스스로 tool_use를 그만두고 텍스트만 반환할 때 자연스럽게 종료된다.

| | pi-agent | oxicode-agent 0.31.x |
|---|---|---|
| `shouldStopAfterTurn` | 사용 안 함 (undefined) | `turn >= max_iterations → true` |
| 루프 종료 조건 | `hasMoreToolCalls === false` (자연스러운) | `max_iterations` 도달 (강제) |
| 최종 텍스트 응답 | **항상 보장** | **보장 안 됨** |

---

## 2. 설계 목표

1. **텍스트 응답 보장**: 에이전트 루프 종료 후 항상 사용자에게 전달할 텍스트 응답이 존재해야 한다.
2. **무한 루프 방지**: 외부 중단(Ctrl+C)은 즉시 처리.
3. **최소 변경**: oxicode-agent의 공개 API를 바꾸지 않는다.

---

## 3. 구현 결과

### 3.1 oxicode-agent 0.32.0 — `max_iterations` 완전 제거

`should_stop_after_turn`에서 `max_iterations` 체크를 제거하고,
**오직 외부 중단(Ctrl+C)만** 확인한다.

```rust
/// Check if the loop should stop after a turn due to external cancellation.
///
/// The loop exits naturally when the LLM stops making tool calls (text-only
/// response). This function only checks for out-of-band cancellation (Ctrl+C).
pub fn should_stop_after_turn(external_stop: &Arc<AtomicBool>) -> bool {
    external_stop.load(Ordering::SeqCst)
}
```

`AgentConfig`에서 `max_iterations` 필드도 제거됨.

### 3.2 루프 동작 변화

```
Before (0.31.x):
  iteration 8: max_iterations 도달 → 강제 종료 → final_content = ""

After (0.32.0):
  iteration 8: LLM이 계속 tool_use 반환 → 루프 계속
  iteration 9: LLM이 텍스트만 반환 → 루프 자연 종료 → final_content = "결과..."
```

LLM이 스스로 "도구 다 썼다, 이제 답변할게"라고 결정한다.
pi-agent와 완전히 동일한 동작.

### 3.3 oxios-kernel 적용

- `AgentRuntimeConfig::max_iterations` 필드 제거
- `AgentConfig` 생성 시 `max_iterations` 생략 (0.32.0에 없음)
- Post-execution summarization을 **안전망**으로 유지

### 3.4 안전망: Post-execution summarization

0.32.0에서 루프가 자연 종료되더라도, LLM이 빈 텍스트를 반환하는 극단적 케이스에 대비해
`agent_runtime.rs`에 안전망을 유지:

```rust
// oxicode 0.32.0 removed max_iterations — the loop now exits naturally
// when the LLM produces a text-only response (pi-agent behavior).
// This block is kept as a safety net in case the LLM returns empty
// text despite a natural exit (rare, but possible).
if final_content.is_empty() && !trajectory_steps.is_empty() {
    let summary_prompt = format!("도구 실행 결과를 바탕으로...");
    match agent.run(summary_prompt).await {
        Ok((response, _)) => { final_content = response.content; }
        ...
    }
}
```

### 3.5 시스템 프롬프트 개선 (부차적)

```
Before: "You have tools for reading, writing, editing files, running commands"
After:  "File tools, Web tools (web_search), Exec, Kernel tools"
        + "웹에서 정보를 가져오는 작업이면 web_search를 먼저 사용하세요"
```

LLM이 적절한 도구를 선택하게 유도하여 불필요한 반복 감소.

---

## 4. 프론트엔드 변경 (oxios-web, 구현 완료)

루프 종료 방식과 무관하게, 프론트엔드에서 도구 활동이 올바르게 표시되도록 수정.

### 4.1 Activity 청크에 assistant placeholder 자동 생성

```typescript
// Before: 마지막 메시지가 assistant가 아니면 activity 무시
if (last?.role !== 'assistant') return s

// After: assistant placeholder 생성 후 activity 첨부
```

### 4.2 done 청크를 activities로 통합

스트리밍(`done`)과 새로고침(`loadSession`)의 렌더링 경로 통일.
`role:'tool'` 메시지 생성 대신 `activities` 배열로 변환.

---

## 5. oxicode 0.32.0 추가 기능 활용

### 5.1 ToolExecutionStart.context

oxicode 0.32.0의 `ToolExecutionStart` 이벤트에 `context: Option<ToolCallContext>` 필드가 추가되었다.
oxios-kernel의 이벤트 콜백과 `KernelEvent::ToolExecutionStarted`에 `context` 필드를 전달하도록 업데이트.

지원되는 컨텍스트:
- `WebSearch { query, engine }` — 검색어와 엔진
- `PageVisit { url, reason, page_title, ... }` — 방문 URL과 결과 메타데이터
- `DataExtraction { target, url, result_count, ... }` — 데이터 추출 정보
- `ScriptStep { current, total, step }` — 스크립트 진행 상황

Web UI에서 이 정보를 활용해 도구 실행을 더 풍부하게 렌더링 가능.

### 5.2 success 판정 개선

```rust
// Before: Stop만 성공
s.success = stop_reason.as_deref() == Some("Stop");

// After: Stop + ToolUse 모두 성공 (0.32.0에서 ToolUse는 자연적 루프 진행)
s.success = matches!(
    stop_reason.as_deref(),
    Some("Stop") | Some("ToolUse")
);
```


| 파일 | 변경 |
|------|------|
| `oxicode-agent/src/agent_loop/helpers.rs` | `should_stop_after_turn`에서 `max_iterations` 제거 |
| `oxicode-agent/src/agent_loop/config.rs` | `AgentLoopConfig::max_iterations` 필드 제거 |
| `oxicode-agent/src/config.rs` | `AgentConfig::max_iterations` 필드 제거 |
| `oxios/Cargo.toml` | `oxicode-sdk = "0.32.0"`, path patch 주석 처리 |
| `oxios/Cargo.lock` | `oxicode-agent/ai/sdk 0.31.6 → 0.32.0` |
| `oxios/crates/oxios-kernel/src/agent_runtime.rs` | `max_iterations` 필드 제거, 안전망 주석 업데이트 |
| `oxios/surface/oxios-web/web/src/stores/chat.ts` | Activity placeholder + done→activities 통일 |

---

## 6. 테스트

- `cargo check` ✅
- `cargo test --package oxios-kernel --lib` ✅ (531 passed, 0 failed)
- oxicode-agent 0.32.0 자체 테스트 (crates.io 배포 전 통과)

---

## 7. 요약

| 항목 | 내용 |
|------|------|
| **문제** | `max_iterations` 강제 종료 시 LLM 텍스트 응답 누락 |
| **근본 원인** | oxicode-agent의 `should_stop_after_turn`이 강제 종료 |
| **해결** | `max_iterations` 완전 제거 → LLM이 자연스럽게 텍스트 응답 생성 |
| **참조** | pi-agent의 `shouldStopAfterTurn`은 아무도 설정하지 않음 |
| **안전망** | oxios-kernel의 post-execution summarization 유지 |
