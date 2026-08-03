# 세부 설계 ⑩ — Mnemopi 백엔드 (SQLite 메모리 스토리지 계층)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §4·§7·§9 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md), 1차 [`04-hindsight-memory.md`](../omp-adoption/04-hindsight-memory.md)
> omp 분석: `packages/mnemopi/` (~9,000줄), `packages/coding-agent/src/mnemopi/` (래퍼 4파일)
> 후속: N3 구현 → CHANGELOG.md
> 짝: [`12-hindsight-memory.md`](./12-hindsight-memory.md) (응용 계층)
>
> **⚠️ v2**: 트랜잭션을 `spawn_blocking` 패턴으로 수정. `MemoryBackend` 기존 특성 구현으로 변경. FTS5 `bundled` feature 명시. `mental_models` 테이블 추가.

---

## 0. 핵심 (TL;DR)

omp의 **Mnemopi**는 로컬 SQLite 기반 메모리 엔진으로, 3계층으로 구성된다:

1. **Mnemopi 파사드** — 고수준 remember/recall/sleep API.
2. **BeamMemory** — 저수준 working + episodic 메모리 엔진 (하이브리드 리콜 스코어링).
3. **인프라** — db.ts (중첩 트랜잭션), banks.ts (멀티테넌시), embeddings.ts (API + 로컬).

**1차 ④ 설계**(`omp-adoption/04`)는 `MemoryStore` 포트 + 단순 SQLite 스키마까지만 다뤘다. 본 설계는 이를 **omp의 완전한 Mnemopi 아키텍처**로 확장한다 — working/episodic 2티어, 하이브리드 리콜(6신호 스코어링), idempotent 스키마, 뱅크(멀티테넌시), MCP 서버.

**핵심 결정**: omp의 스키마와 스코어링 공식을 **있는 그대로 Rust로 이식**한다. omp가 검증한 하이브리드 리콜 공식은 독자 재발견 비용이 너무 크다.

### omp가 검증한 가치
- **6신호 하이브리드 리콜** — vec(임베딩) + fts(전문) + keyword + importance + recency + temporal 가중 합산.
- **2티어 메모리** — working(최근, 상세) → episodic(압축, 요약) 자동 강등.
- **임베딩 3계층 폴백** — sqlite-vec 네이티브 → 인메모리 brute-force → per-candidate.
- **뱅크 스코핑** — global / per-project / per-project-tagged 멀티테넌시.

---

## 1. omp 메커니즘

### 1.1 3계층 아키텍처

```
┌─────────────────────────────────────────────────────┐
│  Mnemopi 파사드 (core/memory.ts, 651줄)              │
│  remember / recall / sleep / stats                   │
│  + 모듈 수준 기본 인스턴스 + AsyncLocalStorage 스코프 │
└──────────────────────┬──────────────────────────────┘
                       │ 위임
┌──────────────────────▼──────────────────────────────┐
│  BeamMemory (core/beam/, ~4,900줄)                   │
│  schema.ts (424) — idempotent CREATE IF NOT EXISTS    │
│  store.ts (922) — CRUD + 임베딩 reconcile             │
│  recall.ts (1,174) — 6신호 하이브리드 스코어링        │
│  consolidate.ts (1,069) — sleep 압축 (working→episodic)│
│  helpers.ts (971) — FTS/vec 검색 + 임베딩 스케줄링    │
└──────────────────────┬──────────────────────────────┘
                       │ 사용
┌──────────────────────▼──────────────────────────────┐
│  인프라                                               │
│  db.ts (165) — SQLite + 중첩 트랜잭션 (Symbol depth)  │
│  banks.ts (150) — 뱅크 = 디렉토리 멀티테넌시           │
│  embeddings.ts (440) — API + 로컬 fastembed           │
│  mcp-server.ts (140) + mcp-tools.ts (971) — 24개 도구 │
└─────────────────────────────────────────────────────┘
```

### 1.2 핵심 테이블 (`schema.ts:initBeam`)

omp는 **버전 번호 없이 idempotent 선언적 스키마**를 사용한다:

