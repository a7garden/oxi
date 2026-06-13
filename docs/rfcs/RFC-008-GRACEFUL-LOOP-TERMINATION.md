# RFC-008: 에이전트 루프 정상 종료 보장

| 메타 | 값 |
|------|-----|
| **상태** | 제안 |
| **작성일** | 2026-06-12 |
| **영역** | `oxi-agent/src/agent_loop/` |
| **영향 크레이트** | `oxi-agent`, `oxios-kernel` |
| **관련** | RFC-003 (Agent/Tool superiority) |

---

## 1. 문제

### 1.1 현상

Oxios Web UI에서 에이전트 실행 후 **도구 호출 아이콘만 보이고 최종 텍스트 응답이 없다.**
"Agent execution completed" 같은 무의미한 폴백 메시지만 표시된다.

### 1.2 근본 원인

oxi-agent의 `should_stop_after_turn()`이 `turn_number >= max_iterations`일 때
에이전트 루프를 **즉시 종료**한다. 이때 마지막 LLM 응답이 `tool_use`였으면
텍스트 응답이 생성되지 않은 채로 루프가 끝난다.

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

| | pi-agent | oxi-agent |
|---|---|---|
| `shouldStopAfterTurn` | 사용 안 함 (undefined) | `turn >= max_iterations → true` |
| 루프 종료 조건 | `hasMoreToolCalls === false` (자연스러운) | `max_iterations` 도달 (강제) |
| 최종 텍스트 응답 | **항상 보장** | **보장 안 됨** |

pi-agent 주석 원문:

> pi-mono: shouldStopAfterTurn is an optional hook. If no hook is defined,
> the loop never stops here — it continues until no more tool calls AND
> no more steering/follow-up messages.

---

## 2. 설계 목표

1. **텍스트 응답 보장**: 에이전트 루프 종료 후 항상 사용자에게 전달할 텍스트 응답이 존재해야 한다.
2. **무한 루프 방지**: `max_iterations` 가드는 유지하되, 종료 방식을 개선한다.
3. **하위 호환**: 기존 `should_stop_after_turn` 시그니처와 `max_iterations` 의미를 유지한다.
4. **최소 변경**: oxi-agent의 공개 API를 바꾸지 않는다.

---

## 3. 설계

### 3.1 개요

`should_stop_after_turn()`이 `max_iterations`에 도달해 `true`를 반환하면,
루프를 즉시 종료하는 대신 **"마지막 턴" 플래그**를 설정하고 한 번 더 LLM을 호출한다.
이때 LLM에게 "더 이상 도구를 호출하지 말고 텍스트로 답변하라"는
stop-requested 시스템 메시지를 주입한다.

```
iteration 8: max_iterations 도달
              → stop_requested = true
              → LLM 호출 (stop-requested 메시지와 함께)
              → LLM이 텍스트 응답 생성 (도구 호출 없이)
              → 루프 정상 종료
              → final_content = "해커뉴스 인기 글 3건..."
```

### 3.2 `should_stop_after_turn` 변경

**Before** (`agent_loop/helpers.rs`):

```rust
pub fn should_stop_after_turn(
    _messages: &[oxi_ai::Message],
    _assistant_message: &oxi_ai::AssistantMessage,
    max_iterations: usize,
    external_stop: &Arc<AtomicBool>,
    turn_number: usize,
) -> bool {
    if external_stop.load(Ordering::SeqCst) { return true; }
    if turn_number >= max_iterations { return true; }
    false
}
```

**After**:

```rust
pub enum StopReason {
    /// 외부 중단 신호 (Ctrl+C). 즉시 종료.
    ExternalStop,
    /// max_iterations 도달. 한 번 더 LLM 호출 후 정상 종료 권장.
    MaxIterationsReached,
}

pub fn should_stop_after_turn(
    _messages: &[oxi_ai::Message],
    _assistant_message: &oxi_ai::AssistantMessage,
    max_iterations: usize,
    external_stop: &Arc<AtomicBool>,
    turn_number: usize,
) -> Option<StopReason> {
    if external_stop.load(Ordering::SeqCst) {
        return Some(StopReason::ExternalStop);
    }
    if turn_number >= max_iterations {
        return Some(StopReason::MaxIterationsReached);
    }
    None
}
```

