# 세부 설계 ④ — Hindsight 메모리 (MemoryStore 충전)

> 상태: 설계 v1 (구현 전 합의용)
> 작성: 2026-06-19
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `packages/mnemopi/` (SQLite 백엔드), `tools/memory-*.ts` (4개 도구 + learn)
> 후속: M2b 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

omp의 **Hindsight**는 세션 간 메모리(retain/recall/reflect/learn)로, 에이전트가 코드베이스 사실을 기억한다. 프로젝트 스코프 — 이 repo에서 배운 것이 다른 repo로 새지 않는다.

**oxi의 핵심 자산**: `MemoryStore` 포트(`ports/mod.rs:683`)가 이미 `put/search/list`를 정의하고 `NoopMemoryStore` 폴백을 갖춘다. **포트만 충전하면 됨** — oxi 도입 기능 중 가장 적은 설계 변경.

### omp가 검증한 가치
- **재학습 비용 제거** — 빌드 명령, 프로젝트 구조, 사용자 선호를 세션마다 재발견하지 않음.
- **부트 시 자동 로드** — 세션 첫 턴에 `recall(project)` 결과를 시스템 프롬프트에 주입.
- **reflect 자동화** — 세션 종료 시 요약을 자동으로 메모리에 저장.

---

## 1. omp 메커니즘

### 1.1 mnemopi 백엔드 (`packages/mnemopi/`)

SQLite 기반 (`bun:sqlite`):
- PRAGMA: `foreign_keys=ON`, `busy_timeout=5000`, `journal_mode=WAL` (영구 DB만).
- 중첩 트랜잭션 지원 (`transaction()` 헬퍼, depth 카운팅).
- 마이그레이션 시스템 (`migrations/`).
- MCP 서버 포함 (`mcp-server.ts`, `mcp-tools.ts`) — 외부에서 메모리 접근.

### 1.2 4개 도구 + learn

| 도구 | omp 파일 | 용도 |
|---|---|---|
| `retain` | `tools/memory-retain.ts` | 메모리 저장 (사실/선호/컨텍스트) |
| `recall` | `tools/memory-recall.ts` | 의미 검색 (임베딩 코사인, top-k) |
| `reflect` | `tools/memory-reflect.ts` | 세션 종료 시 요약 자동 저장 |
| `memory_edit` | `tools/memory-edit.ts` | 기존 메모리 갱신/삭제 |
| `learn` | `tools/learn.ts` | 능동적 학습 (사용자가 가르치기) |

### 1.3 프로젝트 스코프

각 프로젝트의 학습이 해당 프로젝트에 한정. DB는 프로젝트별 분리 또는 `project_id` 컬럼으로 분할.

---

## 2. oxi화 설계

### 2.1 기존 포트 재활용 (변경 최소)

`oxi-sdk/src/ports/mod.rs:683` (이미 정의됨):

```rust
pub trait MemoryStore: Send + Sync + 'static {
    fn put<'a>(&'a self, entry: MemoryEntry)
        -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + 'a>>;
    fn search<'a>(&'a self, query: &[f32], k: usize)
        -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + 'a>>;  // 임베딩 코사인
    fn list<'a>(&'a self, subject: &str)
        -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + 'a>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Option<String>,
    pub subject: String,                  // project_id / scope
    pub kind: String,                     // "fact" | "preference" | "context" | "summary"
    pub content: String,
    pub embedding: Option<Vec<f32>>,      // search용
    pub created_at: Option<String>,
    pub metadata: Option<PortValue>,
}
```

> **이미 있는 계약을 그대로 사용**. 포트는 noop이므로, 미충전 시 4개 도구가 "사용 불가" 응답 → 점진적 강화 정합.

### 2.2 신규 구현: `SqliteMemoryStore`

`oxi-cli/src/store/memory_sqlite.rs`:

```rust
pub struct SqliteMemoryStore {
    db: tokio::sync::Mutex<rusqlite::Connection>,   // !Send 가드 회피 (AGENTS.md pitfall)
    embedder: Arc<dyn EmbeddingProvider>,
}

impl SqliteMemoryStore {
    pub async fn open(path: &Path, embedder: Arc<dyn EmbeddingProvider>) -> Result<Self> {
        let conn = rusqlite::Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        // WAL은 영구 DB만
        if path != Path::new(":memory:") {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        Self::migrate(&conn)?;
        Ok(Self { db: tokio::sync::Mutex::new(conn), embedder })
    }
}

#[async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn put(&self, entry: MemoryEntry) -> Result<(), SdkError> {
        let embedding = if entry.embedding.is_some() {
            entry.embedding.clone()
        } else {
            Some(self.embedder.embed(&entry.content).await?)
        };
        let db = self.db.lock().await;          // tokio::sync::Mutex → .await에 안전
        db.execute(
            "INSERT INTO memories (id, subject, kind, content, embedding, created_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![...],
        )?;
        Ok(())
    }
    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<MemoryEntry>, SdkError> {
        // 코사인 유사도 — 순수 SQL 또는 메모리에서 계산.
        // 소규모(수백~수천)에서는 SQL의 (a*b)/(||a||*||b||) 충분.
        // 대규모는 sqlite-vec 확장 검토 (M2b 이후).
        ...
    }
    async fn list(&self, subject: &str) -> Result<Vec<MemoryEntry>, SdkError> { ... }
}
```

**스키마** (`migrations/0001_init.sql`):
```sql
CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,              -- project_id
    kind TEXT NOT NULL,                 -- fact|preference|context|summary
    content TEXT NOT NULL,
    embedding BLOB,                     -- f32 little-endian 직렬화
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    metadata TEXT                       -- JSON
);
CREATE INDEX idx_memories_subject ON memories(subject);
CREATE INDEX idx_memories_kind ON memories(kind);
```

> **rusqlite 선택**: omp는 `bun:sqlite`. Rust 표준은 `rusqlite` (동기) + `tokio::sync::Mutex` 래핑으로 async-safe. AGENTS.md pitfall(`parking_lot::MutexGuard`는 `!Send` → `.await` 전에 drop) 회피 — `tokio::sync::Mutex` 사용.

### 2.3 임베딩 제공자 — 신규 포트 후보

`MemoryStore::search`는 `&[f32]` 쿼리를 받지만, **쿼리 텍스트 → 임베딩 변환**이 필요. 옵션:

| 옵션 | 설명 | 비고 |
|---|---|---|
| (a) 기존 채팅 제공자 embeddings API | OpenAI/Google 등 embeddings 엔드포인트 | 빠른 도입. oxi-ai 확장 |
| (b) 로컬 모델 | omp `tiny-models` (작은 임베딩 모델) | 오프라인. 의존 무거움 |
| (c) `EmbeddingProvider` 신규 포트 | 다중 제품 지원 | 가장 유연 but 설계 추가 |

**제안**: (c) 포트 정의 + (a) 기본 impl. 포트로 두면 oxios가 자체 임베딩 주입 가능.

```rust
// Port 14 (신규 후보) — EmbeddingProvider
pub trait EmbeddingProvider: Send + Sync + 'static {
    fn embed<'a>(&'a self, text: &'a str)
        -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SdkError>> + Send + 'a>>;
}
pub struct NoopEmbeddingProvider;   // Err(PortNotConfigured)
```

> **결정 필요**: 포트 추가 여부. M2b.1에서 합의. (a)만으로 시작하면 oxi-ai에 embeddings API 클라이언트 추가.

### 2.4 4개 메모리 도구 (oxi-agent)

`oxi-agent/src/tools/memory_*.rs`:

```rust
// memory_retain.rs
pub struct MemoryRetainTool;
impl AgentTool for MemoryRetainTool {
    fn name(&self) -> &str { "memory_retain" }
    fn essential(&self) -> bool { false }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "The fact/preference/context to remember"},
                "kind": {"type": "string", "enum": ["fact", "preference", "context", "summary"]},
                "subject": {"type": "string", "description": "Project scope (defaults to current project)"}
            },
            "required": ["content"]
        })
    }
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        let store = ctx.memory_store.as_ref()
            .ok_or("Memory not configured (set memory_enabled)")?;
        let entry = MemoryEntry { content, kind, subject: ctx.project_id(), ... };
        store.put(entry).await.map_err(|e| e.to_string())?;
        Ok(AgentToolResult::success("Retained."))
    }
}
```

`memory_recall.rs` (search), `memory_reflect.rs` (세션 종료 요약), `memory_edit.rs` (갱신/삭제) 동일 패턴.