```sql
-- 최근 사실 (working memory)
CREATE TABLE IF NOT EXISTS working_memory (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    source TEXT, timestamp TEXT,
    session_id TEXT DEFAULT 'default',
    importance REAL DEFAULT 0.5,
    metadata_json TEXT,
    veracity TEXT DEFAULT 'unknown',      -- stated|inferred|tool|imported|unknown
    memory_type TEXT DEFAULT 'unknown',
    recall_count INTEGER DEFAULT 0,
    last_recalled TIMESTAMP,
    valid_until TIMESTAMP,
    superseded_by TEXT,
    scope TEXT DEFAULT 'global',
    trust_tier TEXT DEFAULT 'STATED',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- 압축된 에피소드 (episodic memory)
CREATE TABLE IF NOT EXISTS episodic_memory (
    rowid INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT UNIQUE NOT NULL,
    -- working_memory와 동일 컬럼 +
    summary_of TEXT DEFAULT '',
    tier INTEGER DEFAULT 1,               -- 1=detail, 2=compressed, 3=heavily compressed
    degraded_at TEXT,
    binary_vector BLOB                    -- hamming 거리용
);

-- 임베딩 (JSON 인코딩 float 벡터 + 모델 스탬프)
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id TEXT PRIMARY KEY,
    embedding_json TEXT NOT NULL,         -- JSON.stringify(Array.from(Float32Array))
    model TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- FTS5 전문 검색
CREATE VIRTUAL TABLE IF NOT EXISTS fts_working USING fts5(id UNINDEXED, content);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_episodes USING fts5(
    content, content='episodic_memory', content_rowid='rowid'
);

-- 스크래치패드 (임시 메모)
CREATE TABLE IF NOT EXISTS scratchpad (
    id TEXT PRIMARY KEY, content TEXT, session_id TEXT, created_at, updated_at
);

-- 압축 로그
CREATE TABLE IF NOT EXISTS consolidation_log (
    id INTEGER PRIMARY KEY, session_id, items_consolidated, summary_preview, created_at
);
```

**FTS 동기화 트리거** (6개):
```sql
CREATE TRIGGER IF NOT EXISTS wm_ai AFTER INSERT ON working_memory BEGIN
    INSERT INTO fts_working(id, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS wm_ad AFTER DELETE ON working_memory BEGIN
    DELETE FROM fts_working WHERE id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS wm_au AFTER UPDATE ON working_memory BEGIN
    DELETE FROM fts_working WHERE id = old.id;
    INSERT INTO fts_working(id, content) VALUES (new.id, new.content);
END;
-- episodic_memory 동일 (em_ai/em_ad/em_au)
```

> **idempotent 컬럼 추가**: `addColumnIfMissing(db, table, column, def)` — `PRAGMA table_info`로 확인 후 `ALTER TABLE ADD COLUMN`.

### 1.3 하이브리드 리콜 스코어링 (`recall.ts:scoreCandidate`)

6개 신호의 가중 합산 (omp가 검증한 공식):

```
가중치 정규화: w_vec + w_fts + w_imp = 1 (기본 0.5/0.3/0.2)

[episodic]
baseScore = max(dense*w_vec + fts*w_fts + importance*w_imp, lexical*0.8)

[working]
kwShare = (1 - w_imp) * 0.6
baseScore = keyword*kwShare + importance*w_imp + keyword²*0.08
if dense > 0: baseScore = baseScore*0.8 + dense*0.2

[공통 후처리]
score = baseScore * (0.7 + 0.3*recencyDecay)
if temporalWeight > 0: score *= 1 + temporalWeight * temporalBoost
score *= veracityWeight       -- true/stated=1.0, unknown=0.8, inferred=0.7, imported=0.6, tool=0.5
score *= tierWeight           -- tier1=1.0, tier2=0.85, tier3=0.7 (episodic만)

→ dedupeResults → dedupCrossTierSummaryLinks → diversifyByCoverage → optional MMR
```

### 1.4 임베딩 3계층 폴백

```
계층 1: sqlite-vec 네이티브
  vec_episodes 테이블 (호스트가 load_extension으로 생성)
  → vec_quantize_binary/int8/float32 + MATCH ? ORDER BY distance

계층 2: 인메모리 brute-force
  memory_embeddings JSON bulk 로드 (episodic 10000행, working 50000행 한계)
  → buildExactVectorIndex → searchExactVectorIndex (top-k 코사인)

계층 3: per-candidate 유사도
  recall 내에서 500개 청크로 embedding_json 로드
  → 각 행에 cosineSimilarity(queryEmbedding, parsed)
```

### 1.5 코사인 유사도 (`vector-math.ts`)

