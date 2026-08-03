# 03. 에이전트 간 조정 (Inter-Agent Coordination)

모듈 경로: `oxicode-sdk/src/coordination/`

---

## 3.1 설계 개요

```
┌─────────────────────────────────────────────────────┐
│                Coordination Layer                    │
│                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────┐ │
│  │  WorkQueue   │  │ SharedMemory │  │ Consensus │ │
│  │              │  │              │  │           │ │
│  │  enqueue()   │  │  read()      │  │  vote()   │ │
│  │  claim()     │  │  write()     │  │  decide() │ │
│  │  complete()  │  │  watch()     │  │           │ │
│  └──────┬───────┘  └──────┬───────┘  └─────┬─────┘ │
│         │                 │                │        │
│         └─────────────────┼────────────────┘        │
│                           │                         │
│                    events (broadcast)                │
└───────────────────────────┼─────────────────────────┘
                            │
                 CoordinatedGroup
                 (fan-out, vote, map-reduce)
```

**원칙:**
- 모든 프리미티브는 `Arc` + `RwLock` 기반으로 thread-safe
- 이벤트는 `broadcast` 채널로 외부에 노출
- Production에서는 Redis/SQLite 백엔드로 교체 가능 (trait 기반)

---

## 3.2 WorkQueue

에이전트 간 작업 분배를 위한 분산 큐.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: String,
    pub work_type: String,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub status: WorkStatus,
    pub claimed_by: Option<String>,
    pub result: Option<WorkResult>,
    pub created_at_ms: u64,
    pub claimed_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub max_retries: usize,
    pub retry_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkStatus {
    Pending, Claimed, InProgress, Completed, Failed, Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkResult {
    pub success: bool,
    pub content: String,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub tokens_used: Option<u64>,
}
```

**핵심 API:**

```rust
impl WorkQueue {
    pub fn new(config: WorkQueueConfig) -> Self;

    /// 작업 등록. ID 반환.
    pub fn enqueue(&self, work_type: impl Into<String>, payload: Value, priority: i32) -> String;

    /// 우선순위가 가장 높은 Pending 아이템을 원자적으로 claim.
    /// work_type_filter로 특정 유형만 선택 가능.
    pub fn claim(&self, agent_id: &str, work_type_filter: Option<&[String]>) -> Option<WorkItem>;

    /// 상태 전이
    pub fn start(&self, item_id: &str) -> anyhow::Result<()>;
    pub fn complete(&self, item_id: &str, result: WorkResult) -> anyhow::Result<()>;
    pub fn retry(&self, item_id: &str) -> anyhow::Result<bool>;
    pub fn cancel(&self, item_id: &str) -> anyhow::Result<()>;

    /// 조회
    pub fn get(&self, item_id: &str) -> Option<WorkItem>;
    pub fn list(&self, filter: Option<WorkStatus>) -> Vec<WorkItem>;
    pub fn stats(&self) -> WorkQueueStats;

    /// 이벤트
    pub fn subscribe(&self) -> broadcast::Receiver<WorkEvent>;
}
```

`claim()`은 write lock 안에서 atomic하게 동작. 두 에이전트가 동시에 claim해도 하나만 성공.

---

## 3.3 SharedMemory

버전 기반 낙관적 잠금(optimistic locking)이 있는 공유 KV 스토어.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryKey {
    pub namespace: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub value: Value,
    pub version: u64,
    pub modified_at_ms: u64,
    pub modified_by: String,
}

impl SharedMemory {
    pub fn new() -> Self;

    /// 읽기
    pub fn read(&self, key: &MemoryKey) -> Option<Value>;
    pub fn read_entry(&self, key: &MemoryKey) -> Option<MemoryEntry>;

    /// 쓰기 (낙관적 잠금)
    /// expected_version이 Some이면 버전 불일치 시 에러
    pub fn write(&self, key: &MemoryKey, value: Value, author: &str, expected_version: Option<u64>) -> anyhow::Result<u64>;

    /// 원자적 증가 (카운터용)
    pub fn increment(&self, key: &MemoryKey, delta: i64, author: &str) -> i64;

    pub fn delete(&self, key: &MemoryKey) -> bool;
    pub fn list_namespace(&self, namespace: &str) -> Vec<MemoryKey>;
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryEvent>;
}
```

**낙관적 잠금 패턴:**

```rust
// 에이전트 A가 읽고 수정
let entry = memory.read_entry(&key);  // version = 3
// ... 로컬 수정 ...
memory.write(&key, new_value, "agent-A", Some(entry.version))?;  // version 3 → 4

// 에이전트 B가 동시에 수정 시도
memory.write(&key, other_value, "agent-B", Some(3))?;  // ❌ Version conflict!
```

---

## 3.4 Consensus

단순 다수결/만장일치 투표. Production에서는 Raft로 교체 가능.

```rust
impl Consensus {
    /// 투표 세션 시작. threshold: 0.5 = 다수결, 1.0 = 만장일치
    pub fn start(&self, vote_id: &str, voters: Vec<String>, threshold: f32);

    /// 투표. 결과 갱신. 임계치 도달 시 즉시 decided.
    pub fn vote(&self, vote_id: &str, voter: &str, value: String) -> anyhow::Result<VoteResult>;

    /// 상태 조회
    pub fn status(&self, vote_id: &str) -> Option<VoteResult>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResult {
    pub decided: bool,
    pub decision: Option<String>,
    pub tally: HashMap<String, usize>,  // value → count
    pub votes_received: usize,
    pub total_voters: usize,
}
```

---

## 3.5 CoordinatedGroup

`AgentHandle` 기반으로 작동. 기존 `AgentGroup`의 제한을 해결.

> **이전 문제:** `CoordinatedGroup::fan_out()`이 `agent.model_id()`를 에이전트 ID로 사용해서, 같은 모델을 쓰는 에이전트 간에 claim 충돌이 발생했음. `AgentHandle::agent_id()`를 사용하도록 수정.

```rust
pub struct CoordinatedGroup {
    handles: Vec<AgentHandle>,
    work_queue: Option<Arc<WorkQueue>>,
    shared_memory: Option<Arc<SharedMemory>>,
    consensus: Option<Arc<Consensus>>,
}

impl CoordinatedGroup {
    // ── Builder ──
    pub fn builder() -> CoordinatedGroupBuilder { ... }

    // ── 전략 ──

    /// Fan-out: 작업을 큐에 넣고 각 에이전트가 claim하여 병렬 처리
    pub async fn fan_out(
        &self,
        work_type: &str,
        payloads: Vec<Value>,
    ) -> Vec<WorkResult>;

    /// 투표: 각 에이전트가 질문에 대해 옵션을 선택
    pub async fn vote(
        &self,
        question: &str,
        options: &[&str],
    ) -> Option<VoteResult>;

    /// Map-reduce: 각 에이전트가 payload를 처리하고 결과를 SharedMemory에 취합
    pub async fn map_reduce(
        &self,
        work_type: &str,
        payloads: Vec<Value>,
        reduce_key: &MemoryKey,
    ) -> anyhow::Result<Vec<WorkResult>>;
}
```

**`fan_out` 구현 (수정됨):**

```rust
pub async fn fan_out(&self, work_type: &str, payloads: Vec<Value>) -> Vec<WorkResult> {
    let queue = self.work_queue.as_ref().expect("WorkQueue required");

    // 1. 모든 작업 등록
    for payload in &payloads {
        queue.enqueue(work_type, payload.clone(), 0);
    }

    // 2. 각 에이전트가 claim + 실행 (agent_id로 식별)
    let mut handles = Vec::new();
    for handle in &self.handles {
        let queue = Arc::clone(queue);
        let handle = handle.clone();

        handles.push(tokio::spawn(async move {
            // AgentHandle의 고유 ID로 claim
            let item = queue.claim(handle.agent_id(), None);
            if let Some(item) = item {
                queue.start(&item.id).ok();
                let prompt = format!("Complete this task:\n{}",
                    serde_json::to_string_pretty(&item.payload).unwrap());
                let start = std::time::Instant::now();
                match handle.run(prompt).await {
                    Ok((response, _)) => {
                        let result = WorkResult { success: true, content: response.content, ... };
                        queue.complete(&item.id, result.clone()).ok();
                        Some(result)
                    }
                    Err(e) => {
                        let result = WorkResult { success: false, error: Some(e.to_string()), ... };
                        queue.complete(&item.id, result.clone()).ok();
                        Some(result)
                    }
                }
            } else {
                None
            }
        }));
    }

    // 3. 결과 수집
    let mut results = Vec::new();
    for handle in handles {
        if let Ok(Some(result)) = handle.await {
            results.push(result);
        }
    }
    results
}
```

---

## 3.6 상호작용 다이어그램

```
Coordinator(AgentHandle-0)  WorkQueue  Worker(AgentHandle-1)  SharedMemory
      │                        │              │                    │
      │── enqueue(task1) ─────▶│              │                    │
      │── enqueue(task2) ─────▶│              │                    │
      │                        │              │                    │
      │                        │◀─ claim(id1) ──── handle.agent_id()│
      │                        │── WorkItem ──▶│                    │
      │                        │              │                    │
      │                        │              │── handle.run() ──▶ │
      │                        │              │                    │
      │                        │◀─ complete() ─│                    │
      │                        │              │                    │
      │◀── WorkEvent ──────────│              │                    │
      │                        │              │                    │
      │── write("results", ...) ──────────────────────────────────▶│
      │                        │              │                    │
      │                        │              │◀── read("results") │
```

---

## 3.7 사용 예시

```rust
use oxicode_sdk::coordination::*;

// 1. 프리미티브 생성
let queue = Arc::new(WorkQueue::new(WorkQueueConfig::default()));
let memory = Arc::new(SharedMemory::new());
let consensus = Arc::new(Consensus::new());

// 2. AgentHandle들을 그룹화
let group = CoordinatedGroup::builder()
    .handle(handle_1)
    .handle(handle_2)
    .handle(handle_3)
    .work_queue(queue.clone())
    .shared_memory(memory.clone())
    .consensus(consensus.clone())
    .build();

// 3. Fan-out: 3개의 파일 리뷰를 병렬로
let results = group.fan_out("code_review", vec![
    json!({"file": "src/main.rs"}),
    json!({"file": "src/lib.rs"}),
    json!({"file": "src/config.rs"}),
]).await;

// 4. SharedMemory로 중간 결과 공유
memory.write(
    &MemoryKey::new("reviews", "summary"),
    json!({"total": results.len(), "issues_found": 5}),
    "coordinator", None,
)?;

// 5. 투표로 결정
let decision = group.vote(
    "Should we refactor the error handling?",
    &["yes", "no", "defer"],
).await;
```
