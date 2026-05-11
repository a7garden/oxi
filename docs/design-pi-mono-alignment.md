# Design: pi-mono Architecture Alignment

pi-mono를 정답지로 삼아 oxi의 아키텍처를 정렬하는 작업 계획.

---

## 문제 요약

oxi는 6가지 Critical/Important 차이가 있음:

1. **TUI가 `MessageUpdate`를 무시** — `TextChunk` 문자열만으로 렌더링 → JSON 섞임
2. **TUI event forwarder가 핵심 이벤트 스킵** — ToolCall, TurnStart/End 등 무시
3. **TUI 모드에서 세션 퍼시스턴스 없음** — `process_events`가 안 돔
4. **Queue dequeue 알림 없음** — enqueue만, consume 알림 없음
5. **`shouldStopAfterTurn` 컨텍스트가 stub**
6. **Abort/Cancel 메커니즘 없음**

근본 원인: TUI 모드가 `prompt_streaming()`을 쓰면서 raw channel만 받고,
`AgentSession`의 `process_events()`(persist, compaction, follow-up)를 우회함.

---

## 설계 원칙

pi-mono의 아키텍처를 그대로 따름:

```
Agent (이벤트 발생)
  → AgentSession._handleAgentEvent() (직렬화된 async 체인)
    → extension 처리
    → 리스너 emit (여기에 TUI가 구독)
    → 세션 퍼시스턴스 (message_end마다)
    → auto-retry / auto-compaction (agent_end 후)
```

oxi에서는 thread boundary 때문에 channel을 써야 하지만,
event 처리 흐름은 동일하게 만들어야 함.

---

## Phase 1: TUI Event Pipeline 정리 (Critical)

### 목표
pi-mono처럼 `MessageUpdate` 이벤트가 TUI까지 전달되어,
provider가 만든 분리된 content blocks (text + toolCall)가 그대로 렌더링되게 함.

### 변경 사항

#### 1.1 `UiEvent` 확장
`app.rs`의 `UiEvent` enum을 pi-mono의 event 구조에 맞춤:

```rust
pub enum UiEvent {
    // Agent lifecycle
    AgentStart,
    AgentEnd,

    // Turn lifecycle
    TurnStart,
    TurnEnd,

    // Message lifecycle — pi-mono의 message_start/update/end
    MessageStart(Message),
    MessageUpdate(Message),   // ← 현재 없음. full snapshot 전달
    MessageEnd(Message),

    // Tool execution
    ToolExecutionStart { id, name, args },
    ToolExecutionEnd { id, name, result, is_error },

    // Token usage
    TokenUsage { ... },

    // Queue
    QueueUpdate { steering, follow_up },

    // Compaction
    CompactionStart { reason },
    CompactionEnd { reason, error },

    // Retry
    RetryStart { attempt, max, error },

    // Image
    ImageBlock { mime, data },
}
```

#### 1.2 Event forwarder 단순화
`app.rs`의 agent worker thread에서 `AgentEvent → UiEvent` 매핑을
pi-mono의 `AgentSession._handleAgentEvent` 구조에 맞춤:

```rust
// pi-mono: _handleAgentEvent가 모든 이벤트를 직렬 처리
// oxi: event_forwarder가 모든 이벤트를 UiEvent로 매핑
match event {
    AgentEvent::AgentStart { .. } => UiEvent::AgentStart,
    AgentEvent::AgentEnd { .. } => UiEvent::AgentEnd,
    AgentEvent::TurnStart { .. } => UiEvent::TurnStart,
    AgentEvent::TurnEnd { .. } => UiEvent::TurnEnd,
    AgentEvent::MessageStart { message } => UiEvent::MessageStart(message),
    AgentEvent::MessageUpdate { message, .. } => UiEvent::MessageUpdate(message),
    AgentEvent::MessageEnd { message } => UiEvent::MessageEnd(message),
    AgentEvent::ToolExecutionStart { .. } => UiEvent::ToolExecutionStart { ... },
    AgentEvent::ToolExecutionEnd { .. } => UiEvent::ToolExecutionEnd { ... },
    // ... etc
}
```

#### 1.3 TUI 핸들러를 MessageUpdate 기반으로 전환
`handlers.rs`의 `handle_ui_event`:

```rust
match event {
    UiEvent::MessageStart(msg) => {
        // assistant 메시지 스트리밍 시작
        state.chat.start_streaming_from_message(msg);
    }
    UiEvent::MessageUpdate(msg) => {
        // pi-mono: updateContent(message) — full snapshot
        // provider가 만든 content blocks 그대로 반영
        state.chat.update_streaming_message(msg);
    }
    UiEvent::MessageEnd(msg) => {
        // 스트리밍 완료 — 메시지 확정
        state.chat.finalize_streaming_message(msg);
    }
    // ...
}
```

#### 1.4 `ChatViewState`에 MessageUpdate 지원 추가
현재 `stream_text_delta(String)`만 있는 `ChatViewState`에
`update_streaming_message(Message)` 추가:

```rust
pub fn update_streaming_message(&mut self, msg: AssistantMessage) {
    if let Some(ref mut s) = self.streaming {
        // content blocks를 provider가 만든 것으로 교체
        s.message.content_blocks = content_blocks_from_msg(msg);
    }
}
```

이렇게 하면 provider가 text/toolCall을 분리한 구조가
그대로 TUI까지 전달됨 → JSON이 섞일 일이 없음.

---

## Phase 2: TUI 모드에서 AgentSession 이벤트 처리 (Critical)