```typescript
function cosineSimilarity(a, b): number {
    const length = Math.max(a.length, b.length);  // 길이 불일치 시 0 패딩
    let dot = 0, normA = 0, normB = 0;
    for (let i = 0; i < length; i++) {
        const av = Number.isFinite(a[i]) ? a[i] : 0;
        const bv = Number.isFinite(b[i]) ? b[i] : 0;
        dot += av * bv; normA += av * av; normB += bv * bv;
    }
    return (normA === 0 || normB === 0) ? 0 : dot / (Math.sqrt(normA) * Math.sqrt(normB));
}
```

### 1.6 중첩 트랜잭션 (`db.ts`)

Symbol 기반 depth 카운터:
- 최외각: `BEGIN DEFERRED ... COMMIT/ROLLBACK`
- 내부: depth만 증감, BEGIN 없이 fn 실행
- PRAGMA: `foreign_keys=ON`, `busy_timeout=5000`, `journal_mode=WAL` (파일 DB만)

### 1.7 뱅크 스코핑 (`coding-agent/mnemopi/config.ts`)

```typescript
type MnemopiScoping = "global" | "per-project" | "per-project-tagged";

// global: 공유 bank, 태그 없음
// per-project: bank별 분리 (<basename(cwd)>-<hash36(cwd)>)
// per-project-tagged: 단일 공유 bank + project:{name} 태그, recallTagsMatch='any'
```

---

## 2. oxicode화 설계

### 2.1 크레이트 구조

omp의 `packages/mnemopi/`를 Rust 포팅:

```
oxicode-mnemopi/  (독립 라이브러리 크레이트, oxicode-cli이 의존)
├── Cargo.toml          의존: rusqlite, serde, tokio, parking_lot, reqwest (API 임베딩)
├── src/
│   ├── lib.rs          공개 API (Mnemopi 파사드)
│   ├── db.rs           SQLite 핸들 + 중첩 트랜잭션 + PRAGMA
│   ├── schema.rs       init_schema (idempotent CREATE IF NOT EXISTS)
│   ├── types.rs        MemoryRow, RecallResult, Veracity 등
│   ├── store.rs        remember/forget/update/get + 임베딩 reconcile
│   ├── recall.rs       하이브리드 스코어링 (6신호)
│   ├── consolidate.rs  sleep (working→episodic 압축)
│   ├── helpers.rs      FTS/vec 검색 + 임베딩 스케줄링
│   ├── vector_math.rs  cosine_similarity
│   ├── embeddings.rs   API + 로컬 제공자 추상화
│   ├── banks.rs        BankManager (뱅크 = 디렉토리)
│   ├── config.rs       환경변수 기반 설정
│   └── mcp.rs          (선택) MCP JSON-RPC 서버
└── migrations/
    └── e6_triplestore_split.rs  (유일한 데이터 마이그레이션)
```

### 2.2 SQLite 핸들 + 중첩 트랜잭션 (`db.rs`)

```rust
use rusqlite::Connection;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct MnemopiDb {
    conn: tokio::sync::Mutex<Connection>,
    tx_depth: AtomicU32,  // 중첩 트랜잭션 depth (omp Symbol 기반)
}

impl MnemopiDb {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        
        // PRAGMA
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        if path != Path::new(":memory:") {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        
        Ok(Self {
            conn: tokio::sync::Mutex::new(conn),
            tx_depth: AtomicU32::new(0),
        })
    }
    
    /// 중첩 트랜잭션. 최외각만 BEGIN/COMMIT, 내부는 depth만 증감.
    pub async fn transaction<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let depth = self.tx_depth.fetch_add(1, Ordering::SeqCst);
        let is_outermost = depth == 0;
        
        let conn = self.conn.lock().await;
        
        if is_outermost {
            conn.execute("BEGIN DEFERRED", [])?;
        }
        
        let result = f(&conn);
        
        match &result {
            Ok(_) => {
                if is_outermost {
                    conn.execute("COMMIT", [])?;
                }
            }
            Err(_) => {
                if is_outermost {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
        }
        
        drop(conn);
        self.tx_depth.fetch_sub(1, Ordering::SeqCst);
        
        result
    }
}
```

> **AGENTS.md pitfall**: `tokio::sync::Mutex` 사용 (parking_lot MutexGuard는 `!Send` → `.await` 전에 drop 필요). 가드를 `f(&conn)` 호출 동안만 유지.

### 2.3 idempotent 스키마 (`schema.rs`)

