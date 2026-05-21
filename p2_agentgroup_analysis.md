# P2 분석: SDK AgentGroup vs oxios OxiosAgentGroup 실제 차이

## 1. 핵심 결론

**oxios는 SDK의 `AgentGroup`을 전혀 사용하지 않는다.** `OxiosAgentGroup`은 SDK `AgentGroup`과 완전히 다른 목적과 설계 철학을 가진 독립 구현체다.

| 항목 | SDK `AgentGroup` | oxios `OxiosAgentGroup` |
|------|-------------------|-------------------------|
| **목적** | 범용 멀티에이전트 실행 (pipeline/parallel/orchestrated) | Seed 기반 서브태스크 분할 + 상태 추적 |
| **입력** | `Arc<Agent>` 벡터 + 프롬프트 문자열 | `Seed` (부모) + 서브태스크 설명 문자열 리스트 |
| **실행** | 자체 `run()` 메서드로 직접 Agent 실행 | 실행 로직 없음 (상태 데이터 모델만 제공) |
| **직렬화** | `Serialize/Deserialize` 없음 | `Serialize/Deserialize` 지원 (StateStore 저장용) |
| **전략** | Pipeline / Parallel / Orchestrated | 항상 병렬 (전략 enum 없음) |
| **상태 관리** | 성공/실패만 추적 | Pending → Running → Completed/Failed 상태 머신 |
| **에이전트 타입** | `Arc<Agent>` (oxi-agent) | `OxiosGroupAgent` (Seed 기반) |

---

## 2. SDK `AgentGroup` 기능 전체 (oxi-sdk/src/agent_group.rs)

### 타입
- **`GroupStrategy`** enum: `Pipeline`, `Parallel { max_concurrency }`, `Orchestrated { leader }`
- **`AgentGroupOutput`**: name, content, success, error
- **`GroupResult`**: results: Vec<AgentGroupOutput>, total_duration_ms
  - `all_succeeded()`, `combined_content()`
- **`AgentGroup`**: agents: Vec<Arc<Agent>>, strategy: GroupStrategy

### 메서드
| 메서드 | 설명 |
|--------|------|
| `new(strategy)` | 전략 지정 빈 그룹 생성 |
| `agent(agent)` | 빌더 패턴으로 에이전트 추가 |
| `len()` / `is_empty()` | 에이전트 수 확인 |
| `run(prompt)` → `GroupResult` | 전략에 따라 전체 실행 |
| `run_pipeline(prompt)` | 순차 실행 (이전 출력이 다음 입력) |
| `run_parallel(prompt, max_concurrency)` | 병렬 실행 (Semaphore 동시성 제한) |
| `run_orchestrated(prompt, leader_idx)` | 리더 에이전트가 작업 분배 |

### 특징
- `Arc<Agent>` 기반 — oxi-agent의 `Agent` 타입에 강결합
- `Agent::run(prompt)` 호출로 LLM 프롬프트 실행
- 실행 자체를 캡슐화 (실행 + 결과 수집이 하나의 객체에)
- `spawn_blocking` 사용 (Agent::run()이 `!Send` future 생성)
- 상태 추적 없음 — 실행 후 결과만 반환

---

## 3. oxios `OxiosAgentGroup` 기능 전체 (oxios-kernel/src/agent_group.rs)

### 타입
- **`OxiosAgentGroupStatus`** enum: `Pending`, `Running`, `Completed`, `Failed` (Serialize/Deserialize)
- **`OxiosGroupAgent`**: id (Uuid), seed (Seed), status, result
- **`OxiosAgentGroup`**: id (Uuid), parent_seed_id (Uuid), agents: Vec<OxiosGroupAgent> (Serialize/Deserialize)

### 메서드
| 메서드 | 설명 |
|--------|------|
| `new(parent_seed, subtask_descriptions)` | 부모 Seed에서 자식 Seed 분할 생성 |
| `pending_agents()` | Pending 상태 에이전트 필터 |
| `completed_agents()` | Completed 상태 에이전트 필터 |
| `failed_agents()` | Failed 상태 에이전트 필터 |
| `all_completed()` | 전체 완료 여부 |
| `any_failed()` | 실패 존재 여부 |
| `completion_pct()` | 완료율 (0.0~1.0) |
| `combined_results()` | 완료된 에이전트 결과 결합 |

### 특징
- `Seed` (ouroboros) 기반 — oxios의 Ouroboros 프로토콜에 강결합
- **실행 로직이 없음** — 순수 데이터 모델 (상태 + 쿼리)
- `Serialize/Deserialize` 지원 → StateStore에 JSON 저장 가능
- UUID 기반 추적 (parent_seed_id로 계보 추적)
- 실행은 orchestrator의 `delegate_via_lifecycle()`에서 담당

---

## 4. Orchestrator 실제 사용 분석

### orchestrator.rs의 `delegate_via_lifecycle()` (866~969행)

```rust
use crate::agent_group::OxiosAgentGroup;  // ← 자체 타입 사용

let group = OxiosAgentGroup::new(parent_seed, descriptions);
```

orchestrator는 **오직 `OxiosAgentGroup`만 사용**. SDK `AgentGroup`은 import도 하지 않는다.

실행 흐름:
1. `OxiosAgentGroup::new()`로 Seed에서 서브태스크 분할
2. `JoinSet`으로 각 `OxiosGroupAgent`의 Seed를 `lifecycle.spawn_and_run()`에 전달
3. 이벤트 발행: `AgentGroupCreated`, `AgentGroupMemberCompleted`
4. 완료 후 `state_store.save_json("agent_groups", ...)`로 영속화

### `delegate_via_a2a()` (808~862행)
A2A 경로에서는 `OxiosAgentGroup`을 사용하지 않고 직접 `SubTask` 벡터를 처리.

