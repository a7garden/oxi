# RFC-006: SDK 고도화 — 멀티 에이전트 오케스트레이션, 메시지 버스, 커널 브릿지

**상태**: 초안  
**우선순위**: P2 — 멀티 에이전트 시스템의 기반  
**현재 완성도**: ~92%  
**목표**: pi 대비 우위 확립 + 에이전트 정의 파일 포맷 표준화  

---

## 1. 현재 상태

oxi-sdk는 pi에 없는 독자적인 SDK 크레이트이다 (9,597 LOC). pi의 `AgentGroup`은 `coding-agent`에 포함된 내부 기능이지만, oxi는 이를 독립 라이브러리로 분리했다.

### oxi-sdk 전체 모듈 현황

#### 핵심 (Core)
| 모듈 | 파일 | 설명 | 상태 |
|------|------|------|------|
| AgentBuilder | agent_builder.rs | 빌더 패턴 에이전트 생성 | ✅ |
| AgentGroup | agent_group.rs | parallel, sequential, fan-out 전략 | ✅ |
| Builder | builder.rs | SDK 진입점 빌더 | ✅ |
| MessageBus | message_bus.rs | pub/sub 메시지 버스 | ✅ |
| KernelBridge | kernel_bridge.rs | 호스트 툴 레지스트리 브릿지 | ✅ |
| ClosureTool | closure_tool.rs | 클로저 기반 툴 생성 | ✅ |
| ToolFactory | tool_factory.rs | 툴 팩토리 패턴 | ✅ |
| MultiProvider | multi_provider.rs | 멀티 프로바이더 로드 밸런싱 | ✅ |
| Routing | routing.rs | 요청 라우팅 제어 | ✅ |
| Metrics | metrics.rs | SDK 메트릭 수집 | ✅ |

#### 조정 (Coordination)
| 모듈 | 파일 | 설명 | 상태 |
|------|------|------|------|
| SharedMemory | coordination/shared_memory.rs | 버전 관리 KV 스토어 (낙관적 락) | ✅ |
| WorkQueue | coordination/work_queue.rs | 우선순위 기반 작업 큐 | ✅ |
| Consensus | coordination/consensus.rs | 투표 기반 합의 (다수결/만장일치) | ✅ |
| CoordinatedGroup | coordination/group_ext.rs | fan-out, vote, map-reduce 전략 | ✅ |

#### 생명주기 (Lifecycle)
| 모듈 | 파일 | 설명 | 상태 |
|------|------|------|------|
| Supervisor | lifecycle/supervisor.rs | 에이전트 풀 관리 (spawn/resume/policy) | ✅ |
| SnapshotStore | lifecycle/snapshot.rs | 에이전트 상태 스냅샷/복원 | ✅ |
| AgentHandle | lifecycle/supervisor.rs | 에이전트 상태 전이 (AtomicU8) | ✅ |

#### 미들웨어 (Middleware)
| 모듈 | 파일 | 설명 | 상태 |
|------|------|------|------|
| Bridge | middleware/bridge.rs | 외부 시스템 브릿지 | ✅ |
| Builtins | middleware/builtins.rs | 내장 미들웨어 | ✅ |
| Plugin | middleware/plugin.rs | 플러그인 시스템 | ✅ |

#### 관측성 (Observability)
| 모듈 | 파일 | 설명 | 상태 |
|------|------|------|------|
| Tracer | observability/trace.rs | 분산 트레이싱 (TraceId, SpanId, RAII guards) | ✅ |
| AuditLog | observability/audit.rs | 보안 감사 로그 | ✅ |
| CostTracker | observability/cost.rs | 토큰/비용 추적 (모델 가격 연동) | ✅ |
| EventStore | observability/event_store.rs | 이벤트 영속 저장 | ✅ |

#### 보안 (Security)
| 모듈 | 파일 | 설명 | 상태 |
|------|------|------|------|
| Authorizer | security/authorizer.rs | 역할 기반 접근 제어 (capability + role hierarchy) | ✅ |
| Capability | security/capability.rs | 권한 세트 관리 | ✅ |
| SecurityMiddleware | security/middleware.rs | 보안 미들웨어 체인 | ✅ |

### 강화 영역 (유일한 갭)