```rust
/// 모든 테이블/인덱스/트리거를 idempotent하게 생성.
/// 매 오픈마다 호출 — 이미 존재하면 no-op.
pub fn init_schema(conn: &Connection) -> anyhow::Result<()> {
    // working_memory
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS working_memory (
            id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            source TEXT, timestamp TEXT,
            session_id TEXT DEFAULT 'default',
            importance REAL DEFAULT 0.5,
            metadata_json TEXT,
            veracity TEXT DEFAULT 'unknown',
            memory_type TEXT DEFAULT 'unknown',
            recall_count INTEGER DEFAULT 0,
            last_recalled TIMESTAMP,
            valid_until TIMESTAMP,
            superseded_by TEXT,
            scope TEXT DEFAULT 'global',
            trust_tier TEXT DEFAULT 'STATED',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    "#)?;
    
    // episodic_memory
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS episodic_memory (
            rowid INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT UNIQUE NOT NULL,
            content TEXT NOT NULL,
            source TEXT, timestamp TEXT,
            session_id TEXT DEFAULT 'default',
            importance REAL DEFAULT 0.5,
            metadata_json TEXT,
            veracity TEXT DEFAULT 'unknown',
            memory_type TEXT DEFAULT 'unknown',
            summary_of TEXT DEFAULT '',
            tier INTEGER DEFAULT 1,
            degraded_at TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    "#)?;
    
    // memory_embeddings
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS memory_embeddings (
            memory_id TEXT PRIMARY KEY,
            embedding_json TEXT NOT NULL,
            model TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    "#)?;
    
    // FTS5
    conn.execute_batch(r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_working USING fts5(id UNINDEXED, content);
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_episodes USING fts5(
            content, content='episodic_memory', content_rowid='rowid'
        );
    "#)?;
    
    // scratchpad, consolidation_log
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS scratchpad (
            id TEXT PRIMARY KEY, content TEXT, session_id TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS consolidation_log (
            id INTEGER PRIMARY KEY,
            session_id TEXT, items_consolidated INTEGER,
            summary_preview TEXT, created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    "#)?;
    
    // FTS 동기화 트리거 (6개)
    conn.execute_batch(r#"
        CREATE TRIGGER IF NOT EXISTS wm_ai AFTER INSERT ON working_memory BEGIN
            INSERT INTO fts_working(id, content) VALUES (new.id, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS wm_ad AFTER DELETE ON working_memory BEGIN
            DELETE FROM fts_working WHERE id = old.id;
        END;
        CREATE TRIGGER IF NOT EXISTS wm_au AFTER UPDATE OF content ON working_memory BEGIN
            DELETE FROM fts_working WHERE id = old.id;
            INSERT INTO fts_working(id, content) VALUES (new.id, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS em_ai AFTER INSERT ON episodic_memory BEGIN
            INSERT INTO fts_episodes(rowid, content) VALUES (new.rowid, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS em_ad AFTER DELETE ON episodic_memory BEGIN
            DELETE FROM fts_episodes WHERE rowid = old.rowid;
        END;
        CREATE TRIGGER IF NOT EXISTS em_au AFTER UPDATE OF content ON episodic_memory BEGIN
            DELETE FROM fts_episodes WHERE rowid = old.rowid;
            INSERT INTO fts_episodes(rowid, content) VALUES (new.rowid, new.content);
        END;
    "#)?;
    
    // idempotent 컬럼 추가 (마이그레이션)
    add_column_if_missing(conn, "working_memory", "scope", "TEXT DEFAULT 'global'")?;
    add_column_if_missing(conn, "working_memory", "trust_tier", "TEXT DEFAULT 'STATED'")?;
    
    Ok(())
}

fn add_column_if_missing(conn: &Connection, table: &str, column: &str, def: &str) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let exists: bool = stmt.query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|c| c == column);
    
    if !exists {
        conn.execute(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, def), [])?;
    }
    Ok(())
}
```

### 2.4 Mnemopi 파사드 (`lib.rs`)