반환 타입을 `bool` → `Option<StopReason>`으로 변경하여 **종료 사유**를 구분한다.

### 3.3 루프 종료 시 텍스트 응답 보장 (`run_loop`)

`run_loop` 내부의 `should_stop_after_turn` 호출 지점:

**Before** (`agent_loop/mod.rs`):

```rust
if should_stop_after_turn(&messages, &assistant_message, ...) {
    return Ok((messages, events));
}
```

**After**:

```rust
match should_stop_after_turn(&messages, &assistant_message, ...) {
    Some(StopReason::ExternalStop) => {
        // 즉시 종료 (사용자 취소)
        return Ok((messages, events));
    }
    Some(StopReason::MaxIterationsReached) => {
        // 한 번 더 LLM 호출: 도구 없이 텍스트로만 답변
        let stop_message = Message::User(UserMessage::new(
            "[system] Maximum iterations reached. Please provide a final text \
             response summarizing your work so far. Do NOT make any more tool calls."
             .to_string(),
        ));
        messages.push(stop_message);

        let final_response = stream_assistant_response(self, &mut messages, &emit).await?;
        new_messages.push(Message::Assistant(final_response.clone()));

        emit(AgentEvent::TurnEnd { ... });
        events.push(AgentEvent::TurnEnd { ... });
        return Ok((messages, events));
    }
    None => { /* 계속 진행 */ }
}
```

### 3.4 `AgentEvent` 확장

새 이벤트를 추가하여 소비자(oxios-kernel)가 정상 종료과 강제 종료를 구분할 수 있게 한다:

```rust
/// max_iterations 도달 후 마지막 요약 턴이 시작됨.
AgentLoopForcedSummary { turn_number: u32 }
```

이 이벤트를 받은 oxios-kernel은 `AgentEvent::AgentEnd`의 `final_content`를
신뢰할 수 있다는 것을 알게 된다.

### 3.5 `max_iterations` 기본값 조정

현재 `AgentLoopConfig::default().max_iterations = 20`.
oxios-kernel의 `AgentRuntimeConfig::max_iterations = 8`.

pi-agent에서는 `shouldStopAfterTurn`을 사용하지 않으므로 사실상 무제한이다.
oxi-agent의 `max_iterations = 20`은 대부분의 작업에서 충분하며,
도달하더라도 3.3의 설계로 텍스트 응답이 보장된다.

oxios-kernel의 `8`은 보수적이지만, 3.3이 구현되면 문제가 되지 않는다.
**변경하지 않는다.**

---

## 4. oxios-kernel 측 변경 (이미 구현 완료)

oxi-agent의 루프 개선이 배포될 때까지, oxios-kernel에서 **워크어라운드**로
post-execution summarization을 구현해 두었다.

### 4.1 `agent_runtime.rs` — Post-execution summarization

```rust
// run_agent() 완료 후
if final_content.is_empty() && !trajectory_steps.is_empty() {
    // 도구 결과를 모아서 한 번 더 LLM 호출
    let tool_summary = trajectory_steps.iter().map(...).collect();
    let summary_prompt = format!(
        "도구 실행 결과:\n\n{}\n\n\
         위 결과를 바탕으로 사용자의 요청에 대해 자연스럽게 한국어로 답변해주세요.",
        tool_summary.join("\n")
    );
    match agent.run(summary_prompt).await {
        Ok((response, _)) => { final_content = response.content; }
        Err(_) => { /* 워크어라운드도 실패하면 빈 응답 */ }
    }
}
```

이 코드는 oxi-agent의 3.3 설계가 구현된 후 제거 가능하다.

### 4.2 시스템 프롬프트 개선

```
Before: "You have tools for reading, writing, editing files, running commands"
After:  "File tools, Web tools (web_search), Exec, Kernel tools"
        + "웹에서 정보를 가져오는 작업이면 web_search를 먼저 사용하세요"
```

이것은 루프 아키텍처 문제와 별개로, **LLM이 적절한 도구를 선택하게 유도**하여
불필요한 반복을 줄이고 `max_iterations` 도달 가능성을 낮추는 방어선이다.

