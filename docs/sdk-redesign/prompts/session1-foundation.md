# 작업: oxi-sdk Foundation Layer 구현

## 프로젝트 위치
/Volumes/MERCURY/PROJECTS/oxi/oxi-sdk/

## 설계 문서
docs/sdk-redesign/ 폴더의 설계 문서를 참고할 것. 특히:
- 00-overview.md (전체 아키텍처, 타입 흐름도, 모듈 레이아웃)
- 01-lifecycle.md (에이전트 수명 주기)
- 04-observability.md (Tracer, AuditLog, CostTracker, EventStore)
- 06-integration.md §6.2 (SdkError)

## 컨텍스트
oxi는 Rust 코딩 에이전트이자 Agent OS(oxios)의 엔진 SDK이다.
현재 oxi-sdk에는 builder, agent_builder, tool_factory, message_bus 등이 구현되어 있다.
이번 작업은 SDK의 **기초 레이어**를 구현하는 것으로, 다른 모든 새 모듈이 의존하게 될 타입들을 만든다.

## 구현할 모듈

### 1. src/error.rs — SdkError enum
thiserror 기반 구조화된 에러 타입. 현재 anyhow만 쓰는 SDK에서 벗어나,
소비자가 match로 에러 분기 처리를 할 수 있게 한다.

필요한 variants:
- ModelNotFound { model_id }
- ProviderNotFound { provider }
- AllProvidersExhausted { attempts }
- AgentNotRunnable { agent_id, status }
- AgentAlreadyRunning { agent_id }
- SnapshotNotFound { agent_id }
- SnapshotCorrupt { agent_id, reason }
- PermissionDenied { subject, capability }
- CapabilityExpired { subject }
- WorkItemNotFound { item_id }
- VersionConflict { key, expected, current }
- VoteNotFound { vote_id }
- MiddlewareBlocked { middleware, reason }
- TokenBudgetExceeded { used, budget }
- CostBudgetExceeded { used, budget }
- RoutingDisabled
- NoRouteAvailable { model_id }
- Internal(#[from] anyhow::Error)

### 2. src/lifecycle/ — 에이전트 수명 주기 관리
sub-modules:
- mod.rs: AgentStatus enum, AgentHandle struct, AgentLifecycleEvent enum
- supervisor.rs: AgentSupervisor, SupervisorPolicy, RestartBackoff
- snapshot.rs: AgentSnapshot, ToolManifest, SnapshotStore trait, FileSnapshotStore

AgentHandle의 핵심:
- 내부에 Arc<Agent>를 래핑
- AtomicU8 기반 상태 전이 (CAS로 thread-safe)
- run(), suspend(), terminate(), cancel()
- switch_model(), set_system_prompt(), add_tool()
- lifecycle 이벤트를 broadcast channel로 emit

AgentSupervisor의 핵심:
- HashMap<String, AgentHandle> 풀 관리
- spawn(config) → AgentHandle 생성
- spawn_child(parent_id, config) → 부모-자식 관계
- suspend(agent_id) → SnapshotStore에 저장
- restore(agent_id) → SnapshotStore에서 복원
- subscribe() → lifecycle 이벤트 구독

SnapshotStore trait은 반드시 async_trait를 사용할 것.
수동 Pin<Box<...>> 패턴은 금지.

### 3. src/observability/ — 관측 가능성
sub-modules:
- trace.rs: Tracer, Span, SpanGuard, SpanContext, TraceId, SpanId
- audit.rs: AuditLog, AuditEntry, AuditFilter
- cost.rs: CostTracker, TokenUsage, CostBreakdown, CostSnapshot
- event_store.rs: EventStore, StoredEvent, EventQuery

Tracer의 핵심:
- TraceId/SpanId는 fastrand::u64로 생성
- SpanGuard는 RAII로 drop 시 자동 end
- start(), start_with_parent(), end(), trace(), query(), subscribe()
- broadcast channel로 span 완료 이벤트 노출

CostTracker의 핵심 (이전 설계에서 누락되었던 것):
- TokenUsage: input, output, cache_read, cache_write 세분화
- TokenUsage::cost(model)으로 모델 단가 곱해 실제 비용 계산
- per_agent_budget, global_budget 설정
- record(agent_id, model, usage)로 누적
- snapshot(agent_id), global_snapshot()으로 조회
- is_over_budget()으로 예산 체크

AuditLog의 핵심:
- append-only 로그 (Vec<AuditEntry>)
- max_entries 초과 시 오래된 항목 제거
- SecurityDecision, ToolExecution, Lifecycle, Custom entry types
- query(AuditFilter), subscribe()

EventStore의 핵심:
- AtomicU64 sequence 기반 append-only 로그
- append(stream_id, event_type, payload) → sequence 반환
- replay(stream_id)로 상태 복원
- query(EventQuery)로 필터링

## oxi-agent 최소 변경
oxi-agent/src/agent.rs에 pub getter 2개를 추가해야 함:

```rust
impl Agent {
    pub fn get_config(&self) -> AgentConfig {
        self.inner.read().config.clone()
    }
    pub fn resolver(&self) -> &Arc<dyn ProviderResolver> {
        &self.resolver
    }
}
```

## lib.rs / prelude.rs 업데이트
새 모듈의 pub use를 lib.rs와 prelude.rs에 추가.

## 규칙
1. cargo fmt --all -- --check 통과해야 함
2. cargo clippy -p oxi-sdk -- -D warnings 통과해야 함
3. cargo test -p oxi-sdk 통과해야 함
4. 기존 테스트 44개가 모두 계속 통과해야 함 (regression 금지)
5. 각 모듈에 #[cfg(test)] mod tests 작성
6. oxi-ai, oxi-agent의 기존 코드를 수정하지 않음 (getter 추가만 예외)
7. 코드 스타일: parking_lot::RwLock, async_trait, thiserror 사용
8. 설계 문서의 타입 이름과 API를 정확히 따를 것