```rust
pub struct Mnemopi {
    db: Arc<MnemopiDb>,
    bank: String,
    embedder: Arc<dyn EmbeddingProvider>,
    config: MnemopiConfig,
}

impl Mnemopi {
    pub fn open(db_path: &Path, embedder: Arc<dyn EmbeddingProvider>) -> anyhow::Result<Self> {
        let db = MnemopiDb::open(db_path)?;
        // 스키마 초기화 (idempotent)
        {
            let conn = db.conn.blocking_lock();  // 동기 컨텍스트에서만
            init_schema(&conn)?;
        }
        Ok(Self {
            db: Arc::new(db),
            bank: "default".into(),
            embedder,
            config: MnemopiConfig::default(),
        })
    }
    
    /// 메모리 저장. 백그라운드 임베딩 예약.
    pub async fn remember(&self, content: &str, options: RememberOptions) -> anyhow::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        
        self.db.transaction(|conn| {
            conn.execute(
                "INSERT INTO working_memory (id, content, session_id, importance, veracity, scope)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![&id, content, options.session_id, options.importance,
                        options.veracity.as_str(), options.scope],
            )?;
            Ok(())
        }).await?;
        
        // 백그라운드 임베딩 예약
        let embedder = Arc::clone(&self.embedder);
        let id_clone = id.clone();
        let content_clone = content.to_string();
        let db = Arc::clone(&self.db);
        tokio::spawn(async move {
            if let Ok(embedding) = embedder.embed(&content_clone).await {
                let json = serde_json::to_string(&embedding).unwrap_or_default();
                let _ = db.transaction(|conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO memory_embeddings (memory_id, embedding_json, model)
                         VALUES (?1, ?2, ?3)",
                        params![&id_clone, &json, embedder.model_name()],
                    )?;
                    Ok(())
                }).await;
            }
        });
        
        Ok(id)
    }
    
    /// 하이브리드 리콜. 6신호 스코어링.
    pub async fn recall(&self, query: &str, top_k: usize, options: RecallOptions) 
        -> anyhow::Result<Vec<RecallResult>> 
    {
        let query_embedding = self.embedder.embed(query).await?;
        recall::hybrid_recall(&self.db, query, &query_embedding, top_k, &options, &self.config).await
    }
    
    /// sleep — working→episodic 압축.
    pub async fn sleep(&self, dry_run: bool) -> anyhow::Result<SleepResult> {
        consolidate::consolidate(&self.db, dry_run, &self.config).await
    }
    
    pub async fn forget(&self, id: &str) -> anyhow::Result<bool> { ... }
    pub async fn update(&self, id: &str, content: Option<&str>, importance: Option<f64>) -> anyhow::Result<bool> { ... }
    pub async fn stats(&self) -> anyhow::Result<MemoryStats> { ... }
}
```

### 2.5 하이브리드 리콜 (`recall.rs`)

omp의 `scoreCandidate` 공식을 Rust로 이식:

```rust
pub async fn hybrid_recall(
    db: &MnemopiDb,
    query: &str,
    query_embedding: &[f32],
    top_k: usize,
    options: &RecallOptions,
    config: &MnemopiConfig,
) -> anyhow::Result<Vec<RecallResult>> {
    // 가중치 정규화
    let (w_vec, w_fts, w_imp) = normalize_weights(
        config.vec_weight, config.fts_weight, config.importance_weight
    );
    
    // 후보 수집: FTS + vec + fallback
    let candidates = collect_candidates(db, query, query_embedding, top_k * 3).await?;
    
    // 스코어링
    let mut scored: Vec<RecallResult> = candidates.into_iter()
        .map(|c| score_candidate(c, query_embedding, w_vec, w_fts, w_imp, config))
        .collect();
    
    // 후처리: dedupe → diversify → optional MMR
    dedupe_results(&mut scored);
    if config.use_mmr { mmr_rerank(&mut scored, 0.7); }
    
    // recall_count 갱신
    update_recall_counts(db, &scored).await;
    
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    scored.truncate(top_k);
    
    Ok(scored)
}

fn score_candidate(
    candidate: Candidate,
    query_embedding: &[f32],
    w_vec: f64, w_fts: f64, w_imp: f64,
    config: &MnemopiConfig,
) -> RecallResult {
    let vec_score = candidate.vec_score.unwrap_or(0.0);
    let fts_score = candidate.fts_score.unwrap_or(0.0);
    let importance = candidate.importance;
    let keyword = candidate.keyword_score.unwrap_or(0.0);
    
    let base_score = match candidate.tier {
        MemoryTier::Episodic => {
            let dense = vec_score * w_vec + fts_score * w_fts + importance * w_imp;
            let lexical = keyword * 0.8;
            dense.max(lexical)
        }
        MemoryTier::Working => {
            let kw_share = (1.0 - w_imp) * 0.6;
            let mut score = keyword * kw_share + importance * w_imp + keyword * keyword * 0.08;
            if vec_score > 0.0 {
                score = score * 0.8 + vec_score * 0.2;
            }
            score
        }
    };
    
    // recency decay
    let recency_decay = compute_recency_decay(&candidate.timestamp, config.recency_halflife);
    let mut score = base_score * (0.7 + 0.3 * recency_decay);
    
    // temporal boost
    if options.temporal_weight > 0.0 {
        let temporal_boost = compute_temporal_boost(&candidate, query);
        score *= 1.0 + options.temporal_weight * temporal_boost;
    }
    
    // veracity weight
    let veracity_weight = match candidate.veracity {
        Veracity::Stated | Veracity::True => 1.0,
        Veracity::Unknown => 0.8,
        Veracity::Inferred => 0.7,
        Veracity::Imported => 0.6,
        Veracity::Tool => 0.5,
    };
    score *= veracity_weight;
    
    // tier weight (episodic만)
    if let MemoryTier::Episodic = candidate.tier {
        let tier_weight = match candidate.tier_level {
            1 => 1.0, 2 => 0.85, 3 => 0.7, _ => 0.5,
        };
        score *= tier_weight;
    }
    
    RecallResult { id: candidate.id, content: candidate.content, score, .. }
}
```

