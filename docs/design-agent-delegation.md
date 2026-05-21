# Design: Agent → AgentLoop Delegation

## 문제

`Agent` (agent.rs)와 `AgentLoop` (agent_loop/)가 같은 agentic loop를
각각 독립적으로 구현하고 있음. TUI는 `Agent.run_with_channel()`을 사용.

`AgentLoop`이 pi-mono에 더 가까움:
- 올바른 이벤트 순서 (AgentStart/End, TurnStart/End, MessageStart/Update/End)
- 올바른 스트리밍 (partial message 누적, content blocks 분리)
- Auto-retry, circuit breaker
- Structured hooks (before/after tool call)

`Agent`의 인라인 loop는:
- Legacy 이벤트 (Start, Complete, TextChunk, ToolStart, ToolComplete)
- Provider의 partial이 빈 상태로 옴 → content blocks 누적 안 됨
- 이벤트 수명주기 불완전 (에러 시 MessageEnd 누락)
- 최근에 패치를 많이 했지만 여전히 취약함

## 해결

**Agent.run_with_channel()이 AgentLoop을 호출하도록 변경.**

Agent는 상태 관리, 도구 등록, 모델 전환, compaction의 공개 API로 유지.
실행은 AgentLoop에 위임.

## 변경 사항

### 1. Agent.run_with_channel() 재작성

```rust
pub async fn run_with_channel(
    &self,
    prompt: String,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<Response> {
    // hooks를 AgentLoopConfig로 변환
    let loop_config = self.build_loop_config();
    
    // AgentLoop 생성
    let agent_loop = AgentLoop::new(
        Arc::clone(&self.inner.read().provider),
        loop_config,
        Arc::clone(&self.tools),
        self.state.clone(),
    );
    
    // hooks 설정
    let hooks = self.hooks.read();
    let mut al = agent_loop;
    if let Some(ref hook) = hooks.before_tool_call {
        al = al.with_before_tool_call(hook.clone());
    }
    if let Some(ref hook) = hooks.after_tool_call {
        al = al.with_after_tool_call(hook.clone());
    }
    
    // steering/follow-up 큐에서 대기 중인 메시지 주입
    let steering = self.drain_steering_messages();
    for msg in steering {
        al.steer(Message::User(UserMessage::new(msg)));
    }
    let follow_ups = self.drain_follow_up_messages();
    for msg in follow_ups {
        al.follow_up(Message::User(UserMessage::new(msg)));
    }
    
    // 실행 — emit callback이 tx로 전달
    let result = al.run(prompt, move |event| {
        let _ = tx.blocking_send(event);  // 또는 try_send
    }).await?;
    
    // 상태 업데이트
    self.sync_state_from_loop(&al);
    
    // Response 생성
    // ...
}
```

### 2. AgentLoop에 emit callback → channel adapter 추가

AgentLoop은 `Fn(AgentEvent)` callback을 받음.
mpsc channel을 callback으로 래핑:

```rust
let tx_clone = tx.clone();
let emit = move |event: AgentEvent| {
    let _ = tx_clone.send(event);  // blocking_send (LocalSet 안이므로)
};
```

문제: AgentLoop의 emit은 `Send + Sync + 'static`이어야 함.
mpsc::Sender도 Send + Sync이므로 OK.

하지만 AgentLoop.run()이 `&self`를 받음 — 상태 업데이트 후
Agent의 state를 동기화해야 함.

### 3. 상태 동기화

AgentLoop은 자체 SharedState를 가짐.
실행 후 Agent의 state를 업데이트:

```rust
fn sync_state_from_loop(&self, al: &AgentLoop) {
    let loop_state = al.state();  // AgentLoop의 SharedState에서 가져옴
    self.state.update(|s| {
        s.messages = loop_state.messages.clone();
        s.iteration = loop_state.iteration;
        s.stop_reason = loop_state.stop_reason;
    });
}
```

더 나은 방법: **Agent와 AgentLoop이 같은 SharedState를 공유.**
AgentLoop::new()에 Agent의 SharedState를 전달하면
동기화가 자동으로 됨.

### 4. Hook 변환

Agent의 `AgentHooks`를 AgentLoop의 hook system으로 변환:

```rust
// AgentHooks.should_stop_after_turn → should_stop_after_turn helper
// (이미 AgentLoop 내부에서 max_iterations 체크)

// AgentHooks.get_steering_messages → steering queue pre-populate
// (AgentLoop.steer()로 미리 주입)

// AgentHooks.before_tool_call → with_before_tool_call()
// AgentHooks.after_tool_call → with_after_tool_call()
```

### 5. 제거할 코드

- `agent.rs`의 인라인 루프 (run_with_channel 내부의 전체 loop 로직)
- `stream_with_retry`, `try_fallback` (AgentLoop이 자체 retry/circuit-breaker 가짐)
- 모든 ProviderEvent 처리 코드 (AgentLoop.streaming.rs이 담당)

Agent.run_with_channel()은 ~30줄의 위임 코드가 됨.

### 6. 영향받는 파일

| 파일 | 변경 |
|------|------|
| `agent.rs` | run_with_channel() 재작성 (위임), 인라인 loop 제거 |
| `agent_loop/mod.rs` | 없음 (이미 올바름) |
| `agent_loop/streaming.rs` | 없음 (이미 partial 누적함) |
| `agent_loop/config.rs` | 없음 |
| `agent_session.rs` | 없음 (Agent 공개 API 동일) |
| `tui/app.rs` | 없음 (이미 MessageUpdate 기반) |

## 실행 계획

1. AgentLoop에 `state()` getter 추가 (SharedState 접근용)
2. Agent.build_loop_config() 추가 (AgentConfig → AgentLoopConfig 변환)
3. Agent.run_with_channel() 재작성
4. 빌드 + 테스트
5. 기존 인라인 loop 코드 제거
6. 다시 빌드 + 설치

## 리스크

- AgentLoop의 emit callback이 blocking_send를 써야 함 (LocalSet 안에서)
- Hook 변환 시 의미 차이 주의
- AgentLoop이 fallback model을 지원하지 않음 → 나중에 추가하거나 Agent 레벨에서 처리
