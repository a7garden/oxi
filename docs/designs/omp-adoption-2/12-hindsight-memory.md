# 세부 설계 ⑨ — Hindsight 메모리 (응용 계층 + mental-models)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §4·§8·§9 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md), 1차 [`04-hindsight-memory.md`](../omp-adoption/04-hindsight-memory.md), [`11-mnemopi-backend.md`](./11-mnemopi-backend.md)
> omp 분석: `hindsight/` (~2,000줄), `tools/memory-*.ts` (4개 + learn)
> 후속: N3 구현 → CHANGELOG.md
> 짝: [`11-mnemopi-backend.md`](./11-mnemopi-backend.md) (스토리지 계층)
>
> **⚠️ v2**: bank 스코핑을 `find_git_root` 기반으로 수정. mental-models 백엔드 의존을 ⑩에 명시. `MemoryBackend` 기반 도구 설계. `complete_text` → `complete` 시그니처 수정.

---

## 0. 핵심 (TL;DR)

omp의 **Hindsight**는 세션 간 메모리 시스템으로, AgentSession 수명 주기에 묶인 `HindsightSessionState`를 중심으로 동작한다. 3가지 축:

1. **백그라운드 자동화** — 첫 턴 자동 recall, 매 N턴 종료 자동 retain, 부트 시 mental-model 로드.
2. **4개 LLM 도구** — `retain`/`recall`/`reflect`/`memory_edit`.
3. **2종 주입 블록** — `<memories>`(휘발성 recall 결과) + `<mental_models>`(안정적 큐레이션).

**1차 ④ + 2차 ⑩과의 관계**: 1차 ④가 `MemoryStore` 포트를 정의하고, 2차 ⑩이 Mnemopi 백엔드를 구현했다면, **본 설계는 그 위의 응용 계층**이다 — 도구, 자동화, mental-models, 프롬프트 주입.

### omp가 검증한 가치
- **재학습 비용 제거** — 빌드 명령, 프로젝트 구조, 사용자 선호를 세션마다 재발견하지 않음.
- **mental-models** — 세션 종료 시 요약을 압축된 "정신 모델"으로 큐레이션. 다음 세션 첫 턴에 주입.
- **retain 디바운스** — 도구 호출 retain을 배치 처리 (16개 또는 5초).
- **recall 1500ms 데드라인** — 부트 시 mental-model 로드가 늦으면 타임아웃하고 빈 블록으로 진행.

---

## 1. omp 메커니즘

### 1.1 HindsightSessionState (`hindsight/state.ts`, 493줄)

세션 단위 상태 관리의 핵심:

```typescript
class HindsightSessionState {
    sessionId: string;
    client: HindsightApi;
    bankId: string;
    retainTags: string[];
    recallTags: string[];
    config: HindsightConfig;
    lastRetainedTurn: number;
    hasRecalledForFirstTurn: boolean;
    lastRecallSnippet: string;
    mentalModelsSnippet: string;
    mentalModelsLoadedAt: number;
    retainQueue: HindsightRetainQueue;
    
    // 핵심 메서드
    recallForContext(query): Promise<string>;        // recall API → <memories> 블록
    retainSession(messages): Promise<void>;          // 세션 요약 저장
    maybeRetainOnAgentEnd(): Promise<void>;          // 자동 retain (매 N턴)
    maybeRecallOnAgentStart(): Promise<void>;        // 첫 턴 자동 recall
    beforeAgentStartPrompt(prompt): Promise<string>; // mental-model 주입
    runMentalModelLoad(scope): Promise<void>;        // mental-model 로드
    refreshMentalModelsSnippet(): Promise<void>;     // 스니펫 갱신
}
```

### 1.2 설정 (`hindsight/config.ts`, 187줄)