---

## 5. 프론트엔드 변경 (oxios-web, 이미 구현 완료)

루프 종료 방식과 무관하게, 프론트엔드에서 도구 활동이 올바르게 표시되도록
수정했다.

### 5.1 RFC-015 activity 청크에 assistant placeholder 자동 생성

```typescript
// Before: 마지막 메시지가 assistant가 아니면 activity 무시
if (last?.role !== 'assistant') return s

// After: assistant placeholder 생성 후 activity 첨부
if (last?.role !== 'assistant') {
  return { messages: [...updated, { role: 'assistant', content: '', activities: [activity] }] }
}
```

### 5.2 done 청크를 activities로 통합

스트리밍(`done`)과 새로고침(`loadSession`)의 렌더링 경로를 통일.
`role:'tool'` 메시지 생성 대신 `activities` 배열로 변환하여 assistant 메시지에 병합.

---

## 6. 마이그레이션 계획

### Phase 1: 워크어라운드 (완료)

oxios-kernel의 `agent_runtime.rs`에서 post-execution summarization으로 대응.
oxi-agent 변경 없이 동작.

### Phase 2: oxi-agent 근본 수정 (이 RFC)

`should_stop_after_turn` 반환 타입 변경 + 루프 내 요약 턴 추가.
oxi-agent의 **공개 API가 변경되지 않으므로** semver에 영향 없음.
`should_stop_after_turn`은 `pub(crate)`이므로 외부 크레이트에 영향 없다.

> **확인 필요**: `should_stop_after_turn`의 가시성이 `pub(crate)`인지
> `pub`인지 확인. `pub`이면 시그니처 변경이 breaking change.

### Phase 3: 워크어라운드 제거

Phase 2 배포 후 oxios-kernel의 post-execution summarization 코드를
`#[deprecated]` 처리하고, 다음 마이너 버전에서 제거.

---

## 7. 대안 고려

### 7.1 `max_iterations` 증가

`8 → 20`으로 늘리면 도달 가능성이 낮아지지만 근본 해결이 아니다.
LLM이 계속 잘못된 도구를 선택하면 여전히 도달한다.

### 7.2 `should_stop_after_turn`을 no-op로 변경

pi-agent처럼 아무것도 하지 않게 만들면 무한 루프 위험이 있다.
악의적이거나 버그 있는 프롬프트에서 LLM이 무한히 tool_use를 반복할 수 있다.

### 7.3 LLM에게 tool_use 금지 지시를 시스템 프롬프트에 추가

"max_iterations에 가까워지면 tool_use를 멈춰라" 같은 지시.
신뢰할 수 없다 — LLM이 따르지 않을 수 있다.

---

## 8. 테스트 계획

### 단위 테스트

| 테스트 | 설명 |
|--------|------|
| `test_should_stop_returns_none` | `turn < max_iterations` → `None` |
| `test_should_stop_returns_max_iterations` | `turn >= max_iterations` → `Some(MaxIterationsReached)` |
| `test_should_stop_returns_external` | `external_stop == true` → `Some(ExternalStop)` |

### 통합 테스트

| 테스트 | 설명 |
|--------|------|
| `test_loop_produces_final_text` | `max_iterations` 도달 후 `AgentEnd.messages`의 마지막이 텍스트-only assistant |
| `test_external_stop_no_summary` | 외부 중단 시 summary 턴 없이 즉시 종료 |
| `test_natural_exit_no_extra_turn` | LLM이 자연스럽게 tool_use 중단하면 summary 턴 없이 종료 |

---

## 9. 요약

| 항목 | 내용 |
|------|------|
| **문제** | `max_iterations` 강제 종료 시 LLM 텍스트 응답 누락 |
| **근본 원인** | oxi-agent의 `should_stop_after_turn`이 즉시 종료 |
| **해결** | `max_iterations` 도달 시 한 번 더 LLM 호출 (stop-requested 메시지와 함께) |
| **워크어라운드** | oxios-kernel의 post-execution summarization (이미 구현) |
| **근본 수정** | oxi-agent `should_stop_after_turn` + `run_loop` 변경 |
