# oxi-sdk Dormant 모듈 활성화 요청 — 2차

> **요청자**: oxios 팀
> **날짜**: 2026-06-03
> **관련**: 1차 요청서 (`docs/design/sdk-dormant-modules-activation.md`)
> **현황**: 0.26.1에 반영 안 됨. 모든 dormant 파일이 여전히 mod 등록 누락 상태.

---

## 현재 상태

0.26.1에서 파일은 모두 존재하지만 **단 하나도 mod.rs에 등록되지 않았다.**
oxios 측에서는 0.26.1로 업그레이드를 완료했고 테스트도 전부 통과하지만,
dormant 모듈을 사용할 수 없어 Phase C 이후 진행이 막혀 있다.

---

## 해야 할 일 (우선순위 순)

### 🔴 Step 1. Cargo.toml에 의존 추가

```toml
# oxi-sdk/Cargo.toml [dependencies]에 추가
blake3 = "1"
chrono = { version = "0.4", features = ["serde"] }
glob = "0.3"
```

- `blake3`: `audit_trail.rs`가 해시 체인에 사용
- `chrono`: `audit_trail.rs`, `rbac.rs`, `audit_sink.rs`가 타임스탬프에 사용
- `glob`: `permissions.rs`가 경로 패턴 매칭에 사용

> 참고: `chrono`는 이미 `oxi-ai`에 있고 `glob`은 이미 `oxi-agent`에 있으므로
> 실제 새 다운로드는 `blake3` 하나뿐이다.

### 🔴 Step 2. EventBus 활성화

```diff
  // src/lib.rs
+ pub mod event_bus;
  pub mod agent_builder;
  ...
```

```diff
  // src/lib.rs re-exports 섹션에 추가
+ pub use event_bus::EventBus;
```

### 🔴 Step 3. AgentPool 활성화 + stub 실구현

```diff
  // src/lifecycle/mod.rs
  mod snapshot;
  mod supervisor;
+ mod agent_pool;

- pub use snapshot::{...};
- pub use supervisor::{...};
+ pub use agent_pool::AgentPool;
+ pub use snapshot::{...};
+ pub use supervisor::{...};
```

그리고 `agent_pool.rs`의 stub을 실 구현으로 교체:

```rust
// 변경 전 (stub)
pub fn export_state(&self, id: &str) -> Option<serde_json::Value> {
    let agents = self.agents.read();
    let _agent = agents.get(id)?;
    Some(serde_json::json!({ "agent_id": id }))
}

pub fn import_state(&self, _id: &str, _state: serde_json::Value) -> bool {
    let agents = self.agents.read();
    !agents.is_empty()
}

// 변경 후 (실구현 — Agent에 이미 export_state/import_state가 있음)
pub fn export_state(&self, id: &str) -> Option<serde_json::Value> {
    let agents = self.agents.read();
    let agent = agents.get(id)?;
    agent.export_state().ok()
}

pub fn import_state(&self, id: &str, state: serde_json::Value) -> bool {
    let agents = self.agents.read();
    if let Some(agent) = agents.get(id) {
        agent.import_state(state).is_ok()
    } else {
        false
    }
}
```

### 🔴 Step 4. AuditTrail 활성화

```diff
  // src/observability/mod.rs
  mod audit;
+ mod audit_trail;
  mod cost;
  mod event_store;
  mod trace;

  pub use audit::{AuditEntry, AuditFilter, AuditLog};
+ pub use audit_trail::{
+     AuditAction, AuditError, AuditPersistence, AuditTrail, AgentId as AuditAgentId,
+     HashDigest, TrailEntry,
+ };
  pub use cost::{...};
```

### 🔴 Step 5. Capability 서브모듈 생성

현재 `security/capability.rs`는 flat 파일이다.
`gate.rs`와 `context.rs`가 다음 경로를 참조한다:

```rust
// gate.rs
use crate::security::capability::types::{ResourceRef, Rights};

// context.rs
use crate::security::capability::types::CSpace;
use crate::security::capability::resolve::resolve_cspace;
```

이 경로가 작동하려면 `capability/` 디렉토리 구조가 필요하다.

**해결 방안**: `capability.rs`를 `capability/` 디렉토리로 전환:

```
src/security/capability/
├── mod.rs         — pub mod types; mod resolve; + 기존 capability.rs의
                     Capability, CapabilitySet, StringPattern 등을
                     mod.rs에 그대로 유지하거나 types.rs로 이동
├── types.rs       — CSpace, Rights, ResourceRef, Issuer 등 seL4 스타일 타입
└── resolve.rs     — resolve_cspace() 함수
```

이 타입들은 oxios-kernel의 `crates/oxios-kernel/src/capability/`에서 가져오면 된다:

| oxios 원본 | SDK 대상 | 변경점 |
|-----------|---------|--------|
| `capability/types.rs` (426줄) | `security/capability/types.rs` | `AgentId`(Uuid) → `String` |
| `capability/resolve.rs` (193줄) | `security/capability/resolve.rs` | `AgentId`(Uuid) → `String` |
| `capability/template.rs` (312줄) | `security/capability/template.rs` | 선택사항, `context.rs`가 사용 |

### 🟡 Step 6. Security dormant 파일 활성화