```typescript
interface HindsightConfig {
    bankId: string;
    bankIdPrefix: string;
    scoping: "global" | "per-project" | "per-project-tagged";
    bankMission: string;
    
    autoRecall: boolean;            // 첫 턴 자동 recall, 기본 true
    autoRetain: boolean;            // 매 N턴 자동 retain, 기본 true
    retainMode: "full-session" | "last-turn";
    retainEveryNTurns: number;      // 기본 3
    retainOverlapTurns: number;     // 기본 2
    
    recallBudget: "low" | "mid" | "high";  // 기본 mid
    recallMaxTokens: number;        // 기본 1024
    recallTypes: string[];          // 기본 ["world", "experience"]
    recallContextTurns: number;     // 기본 1
    recallMaxQueryChars: number;    // 기본 800
    
    mentalModelsEnabled: boolean;   // 기본 true
    mentalModelAutoSeed: boolean;   // 기본 true
    mentalModelRefreshIntervalMs: number;  // 기본 300000 (5분)
    mentalModelMaxRenderChars: number;     // 기본 16000
}
```

### 1.3 retain 디바운스 큐 (`HindsightRetainQueue`)

```typescript
class HindsightRetainQueue {
    RETAIN_FLUSH_BATCH_SIZE = 16;
    RETAIN_FLUSH_INTERVAL_MS = 5000;
    
    queue: MemoryItem[] = [];
    flushTimer?: NodeJS.Timeout;
    
    enqueue(item: MemoryItem): void {
        this.queue.push(item);
        if (this.queue.length >= RETAIN_FLUSH_BATCH_SIZE) {
            this.flush();
        } else if (!this.flushTimer) {
            this.flushTimer = setTimeout(() => this.flush(), RETAIN_FLUSH_INTERVAL_MS);
            this.flushTimer.unref?.();
        }
    }
    
    async flush(): Promise<void> {
        if (this.queue.length === 0) return;
        const batch = this.queue.splice(0);
        try {
            await this.client.retainBatch(batch, { async: true });
        } catch (e) {
            // flush 실패 시 UI 알림 (LLM에게는 비노출)
            this.session.emitNotice("warning", "Memory retain failed");
        }
    }
}
```

### 1.4 mental-models (`hindsight/mental-models.ts`, 429줄)

세션 종료 시 요약을 압축된 "정신 모델"로 큐레이션:

```
세션 종료
  → 세션 엔트리에서 핵심 사실 추출
  → 기존 mental-model 스냅샷 로드
  → 새 사실을 기존 모델에 통합 (LLM 호출)
  → 새 mental-model 스냅샷 저장
  → 다음 세션 첫 턴에 <mental_models> 블록으로 주입
```

- **비동기 operation**: `createMentalModel`이 `operation_id` 반환, 폴링으로 완료 확인.
- **TTL 갱신**: `mentalModelRefreshIntervalMs`(5분)마다 스니펫 갱신.
- **히스토리**: 이전 스냅샷 보존 (`getMentalModelHistory`).
- **auto-seed**: 첫 세션이면 seed 메모리에서 초기 모델 생성.

### 1.5 프롬프트 주입

두 종류 블록을 시스템 프롬프트(또는 개발자 지시문)에 주입:

```xml
<memories>
Current time: 2026-06-19T10:00:00Z

1. [fact] The project uses Rust 2024 edition with a 5-crate workspace.
2. [preference] User prefers Korean for TUI prose, English for code.
3. [experience] Last session: refactored auth module to use port pattern.
</memories>

<mental_models>
## Project Architecture
oxi is a Rust port of pi-mono with 5 crates: oxi-ai (foundation), oxi-agent
(runtime), oxi-tui (widgets), oxi-sdk (ports), oxi-cli (binary)...

## User Preferences
- Korean prose, English code
- Prefers port-based architecture
- Uses parking_lot over std::sync
</mental_models>
```

### 1.6 4개 도구

| 도구 | omp 파일 | 동작 |
|---|---|---|
| `retain` | `tools/memory-retain.ts` (89) | 메모리 저장 (사실/선호/컨텍스트). retainQueue에 enqueue |
| `recall` | `tools/memory-recall.ts` (102) | 의미 검색 → 결과를 도구 출력으로 반환 |
| `reflect` | `tools/memory-reflect.ts` (88) | 세션 종료 시 요약 자동 저장 (별도 LLM 호출) |
| `memory_edit` | `tools/memory-edit.ts` (59) | 기존 메모리 갱신/삭제 (Mnemopi 전용) |
| `learn` | `tools/learn.ts` (141) | 능동 학습 (사용자가 에이전트에게 가르치기) |