### 문제
TUI 모드는 `prompt_streaming()`으로 raw `AgentEvent` channel만 받고,
`AgentSession.process_events()` (persist, compaction, follow-up)를 우회함.

### 해결
pi-mono처럼 모든 이벤트가 AgentSession을 거치게 함.

#### 2.1 AgentSession이 streaming도 처리
`prompt_streaming()` 대신, AgentSession 자체가
이벤트를 subscribe해서 처리:

```rust
// AgentSession.prompt()가 streaming도 처리하도록 수정
pub async fn prompt(&self, text: String, options: PromptOptions) -> Result<()> {
    // ... validation ...

    let (event_tx, mut event_rx) = mpsc::channel(256);

    // Agent를 실행하고 이벤트를 받음
    let handle = self.run_agent_async(text, event_tx);

    // pi-mono처럼 이벤트를 직렬 처리
    while let Some(event) = event_rx.recv().await {
        self.process_single_event(&event).await;
        // TUI를 위한 channel에도 전달
        if let Some(ref tx) = self.ui_event_tx {
            let _ = tx.send(event.clone());
        }
    }
}
```

#### 2.2 세션 퍼시스턴스
`process_single_event`에서 pi-mono처럼 `message_end`마다 저장:

```rust
async fn process_single_event(&self, event: &AgentEvent) {
    // Extension 처리
    self.forward_to_extensions(event).await;

    // 리스너 emit
    self.emit(SessionEvent::Agent(event.clone()));

    // pi-mono: message_end마다 persist
    if let AgentEvent::MessageEnd { message } = event {
        self.persist_message(message);
    }

    // pi-mono: agent_end 후 compaction + retry 체크
    if let AgentEvent::AgentEnd { .. } = event {
        self.check_auto_compaction().await;
        self.resolve_retry();
    }

    // Queue drain notification (pi-mono: message_start(user)에서)
    if let AgentEvent::MessageStart { message } = event {
        if message.role() == "user" {
            self.check_queue_drain(message);
        }
    }
}
```

---

## Phase 3: Queue Drain Notification (Important)

### 문제
pi-mono는 `message_start(user)` 이벤트가 오면 steering/follow-up 큐에서
해당 메시지를 제거하고 `queue_update`를 발행.
oxi는 enqueue만 알림.

### 해결
AgentSession의 이벤트 처리에서 queue drain 감지:

```rust
fn check_queue_drain(&self, message: &Message) {
    let text = extract_user_text(message);
    if let Some(text) = text {
        let mut steering = self.steering_messages.write();
        if let Some(idx) = steering.iter().position(|m| m == &text) {
            steering.remove(idx);
            drop(steering);
            self.emit_queue_update();
            return;
        }
        drop(steering);

        let mut follow_up = self.follow_up_messages.write();
        if let Some(idx) = follow_up.iter().position(|m| m == &text) {
            follow_up.remove(idx);
            drop(follow_up);
            self.emit_queue_update();
        }
    }
}
```

---

## Phase 4: shouldStopAfterTurn 컨텍스트 (Important)

### 문제
pi-mono는 `{ message, toolResults, context, newMessages }`를 전달.
oxi Agent의 hook은 dummy message + 빈 tool_results.

### 해결
`ShouldStopAfterTurnContext`에 실제 데이터 제공:

```rust
// agent.rs run_with_channel 내부:
let ctx = ShouldStopAfterTurnContext {
    message: assistant_message.clone(),
    tool_results: tool_results.clone(),  // ← 실제 데이터
    iteration: iteration,
};
if hooks.should_stop_after_turn.as_ref()?(ctx) {
    break 'outer;
}
```

---

## Phase 5: Abort/Cancel (Important)

### 문제
pi-mono는 `AbortController`로 provider, tools, hooks에 signal 전파.
oxi는 cancel 메커니즘 없음.

### 해결
`CancellationToken` 패턴 도입:

```rust
// oxi-ai에 CancellationToken 추가
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

// Provider::stream()에 option으로 전달
pub struct StreamOptions {
    pub cancellation_token: Option<CancellationToken>,
    // ...
}

// Tool::execute()에도 전달
pub trait AgentTool {
    async fn execute(&self, id: &str, params: Value, 
        cancel: Option<CancellationToken>) -> Result<AgentToolResult>;
}

// Agent.run_with_channel에서 전파
// abort() 호출 → token.cancel() → provider stream 중단
```

---

## Phase 6: 구조적 정리 (Architecture Debt)

### 6.1 Agent → AgentLoop 통합
`Agent.run_with_channel()`의 중복 로직을 `AgentLoop`에 위임.
Agent는 상태 관리 + lifecycle만 담당.

### 6.2 Legacy 이벤트 제거
`TextChunk`, `ToolStart`, `ToolComplete`, `Start`, `Complete` 제거.
`MessageStart/Update/End`, `AgentStart/End`만 사용.

---

## 실행 순서

| 순서 | Phase | 영향 범위 | 의존성 |
|------|-------|----------|--------|
| 1 | Phase 1.1-1.4 | UiEvent, handlers, chat.rs | 없음 |
| 2 | Phase 2.1-2.2 | agent_session.rs, app.rs | Phase 1 |
| 3 | Phase 3 | agent_session.rs | Phase 2 |
| 4 | Phase 4 | agent.rs | 없음 |
| 5 | Phase 5 | oxi-ai, oxi-agent | Phase 6 후에 자연스럽게 |
| 6 | Phase 6 | oxi-agent 전체 | Phase 1-3 후 |

Phase 1-4가 사용자 경험에 직접 영향. Phase 5-6은 별도 PR로.