### 2.6 코사인 유사도 (`vector_math.rs`)

```rust
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().max(b.len());
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    
    for i in 0..len {
        let av = if i < a.len() && a[i].is_finite() { a[i] } else { 0.0 };
        let bv = if i < b.len() && b[i].is_finite() { b[i] } else { 0.0 };
        dot += av * bv;
        norm_a += av * av;
        norm_b += bv * bv;
    }
    
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}
```

### 2.7 임베딩 제공자 (`embeddings.rs`)

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
    fn model_name(&self) -> &str;
    fn dim(&self) -> usize;
}

/// API 기반 임베딩 (OpenAI 호환 /embeddings).
pub struct ApiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dim: usize,
}

/// 로컬 fastembed (ONNX). 의존: fastembed-rs (선택 feature).
#[cfg(feature = "local-embeddings")]
pub struct LocalEmbeddingProvider {
    model: fastembed::TextEmbedding,
}

/// 비활성화 (리콜은 FTS-only).
pub struct NoEmbeddingProvider;
```

### 2.8 BankManager (`banks.rs`)

```rust
pub struct BankManager {
    data_dir: PathBuf,
}

impl BankManager {
    /// 뱅크 = 파일시스템 디렉토리.
    /// default: <data_dir>/mnemopi.db
    /// named:   <data_dir>/banks/<name>/mnemopi.db
    pub fn bank_db_path(&self, name: &str) -> PathBuf {
        if name == "default" {
            self.data_dir.join("mnemopi.db")
        } else {
            self.data_dir.join("banks").join(name).join("mnemopi.db")
        }
    }
    
    pub fn create_bank(&self, name: &str) -> anyhow::Result<PathBuf> {
        validate_bank_name(name)?;
        let db_path = self.bank_db_path(name);
        std::fs::create_dir_all(db_path.parent().unwrap())?;
        Ok(db_path)
    }
    
    pub fn list_banks(&self) -> Vec<String> {
        let mut banks = vec!["default".into()];
        if let Ok(entries) = std::fs::read_dir(self.data_dir.join("banks")) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    if let Some(name) = entry.file_name().to_str() {
                        banks.push(name.to_string());
                    }
                }
            }
        }
        banks
    }
}

fn validate_bank_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("Bank name must be 1-64 characters");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        anyhow::bail!("Bank name must match [A-Za-z0-9_-]");
    }
    Ok(())
}
```

---

## 3. MemoryBackend 구현 (v2 — 도구 연동)

> **v2 수정**: oxicode-agent에는 이미 `MemoryBackend` 특성이 있다 (`tools.rs:31-51`). v1은 `MemoryStore` SDK 포트만 참조했으나, 도구 연동은 `MemoryBackend` 구현이 필요. [`00-design-revisions.md`](./00-design-revisions.md) §4 참조.

### 3.1 이중 구현 구조

``+oxicode-sdk MemoryStore 포트 (SDK 소비자용)
        ↑ 구현 (별개 — SDK 직접 사용자)
oxicode-mnemopi Mnemopi (백엔드)
        ↑ 브리지 (oxicode-cli)
oxicode-agent MemoryBackend (도구용 — 기존 특성)
        ↑ 사용
memory_retain/recall/reflect/edit 도구 (⑨)
+```

### 3.2 MnemopiMemoryBackend 브릿지