### 1.7 bank 스코핑 (`hindsight/bank.ts`, 134줄)

```typescript
type HindsightScoping = "global" | "per-project" | "per-project-tagged";

// global: 공유 bank, 태그 없음
// per-project: bank별 분리 ({prefix}-{bank}-{projectLabel})
// per-project-tagged: 단일 공유 bank + project:{name} 태그
//   recallTagsMatch='any' → 비태그 'global' 메모리도 함께 노출

function projectLabel(cwd: string): string {
    return basename(cwd);  // git 루트 basename (동일 repo worktree는 같은 태그)
}
```

---

## 2. oxi화 설계

### 2.1 모듈 구조

```
oxi-cli/src/hindsight/
├── mod.rs              HindsightSessionState + 공개 API
├── config.rs           HindsightConfig (환경변수 + settings)
├── state.rs            세션 상태 관리 (자동 recall/retain)
├── mental_models.rs    mental-model 큐레이션
├── retain_queue.rs     디바운스 배치 큐
├── prompt.rs           <memories> / <mental_models> 블록 조립
└── bank.rs             bank 스코핑 (⑩ Mnemopi BankManager 위임)
```

### 2.2 HindsightSessionState (`state.rs`)

```rust
pub struct HindsightSessionState {
    session_id: String,
    memory: Arc<Mnemopi>,               // ⑩ 백엔드
    config: HindsightConfig,
    bank_scope: BankScope,
    
    last_retained_turn: AtomicU32,
    has_recalled_first_turn: AtomicBool,
    last_recall_snippet: parking_lot::RwLock<String>,
    mental_models_snippet: parking_lot::RwLock<String>,
    mental_models_loaded_at: parking_lot::RwLock<Option<Instant>>,
    
    retain_queue: Arc<HindsightRetainQueue>,
}

impl HindsightSessionState {
    pub fn new(
        session_id: String,
        memory: Arc<Mnemopi>,
        config: HindsightConfig,
        cwd: &Path,
    ) -> Self {
        let bank_scope = compute_bank_scope(&config, cwd);
        Self {
            session_id,
            memory,
            config,
            bank_scope,
            last_retained_turn: AtomicU32::new(0),
            has_recalled_first_turn: AtomicBool::new(false),
            last_recall_snippet: Default::default(),
            mental_models_snippet: Default::default(),
            mental_models_loaded_at: Default::default(),
            retain_queue: Arc::new(HindsightRetainQueue::new(/* ... */)),
        }
    }
    
    /// 첫 턴 자동 recall. 시스템 프롬프트에 <memories> 블록 주입.
    pub async fn maybe_recall_on_start(&self, first_prompt: &str) -> anyhow::Result<String> {
        if !self.config.auto_recall || self.has_recalled_first_turn.load(Ordering::SeqCst) {
            return Ok(first_prompt.to_string());
        }
        
        let results = self.memory.recall(first_prompt, self.config.recall_budget_count(), 
            RecallOptions::default()).await?;
        
        let snippet = if results.is_empty() {
            String::new()
        } else {
            format_memories_block(&results)
        };
        
        *self.last_recall_snippet.write() = snippet.clone();
        self.has_recalled_first_turn.store(true, Ordering::SeqCst);
        
        // 프롬프트에 주입
        Ok(inject_memories_block(first_prompt, &snippet))
    }
    
    /// 매 턴 종료 시 자동 retain 검사.
    pub async fn maybe_retain_on_end(&self, turn: u32, messages: &[Message]) -> anyhow::Result<()> {
        if !self.config.auto_retain { return Ok(()); }
        
        let last = self.last_retained_turn.load(Ordering::SeqCst);
        if turn - last < self.config.retain_every_n_turns as u32 { return Ok(()); }
        
        self.retain_session(messages).await?;
        self.last_retained_turn.store(turn, Ordering::SeqCst);
        Ok(())
    }
    
    /// 세션 요약 저장. retainMode에 따라 전체 또는 마지막 턴.
    pub async fn retain_session(&self, messages: &[Message]) -> anyhow::Result<()> {
        let content = match self.config.retain_mode {
            RetainMode::FullSession => serialize_full_session(messages),
            RetainMode::LastTurn => serialize_last_turns(messages, self.config.retain_overlap_turns),
        };
        
        let document_id = match self.config.retain_mode {
            RetainMode::FullSession => self.session_id.clone(),
            RetainMode::LastTurn => format!("{}-{}", self.session_id, turn_epoch()),
        };
        
        self.retain_queue.enqueue(MemoryItem {
            content,
            document_id,
            tags: self.bank_scope.retain_tags.clone(),
            scope: self.bank_scope.bank_id.clone(),
            ..Default::default()
        }).await;
        
        Ok(())
    }
}
```

