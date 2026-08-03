# Legacy Cleanup & Incomplete Features Audit

**Date**: 2026-06-01
**Scope**: oxicode + oxios 전체
**Purpose**: 미완성 기능, 레거시 코드, 과도한 복잡성, dead code를 체계적으로 파악하고 우선순위를 정한다.

---

## Table of Contents

1. [Incomplete Features — 연결만 하면 작동](#1-incomplete-features--연결만-하면-작동)
2. [Incomplete Features — 로직이 없음](#2-incomplete-features--로직이-없음)
3. [Skeleton Features — 인프라만 있고 실제 구현이 없음](#3-skeleton-features--인프라만-있고-실제-구현이-없음)
4. [Mislayered Dependency — oxicode-cli의 불필요한 oxicode-sdk 의존](#4-mislayered-dependency--oxicode-cli의-불필요한-oxicode-sdk-의존)
5. [Redundant Re-export Layer — oxios-kernel lib.rs](#5-redundant-re-export-layer--oxios-kernel-librs)
6. [Dead Code — 호출되지 않는 함수/타입](#6-dead-code--호출되지-않는-함수타입)
7. [Summary Table](#7-summary-table)

---

## 1. Incomplete Features — 연결만 하면 작동

### 1.1 TUI 세션 브랜치 전환

**Location**: `oxicode-cli/src/tui/handlers.rs:987`
**Priority**: 🔴 High (사용자가 UI에서 선택해도 아무 일이 안 일어남)
**Effort**: Small (~20줄)

**Current State**:
- `SessionNavigator` (1451줄)이 `oxicode-store/src/session_navigation.rs`에 **완전히 구현**되어 있음 — 트리 순회, 브랜치 찾기, 부모-자식 관리, 브랜치 생성 전부 작동.
- 트리 오버레이 UI도 `tui/overlay/tree_navigator.rs`에 구현되어 있음.
- 그러나 `OverlayAction::NavigateToEntry` 핸들러에서 실제 브랜치 전환을 하지 않고 notification만 표시:

```rust
OverlayAction::NavigateToEntry { entry_id } => {
    state.overlay_state = None;
    // TODO: integrate with SessionNavigator::navigate_tree() for branch switching
    state.add_notification(
        format!("Selected entry: {}", &entry_id[..8.min(entry_id.len())]),
        NotificationKind::Info,
    );
    return None;
}
```

**Target State**:
- 사용자가 트리에서 항목을 선택하면 해당 entry로 세션이 전환됨.
- `SessionNavigator::navigate_tree()`를 호출하여 target entry를 찾고, session manager를 통해 branch switch를 수행.

**Implementation Steps**:
1. `TuiState`에 `SessionNavigator` 인스턴스 또는 접근 수단 추가.
2. `NavigateToEntry` 핸들러에서 `navigator.navigate_to(&entry_id)` 호출.
3. Session manager에게 브랜치 전환 요청.
4. TUI가 전환된 브랜치의 메시지를 다시 렌더링.

---

### 1.2 oxios-web Chat API tool_calls 누락

**Location**: `oxios/surface/oxios-web/src/routes/chat.rs`
**Priority**: 🟡 Medium
**Effort**: Medium

**Current State**:
```rust
// TODO: populate tool_calls from trajectory_steps once kernel provides them
```

Chat API 응답에 `tool_calls` 필드가 비어있음. 커널이 trajectory_steps를 제공하면 연결해야 함.

**Target State**: API 응답에 tool 호출 이력이 포함됨.

**Blocker**: 커널쪽에서 trajectory_steps 데이터를 API로 노출해야 함.

---

### 1.3 oxios-web A2A 메시지 로깅

**Location**: `oxios/surface/oxios-web/src/routes/a2a.rs`
**Priority**: 🟢 Low
**Effort**: Small

**Current State**:
```rust
// TODO: Implement message logging in kernel A2AProtocol
```

에이전트 간 메시지가 로깅되지 않음.

---

### 1.4 oxios-cli 모델/페르소나 전환

**Location**: `oxios/channels/oxios-cli/src/interactive.rs`
**Priority**: 🟡 Medium
**Effort**: Medium

**Current State**:
```rust
// TODO: wire to kernel model switching
// TODO: wire to kernel persona switching
```

CLI에 명령어는 있지만 실제 커널 API로 연결이 안 됨.

**Blocker**: 커널의 model switching / persona switching API 필요.

---

## 2. Incomplete Features — 로직이 없음

### 2.1 Setup 위저드 OAuth 플로우

**Location**: `oxicode-cli/src/tui/handlers.rs:1150`
**Priority**: 🟡 Medium
**Effort**: Large

**Current State**:
Setup 오버레이에 "API Key"와 "OAuth" 두 선택지가 표시됨. OAuth를 선택하면:

```rust
1 => {
    // OAuth — not yet implemented, just go to provider select
    let providers = build_provider_list(is_config);
    state.overlay = wrap_step(
        &state.overlay,
        SetupStep::SelectProvider { ... },
    );
}
```

아무 OAuth 플로우도 실행하지 않고 바로 provider 선택 화면으로 넘어감. **사용자에게 작동하는 것처럼 보이지만 실제로는 무시됨.**

이미 존재하는 인프라:
- `AuthCredential::OAuth` 타입 (`oxicode-store/src/auth_storage.rs`)
- 토큰 갱신 로직, `save_token`, `load_token`
- `oxicode-ai/src/oauth.rs`의 OAuth 헬퍼

없는 것:
- 브라우저 리다이렉트 → 콜백 → 토큰 교환 플로우
- TUI에서의 OAuth 인증 UX

**Target State**:
- OAuth 선택 시 브라우저를 열어 인증 페이지로 리다이렉트.
- 콜백을 받아 토큰을 저장.
- 저장된 토큰으로 provider 생성.

**Options**:
- A) OAuth 선택지를 당분간 숨기고 "API Key"만 표시.
- B) 로컬 서버를 띄워 콜백을 받는 표준 OAuth 플로우 구현.

---

## 3. Skeleton Features — 인프라만 있고 실제 구현이 없음

### 3.1 WorkerManager 12개 워커

**Location**: `oxios/crates/oxios-kernel/src/workers/mod.rs` (~400줄)
**Priority**: 🟢 Low (실험적 기능)
**Status**: Skeleton — 인프라 완성, 실제 로직 없음

**Current State**:
12개 워커 타입이 정의되어 있고 dispatch/status/enable/disable 로직이 완전히 구현됨. 그러나 실제 워커 실행은 **전부 하드코딩된 문자열 반환**:

```rust
fn execute_worker(&self, worker_type: WorkerType) -> Result<String, String> {
    match worker_type {
        WorkerType::Audit => Ok("Security scan complete: no vulnerabilities found".to_string()),
        WorkerType::Optimize => Ok("Performance analysis complete: 3 optimization opportunities identified".to_string()),
        WorkerType::Ultralearn => Ok("Deep knowledge acquisition: 5 new patterns processed".to_string()),
        // ... 전부 가짜 출력
    }
}
```

**Workers 목록**:
| Worker | Purpose | 실제 구현 |
|--------|---------|----------|
| Ultralearn | Deep knowledge acquisition | ❌ 가짜 |
| Audit | Security analysis | ❌ 가짜 |
| Optimize | Performance optimization | ❌ 가짜 |
| Consolidate | Memory consolidation | ❌ 가짜 |
| Predict | Predictive preloading | ❌ 가짜 |
| Map | Codebase mapping | ❌ 가짜 |
| Deepdive | Deep code analysis | ❌ 가짜 |
| Document | Auto-documentation | ❌ 가짜 |
| Refactor | Refactoring suggestions | ❌ 가짜 |
| Benchmark | Performance benchmarking | ❌ 가짜 |
| Testgaps | Test coverage analysis | ❌ 가짜 |
| Learning | Neural pattern training | ❌ 가짜 |

**Target State**: 각 워커가 실제 서브시스템(ReasoningBank, SONA, embedding engine 등)과 연결되어 의미 있는 작업을 수행.

**Note**: 이것은 "나중에 구현할 예정"인 실험적 기능으로 보임. 당장 제거할 필요는 없지만, API consumer에게 이것이 실제 작동하는 기능이 아님을 명확히 해야 함.

---

### 3.2 WasmSandbox (feature off 시)

**Location**: `oxios/crates/oxios-kernel/src/wasm_sandbox.rs` (385줄)
**Priority**: 🟢 Low
**Status**: Feature-gated skeleton

**Current State**:
- `#[cfg(feature = "wasm-sandbox")]`: wasmtime 기반 실제 구현 (385줄)
- `#[cfg(not(feature = "wasm-sandbox"))]`: 모든 메서드가 실패/빈 값을 반환하는 stub

이 feature가 활성화된 적이 있는지 불명확. 비활성화 시 에러 타입/설정 구조체만 100줄의 dead weight.

**Options**:
- A) 실험적 기능으로 유지 (현재 상태).
- B) `wasm-sandbox` feature가 활성화되지 않으면 모듈 전체를 `#[cfg]`로 감싸서 컴파일에서 제외.

---

### 3.3 Memory Compaction 5단계 계층

**Location**: `oxios/crates/oxios-kernel/src/memory/compaction.rs`
**Priority**: 🟢 Low
**Status**: 설계만 됨

**Current State**:
`CompactionLevel` enum으로 5단계 압축 계층을 정의:

```
Raw → Daily → Weekly → Monthly → Root
```

enum과 4개 메서드(`threshold`, `dir_name`, `all`, `next`)가 `#[allow(dead_code)]`로 표시됨. `CompactionTree` 자체는 `dream.rs`에서 사용되지만, **enum 메서드들은 아무데서도 호출 안 됨.** 실제 압축은 `rule_based_compact()`라는 first/last 줄만 보존하는 단순 로직.

**Target State**: 각 레벨이 실제로 압축을 수행하고 상위 레벨로 승격.

---

### 3.4 Orchestrator 세션 복원

**Location**: `oxios/crates/oxios-kernel/src/orchestrator.rs`
**Priority**: 🟡 Medium
**Status**: 빈 함수

```rust
/// Restore sessions from persisted state (stub).
/// TODO: Implement session persistence restoration.
pub async fn restore_sessions(&self) -> Result<()> {
    // Stub — not yet implemented
    Ok(())
}
```

공개 API로 노출되어 있지만 아무것도 안 함.

---

## 4. Mislayered Dependency — oxicode-cli의 불필요한 oxicode-sdk 의존

**Location**: `oxicode-cli/Cargo.toml`, `oxicode-cli/src/lib.rs`
**Priority**: 🔴 High (빌드 의존성 트리 대폭 감소)
**Effort**: Small (~30줄 변경)

### Current State

`oxicode-cli/Cargo.toml`:
```toml
oxicode-sdk = { version = "0.25.5", path = "../oxicode-sdk" }
```

`oxicode-cli/src/lib.rs` — `App::new()`:
```rust
let engine = OxicodeBuilder::new().with_builtins().build();
let _ = engine.resolve_model(...);           // 결과 버림
let provider = engine.create_provider(...)?; // 실제 사용
// engine은 App 필드로 저장되지만 다시는 쓰이지 않음
```

`engine()` 접근자도 있지만 **프로젝트 전체에서 한 번도 호출되지 않음** (`#[allow(dead_code)]`).

### Problem

`create_provider()` 한 번을 위해 oxicode-sdk 전체를 빌드 의존성으로 끌어옴. oxicode-sdk는 oxicode-ai + oxicode-agent + oxicode-store + security/middleware/observability/workflow_dsl(6500줄)을 포함. oxicode-cli는 실제로 oxicode-ai, oxicode-agent, oxicode-store를 **직접 import**해서 사용하고 있음:

```rust
use oxicode_agent::{Agent, AgentConfig, AgentEvent};
use oxicode_ai::{Model, Api, ...};
use oxicode_store::settings::Settings;
```

oxicode-sdk를 통한 간접 접근이 아니라 처음부터 직접 사용 중.

### Why This Happened

초기에 oxicode-cli를 oxicode-sdk 기반으로 마이그레이션하려 했으나, 실제로는 oxicode-sdk의 기능(엔진 빌더) 하나만 쓰고 나머지는 여전히 하위 크레이트를 직접 사용.

### Target State

```rust
// Before:
use oxicode_sdk::OxicodeBuilder;
let engine = OxicodeBuilder::new().with_builtins().build();
let provider = engine.create_provider(&provider_name)?;

// After:
use oxicode_ai::{create_builtin_provider_with_options, create_builtin_provider};
let provider = create_builtin_provider_with_options(&provider_name, api_key, base_url)
    .or_else(|_| create_builtin_provider(&provider_name))
    .map(Arc::from)?;
```

### Changes Required

1. `oxicode-cli/Cargo.toml`에서 `oxicode-sdk` 의존성 제거.
2. `lib.rs`에서 `use oxicode_sdk::OxicodeBuilder` 제거.
3. `App` 구조체에서 `engine: oxicode_sdk::Oxicode` 필드 제거.
4. `App::engine()` 메서드 제거.
5. `App::new()`에서 provider 생성을 `oxicode_ai::create_builtin_provider_with_options`로 교체.
6. `model_db::register_model` / `model_db::get_provider_models` 등 model_db 함수는 이미 `oxicode-ai`에 있으므로 직접 사용.

### Impact

- 빌드 의존성 트리에서 oxicode-sdk + 그 하위 모듈(security, middleware, observability, workflow_dsl) 제거.
- oxicode-cli 컴파일 시간 감소.
- 의존성 방향 명확화: oxicode-cli → oxicode-ai, oxicode-agent, oxicode-store (직접).

---

## 5. Redundant Re-export Layer — oxios-kernel lib.rs

**Location**: `oxios/crates/oxios-kernel/src/lib.rs`
**Priority**: 🟡 Medium (public surface 정리, 컴파일 시간)
**Effort**: Small

### Current State

`lib.rs`에 3개의 oxicode_sdk re-export 블록이 있음:

**Block 1** — Top-level re-export (25개):
```rust
pub use oxicode_sdk::{
    Agent, AgentConfig, AgentEvent, AgentTool, AgentToolResult,
    CircuitBreakerConfig, KernelToolProvider, MessageBus, MiddlewarePipeline,
    Model, Oxicode, OxicodeBuilder, Provider, ProviderCircuitBreaker, ProviderOptions,
    RoutingControl, ToolContext, ToolError, ToolExecutionMode, ToolRegistry,
};
```

**Block 2** — `sdk_exports` 모듈 (33개):
```rust
pub mod sdk_exports {
    pub use oxicode_sdk::{
        AgentBuilder, AgentGroup as SdkAgentGroup, AgentHandle, ...
    };
}
```

**Block 3** — `coordination.rs` re-export (16개):
```rust
pub use oxicode_sdk::{
    Consensus, CoordinatedGroup, CoordinatedGroupBuilder, GroupResult,
    MemoryEntry, MemoryEvent, MemoryKey, SharedMemory, VoteResult,
    WorkEvent, WorkItem, WorkQueue, WorkQueueConfig, ...
};
```

**Block 4** — `CircuitBreaker` alias (1개):
```rust
pub use oxicode_sdk::ProviderCircuitBreaker as CircuitBreaker;
```

### Problem

oxios 프로젝트 내의 모든 코드가 `use oxicode_sdk::X`로 **직접 참조**. `oxios_kernel::Agent`, `oxios_kernel::sdk_exports::*`, `oxios_kernel::coordination::Consensus` 등으로 접근하는 곳이 **프로젝트 전체에 0개**.

다른 crate (oxios-cli, oxios-web, oxios-ouroboros)도 각자 직접 `oxicode-sdk`를 의존하고 있어서 `oxios_kernel`을 통한 간접 접근이 필요 없음.

추가 문제: `MemoryEntry`가 `coordination.rs`와 `memory/mod.rs` 양쪽에서 re-export되어 **이름 충돌 위험**.

### Target State

- Block 1 (top-level 25개): 제거. 내부에서는 이미 `use oxicode_sdk::`로 직접 참조 중.
- Block 2 (`sdk_exports` 모듈): **모듈 전체 제거.** 소비자 0.
- Block 3 (`coordination.rs` re-export): 제거. `MemoryEntry` 충돌 해소.
- Block 4 (`CircuitBreaker` alias): 제거. 소비자 0.

### Rationale

이것은 "SDK 완성도"가 아님. oxios-kernel은 oxicode_sdk의 re-exporter가 아니라 자체 로직을 가진 크레이트. 다른 crate이 oxicode_sdk 타입이 필요하면 각자 `oxicode-sdk`를 의존하면 됨 (이미 그렇게 하고 있음).

---

## 6. Dead Code — 호출되지 않는 함수/타입

### 6.1 oxios-ouroboros: `degraded_seed()`

**Location**: `oxios/crates/oxios-ouroboros/src/degraded.rs`
**Priority**: 🟢 Low
**Status**: dead code + TODO

```rust
/// TODO: Connect to `generate_seed()` fallback when full integration is done.
#[allow(dead_code)]
pub fn degraded_seed(interview: &InterviewResult) -> Seed { ... }
```

LLM 장애 시 폴백으로 쓸 의도였으나 호출되는 곳이 없음. 다른 degraded 함수(`degraded_interview`, `degraded_evaluation`)는 실제로 사용 중.

**Action**: `generate_seed()` 연동이 완료될 때까지 `#[allow(dead_code)]` 유지. 연동 시점에 호출부를 추가하고 `#[allow(dead_code)]` 제거.

---

### 6.2 oxios-kernel: `memory/auto_protect.rs` — `record_access()`

**Location**: `oxios/crates/oxios-kernel/src/memory/auto_protect.rs`
**Priority**: 🟢 Low

```rust
#[allow(dead_code)]
pub fn record_access(&self, key: &str) { ... }
```

메모리 접근 기록 기능. 자동 보호(heatmap 기반 중요도 추적)의 일부이지만 아직 호출되지 않음.

**Action**: auto-protect 기능이 활성화될 때까지 유지.

---

### 6.3 oxios-kernel: `memory/hyperbolic.rs` — `c()` 메서드

**Location**: `oxios/crates/oxios-kernel/src/memory/hyperbolic.rs`
**Priority**: 🟢 Low

```rust
#[allow(dead_code)]
pub fn c(&self) -> f64 { self.curvature }
```

하이퍼볼릭 메모리 공간의 곡률 값 접근자. 미래 하이퍼볼릭 임베딩 기능을 위한 것.

**Action**: 해당 기능 구현 시 활용.

---

## 7. Summary Table

| # | 항목 | 분류 | 위치 | Priority | Effort | Blocker |
|---|------|------|------|----------|--------|---------|
| 1 | 세션 브랜치 전환 | 미완성(연결) | oxicode-cli/handlers.rs:987 | 🔴 High | S | 없음 |
| 2 | Web chat tool_calls | 미완성(연결) | oxios-web/chat.rs | 🟡 Medium | M | 커널 API |
| 3 | Web A2A 로깅 | 미완성(연결) | oxios-web/a2a.rs | 🟢 Low | S | 커널 API |
| 4 | CLI 모델/페르소나 전환 | 미완성(연결) | oxios-cli/interactive.rs | 🟡 Medium | M | 커널 API |
| 5 | Setup OAuth 플로우 | 미완성(로직) | oxicode-cli/handlers.rs:1150 | 🟡 Medium | L | 없음 |
| 6 | WorkerManager 12개 | Skeleton | oxios-kernel/workers/ | 🟢 Low | XL | 없음 |
| 7 | WasmSandbox (feature off) | Skeleton | oxios-kernel/wasm_sandbox.rs | 🟢 Low | — | 없음 |
| 8 | Memory 5단계 압축 | Skeleton | oxios-kernel/memory/compaction.rs | 🟢 Low | L | 없음 |
| 9 | Orchestrator 세션 복원 | Skeleton | oxios-kernel/orchestrator.rs | 🟡 Medium | M | 없음 |
| 10 | oxicode-cli → oxicode-sdk 의존 | Mislayered | oxicode-cli/Cargo.toml | 🔴 High | S | 없음 |
| 11 | lib.rs dead re-export 74개 | Redundant | oxios-kernel/lib.rs | 🟡 Medium | S | 없음 |
| 12 | degraded_seed() | Dead code | oxios-ouroboros/degraded.rs | 🟢 Low | — | 없음 |
| 13 | record_access() | Dead code | oxios-kernel/memory/auto_protect.rs | 🟢 Low | — | 없음 |
| 14 | hyperbolic c() | Dead code | oxios-kernel/memory/hyperbolic.rs | 🟢 Low | — | 없음 |

---

## Recommended Action Order

1. **#10** oxicode-cli → oxicode-sdk 의존 정리 — 즉시, 빌드 시간 감소 효과 큼
2. **#1** 세션 브랜치 전환 연결 — 즉시, 작업량 적고 사용자 경험 개선 큼
3. **#11** oxios-kernel dead re-export 정리 — 즉시, 코드 정리
4. **#5** Setup OAuth — #5a (OAuth 선택지 숨기기) 또는 #5b (실제 구현) 결정
5. **#9** Orchestrator 세션 복원 — 다음 스프린트
6. **#6, #7, #8** Skeleton 기능들 — 실험적 기능으로 분류하고 명시적으로 문서화
7. **#2, #3, #4** 미연결 항목 — 커널 API 완성 후 연결
