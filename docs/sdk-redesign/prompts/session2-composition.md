# 작업: oxi-sdk Composition Layer 구현

## 프로젝트 위치
/Volumes/MERCURY/PROJECTS/oxi/oxi-sdk/

## 설계 문서
docs/sdk-redesign/ 폴더의 설계 문서를 참고할 것. 특히:
- 00-overview.md (전체 아키텍처, 타입 흐름도)
- 02-security.md (Capability, Authorizer, SecurityMiddleware)
- 03-coordination.md (WorkQueue, SharedMemory, Consensus, CoordinatedGroup)
- 05-middleware.md (MiddlewarePipeline, MiddlewareBridge, builtins)
- 06-integration.md §6.3 (RoutingControl), §6.4-6.5 (builder 확장)

## 컨텍스트
oxi는 Rust 코딩 에이전트이자 Agent OS(oxios)의 엔진 SDK이다.
Session 1에서 기초 레이어(error.rs, lifecycle/, observability/)를 이미 구현했다.
이번 작업은 그 기초 위에 **조합 레이어**를 구현하는 것이다.

Session 1이 구현한 타입을 import하여 사용:
- crate::error::SdkError
- crate::lifecycle::{AgentHandle, AgentStatus, AgentLifecycleEvent, AgentSnapshot, AgentSupervisor, SupervisorPolicy, SnapshotStore, FileSnapshotStore}
- crate::observability::{Tracer, TraceId, SpanId, Span, SpanContext, AuditLog, AuditEntry, CostTracker, TokenUsage, CostBreakdown, EventStore, StoredEvent}

각 타입이 이미 존재한다고 가정하고 구현할 것.
(Session 1이 완료되기 전이면 일단 `use crate::...`로 선언만 하고 컴파일은 나중에 맞춤)

## 구현할 모듈

### 1. src/security/ — Capability-based 보안
sub-modules:
- capability.rs: Capability enum, StringPattern, CapabilitySet, CapabilitySubject
- authorizer.rs: Authorizer, DefaultPolicy, role hierarchy (define_role, bind_role)
- middleware.rs: SecurityMiddleware (→ Middleware trait 구현체)

Capability 핵심:
- 14개 variant (FileRead/Write/Edit/List/Find, Bash, Network, WebBrowse, Subagent, BusRead/Write, EnvRead, ToolUse, McpAccess)
- 각 variant는 파라미터화됨 (path_pattern, allowed_commands, allowed_domains 등)

CapabilitySet 핵심 presets:
- all() — 전체 권한
- read_only(workspace) — 파일 읽기만
- coding(workspace) — 표준 코딩 권한 (git, cargo, npm 등)
- research(workspace) — 읽기 + 웹 브라우징
- browser(workspace) — 읽기 + 브라우징 + 출력 디렉토리만 쓰기

Authorizer 핵심:
- grants: HashMap<CapabilitySubject, CapabilitySet> 직접 권한
- roles: HashMap<String, CapabilitySet> 역할 정의
- role_bindings: HashMap<String, Vec<String>> 에이전트→역할 매핑
- check() 시 직접 권한 → 역할 상속 → 기본 정책 순서로 평가
- 모든 check() 결과를 AuditLog에 기록

SecurityMiddleware 핵심:
- required_capability(tool_name, params)로 툴 호출에서 필요 권한 추론
- Middleware trait의 BeforeTool phase에서 권한 체크

### 2. src/coordination/ — 에이전트 간 조정
sub-modules:
- work_queue.rs: WorkQueue, WorkItem, WorkStatus, WorkResult, WorkQueueStats
- shared_memory.rs: SharedMemory, MemoryKey, MemoryEntry, MemoryEvent
- consensus.rs: Consensus, Vote, VoteResult
- group_ext.rs: CoordinatedGroup, CoordinatedGroupBuilder

WorkQueue 핵심:
- Vec<WorkItem> + parking_lot::RwLock 기반
- enqueue(work_type, payload, priority) → ID
- claim(agent_id, work_type_filter) → 우선순위最高的 Pending 아이템을 원자적으로 claim
- complete(item_id, result), retry(item_id), cancel(item_id)
- broadcast::Sender<WorkEvent>로 이벤트 노출

SharedMemory 핵심:
- HashMap<MemoryKey, MemoryEntry> + version 기반 optimistic locking
- read(key), write(key, value, author, expected_version?), increment(key, delta, author)
- 버전 불일치 시 SdkError::VersionConflict

Consensus 핵심:
- start(vote_id, voters, threshold)
- vote(vote_id, voter, value) → VoteResult (threshold 도달 시 decided=true)
- 0.5 = 다수결, 1.0 = 만장일치