### 2.3 retain 디바운스 큐 (`retain_queue.rs`)

```rust
const RETAIN_FLUSH_BATCH_SIZE: usize = 16;
const RETAIN_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

pub struct HindsightRetainQueue {
    queue: tokio::sync::Mutex<Vec<MemoryItem>>,
    memory: Arc<Mnemopi>,
    flush_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    notice_tx: Option<tokio::sync::mpsc::UnboundedSender<Notice>>,
}

impl HindsightRetainQueue {
    pub async fn enqueue(&self, item: MemoryItem) {
        let mut queue = self.queue.lock().await;
        queue.push(item);
        
        if queue.len() >= RETAIN_FLUSH_BATCH_SIZE {
            drop(queue);
            self.flush().await;
        } else {
            // 디바운스 타이머 설정
            let mut handle = self.flush_handle.lock().await;
            if handle.is_none() {
                let queue = Arc::clone(&self.memory_arc());
                let notice_tx = self.notice_tx.clone();
                let h = tokio::spawn(async move {
                    tokio::time::sleep(RETAIN_FLUSH_INTERVAL).await;
                    // flush 호출
                });
                *handle = Some(h);
            }
        }
    }
    
    pub async fn flush(&self) {
        let batch = {
            let mut queue = self.queue.lock().await;
            if queue.is_empty() { return; }
            queue.drain(..).collect::<Vec<_>>()
        };
        
        // 배치 저장 (Mnemopi remember_batch)
        if let Err(e) = self.memory.remember_batch(&batch).await {
            if let Some(tx) = &self.notice_tx {
                let _ = tx.send(Notice::warning(format!("Memory retain failed: {}", e)));
            }
            // 실패 시 아이템을 큐로 되돌리지 않음 — omp 동작 (drop)
        }
    }
}
```

### 2.4 mental-models (`mental_models.rs`)

```rust
pub struct MentalModelManager {
    memory: Arc<Mnemopi>,
    config: HindsightConfig,
    current_snippet: parking_lot::RwLock<String>,
    loaded_at: parking_lot::RwLock<Option<Instant>>,
}

impl MentalModelManager {
    /// 부트 시 mental-model 로드. 1500ms 데드라인.
    pub async fn load_on_boot(&self, scope: &str) -> anyhow::Result<()> {
        let load_future = self.fetch_or_seed(scope);
        
        match tokio::time::timeout(Duration::from_millis(1500), load_future).await {
            Ok(Ok(snippet)) => {
                *self.current_snippet.write() = snippet;
                *self.loaded_at.write() = Some(Instant::now());
            }
            Ok(Err(e)) => {
                tracing::warn!("Mental model load failed: {}", e);
                // 빈 스니펫으로 진행 — recall이 복구
            }
            Err(_) => {
                tracing::warn!("Mental model load timed out (1500ms)");
                // 백그라운드에서 계속 로드, 다음 턴에 사용
            }
        }
        
        Ok(())
    }
    
    async fn fetch_or_seed(&self, scope: &str) -> anyhow::Result<String> {
        // 1. 기존 mental-model 스냅샷 조회
        let existing = self.memory.get_mental_model(scope).await?;
        
        if let Some(model) = existing {
            // TTL 확인
            if let Some(loaded_at) = *self.loaded_at.read() {
                if loaded_at.elapsed() < Duration::from_millis(self.config.mm_refresh_interval_ms) {
                    return Ok(self.current_snippet.read().clone());
                }
            }
            // 갱신
            return self.refresh(scope).await;
        }
        
        // 2. auto-seed: seed 메모리에서 초기 모델 생성
        if self.config.mental_model_auto_seed {
            return self.seed_initial_model(scope).await;
        }
        
        Ok(String::new())
    }
    
    /// 세션 종료 시 mental-model 갱신.
    pub async fn refresh_on_session_end(&self, session_summary: &str) -> anyhow::Result<()> {
        let existing = self.current_snippet.read().clone();
        
        // LLM으로 새 사실을 기존 모델에 통합
        let updated = self.integrate_with_llm(&existing, session_summary).await?;
        
        // 새 스냅샷 저장
        self.memory.save_mental_model(scope, &updated).await?;
        
        *self.current_snippet.write() = updated;
        *self.loaded_at.write() = Some(Instant::now());
        
        Ok(())
    }
    
    async fn integrate_with_llm(&self, existing: &str, new_facts: &str) -> anyhow::Result<String> {
        // 별도 LLM 호출 — 기존 mental-model + 새 세션 요약을 통합
        let prompt = format!(
            "Update the mental model with new information from this session.\n\n\
             Existing model:\n{}\n\n\
             New session summary:\n{}\n\n\
             Produce an updated mental model (max {} chars).",
            existing, new_facts, self.config.mm_max_render_chars
        );
        
        let response = oxi_ai::high_level::complete_text(
            &self.llm_provider, &self.llm_model, &prompt, 2000
        ).await?;
        
        Ok(response)
    }
}
```

