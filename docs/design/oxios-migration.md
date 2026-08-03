# oxicode-sdk ← oxios-kernel 마이그레이션 설계서

> **목표**: oxios-kernel에 있는 더 강한 구현을 oxicode-sdk로 끌어올려서  
> SDK를 "어디서든 쓸 수 있는 에이전트 인프라"로 만들고,  
> oxios-kernel은 thin wrapper + OS 전용 로직만 남기는 것.
>
> **날짜**: 2026-06-01

---

## 마이그레이션 대상 및 순서

| # | 모듈 | oxios-kernel 경로 | 라인 | oxicode-sdk 교체 대상 |
|---|------|-------------------|------|-------------------|
| 1 | AuditTrail | `src/audit_trail.rs` | 1134 | `observability/audit.rs` (296줄) |
| 2 | EventBus | `src/event_bus.rs` | 595 | `message_bus.rs` (359줄) |
| 3 | Supervisor | `src/supervisor.rs` | 663 | `lifecycle/supervisor.rs` (1060줄) |
| 4 | AccessManager | `src/access_manager/` | 3681 | `security/` (1176줄) |
| 5 | Capability | `src/capability/` | 958 | (Security의 일부로 병합) |

---

## 공통 원칙

### 1. 의존성 분리

oxios-kernel 코드는 `crate::types::AgentId`, `crate::state_store::StateStore` 등에 의존합니다.
마이그레이션 시:

- **AgentId** → `String` 또는 제네릭으로 변경 (oxicode-sdk는 특정 타입 강제 안 함)
- **StateStore** → trait으로 추상화하거나 oxicode-sdk 내부 타입 사용
- **Seed, Phase, Ouroboros** → oxios-kernel에만 두기 (SDK는 모름)

### 2. 소스 경로

```
OXIOS_KERNEL=/Volumes/MERCURY/PROJECTS/oxios/crates/oxios-kernel/src
OXICODE_SDK=/Volumes/MERCURY/PROJECTS/oxicode/oxicode-sdk/src
```

실제 코드를 복사/수정해서 가져옵니다.

---

## Phase 1: AuditTrail → oxicode-sdk

### 현황

| | oxicode-sdk `AuditLog` | oxios-kernel `AuditTrail` |
|---|---|---|
| 보안 | 없음 | **blake3 Merkle hash chain** (변조 감지) |
| 영속화 | 없음 | **StateStore** 연동 (JSON 저장/복원) |
| 쿼리 | agent_id, entry_type, after_ms | **seq 범위, by_agent, by_action, by_action_type** |
| 내보내기 | 없음 | **export_json, export_all_json** |
| 검증 | 없음 | **verify() — 체인 무결성 확인** |

### 작업

**1-1. 파일 복사**

```
$OXIOS_KERNEL/audit_trail.rs → $OXICODE_SDK/observability/audit_trail.rs
```

**1-2. 의존성 제거**

```rust
// 변경 전 (oxios-kernel)
use crate::state_store::StateStore;

impl StateStore {
    pub fn save_audit_entries(&self, entries: &[AuditEntry]) -> Result<()> { ... }
    pub fn load_audit_entries(&self) -> Result<Vec<AuditEntry>> { ... }
}
```

```rust
// 변경 후 (oxicode-sdk)
// StateStore 의존 제거 → trait으로 추상화
pub trait AuditPersistence: Send + Sync {
    fn save(&self, entries: &[AuditEntry]) -> anyhow::Result<()>;
    fn load(&self) -> anyhow::Result<Vec<AuditEntry>>;
}

// AuditTrail에서 직접 flush/restore 대신 외부에서 호출
impl AuditTrail {
    pub fn flush_to(&self, store: &dyn AuditPersistence) -> anyhow::Result<()> {
        let entries = self.all_entries();
        store.save(&entries)
    }
    
    pub fn restore_from_store(&self, store: &dyn AuditPersistence) -> anyhow::Result<()> {
        let entries = store.load()?;
        self.restore_from(entries);
        Ok(())
    }
}
```

**1-3. AgentId → String**

```rust
// 변경 전
pub type AgentId = String;  // oxios-kernel에서도 String이었음
```

변경 불필요. 이미 `String`.

**1-4. Cargo.toml 의존성 추가**

```toml
# oxicode-sdk/Cargo.toml
[dependencies]
blake3 = "1"          # AuditTrail 해시 체인
chrono = "0.4"        # 타임스탬프 (이미 있을 수 있음)
```

**1-5. 기존 AuditLog 삭제, AuditTrail로 교체**

```
$OXICODE_SDK/observability/audit.rs  → 삭제
$OXICODE_SDK/observability/audit_trail.rs → 새 파일 (oxios에서 가져옴)
```

