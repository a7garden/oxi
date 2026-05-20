# P1 분석: EventBus vs MessageBus 중복 검증

## 1. MessageBus (`oxi-sdk/src/message_bus.rs`)

### 구조
- **타입**: `tokio::sync::broadcast` 기반 pub/sub 채널
- **메시지**: `InterAgentMessage` (struct)
  - `from: String`, `to: Option<String>`, `message_type: String`, `payload: serde_json::Value`, `timestamp_ms: u64`
- **기능**: direct 메시지, broadcast 메시지, subscriber 관리
- **특징**: **generic payload** — 임의의 JSON을 실어나르는 범용 메시징

### oxios 내 사용처
- `A2aApi`가 내부 필드로 보유 (`a2a_api.rs:13`)
- `oxios-kernel/src/lib.rs`에서 re-export
- **실제 비즈니스 로직에서 사용하는 곳 없음** — A2aApi만 생성하고, 커널 내부 어디에서도 `.publish()`를 호출하지 않음

---

## 2. EventBus (`oxios-kernel/src/event_bus.rs`)

### 구조
- **타입**: `tokio::sync::broadcast` 기반 pub/sub 채널
- **메시지**: `KernelEvent` (enum, 20개 variant)
  - `AgentCreated`, `AgentStarted`, `AgentStopped`, `AgentFailed`, `MessageReceived`, `SeedCreated`, `EvaluationComplete`, `PhaseStarted`, `PhaseCompleted`, `AgentOutput`, `ApprovalRequested`, `ApprovalResolved`, `MemoryStored`, `MemoryRecalled`, `AgentGroupCreated`, `AgentGroupMemberCompleted`, `SpaceCreated`, `SpaceActivated`, `SpaceArchived`, `KnowledgeCrossReferenced`, `SpacesMerged`
- **기능**: typed event publish/subscribe, audit trail 연동
- **특징**: **domain-specific typed events** — 커널 수명주기의 모든 상태 변화를 강타입으로 표현

### oxios 내 사용처 (광범위)
| 모듈 | 용도 |
|------|------|
| `supervisor.rs` | 에이전트 생성/시작/중지/실패 이벤트 발행 |
| `agent_lifecycle.rs` | 에이전트 정지 이벤트 발행 |
| `a2a.rs` | `A2AProtocol`, `AgentCardRegistry`에서 에이전트 등록/해제/메시지 수신 이벤트 발행 |
| `kernel_bridge.rs` | 테스트/초기화에서 EventBus 생성 |
| `space_tool.rs` | Space 생성 이벤트 |
| `tools/a2a_tools.rs` | 테스트에서 생성 |
| `integration_tests.rs` | 대부분의 통합 테스트에서 핵심 역할 |
| `e2e_test.rs` | E2E 테스트에서 핵심 역할 |

---

## 3. 결론: 서로 다른 추상화 레벨

| 기준 | MessageBus (oxi-sdk) | EventBus (oxios-kernel) |
|------|---------------------|------------------------|
| **목적** | 범용 inter-agent 메시징 | 커널 수명주기 이벤트 시스템 |
| **메시지 타입** | generic (`String` + `Value`) | typed enum (`KernelEvent`) |
| **소비자** | 에이전트 간 자유 형식 통신 | 커널 내부 모듈 + audit trail |
| **실제 사용** | oxios 내에서 **사실상 미사용** | oxios의 **핵심 인프라** (수십 곳) |
| **계층** | SDK 라이브러리 계층 | 커널 런타임 계층 |
| **audit 연동** | 없음 | `attach_audit_trail()` 제공 |

### 역할 중복 여부: **NO** (개념적으로 다름)

두 버스는 같은 메커니즘(`tokio::broadcast`)을 사용하지만, **완전히 다른 목적**을 가짐:

1. **MessageBus** → "에이전트 A가 에이전트 B에게 임의 메시지를 보내는" user-space 메시징 (SDK 수준)
2. **EventBus** → "커널이 에이전트 수명주기 상태 변화를 시스템 전체에 알리는" kernel-space 이벤트 (런타임 수준)

비유: MessageBus는 **우편물 배달**, EventBus는 **운영체제 시그널/인터럽트**.

### 통합이 이득인지: **NO** (오히려 해로움)

- EventBus의 `KernelEvent` enum은 커널 전용 domain 지식(Seed, Phase, Space, Memory, Approval 등)을 포함 → SDK에 넣으면 SDK가 커널에 종속됨
- MessageBus의 generic payload는 커널 event에 부적합 → 타입 안전성 상실
- 실제로 MessageBus는 oxios 내에서 거의 사용되지 않으므로, 통합하면 사용되지 않는 generic 채널이 커널을 오염시킴

---

## 4. 권장 사항

### 현상 유지 (권장)
두 버스는 서로 다른 추상화 레벨에서 올바르게 동작 중. 통합 불필요.

### 정리 가능한 항목 (선택)
- `A2aApi.message_bus` 필드가 **실제로 사용되지 않음** — dead code 정리 대상
- `oxios-kernel/src/lib.rs`의 `InterAgentMessage`, `MessageBus` re-export도 현재 사용되지 않음
- 이 정리는 P2 이하 (기능에 영향 없음)

### 주의사항
- MessageBus는 oxi-cli 등 oxi 자체 프로젝트에서 직접 사용될 수 있으므로, oxi-sdk에서 제거하면 안 됨
- EventBus는 oxios 전용이므로 oxi-sdk에 올리면 안 됨