### 2.5 프롬프트 주입 (`prompt.rs`)

```rust
/// <memories> 블록 조립.
pub fn format_memories_block(results: &[RecallResult]) -> String {
    if results.is_empty() { return String::new(); }
    
    let mut out = String::from("<memories>\n");
    out.push_str(&format!("Current time: {}\n\n", chrono::Utc::now().to_rfc3339()));
    
    for (i, r) in results.iter().enumerate() {
        let kind = r.kind.as_deref().unwrap_or("fact");
        out.push_str(&format!("{}. [{}] {}\n", i + 1, kind, r.content));
    }
    
    out.push_str("</memories>");
    out
}

/// <mental_models> 블록 조립.
pub fn format_mental_models_block(snippet: &str) -> String {
    if snippet.is_empty() { return String::new(); }
    
    format!("<mental_models>\n{}\n</mental_models>", snippet)
}

/// 시스템 프롬프트에 memories + mental_models 블록 주입.
pub fn inject_into_system_prompt(
    system_prompt: &str,
    memories: &str,
    mental_models: &str,
) -> String {
    let mut result = system_prompt.to_string();
    
    if !memories.is_empty() {
        result.push_str("\n\n");
        result.push_str(memories);
    }
    
    if !mental_models.is_empty() {
        result.push_str("\n\n");
        result.push_str(mental_models);
    }
    
    result
}
```

### 2.6 bank 스코핑 (`bank.rs`)

```rust
pub struct BankScope {
    pub bank_id: String,
    pub retain_tags: Vec<String>,
    pub recall_tags: Vec<String>,
    pub recall_tags_match: TagMatch,
}

#[derive(Debug, Clone, Copy)]
pub enum TagMatch {
    All,  // 모든 태그 일치
    Any,  // 하나라도 일치
}

pub fn compute_bank_scope(config: &HindsightConfig, cwd: &Path) -> BankScope {
    // v2: git 루트 기반 project label (omp #2412 수정 반영)
    // oxi-cli::storage::find_git_root 재사용
    let project_label = oxi_cli::storage::find_git_root(cwd)
        .and_then(|root| root.file_name()?.to_str().map(String::from))
        .or_else(|| cwd.file_name().and_then(|n| n.to_str().map(String::from))
        .unwrap_or_else(|| "default".into());
    
    let global_bank = config.bank_id.clone();
    
    match config.scoping {
        HindsightScoping::Global => BankScope {
            bank_id: global_bank,
            retain_tags: vec![],
            recall_tags: vec![],
            recall_tags_match: TagMatch::All,
        },
        HindsightScoping::PerProject => {
            let project_bank = format!("{}-{}", config.bank_id_prefix, project_label);
            BankScope {
                bank_id: project_bank.clone(),
                retain_tags: vec![],
                recall_tags: vec![],
                recall_tags_match: TagMatch::All,
            }
        },
        HindsightScoping::PerProjectTagged => {
            let project_tag = format!("project:{}", project_label);
            BankScope {
                bank_id: global_bank.clone(),
                retain_tags: vec![project_tag.clone()],
                recall_tags: vec![project_tag],  // global(비태그)도 함께 노출 (Any)
                recall_tags_match: TagMatch::Any,
            }
        },
    }
}
```