export 이름 변경:
```rust
// oxicode-sdk observability/mod.rs
pub use audit_trail::{AuditAction, AuditEntry, AuditError, AuditTrail, HashDigest};
```

**1-6. oxios-kernel 변경**

```rust
// oxios-kernel/src/audit_trail.rs → 전체 삭제
// 대신 oxicode_sdk에서 re-export
pub use oxicode_sdk::{AuditAction, AuditEntry, AuditTrail, ...};

// StateStore 연동은 oxios-kernel에만 남김
impl oxicode_sdk::AuditPersistence for StateStore { ... }
```

### 새 Cargo 의존성

`blake3` (audit_trail), `chrono` (타임스탬프)

---

## Phase 2: EventBus → oxicode-sdk

### 현황

| | oxicode-sdk `MessageBus` | oxios-kernel `EventBus` |
|---|---|---|
| 메시지 타입 | `InterAgentMessage` 1개 | **20개 `KernelEvent` variant** |
| Audit 연동 | 없음 | **attach_audit_trail()** |
| 변환 | 없음 | **kernel_event_to_audit_action()** |

### 문제: KernelEvent에 oxios 전용 variant가 섞여 있음

```rust
pub enum KernelEvent {
    // 범용 (oxicode-sdk에 와야 함)
    AgentCreated { id, name },
    AgentStarted { id },
    AgentStopped { id },
    AgentFailed { id, error },
    MessageReceived { from, content },
    AgentOutput { session_id, agent_id, output },
    ApprovalRequested { id, action, resource, reason },
    ApprovalResolved { id, approved },

    // oxios 전용 (커널에만)
    SeedCreated { seed_id },              // ouroboros
    EvaluationComplete { seed_id, passed }, // ouroboros
    PhaseStarted { session_id, phase },    // ouroboros
    PhaseCompleted { ... },                // ouroboros
    EvolutionStarted { ... },              // ouroboros
    EvolutionMaxReached { ... },           // ouroboros
    AgentGroupCreated { ... },             // oxios 그룹
    AgentGroupMemberCompleted { ... },     // oxios 그룹
    ProjectCreated { ... },                // oxios 프로젝트
    ProjectActivated { ... },              // oxios 프로젝트
    MemoryStored { ... },                  // oxios 메모리
    MemoryRecalled { ... },                // oxios 메모리
}
```

### 작업

**2-1. oxicode-sdk에 제네릭 EventBus 추가**

```rust
// oxicode-sdk/src/event_bus.rs

/// 제네릭 이벤트 버스 — 어떤 이벤트 타입이든 가능
pub struct EventBus<E: Clone + Send + 'static> {
    tx: broadcast::Sender<E>,
}

impl<E: Clone + Send + 'static> EventBus<E> {
    pub fn new(capacity: usize) -> Self { ... }
    pub fn subscribe(&self) -> broadcast::Receiver<E> { ... }
    pub fn publish(&self, event: E) -> anyhow::Result<()> { ... }
}
```

**2-2. oxios-kernel에 KernelEvent 정의 유지**

```rust
// oxios-kernel/src/event_bus.rs
// EventBus는 oxicode_sdk::EventBus<KernelEvent>의 type alias로 변경

pub type KernelEventBus = oxicode_sdk::EventBus<KernelEvent>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KernelEvent { ... }  // 20개 variant 그대로
```

**2-3. audit 연동은 oxios-kernel에서**

```rust
// oxios-kernel에서
let bus: EventBus<KernelEvent> = EventBus::new(256);
bus.attach_audit_trail(audit_trail); // 이 로직은 커널에만 있음
```

**2-4. 기존 MessageBus 삭제**

```
$OXICODE_SDK/message_bus.rs → 삭제
```

`MessageBus`를 쓰는 곳이 없으므로 (oxios 안 씀, oxicode-cli 안 씀) 바로 삭제 가능.

### 새 의존성

없음 (broadcast는 이미 사용 중)

---

## Phase 3: Supervisor → oxicode-sdk

### 현황

| | oxicode-sdk `AgentSupervisor` | oxios-kernel `Supervisor` + `AgentPool` |
|---|---|---|
| 핵심 | AgentHandle 상태머신 | **trait Supervisor + AgentPool** |
| 상태 관리 | 스냅샷 | **export/import state (JSON)** |
| 풀 | 없음 | **AgentPool (HashMap<AgentId, Arc<Agent>>)** |
| 이벤트 | lifecycle events | **EventBus 연동** |
| 의존성 | 독립적 | AgentRuntime, EventBus, ResourceMonitor에 의존 |