### `delegate_subtasks()` (767~775행)
단일 서브태스크 → 직접 실행 / 복수 서브태스크 → A2A 우선 → fallback으로 `delegate_via_lifecycle()` (OxiosAgentGroup 사용).

---

## 5. Grep 결과 요약

```
# oxios-kernel 전체에서 SdkAgentGroup 실제 사용
→ 0건 (주석에만 언급, import 없음)

# AgentGroup 사용 패턴
→ orchestrator.rs:867: use crate::agent_group::OxiosAgentGroup
→ lib.rs:90: pub use agent_group::{OxiosAgentGroup, OxiosAgentGroupStatus, OxiosGroupAgent}

# oxi-sdk 의존
→ Cargo.toml에 oxi-sdk = { workspace = true } 존재
→ tools/, onboarding.rs, engine.rs, agent_runtime.rs, credential.rs에서 광범위 사용
→ 단 AgentGroup 관련 타입은 단 한 곳도 import하지 않음
```

---

## 6. OxiosAgentGroup에만 있는 기능 (SDK AgentGroup에 없음)

| 기능 | 설명 |
|------|------|
| **Seed 기반 에이전트 생성** | 부모 Seed에서 자식 Seed 자동 분할 (generation, parent_seed_id) |
| **상태 머신** | Pending → Running → Completed/Failed 상태 전환 추적 |
| **직렬화** | `Serialize/Deserialize` → StateStore에 JSON 저장 가능 |
| **UUID 기반 ID** | group id, parent_seed_id, agent id 모두 Uuid |
| **완료율 계산** | `completion_pct()` — 진행 상황 모니터링용 |
| **상태별 필터링** | `pending_agents()`, `completed_agents()`, `failed_agents()` |
| **Seed 계보 추적** | `parent_seed_id`로 부모-자식 관계 파악 |
| **이벤트 버스 연동** | `AgentGroupCreated`, `AgentGroupMemberCompleted` 이벤트 발행 |
| **영속화** | StateStore에 저장, GitLayer로 커밋 |

## 7. SDK AgentGroup에만 있는 기능 (OxiosAgentGroup에 없음)

| 기능 | 설명 |
|------|------|
| **Pipeline 전략** | 순차 실행 (이전 출력이 다음 입력) |
| **Orchestrated 전략** | 리더 에이전트가 작업 분배 |
| **동시성 제한** | Semaphore 기반 max_concurrency |
| **자체 실행** | `run()` 메서드로 직접 에이전트 실행 (별도 실행기 불필요) |
| **타이밍 측정** | `total_duration_ms` |

---

## 8. 통합 가능성 분석

### 두 타입은 **설계 목적이 근본적으로 다름**

| 차원 | SDK AgentGroup | OxiosAgentGroup |
|------|---------------|-----------------|
| 추상화 레벨 | 실행 엔진 | 데이터 모델 |
| 에이전트 표현 | `Arc<Agent>` | `Seed` |
| 책임 | 실행 + 결과 수집 | 상태 추적 + 직렬화 |
| 생명주기 | 일회성 (run → 결과) | 장기 추적 (상태 변경 이벤트) |

### 통합 방안 옵션

#### 옵션 A: SDK AgentGroup에 상태/직렬화 추가 (비권장)
- SDK AgentGroup에 `Serialize/Deserialize`, 상태 머신, UUID 추가
- 문제: SDK는 oxi-agent의 `Agent`에 의존 → oxios의 `Seed`와 호환 불가
- AgentGroup은 실행 책임이 있고, OxiosAgentGroup은 데이터 모델 → 역할 혼재

#### 옵션 B: OxiosAgentGroup을 SDK로 이동 (부분 가능)
- OxiosAgentGroup의 **데이터 모델 부분만** (상태, 직렬화, Seed 분할) SDK에 추가
- `Seed` 타입 의존이 문제 → oxios-ouroboros 크레이트가 SDK에 없음
- 실행 로직은 orchestrator에 유지

#### 옵션 C: 트레이트 기반 추상화 (가장 현실적)
```rust
// oxi-sdk에 추가
pub trait AgentGroupModel: Send + Sync {
    type AgentId;
    type AgentEntry;
    
    fn new_from_descriptions(descriptions: Vec<String>) -> Self;
    fn pending_agents(&self) -> Vec<&Self::AgentEntry>;
    fn completed_agents(&self) -> Vec<&Self::AgentEntry>;
    fn all_completed(&self) -> bool;
    fn completion_pct(&self) -> f64;
    fn combined_results(&self) -> String;
}
```
- OxiosAgentGroup이 이 트레이트를 구현
- SDK AgentGroup도 이 트레이트를 구현
- 공통 기능은 트레이트로, 특화 기능은 각자 유지

#### 옵션 D: 현상 유지 (권장)
- 두 타입은 **다른 계층의 추상화**
- SDK AgentGroup: 범용 멀티에이전트 실행 프리미티브
- OxiosAgentGroup: Ouroboros 프로토콜 특화 상태 모델
- 강제 통합은 양쪽 모두에 불필요한 의존성 유발

### 결론

**현상 유지(옵션 D)가 가장 합리적이다.** 두 타입은 이름이 비슷하지만:
- SDK AgentGroup은 **"실행 전략"** (어떻게 여러 에이전트를 실행할까)
- OxiosAgentGroup은 **"상태 모델"** (여러 에이전트의 진행 상황을 어떻게 추적할까)

이 둘을 합치면 SRP(단일 책임 원칙) 위반이다. oxios의 orchestrator는 OxiosAgentGroup을 데이터 모델로만 사용하고, 실제 실행은 `AgentLifecycleManager`에 위임하는 패턴이 올바른 분리다.