CoordinatedGroup 핵심:
- handles: Vec<AgentHandle> (절대 model_id()를 에이전트 ID로 사용하지 말 것!)
- fan_out(work_type, payloads) — 큐에 넣고 각 handle이 claim하여 병렬 실행
- vote(question, options) — 각 handle에게 질문하여 투표
- map_reduce(work_type, payloads, reduce_key) — 실행 후 SharedMemory에 취합

### 3. src/middleware/ — Middleware Pipeline → AgentHooks Bridge
sub-modules:
- mod.rs: Middleware trait, MiddlewarePhase, MiddlewareContext, MiddlewareData, MiddlewareResult, MiddlewareAction, MiddlewarePipeline
- bridge.rs: MiddlewareBridge (Pipeline을 AgentHooks로 변환하는 핵심 adapter)
- builtins.rs: RateLimitMiddleware, LoggingMiddleware, TokenBudgetMiddleware, ContentFilterMiddleware
- plugin.rs: PluginLoader, PluginManifest

Middleware trait 핵심:
- async fn name(), phases(), handle(&MiddlewareContext) → MiddlewareResult
- phases(): BeforeLlm, AfterLlm, BeforeTool, AfterTool, BeforeRun, AfterRun
- handle() → Continue/Block/Terminate + modified_data + reason

MiddlewarePipeline 핵심:
- Vec<Arc<dyn Middleware>>에 등록 순서대로 실행
- 첫 non-Continue 결과에서 체인 중단

MiddlewareBridge 핵심 (이 설계의 가장 중요한 부분):
- oxi-agent의 AgentHooks는 before_tool_call: Option<Box<dyn Fn>> 하나만 지원
- 여러 middleware를 하나의 AgentHooks로 컴파일해야 함
- into_hooks(pipeline, agent_id, terminate_flag) → AgentHooks
- before_tool_call 클로저 내에서 pipeline.execute(BeforeTool) 호출
- after_tool_call 클로저 내에서 pipeline.execute(AfterTool) 호출
- should_stop_after_turn에서 terminate_flag 체크
- AgentHooks가 동기 콜백이므로 tokio::runtime::Handle::current().block_on() 사용

TokenBudgetMiddleware 핵심:
- AfterLlm phase에서 토큰 누적
- Arc<AtomicU64>로 누적값 추적
- Option<Arc<CostTracker>>로 비용 기반 종료도 지원
- 초과 시 Terminate 반환

### 4. src/routing.rs — 런타임 라우팅 제어
RoutingControl struct:
- enabled: Arc<AtomicBool>
- config: Arc<RwLock<RoutingConfig>>
- set_enabled(), is_enabled()
- update_config(f), set_fallback_models(), exclude_model(), unexclude_model()

### 5. 기존 모듈 확장
builder.rs:
- OxiBuilder에 supervisor() → SupervisorBuilder 메서드 추가
- SupervisorBuilder: policy(), snapshot_dir(), with_audit(), with_authorizer(), with_tracer(), with_cost_tracker(), build()

agent_builder.rs:
- 새 필드: middlewares, capabilities, authorizer, tracer, audit_log, cost_tracker
- capabilities(), coding_capabilities(), readonly_capabilities()
- authorizer(), tracer(), audit_log(), cost_tracker()
- middleware(), with_rate_limit(), with_token_budget(), with_logging()
- build()에서 MiddlewareBridge로 파이프라인을 AgentHooks로 변환

kernel_bridge.rs:
- KernelToolContext에 with_capabilities(caps), capability_subject() 추가

lib.rs / prelude.rs:
- 새 모듈의 pub use 추가

## 규칙
1. cargo fmt --all -- --check 통과해야 함
2. cargo clippy -p oxi-sdk -- -D warnings 통과해야 함
3. cargo test -p oxi-sdk 통과해야 함
4. 기존 테스트 44개가 모두 계속 통과해야 함 (regression 금지)
5. 각 모듈에 #[cfg(test)] mod tests 작성
6. oxi-ai, oxi-agent의 기존 코드를 수정하지 않음
7. 코드 스타일: parking_lot::RwLock, async_trait, thiserror 사용
8. 설계 문서의 타입 이름과 API를 정확히 따를 것
9. CoordinatedGroup에서 절대 agent.model_id()를 에이전트 식별자로 사용하지 말 것. AgentHandle::agent_id() 사용.
10. SecurityMiddleware는 반드시 Middleware trait을 구현할 것 (독립 함수가 아님).
11. MiddlewareBridge는 실제로 AgentHooks를 생성하는 구현체여야 함 (설명만 있는 클래스 금지).