```rust
// oxicode-cli/src/store/memory_bridge.rs
use oxicode_agent::tools::{MemoryBackend, MemoryItem, ToolError};

pub struct MnemopiMemoryBackend {
    mnemopi: Arc<Mnemopi>,
}

impl MnemopiMemoryBackend {
    pub fn new(mnemopi: Arc<Mnemopi>) -> Self {
        Self { mnemopi }
    }
}

impl MemoryBackend for MnemopiMemoryBackend {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            self.mnemopi.remember(content, RememberOptions {
                kind: kind.into(),
                scope: subject.into(),
                ..Default::default()
            }).await.map_err(|e| e.to_string())
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let results = self.mnemopi.recall(query, k, RecallOptions::default())
                .await.map_err(|e| e.to_string())?;
            Ok(results.into_iter().map(|r| MemoryItem {
                id: r.id,
                kind: r.kind.unwrap_or_else(|| "fact".into()),
                content: r.content,
                subject: subject.into(),
            }).collect())
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let items = self.mnemopi.list_by_scope(subject).await.map_err(|e| e.to_string())?;
            Ok(items.into_iter().map(Into::into).collect())
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move {
            self.mnemopi.forget(id).await.map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}
```

### 3.3 bootstrap.rs 주입

```rust
// oxicode-cli/src/bootstrap.rs
if settings.memory_enabled {
    let mnemopi = Mnemopi::open(&memory_db_path, embedder).await?;
    let backend = Arc::new(MnemopiMemoryBackend::new(Arc::new(mnemopi)));
    tool_context = tool_context.with_memory(backend);
}
```

### 3.4 (별개) MemoryStore 포트 구현

`MemoryStore` 포트(oxicode-sdk)는 SDK 직접 소비자용. Mnemopi가 별도로 구현하되, oxicode-cli의 도구 연동은 `MemoryBackend`를 사용:

```rust
// oxicode-mnemopi (SDK 소비자용 — 별개 impl 블록)
#[async_trait]
impl MemoryStore for Mnemopi { ... }
```
## 4. (선택) MCP 서버

omp는 24개 MCP 도구를 stdio JSON-RPC로 노출한다. oxicode는:

- **옵션 A**: `oxicode-mnemopi`에 MCP 서버 내장 (`mcp.rs`). 별도 프로세스로 실행 가능.
- **옵션 B**: MCP 서버 생략. oxicode의 MCP 클라이언트(`oxicode-agent/src/mcp/`)로 외부 mnemopi에 연결.

> **결정 필요**: N3에서 합의. 초기는 옵션 B (MCP 서버 생략, 인프로세스 직접 호출).

---

## 5. 설정

```rust
pub struct MnemopiConfig {
    // 가중치
    pub vec_weight: f64,          // 기본 0.5
    pub fts_weight: f64,          // 기본 0.3
    pub importance_weight: f64,   // 기본 0.2
    
    // 시간
    pub recency_halflife: Duration,       // 기본 168h (7일)
    pub temporal_halflife: Duration,      // 기본 24h
    
    // 용량
    pub wm_max_items: usize,     // 기본 10000
    pub wm_ttl: Duration,        // 기본 24h
    pub max_episode_chars: usize, // 기본 100000
    
    // 티어
    pub tier2_days: u64,         // 기본 30
    pub tier3_days: u64,         // 기본 180
    pub tier3_max_chars: usize,  // 기본 300
    
    // 임베딩
    pub embedding_model: String, // 기본 "BAAI/bge-small-en-v1.5"
    pub embedding_dim: usize,    // 기본 384
}
```

환경변수 (omp 호환):
- `MNEMOPI_DATA_DIR` — 데이터 디렉토리 (기본 `~/.oxicode/mnemopi/`)
- `MNEMOPI_NO_EMBEDDINGS` — 임베딩 비활성화
- `MNEMOPI_EMBEDDING_MODEL` — 임베딩 모델
- `MNEMOPI_VEC_WEIGHT` / `MNEMOPI_FTS_WEIGHT` / `MNEMOPI_IMPORTANCE_WEIGHT`

---