Step 1, 4, 5가 완료된 후:

```diff
  // src/security/mod.rs
  mod authorizer;
- mod capability;
+ pub mod capability;  // 디렉토리로 전환됨
  pub mod middleware;
+ mod audit_sink;
+ mod context;
+ mod exec_policy;
+ mod gate;
+ mod permissions;
+ mod rbac;

  pub use authorizer::{Authorizer, DefaultPolicy};
- pub use capability::{Capability, CapabilitySet, CapabilitySubject, StringPattern};
+ pub use capability::{Capability, CapabilitySet, CapabilitySubject, StringPattern};
+ pub use audit_sink::{AuditEvent, AuditSink, TrailAuditSink, TracingAuditSink};
+ pub use context::AgentContext;
+ pub use exec_policy::{AllowlistMode, ExecPolicy};
+ pub use gate::{AccessDenied, AccessGate, CheckRequest, DenyLayer, PathMode};
+ pub use permissions::{AgentPermissions, PermAuditEntry, PermissionUpdate};
+ pub use rbac::{
+     Action, ApprovalStatus, PendingApproval, RbacAuditEntry, RbacManager, RbacPolicy,
+     Role, Subject,
+ };
  pub use middleware::SecurityMiddleware;
```

### 🟢 Step 7. lib.rs re-export 업데이트

```diff
  // src/lib.rs — re-export 섹션에 추가

+ // Composition Layer — EventBus
+ pub use event_bus::EventBus;

  // Lifecycle
  pub use lifecycle::{
      AgentHandle, AgentLifecycleEvent, AgentSnapshot, AgentStatus, AgentSupervisor,
      FileSnapshotStore, RestartBackoff, SnapshotStore, SupervisorPolicy, ToolManifest,
+     AgentPool,
  };

  // Observability
  pub use observability::{
      AuditEntry, AuditFilter, AuditLog,
+     AuditAction, AuditError, AuditPersistence, AuditTrail, HashDigest, TrailEntry,
      CostBreakdown, CostSnapshot, CostTracker, CostTrackerConfig, ...
  };

  // Security
  pub use security::{
      Authorizer, Capability, CapabilitySet, CapabilitySubject, DefaultPolicy,
      SecurityMiddleware, StringPattern,
+     AccessDenied, AccessGate, AgentContext, AllowlistMode, AuditEvent, AuditSink,
+     CheckRequest, DenyLayer, ExecPolicy, PathMode, TrailAuditSink, TracingAuditSink,
+     AgentPermissions, PermissionUpdate, PermAuditEntry,
+     Action, ApprovalStatus, PendingApproval, RbacManager, RbacPolicy, Role, Subject,
  };
```

---

## Step 간 의존성

```
Step 1 (Cargo.toml) ──────────────────────────────────────┐
  │                                                        │
  ├── Step 2 (EventBus) ← 독립, 즉시                       │
  │                                                        │
  ├── Step 3 (AgentPool) ← 독립, 즉시                      │
  │                                                        │
  ├── Step 4 (AuditTrail) ← Step 1 (blake3, chrono)        │
  │                                                        │
  ├── Step 5 (Capability 서브모듈) ← oxios 코드 마이그레이션 │
  │    │                                                   │
  │    ├── Step 6 (Security 활성화)                         │
  │    │    ← Step 1 (glob, chrono)                        │
  │    │    ← Step 4 (AuditTrail for audit_sink)            │
  │    │    ← Step 5 (Capability for gate, context)         │
  │    │                                                   │
  │    └── Step 7 (lib.rs re-export) ← Step 2~6 완료 후     │
  │                                                        │
  └── cargo test 통과 ← Step 7 완료 후                      │
```

---

## 우리가 바로 할 수 있는 것 (Step 2, 3만 완료되면)

oxios는 EventBus와 AgentPool만 활성화되어도 Phase C, E를 진행할 수 있다:

| oxi Step | oxios Phase | 내용 |
|----------|------------|------|
| Step 2 (EventBus) | Phase C | event_bus.rs 간소화 (~475줄 절감) |
| Step 3 (AgentPool) | Phase E | supervisor의 AgentPool을 SDK에서 가져오기 |

나머지 Phase는 전체 활성화 후 진행:

| oxi Step | oxios Phase | 내용 |
|----------|------------|------|
| Step 4 (AuditTrail) | Phase F | audit_trail.rs 중복 제거 (~1134줄 절감) |
| Step 6 (Security) | Phase D | AgentBuilder에 capabilities/authorizer 통합 |

---

## 최소 요청 (빠른 진행을 위해)

시간이 부족하다면 **Step 1 + Step 2 + Step 3**만 먼저 해줘도 oxios Phase A~E를 진행할 수 있다.
나머지는 다음 버전(0.26.2)에서 해도 된다.

```
Step 1: Cargo.toml에 blake3, chrono, glob 추가        (1분)
Step 2: lib.rs에 "pub mod event_bus;" 추가             (1분)
Step 3: lifecycle/mod.rs에 "mod agent_pool;" 추가       (1분)
        + agent_pool.rs의 export_state/import_state 실구현 (5분)
```

총 8분이면 oxios의 Phase A~E가 Unblock된다.