| 기능 | 설명 | 우선순위 | 비고 |
|------|------|---------|------|
| 에이전트 정의 검증 | 마크다운 YAML frontmatter 스키마 검증 | P1 | 유일한 미구현 기능 |
| 백프레셔 | 병렬 실행 시 리소스 제한 | P2 | Semaphore 기반, 기존 AgentGroup에 통합 가능 |

> **참고**: 이전 버전 RFC에서 제안하던 Blackboard, 관측성, 보안, WorkflowEngine은 모두 이미 구현되어 있다. 각각 SharedMemory, observability/, security/, coordination/ 모듈로 대체된다.

---

## 2. 아키텍처

### 2.1 에이전트 정의 검증 (유일한 신규 기능)

```rust
/// oxi-sdk/src/agent_definition.rs — 신규

/// 에이전트 정의 (마크다운 YAML frontmatter)
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub scope: AgentScope,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub max_subagent_depth: u8,
    #[serde(default)]
    pub default_context: DefaultContext,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub enum AgentScope {
    #[default]
    User,
    Project,
    Both,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub enum DefaultContext {
    #[default]
    Fresh,
    Fork,
}

impl AgentDefinition {
    /// 마크다운 파일에서 로드
    pub fn from_markdown(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        
        // YAML frontmatter 추출 (--- 사이)
        let frontmatter = extract_frontmatter(&content)?;
        let mut def: AgentDefinition = serde_yaml::from_str(&frontmatter)?;
        
        // 본문이 있으면 system_prompt로 사용
        let body = extract_body(&content);
        if !body.is_empty() && def.system_prompt.is_none() {
            def.system_prompt = Some(body);
        }
        
        def.validate()?;
        Ok(def)
    }
    
    /// 검증
    fn validate(&self) -> Result<()> {
        // 이름: a-z, 0-9, -, max 64
        validate_agent_name(&self.name)?;
        
        // 설명: max 1024 chars
        if self.description.len() > 1024 {
            return Err(anyhow!("Description too long (max 1024 chars)"));
        }
        
        // max_subagent_depth: max 10
        if self.max_subagent_depth > 10 {
            return Err(anyhow!("max_subagent_depth too high (max 10)"));
        }
        
        Ok(())
    }
}

/// 에이전트 발견 (pi의 에이전트 발견 패턴 이식)
pub struct AgentDiscovery;

impl AgentDiscovery {
    /// 모든 에이전트 정의 검색
    pub fn discover(cwd: &Path) -> Result<Vec<(String, AgentDefinition)>> {
        let mut agents = HashMap::new();
        
        // 1. 글로벌: ~/.oxi/agents/
        discover_from_dir(home_dir().join(".oxi/agents"), &mut agents)?;
        
        // 2. 프로젝트: .oxi/agents/
        discover_from_dir(cwd.join(".oxi/agents"), &mut agents)?;
        
        // 프로젝트가 글로벌을 오버라이드
        Ok(agents.into_iter().map(|(k, v)| (k, v)).collect())
    }
}
```

### 2.2 기존 조정 모듈과의 관계

이전 RFC에서 제안하던 기능이 이미 다음 모듈로 구현되어 있다:

#### Blackboard → SharedMemory (이미 구현됨)

`coordination/shared_memory.rs`는 버전 관리 KV 스토어로, blackboard 패턴의 완전한 구현이다:

```rust
/// 이미 구현된 SharedMemory (blackboard 패턴)
pub struct SharedMemory {
    data: RwLock<HashMap<MemoryKey, MemoryEntry>>,
    tx: broadcast::Sender<MemoryEvent>,
}

// 기능:
// - read(): 키 읽기
// - write(): 낙관적 락 쓰기 (버전 충돌 감지)
// - atomic_increment(): 카운터 원자 증가
// - subscribe(): broadcast 채널로 변경 알림
// - MemoryEvent::Written / MemoryEvent::Deleted 이벤트
```

**결론**: 별도 Blackboard 모듈은 불필요. SharedMemory를 직접 사용.

#### WorkflowEngine → 기존 coordination 모듈 (이미 구현됨)

| WorkflowStep | 기존 구현 |
|-------------|----------|
| Run | AgentBuilder + AgentHandle |
| Parallel | AgentGroup::parallel() + Semaphore |
| Chain | AgentGroup::sequential() |
| ForEach | CoordinatedGroup::map_reduce() |
| If (조건부) | 런타임 로직으로 처리 |
| SetState | SharedMemory::write() |
| Vote | Consensus::cast_vote() |