---

## 3. 4개 메모리 도구 (oxi-agent)

### 3.1 retain 도구

`oxi-agent/src/tools/memory_retain.rs`:

```rust
pub struct MemoryRetainTool {
    hindsight: Option<Arc<HindsightSessionState>>,
}

impl AgentTool for MemoryRetainTool {
    fn name(&self) -> &str { "memory_retain" }
    fn essential(&self) -> bool { false }
    fn description(&self) -> &str {
        "Save a fact, preference, or context to project memory. \
         Persists across sessions. Project-scoped by default."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "The fact/preference/context to remember"},
                "kind": {"type": "string", "enum": ["fact", "preference", "context", "summary"]},
                "importance": {"type": "number", "minimum": 0.0, "maximum": 1.0, "description": "0=trivial, 1=critical"}
            },
            "required": ["content"]
        })
    }
    
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        // v2: MemoryBackend 능력 사용 (ToolContext.memory)
        let backend = ctx.memory.as_ref()
            .ok_or("Memory not configured")?;  // ToolError = String

        let content = params["content"].as_str()
            .ok_or("content required")?;
        let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("fact");
        let importance = params.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5);
        
        // retainQueue에 enqueue (디바운스)
        hindsight.retain_queue.enqueue(MemoryItem {
            content,
            kind: kind.into(),
            importance,
            scope: hindsight.bank_scope.bank_id.clone(),
            tags: hindsight.bank_scope.retain_tags.clone(),
            ..Default::default()
        }).await;
        
        Ok(AgentToolResult::success(format!("Retained [{}] to memory.", kind)))
    }
}
```

### 3.2 recall 도구

```rust
pub struct MemoryRecallTool {
    hindsight: Option<Arc<HindsightSessionState>>,
}

impl AgentTool for MemoryRecallTool {
    fn name(&self) -> &str { "memory_recall" }
    fn description(&self) -> &str {
        "Search project memory for relevant facts, preferences, and context. \
         Semantic search across all past sessions."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Natural language query"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Max results"}
            },
            "required": ["query"]
        })
    }
    
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        let hindsight = self.hindsight.as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("Memory not configured".into()))?;
        
        let query = params["query"].as_str()
            .ok_or(ToolError::InvalidParams("query required".into()))?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        
        let results = hindsight.memory.recall(query, limit, RecallOptions::default()).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        
        if results.is_empty() {
            return Ok(AgentToolResult::success("No memories found for this query."));
        }
        
        let formatted = results.iter()
            .enumerate()
            .map(|(i, r)| format!("{}. [{}] {}", i + 1, r.kind.as_deref().unwrap_or("fact"), r.content))
            .collect::<Vec<_>>()
            .join("\n");
        
        Ok(AgentToolResult::success(format!("Found {} memories:\n\n{}", results.len(), formatted)))
    }
}
```

### 3.3 reflect 도구

```rust
pub struct MemoryReflectTool {
    hindsight: Option<Arc<HindsightSessionState>>,
}

impl AgentTool for MemoryReflectTool {
    fn name(&self) -> &str { "memory_reflect" }
    fn description(&self) -> &str {
        "Summarize the current session and save to memory. \
         Call at the end of a productive session to preserve learnings."
    }
    
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        let hindsight = self.hindsight.as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("Memory not configured".into()))?;
        
        // 세션 메시지에서 요약 생성 (별도 LLM 호출)
        let messages = ctx.session_messages;
        let summary = generate_session_summary(ctx.provider, ctx.model, messages).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        
        // mental-model 갱신
        hindsight.mental_models.refresh_on_session_end(&summary).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        
        // 요약을 메모리에 저장
        hindsight.memory.remember(&summary, RememberOptions {
            kind: "summary".into(),
            scope: hindsight.bank_scope.bank_id.clone(),
            ..Default::default()
        }).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        
        Ok(AgentToolResult::success("Session reflected to memory. Mental model updated."))
    }
}
```