> **ToolContext 확장**: `memory_store: Option<Arc<dyn MemoryStore>>` + `project_id()` 헬퍼.

### 2.5 부트 시 자동 로드 (`bootstrap.rs` / `agent_session.rs`)

세션 첫 턴:
```rust
if settings.memory_enabled {
    let store = oxi.memory_store();
    let memories = store.list(&project_id).await.unwrap_or_default();
    if !memories.is_empty() {
        let memory_block = format_memory_for_prompt(&memories);
        system_prompt.push_str(&format!("\n\n## Project Memory\n{}", memory_block));
    }
}
```

세션 종료 시 (reflect):
```rust
// agent_session.rs 종료 훅
if settings.memory_enabled && settings.memory_reflect {
    let summary = summarize_session(&session_entries).await;
    store.put(MemoryEntry { kind: "summary", content: summary, ... }).await?;
}
```

### 2.6 설정

```rust
pub struct Settings {
    pub memory_enabled: bool,            // 기본 false
    pub memory_reflect: bool,            // 세션 종료 자동 요약, 기본 false
    pub memory_db_path: Option<PathBuf>, // 기본 ~/.oxi/memory/<project>.db
}
```

---

## 3. 의존성 & 마일스톤 (M2b)

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| M2b.1 | 임베딩 제공자 결정 — `EmbeddingProvider` 포트 또는 oxi-ai 확장 | — |
| M2b.2 | `SqliteMemoryStore` + 스키마 + 마이그레이션 | M2b.1 |
| M2b.3 | 코사인 search (순수 SQL) | M2b.2 |
| M2b.4 | 4개 메모리 도구 (retain/recall/reflect/edit) | M2b.2 |
| M2b.5 | ToolContext `memory_store` 주입 + project_id | M2b.4 |
| M2b.6 | 부트 시 recall 주입 + 종료 시 reflect | M2b.5 |
| M2b.7 | settings (`memory_enabled` 등) | M2b.6 |

> **M1/M3와 병렬 가능**: MemoryStore 포트는 이미 있고, 메모리 도구는 edit/read/loop와 무관.
> ④는 마스터 플랜 M2에 속한다 (② Internal URL Router와 병렬). M3(③ TTSR)과도 독립.

---

## 4. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| 임베딩 제공자 | 🟡 포트 제안 | (c) `EmbeddingProvider` 포트 + (a) 기본 impl. M2b.1 합의 |
| 코사인 search 성능 | 🟢 순수 SQL | 수천 건까지 충분. 대규모는 `sqlite-vec` 확장 (후속) |
| 프라이버시 | 🟡 프로젝트 스코프 | 코드베이스 사실이 디스크 저장. `memory_enabled` 기본 false로 옵트인 |
| reflect 요약 품질 | 🟢 별도 모델 호출 | 세션 종료 시 별도 LLM 호출로 요약. 비용 발생 but 설정 토글 |
| MCP 서버 (외부 접근) | 🟢 범위 외 | omp는 mnemopi MCP 서버 포함. oxi는 MCP 브릿지 별도 |
| `sqlite-vec` 확장 | 🔴 후속 | 대규모 벡터 검색. M2b 완료 후 평가 |

---

## 5. 부록: omp → oxi 매핑

| omp 파일 | oxi 위치 |
|---|---|
| `packages/mnemopi/src/db.ts` | `oxi-cli/src/store/memory_sqlite.rs` (러스트 러웨핑) |
| `packages/mnemopi/src/migrations/` | `oxi-cli/src/store/memory_migrations/` |
| `packages/mnemopi/src/types.ts` | `oxi-sdk/src/ports/mod.rs` (`MemoryEntry`) — 이미 정의 |
| `tools/memory-retain.ts` | `oxi-agent/src/tools/memory_retain.rs` |
| `tools/memory-recall.ts` | `oxi-agent/src/tools/memory_recall.rs` |
| `tools/memory-reflect.ts` | `oxi-agent/src/tools/memory_reflect.rs` |
| `tools/memory-edit.ts` | `oxi-agent/src/tools/memory_edit.rs` |
| `tools/learn.ts` | (검토 — 능동 학습 UX) |
| `mnemopi/src/mcp-server.ts` | (범위 외 — MCP 브릿지 별도) |