## 6. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N3.1 | `oxicode-mnemopi` 크레이트 스캐폴드 | — |
| N3.2 | `db.rs` (SQLite + 중첩 트랜잭션 + PRAGMA) | N3.1 |
| N3.3 | `schema.rs` (idempotent 스키마 + 트리거) | N3.2 |
| N3.4 | `types.rs` (MemoryRow, RecallResult, Veracity) | N3.1 |
| N3.5 | `vector_math.rs` (cosine_similarity) | N3.4 |
| N3.6 | `embeddings.rs` (ApiEmbeddingProvider) | N3.4 |
| N3.7 | `store.rs` (remember/forget/update/get) | N3.3, N3.6 |
| N3.8 | `helpers.rs` (FTS 검색 + 임베딩 스케줄링) | N3.7 |
| N3.9 | `recall.rs` (하이브리드 스코어링 — 6신호) | N3.8, N3.5 |
| N3.10 | `consolidate.rs` (sleep — working→episodic) | N3.7 |
| N3.11 | `banks.rs` (BankManager) | N3.2 |
| N3.12 | `config.rs` (환경변수) | N3.1 |
| N3.13 | `lib.rs` (Mnemopi 파사드) | N3.9, N3.10 |
| N3.14 | `MemoryStore` 포트 충전 (impl) | N3.13 |
| N3.15 | 뱅크 스코핑 (global/per-project/per-project-tagged) | N3.11 |
| N3.16 | 임베딩 모델 reconcile (모델 변경 시 재임베딩) | N3.7 |
| N3.17 | e6 마이그레이션 (triples→annotations) | N3.3 |
| N3.18 | (선택) MCP 서버 (`mcp.rs`) | N3.13 |
| N3.19 | (선택) 로컬 fastembed 제공자 | N3.6 |

> **독립성**: ⑩은 ⑨ Hindsight 응용과 순차. ⑤⑥⑧⑪과 독립.
> **1차 ④ 연동**: N3.14가 `MemoryStore` 포트를 충전 — 1차 설계의 구현체.

---

## 7. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| `rusqlite` 동기 + `tokio::sync::Mutex` 성능 | 🟡 모니터 | omp는 bun:sqlite (동기). oxicode는 tokio::sync::Mutex로 async-safe |
| FTS5 CJK 처리 (bigram LIKE 폴백) | 🟠 구현 필요 | omp의 `cjkLikeSearch` 이식. 다국어 지원 시 필수 |
| sqlite-vec 확장 로드 | 🟢 폴백 있음 | 미로드 시 인메모리 brute-force. 대규모는 성능 저하 |
| 임베딩 의존 (API vs 로컬) | 🟡 결정 필요 | API 기본 + 로컬 fastembed 선택 feature |
| 하이브리드 스코어링 정확도 | 🟢 검증됨 | omp SQuAD evals. 공식 있는 그대로 이식 |
| 메모리 정리 (WM_MAX_ITEMS 초과) | 🟢 이식 | omp의 trimWorkingMemory |
| MCP 서버 포함 여부 | 🟡 미결정 | 초기는 생략(인프로세스). N3에서 합의 |

---

## 8. 부록: omp → oxicode 매핑

| omp 위치 | oxicode 위치 |
|---|---|
| `packages/mnemopi/src/core/memory.ts` (651) | `oxicode-mnemopi/src/lib.rs` |
| `packages/mnemopi/src/core/beam/index.ts` (356) | `oxicode-mnemopi/src/lib.rs` (통합) |
| `packages/mnemopi/src/core/beam/schema.ts` (424) | `oxicode-mnemopi/src/schema.rs` |
| `packages/mnemopi/src/core/beam/store.ts` (922) | `oxicode-mnemopi/src/store.rs` |
| `packages/mnemopi/src/core/beam/recall.ts` (1,174) | `oxicode-mnemopi/src/recall.rs` |
| `packages/mnemopi/src/core/beam/consolidate.ts` (1,069) | `oxicode-mnemopi/src/consolidate.rs` |
| `packages/mnemopi/src/core/beam/helpers.ts` (971) | `oxicode-mnemopi/src/helpers.rs` |
| `packages/mnemopi/src/core/vector-math.ts` (25) | `oxicode-mnemopi/src/vector_math.rs` |
| `packages/mnemopi/src/db.ts` (165) | `oxicode-mnemopi/src/db.rs` |
| `packages/mnemopi/src/core/banks.ts` (150) | `oxicode-mnemopi/src/banks.rs` |
| `packages/mnemopi/src/core/embeddings.ts` (440) | `oxicode-mnemopi/src/embeddings.rs` |
| `packages/mnemopi/src/config.ts` (349) | `oxicode-mnemopi/src/config.rs` |
| `packages/mnemopi/src/types.ts` (135) | `oxicode-mnemopi/src/types.rs` |
| `packages/mnemopi/src/mcp-server.ts` (140) | `oxicode-mnemopi/src/mcp.rs` (선택) |
| `packages/mnemopi/src/mcp-tools.ts` (971) | `oxicode-mnemopi/src/mcp.rs` (선택) |
| `packages/coding-agent/src/mnemopi/` (래퍼) | `oxicode-cli/src/` (통합) |