### 3.4 memory_edit 도구

```rust
pub struct MemoryEditTool {
    hindsight: Option<Arc<HindsightSessionState>>,
}

impl AgentTool for MemoryEditTool {
    fn name(&self) -> &str { "memory_edit" }
    fn description(&self) -> &str {
        "Update or delete an existing memory by ID. Use to correct outdated facts."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "memory_id": {"type": "string"},
                "action": {"type": "string", "enum": ["update", "delete", "invalidate"]},
                "content": {"type": "string", "description": "New content (for update)"}
            },
            "required": ["memory_id", "action"]
        })
    }
    
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        let hindsight = self.hindsight.as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("Memory not configured".into()))?;
        
        let id = params["memory_id"].as_str()
            .ok_or(ToolError::InvalidParams("memory_id required".into()))?;
        let action = params["action"].as_str()
            .ok_or(ToolError::InvalidParams("action required".into()))?;
        
        match action {
            "update" => {
                let content = params["content"].as_str()
                    .ok_or(ToolError::InvalidParams("content required for update".into()))?;
                hindsight.memory.update(id, Some(content), None).await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                Ok(AgentToolResult::success("Memory updated."))
            }
            "delete" => {
                hindsight.memory.forget(id).await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                Ok(AgentToolResult::success("Memory deleted."))
            }
            "invalidate" => {
                hindsight.memory.invalidate(id).await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                Ok(AgentToolResult::success("Memory invalidated."))
            }
            _ => Err(ToolError::InvalidParams(format!("Unknown action: {}", action))),
        }
    }
}
```

---

## 4. 에이전트 루프 통합

### 4.1 세션 시작 훅

```rust
// agent_session.rs — 세션 시작
if let Some(hindsight) = &self.hindsight {
    // 1. mental-model 로드 (1500ms 데드라인)
    if let Err(e) = hindsight.mental_models.load_on_boot(&scope).await {
        tracing::warn!("Mental model load failed: {}", e);
    }
    
    // 2. 첫 턴 recall
    if let Err(e) = hindsight.maybe_recall_on_start(&first_prompt).await {
        tracing::warn!("Auto recall failed: {}", e);
    }
}
```

### 4.2 턴 종료 훅

```rust
// agent_session.rs — 매 턴 종료
if let Some(hindsight) = &self.hindsight {
    if let Err(e) = hindsight.maybe_retain_on_end(turn_number, &messages).await {
        tracing::warn!("Auto retain failed: {}", e);
    }
}
```

### 4.3 세션 종료 훅

```rust
// agent_session.rs — 세션 종료
if let Some(hindsight) = &self.hindsight {
    // retain queue flush
    hindsight.retain_queue.flush().await;
    
    // mental-model 갱신 (설정 시)
    if self.settings.memory_reflect {
        if let Err(e) = hindsight.reflect_session(&messages).await {
            tracing::warn!("Reflect failed: {}", e);
        }
    }
}
```

---

## 5. 설정

```rust
pub struct Settings {
    // 1차 ④
    pub memory_enabled: bool,             // 기본 false
    
    // 2차 ⑨ (Hindsight 응용)
    pub memory_auto_recall: bool,         // 기본 true
    pub memory_auto_retain: bool,         // 기본 true
    pub memory_retain_every_n_turns: u32, // 기본 3
    pub memory_retain_mode: RetainMode,   // 기본 LastTurn
    pub memory_recall_budget: RecallBudget, // 기본 Mid
    
    // mental-models
    pub memory_mental_models_enabled: bool,  // 기본 true
    pub memory_mental_model_auto_seed: bool, // 기본 true
    pub memory_mental_model_refresh_interval: Duration, // 기본 5분
    pub memory_reflect: bool,             // 세션 종료 자동 reflect, 기본 false
    
    // bank 스코핑
    pub memory_scoping: HindsightScoping, // 기본 PerProjectTagged
}
```

---

