# oxicode-sdk 설계 문서: Agent OS를 위한 차세대 SDK

**버전:** 0.23.0 Draft  
**작성일:** 2026-05-25  
**범위:** oxicode-sdk 전체 재설계 + 새로운 Crate 분리 권고

---

## 문서 구성

| # | 파일 | 내용 |
|---|------|------|
| 00 | `overview.md` | 설계 원칙, 아키텍처 개요, 평가 매트릭스 (본 파일) |
| 01 | `01-lifecycle.md` | 에이전트 수명 주기 (spawn/suspend/resume/checkpoint/terminate) |
| 02 | `02-security.md` | Capability-based 보안/샌드박스 모델 |
| 03 | `03-coordination.md` | 에이전트 간 조정 (WorkQueue + SharedMemory + Consensus) |
| 04 | `04-observability.md` | 분산 Tracing + Audit Trail + Event Sourcing |
| 05 | `05-middleware.md` | Middleware Chain + Dynamic Plugin 시스템 |
| 06 | `06-integration.md` | 기존 코드와의 통합 방안, OxicodeBuilder/AgentBuilder 확장, 마이그레이션 |

---

## 0.1 기존 아키텍처와의 정합성

```
oxicode-ai (provider abstraction) ← oxicode-agent (tool loop) ← oxicode-sdk (multi-agent orchestration)
                                                          ↑
                                                      oxicode-store (session persistence)
```

모든 새로운 추상화는 기존 oxicode-ai, oxicode-agent의 개념을 **확장**하고, **오버라이드**하지 않는다.

기존 코드 변경 최소화:
- `oxicode-ai`: 변경 없음
- `oxicode-agent`: `Agent`에 `config()` getter 등 소소한 추가만
- `oxicode-sdk`: 새 모듈 추가 위주

---

## 0.2 다섯 가지 핵심 강화 목표

| 목표 | 현재 상태 | 목표 상태 |
|------|-----------|-----------|
| **에이전트 수명 주기** | 생성/실행만 (one-shot) | spawn → suspend → resume → checkpoint → terminate |
| **보안/샌드박스** | `permissions: Vec<String>` (선언만) | Capability-based enforcement + resource limits |
| **에이전트 간 조정** | broadcast-only MessageBus | work queue + shared memory + consensus |
| **관측 가능성** | AgentMetrics (basic) | distributed tracing + audit trail + event sourcing |
| **확장성** | ClosureTool만 | middleware chain + dynamic plugin loading |

---

## 0.3 새 모듈 레이아웃

```
oxicode-sdk/src/
├── lifecycle/           # NEW: Agent lifecycle management
│   ├── mod.rs           # AgentSnapshot, AgentStatus, AgentHandle, AgentLifecycleEvent
│   ├── supervisor.rs    # AgentSupervisor, SupervisorPolicy, SnapshotStore
│   └── snapshot.rs      # FileSnapshotStore, serialization helpers
├── security/            # NEW: Capability-based permissions
│   ├── mod.rs
│   ├── capability.rs    # Capability, CapabilitySubject, CapabilitySet
│   ├── authorizer.rs    # Authorizer (grant/check/revoke)
│   └── middleware.rs    # SecurityMiddleware (tool execution wrapper)
├── coordination/        # NEW: Inter-agent coordination
│   ├── mod.rs
│   ├── work_queue.rs    # WorkQueue, WorkItem, WorkStatus
│   ├── shared_memory.rs # SharedMemory, MemoryKey, MemoryEntry
│   ├── consensus.rs     # Consensus, Vote, VoteResult
│   └── group_ext.rs     # CoordinatedGroup (AgentGroup 확장)
├── observability/       # NEW: Tracing + Audit + Event Sourcing
│   ├── mod.rs
│   ├── trace.rs         # Tracer, Span, SpanContext, TraceId
│   ├── audit.rs         # AuditLog, AuditEntry, SecurityDecision
│   └── event_store.rs   # EventStore, StoredEvent, EventQuery
├── middleware/          # NEW: Hook chain + pipeline
│   ├── mod.rs           # Middleware trait, MiddlewarePipeline
│   ├── builtins.rs      # RateLimitMiddleware, LoggingMiddleware, etc.
│   └── plugin.rs        # PluginLoader, PluginManifest
│
│  ── 기존 모듈 (확장) ──
├── lib.rs               # re-export 확장
├── builder.rs           # OxicodeBuilder에 security/observability/lifecycle 옵션 추가
├── agent_builder.rs     # AgentBuilder에 capabilities/middleware 옵션 추가
├── kernel_bridge.rs     # KernelToolContext에 capabilities 통합
├── agent_group.rs       # 기존 유지
├── message_bus.rs       # 기존 유지
├── closure_tool.rs      # 기존 유지
├── multi_provider.rs    # 기존 유지
├── metrics.rs           # observability와 통합
├── tool_factory.rs      # 기존 유지
└── prelude.rs           # 새 타입 re-export 추가
```

---

## 0.4 평가 매트릭스 (Before → After)

| 차원 | Before | After | 향상 포인트 |
|------|--------|-------|-------------|
| **API 설계** | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | lifecycle, capability builder 통합 |
| **확장성** | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | middleware chain + plugin loading |
| **멀티 에이전트** | ⭐⭐ | ⭐⭐⭐⭐⭐ | WorkQueue + SharedMemory + Consensus |
| **수명 주기** | ⭐ | ⭐⭐⭐⭐⭐ | AgentHandle + Supervisor + SnapshotStore |
| **보안/샌드박스** | ⭐ | ⭐⭐⭐⭐⭐ | Capability system + SecurityMiddleware |
| **관측 가능성** | ⭐⭐ | ⭐⭐⭐⭐⭐ | Tracer + AuditLog + EventStore |
| **DX** | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | prelude + builder + capability presets |

---

## 0.5 의존성 추가

```toml
# Cargo.toml additions
[dependencies]
uuid = { version = "1", features = ["v4", "serde"] }
glob = "0.3"           # capability path matching
chrono = "0.4"         # timestamps (이미 oxicode-ai에서 사용)

# 기존 유지
oxicode-ai = { version = "0.22.0", path = "../oxicode-ai" }
oxicode-agent = { version = "0.22.0", path = "../oxicode-agent" }
anyhow = "1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
serde = { version = "1", features = ["derive"] }
parking_lot = "0.12"
async-trait = "0.1"
tracing = "0.1"
```

---

## 0.6 구현 우선순위

| Phase | 모듈 | 예상 공수 | 의존성 |
|-------|------|-----------|--------|
| **Phase 1** | lifecycle (AgentHandle + Supervisor) | 2-3일 | oxicode-agent Agentconfig() 추가 |
| **Phase 2** | security (Capability + Authorizer + SecurityMiddleware) | 2-3일 | Phase 1 (agent_id) |
| **Phase 3** | coordination (WorkQueue + SharedMemory) | 2일 | Phase 1 (AgentHandle) |
| **Phase 4** | observability (Tracer + AuditLog) | 2일 | Phase 2 (security audit) |
| **Phase 5** | middleware (MiddlewarePipeline + PluginLoader) | 1-2일 | Phase 2, 4 |
| **Phase 6** | integration (builder 확장, prelude, 테스트) | 1-2일 | Phase 1-5 |

**총 예상 공수:** 10-14일