### 작업

**3-1. AgentPool을 oxicode-sdk로 가져오기**

```rust
// oxicode-sdk/src/lifecycle/agent_pool.rs (새 파일)
// oxios-kernel/src/supervisor.rs의 AgentPool 구조체 복사
// 의존성: AgentId → String으로 변경

pub struct AgentPool {
    agents: RwLock<HashMap<String, Arc<Agent>>>,
}

impl AgentPool {
    pub fn new() -> Self { ... }
    pub fn insert(&self, id: String, agent: Arc<Agent>) { ... }
    pub fn get(&self, id: &str) -> Option<Arc<Agent>> { ... }
    pub fn remove(&self, id: &str) -> Option<Arc<Agent>> { ... }
    pub fn export_state(&self, id: &str) -> Option<serde_json::Value> { ... }
    pub fn import_state(&self, id: &str, state: serde_json::Value) -> bool { ... }
}
```

의존성: `oxicode_agent::Agent`만 (이미 oxicode-sdk에 있음). 외부 크레이트 의존 없음.

**3-2. Supervisor trait을 oxicode-sdk로 가져오기**

```rust
// oxicode-sdk/src/lifecycle/supervisor.rs (재작성)
// oxios-kernel의 Supervisor trait + BasicSupervisor 가져오되,
// Seed, AgentRuntime 의존 제거

#[async_trait]
pub trait Supervisor: Send + Sync {
    async fn fork(&self, name: &str) -> anyhow::Result<String>;
    async fn exec(&self, id: &str) -> anyhow::Result<()>;
    async fn wait(&self, id: &str) -> anyhow::Result<AgentStatus>;
    async fn kill(&self, id: &str) -> anyhow::Result<()>;
    fn list(&self) -> anyhow::Result<Vec<AgentInfo>>;
    fn pool(&self) -> &AgentPool;
}
```

**3-3. BasicSupervisor 간소화**

oxios-kernel의 BasicSupervisor는 `AgentRuntime`, `EventBus`, `ResourceMonitor`에 의존.
oxicode-sdk 버전에서는:

```rust
// oxicode-sdk: 코어 Supervisor. 실행 로직은 외부에서 주입
pub struct BasicSupervisor {
    agents: RwLock<HashMap<String, AgentInfo>>,
    handles: RwLock<HashMap<String, AgentHandle>>,
    pool: AgentPool,
}

// 실행 로직은 트레이트로 분리
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn execute(&self, agent_id: &str, prompt: String) -> anyhow::Result<String>;
}
```

oxios-kernel의 `BasicSupervisor`는 `AgentExecutor`를 구현해서 주입.

**3-4. oxios-kernel 변경**

```rust
// oxios-kernel/src/supervisor.rs → 삭제
// 대신 oxicode_sdk::Supervisor trait 구현

pub struct OxiosSupervisor {
    inner: oxicode_sdk::BasicSupervisor,
    runtime: Arc<AgentRuntime>,
    event_bus: EventBus<KernelEvent>,
    resource_monitor: Option<Arc<ResourceMonitor>>,
}

impl oxicode_sdk::Supervisor for OxiosSupervisor { ... }
```

### 삭제 대상 (oxicode-sdk)

- `lifecycle/supervisor.rs` 기존 1060줄 → 재작성 (약 400줄로 간소화)
- `lifecycle/snapshot.rs` → AgentPool의 export/import로 대체 가능하면 삭제

---

## Phase 4: AccessManager + Capability → oxicode-sdk

### 현황

| | oxicode-sdk `security/` | oxios-kernel `access_manager/` + `capability/` |
|---|---|---|
| 권한 모델 | 16개 Capability enum | **seL4 Capability (Rights 비트플래그)** |
| RBAC | Authorizer (3단계) | **RbacManager (Role/Policy/Approval)** |
| 샌드박싱 | 없음 | **PathMode, workspace 격리, glob 패턴** |
| 접근 제어 | SecurityMiddleware | **AccessGate (4-layer: CSpace→RBAC→Perms→ExecConfig)** |
| 감사 연동 | 없음 | **AuditSink trait → AuditTrail** |
| 총 라인 | 1176 | **4639** |

### 작업

**4-1. capability/ 전체 복사**

```
$OXIOS_KERNEL/capability/ → $OXICODE_SDK/security/capability/
  types.rs     (426줄) — Capability, CSpace, Rights, ResourceRef, Issuer
  template.rs  (312줄) — CapabilityTemplate (worker/standard/operator/supervisor)
  resolve.rs   (193줄) — resolve_cspace()
  mod.rs       (27줄)
```