## 6. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N3.20 | `HindsightConfig` + 환경변수 | ⑩ N3.13 |
| N3.21 | `BankScope` + `compute_bank_scope` | ⑩ N3.15 |
| N3.22 | `HindsightRetainQueue` (디바운스 배치) | ⑩ N3.13 |
| N3.23 | `MemoryRetainTool` (oxi-agent) | N3.22 |
| N3.24 | `MemoryRecallTool` (oxi-agent) | ⑩ N3.9 |
| N3.25 | `MemoryReflectTool` (oxi-agent) | N3.24 |
| N3.26 | `MemoryEditTool` (oxi-agent) | ⑩ N3.7 |
| N3.27 | `format_memories_block` + `format_mental_models_block` | — |
| N3.28 | `inject_into_system_prompt` | N3.27 |
| N3.29 | `HindsightSessionState` (자동 recall/retain) | N3.22, N3.28 |
| N3.30 | 에이전트 루프 통합 (시작/턴 종료/세션 종료 훅) | N3.29 |
| N3.31 | `MentalModelManager` (fetch/seed/refresh) | ⑩ N3.13 |
| N3.32 | mental-model LLM 통합 (요약 압축) | N3.31 |
| N3.33 | 1500ms 부트 데드라인 | N3.31 |
| N3.34 | `/memory` 슬래시 명령 (view/stats/diagnose) | N3.29 |
| N3.35 | `learn` 도구 (능동 학습 UX) | N3.23 |

> **⑩ 의존**: 본 설계의 모든 도구/자동화는 ⑩ Mnemopi 백엔드가 구현된 후 동작.
> **1차 ④ 연동**: `MemoryStore` 포트를 ⑩이 충전하고, 본 설계가 그 위의 응용을 구현.

---

## 7. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| mental-model 요약 품질 (LLM 의존) | 🟢 별도 모델 | `smol` 역할 모델 사용. 비용 토글 |
| retain 디바운스 타이머 (tokio::spawn) | 🟢 해결 | `tokio::time::sleep` + abort handle |
| 1500ms 부트 데드라인 | 🟢 이식 | omp 계약. 타임아웃 시 빈 블록 |
| per-project-tagged bank 스코핑 | 🟡 검증 | global 메모리와 프로젝트 메모리 혼합. recallTagsMatch='any' |
| reflect 비용 (세션 종료 LLM 호출) | 🟠 위험 | `memory_reflect` 기본 false. opt-in |
| learn 도구 UX (대화형 가르치기) | 🔴 후순위 | N3.35. 별도 UX 설계 필요 |
| memories 블록 컨텍스트 소모 | 🟡 모니터 | recallMaxTokens=1024 기본. 긴 세션에서 비용 |

---

## 8. 부록: omp → oxi 매핑

| omp 위치 | oxi 위치 |
|---|---|
| `hindsight/state.ts` (493) | `oxi-cli/src/hindsight/state.rs` |
| `hindsight/config.ts` (187) | `oxi-cli/src/hindsight/config.rs` |
| `hindsight/client.ts` (624) | ⑩ Mnemopi 직접 호출 (HTTP 클라이언트 불필요) |
| `hindsight/bank.ts` (134) | `oxi-cli/src/hindsight/bank.rs` (⑩ BankManager 위임) |
| `hindsight/mental-models.ts` (429) | `oxi-cli/src/hindsight/mental_models.rs` |
| `hindsight/content.ts` (210) | `oxi-cli/src/hindsight/prompt.rs` |
| `hindsight/transcript.ts` (71) | `oxi-cli/src/hindsight/state.rs` (통합) |
| `tools/memory-retain.ts` (89) | `oxi-agent/src/tools/memory_retain.rs` |
| `tools/memory-recall.ts` (102) | `oxi-agent/src/tools/memory_recall.rs` |
| `tools/memory-reflect.ts` (88) | `oxi-agent/src/tools/memory_reflect.rs` |
| `tools/memory-edit.ts` (59) | `oxi-agent/src/tools/memory_edit.rs` |
| `tools/learn.ts` (141) | `oxi-agent/src/tools/learn.rs` (후순위 N3.35) |
| `tools/memory-render.ts` (202) | `oxi-tui/src/widgets/tool_renderer.rs` (memory 분기) |
| `mnemopi/state.ts` (630, coding-agent 래퍼) | `oxi-cli/src/hindsight/state.rs` (통합) |