**결론**: 선언적 DSL YAML 파일을 통한 워크플로우 정의만 추가하면 된다. 실행 엔진은 기존 coordination 모듈로 충분.

### 2.3 선언적 워크플로우 DSL (YAML 파싱만 추가)

```rust
/// oxi-sdk/src/workflow_dsl.rs — 신규 (파싱 레이어만)

/// YAML 워크플로우 정의를 기존 coordination 모듈 호출로 변환
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub name: String,
    pub description: String,
    pub steps: Vec<WorkflowStepDef>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum WorkflowStepDef {
    Run { agent: String, task: String, output: Option<String> },
    Parallel { agents: Vec<String>, task: String, concurrency: Option<usize> },
    Chain { steps: Vec<WorkflowStepDef> },
    ForEach { items_key: String, agent: String, task_template: String, concurrency: Option<usize> },
    Vote { agents: Vec<String>, question: String, threshold: Option<f32> },
    SetState { key: String, namespace: String, value: Value },
}

/// WorkflowDefinition을 CoordinatedGroup 실행 계획으로 변환
impl WorkflowDefinition {
    pub fn into_execution_plan(self, memory: Arc<SharedMemory>) -> ExecutionPlan {
        // YAML → CoordinatedGroup / AgentGroup 호출 매핑
        // Parallel → AgentGroup::parallel()
        // Chain → AgentGroup::sequential()
        // ForEach → CoordinatedGroup::map_reduce()
        // Vote → Consensus::start() + cast_vote()
        // SetState → SharedMemory::write()
        todo!("YAML → 기존 coordination API 매핑")
    }
}
```

### 2.4 기존 관측성 모듈 (이미 구현됨)

```
observability/
├── trace.rs       # Tracer: TraceId, SpanId, RAII Span guards
├── audit.rs       # AuditLog: 보안 결정 기록 (Authorizer와 연동)
├── cost.rs        # CostTracker: 모델별 토큰/비용 추적
└── event_store.rs # EventStore: 이벤트 영속 저장
```

모델 가격 데이터가 `oxi-ai::Model`에 포함되어 CostTracker와 연동된다.

### 2.5 기존 보안 모듈 (이미 구현됨)

```
security/
├── authorizer.rs  # 역할 기반 접근 제어 (direct grants → role inheritance → default policy)
├── capability.rs  # CapabilitySet, CapabilitySubject
└── middleware.rs   # 보안 미들웨어 체인
```

Authorizer는 AuditLog와 연동되어 모든 접근 결정을 기록한다.

---

## 3. 구현 계획

### Phase 1: 에이전트 정의 (1주) — 유일한 신규 작업

| 작업 | 산출물 |
|------|--------|
| AgentDefinition | YAML frontmatter 파싱 + 검증 |
| AgentDiscovery | ~/.oxi/agents/ + .oxi/agents/ 검색 |
| WorkflowDefinition | YAML 워크플로우 → 기존 coordination API 매핑 |
| 문서화 | 에이전트 정의 포맷 가이드 |

### Phase 2: 백프레셔 (3일) — 기존 모듈 개선

| 작업 | 산출물 |
|------|--------|
| AgentGroup에 Semaphore | 병렬 실행 동시성 제한 |
| WorkQueue 백프레셔 | 큐 크기 임계치 + 대기 |

> **참고**: Phase 2의 Blackboard, 관측성, 보안, WorkflowEngine은 이미 구현되어 있어 삭제됨.

---

## 4. 성공 기준

- [ ] 에이전트 정의: YAML frontmatter + 이름/설명 검증 + 발견
- [ ] 워크플로우 DSL: YAML → 기존 coordination 모듈 매핑
- [x] 상태 공유: SharedMemory (버전 관리 KV + broadcast 알림) — 이미 구현됨
- [x] 관측성: Tracer + AuditLog + CostTracker + EventStore — 이미 구현됨
- [x] 보안: Authorizer + Capability + SecurityMiddleware — 이미 구현됨
- [x] 조정: WorkQueue + Consensus + CoordinatedGroup — 이미 구현됨
- [x] 생명주기: Supervisor + SnapshotStore + AgentHandle — 이미 구현됨
- [x] 기존 AgentBuilder/AgentGroup 호환성 유지