의존성: `crate::types::AgentId` → `String`으로 변경. 외부 의존 없음.

**4-2. permissions.rs 복사**

```
$OXIOS_KERNEL/access_manager/permissions.rs → $OXICODE_SDK/security/permissions.rs
```

의존성: 없음 (chrono, glob, serde만). 깔끔하게 복사 가능.

**4-3. rbac.rs 복사**

```
$OXIOS_KERNEL/access_manager/rbac.rs → $OXICODE_SDK/security/rbac.rs
```

의존성: `crate::types::AgentId` → `String`. chrono, uuid만 추가.

**4-4. gate.rs 복사**

```
$OXIOS_KERNEL/access_manager/gate.rs → $OXICODE_SDK/security/gate.rs
```

의존성:
- `crate::access_manager::*` → 같은 security 모듈 내 참조로 변경
- `crate::capability::*` → `super::capability::*`
- `crate::config::ExecConfig` → oxicode-sdk에 `ExecPolicy` 구조체 새로 정의

**4-5. context.rs 복사**

```
$OXIOS_KERNEL/access_manager/context.rs → $OXICODE_SDK/security/context.rs
```

의존성: `crate::capability::CSpace` → `super::capability::CSpace`

**4-6. audit_sink.rs 복사**

```
$OXIOS_KERNEL/access_manager/audit_sink.rs → $OXICODE_SDK/security/audit_sink.rs
```

의존성: `crate::audit_trail::*` → `crate::observability::*`

**4-7. 기존 security/ 삭제 후 재구성**

```
$OXICODE_SDK/security/
  mod.rs          (새로 작성)
  capability/     (oxios에서 가져옴)
    mod.rs
    types.rs
    template.rs
    resolve.rs
  permissions.rs  (oxios에서 가져옴)
  rbac.rs         (oxios에서 가져옴)
  gate.rs         (oxios에서 가져옴)
  context.rs      (oxios에서 가져옴)
  audit_sink.rs   (oxios에서 가져옴)
```

**삭제**: 기존 `authorizer.rs`, `capability.rs`, `middleware.rs`

### 새 Cargo 의존성

`glob` (permissions 경로 패턴 매칭), `uuid` (CapabilityId)

---

## Phase 5: 안 쓰는 모듈 정리

Phase 1-4 완료 후:

### 삭제

| 파일 | 이유 |
|------|------|
| `coordination/shared_memory.rs` | oxios에서 안 씀, 인메모리만 있음 |
| `coordination/work_queue.rs` | oxios에서 안 씀 |
| `coordination/consensus.rs` | oxios에서 안 씀 |
| `coordination/group_ext.rs` | oxios에서 안 씀 (자체 OxiosAgentGroup 있음) |
| `lifecycle/snapshot.rs` | AgentPool의 export/import로 대체 |
| `middleware/builtins.rs` | oxios에서 안 씀 |
| `middleware/bridge.rs` | oxios에서 안 씀 |

### 유지

| 파일 | 이유 |
|------|------|
| `agent_builder.rs` | oxios에서 AgentBuilder 사용 |
| `agent_group.rs` | Pipeline/Parallel은 범용적 |
| `builder.rs` | OxicodeBuilder — oxios가 사용 |
| `closure_tool.rs` | oxios에서 사용 |
| `kernel_bridge.rs` | trait — oxios에서 구현 |
| `message_bus.rs` → `event_bus.rs` | 제네릭으로 교체 |
| `multi_provider.rs` | oxios에서 사용 |
| `routing.rs` | oxios에서 사용 |
| `workflow_dsl.rs` | 파서 — 나중에 실행 엔진 추가 가능 |
| `observability/cost.rs` | oxios에서 사용 |
| `observability/trace.rs` | oxios에서 사용 |

---

## 예상 라인 수 변화

| | 현재 | 마이그레이션 후 | 변화 |
|---|---|---|---|
| oxicode-sdk 전체 | 10,274 | ~14,000 | +3,700 (oxios 코드 흡수) |
| oxios-kernel | ~25,000 | ~18,000 | -7,000 (SDK로 이관) |
| **중복 코드** | ~5,000 | **0** | 제거 |

---

## 구현 순서 및 의존성

```
Phase 1: AuditTrail (독립적, 다른 것에 의존 안 함)
   ↓
Phase 2: EventBus (독립적)
   ↓
Phase 3: Supervisor (AgentPool만 있으면 됨)
   ↓
Phase 4: AccessManager + Capability (AuditTrail 필요 → Phase 1 선행)
   ↓
Phase 5: 안 쓰는 모듈 삭제 (전체 완료 후)
```

각 Phase는 독립적으로 PR 가능.
