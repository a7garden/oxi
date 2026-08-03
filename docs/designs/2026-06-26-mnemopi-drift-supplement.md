# Mnemopi 포팅 — v16.1.21 드리프트 분석 및 설계 서플먼트

> 작성: 2026-06-26
> 기반 설계: [`omp-adoption-2/11-mnemopi-backend.md`](./omp-adoption-2/11-mnemopi-backend.md) (936줄, v2, 2026-06-19)
> omp 소스: `oh-my-pi` v16.1.21 (클론 `018a963`)
> 성격: 기존 설계의 **증보/수정** — 중복 작성이 아님. 기존 설계를 먼저 읽을 것.

---

## 0. 왜 서플먼트인가

기존 `11-mnemopi-backend.md`는 아키텍처, 스키마, 스코어링 공식, 크레이트 구조를 이미 다루고 있다. 본 문서가 해결하는 것은:

1. **스코프 현실 점검** — 기존 설계가 omp mnemopi의 ~7.5K줄만 매핑했으나, 실제는 ~20K줄 / 40+ 파일
2. **v16.1.1 → v16.1.21 드리프트** — 세션 상태 계층, 백엔드 추상화, 18개 고급 모듈이 신규 추가됨
3. **trait 설계 정정** — `MemoryStore` 포트를 부풀리지 않는 분리 전략 (Advisory 반영)
4. **기반 정정** — SQLite는 이미 oxicode-cli에 있음; fastembed-rs 매핑 명시

---

## 1. 스코프 현실 — 기존 설계가 놓친 것

### 1.1 실제 규모 vs 설계 매핑

| 계층 | omp 줄 수 | 기존 설계 매핑 | 누락 |
|------|----------|---------------|------|
| mnemopi/core (beam 포함) | ~15,300 | ~7,500 (12 파일) | **~7,800줄 / 20+ 파일** |
| coding-agent/mnemopi/ | ~1,930 | §3 브릿지만 (30줄 스케치) | **세션 상태 전체 (662줄) + backend.ts (612줄)** |
| coding-agent/memory-backend/ | ~345 | 없음 | **백엔드 추상화 전체** |
| **합계** | **~17,600** | **~7,530** | **~10,000줄** |

### 1.2 기존 설계 모듈 목록(§2.1)에 없는 코어 모듈

기존 설계가 나열한 12개 모듈 외에, omp mnemopi/core/에는 **20개 추가 모듈**이 있다:

| 모듈 | 줄 | 용도 | 포팅 단계 |
|------|-----|------|-----------|
| `polyphonic-recall.ts` | 563 | 다중 쿼리 팬아웃 리콜 | Phase 3 |
| `episodic-graph.ts` | 708 | 에피소드 간 그래프 관계 | Phase 3 |
| `shmr.ts` | 560 | Semantic Hierarchical Memory Retrieval | Phase 3 |
| `patterns.ts` | 484 | 메모리 패턴 매칭 | Phase 3 |
| `veracity-consolidation.ts` | 477 | 신뢰도 기반 통합 | Phase 3 |
| `local-llm.ts` | 474 | 로컬 LLM 백엔드 | Phase 3 |
| `triples.ts` | 452 | 트리플 스토어 (SPO) | Phase 3 |
| `typed-memory.ts` | 407 | 타입화된 메모리 | Phase 3 |
| `streaming.ts` | 419 | 스트리밍 recall | Phase 3 |
| `extraction.ts` | 338 | LLM fact 추출 | Phase 2 |
| `binary-vectors.ts` | 317 | 바이너리 벡터 양자화 | Phase 2 |
| `entities.ts` | 263 | 정규식 엔티티 추출 | Phase 2 |
| `annotations.ts` | 457 | memory ↔ annotation 연결 | Phase 3 |
| `temporal-parser.ts` | 363 | 시간 표현 파싱 | Phase 2 |
| `query-cache.ts` | 353 | 임베딩 쿼리 캐시 (LRU 512) | Phase 2 |
| `plugins.ts` | 375 | 플러그인 시스템 | Phase 3 (선택) |
| `synonyms.ts` | 197 | 동의어 확장 | Phase 2 |
| `query-intent.ts` | 139 | 질의 의도 분류 (가중치 동적 조정) | Phase 2 |
| `mmr.ts` | 71 | Maximal Marginal Relevance | Phase 2 |
| `weibull.ts` | 124 | Weibull 감쇠 분포 | Phase 2 |
| `content-sanitizer.ts` | 136 | 민감 정보 마스킹 | Phase 2 |
| `chat-normalize.ts` | 160 | 채팅 정규화 | Phase 2 |
| 기타 (aaak, cost-log, token-counter 등) | ~620 | 보조 유틸리티 | Phase 2-3 |

### 1.3 세션 상태 계층 (기존 설계 완전 누락)

omp의 `coding-agent/src/mnemopi/`가 메모리 자동화의 핵심이다:

```
mnemopi/state.ts     (662줄)  — MnemopiSessionState (세션 라이프사이클)
mnemopi/backend.ts   (612줄)  — mnemopiBackend: MemoryBackend (4-백엔드 시스템)
mnemopi/config.ts    (267줄)  — MnemopiBackendConfig (세션 설정)
mnemopi/embed-client.ts (246) — fastembed 서브프로세스 클라이언트
mnemopi/embed-worker.ts (113) — fastembed 워커 (별 프로세스)
```

기존 설계 §3.2는 `MnemopiMemoryBackend` 브릿지(30줄)만 스케치했다. **세션 라이프사이클 전체** — auto-recall, auto-retain, consolidate-on-dispose, bank scoping, compaction hook — 가 빠져 있다. 이는 `12-hindsight-memory.md`가 다루도록 의도되었으나, 해당 문서도 hindsight 원격 백엔드 기준이라 mnemopi 로컬 세션 상태를 완전히 커버하지 않는다.

### 1.4 백엔드 추상화 (기존 설계 완전 누락)

omp는 4-백엔드 상호 배타적 선택 패턴을 사용한다:

```
memory-backend/resolve.ts → settings.memory.backend
  ├── "off"       → no-op (도구 숨김)
  ├── "local"     → rollout 요약 → learned.md
  ├── "hindsight" → 원격 메모리 서버
  └── "mnemopi"   → 로컬 SQLite 벡터 메모리
```

`MemoryBackend` interface (`types.ts:95-166`)는 omp의 도구-백엔드 계약이다:

```typescript
interface MemoryBackend {
    start(options): void;                                    // 세션 시작
    buildDeveloperInstructions(agentDir, settings): string;  // 시스템 프롬프트 주입
    clear(agentDir, cwd): void;                              // 전체 삭제
    enqueue(agentDir, cwd): void;                            // 강제 consolidation
    status?(context): MemoryBackendStatus;
    search?(context, query, options): SearchResult;
    save?(context, input): SaveResult;
    stats?(agentDir, cwd): string;
    diagnose?(agentDir, cwd): string;
    beforeAgentStartPrompt?(session, prompt): string;        // 첫 턴 recall 주입
    preCompactionContext?(messages, settings): string;       // compaction 컨텍스트
}
```

oxicode의 `MemoryBackend` trait (`tools.rs:35-57`)는 `put`/`search`/`list`/`delete`만 있다. omp의 라이프사이클 훅 6개가 없다.

---

## 2. Trait 설계 정정 — MemoryStore를 부풀리지 마라

### 2.1 문제

omp의 Mnemopi 엔진은 remember/recall/sleep/forget/update/scratchpad/banks/consolidate까지 단일 `BeamMemory` 클래스에 가지고 있다. 이를 oxicode의 `MemoryStore` 포트에 그대로 올리면 포트가 엔진 구현에 종속된다 — SDK 사용자(oxios 등)가 구현하기 어려워진다.

### 2.2 해결 — 3계층 분리

oxicode는 이미 더 깔끔한 단층을 가지고 있다. 이를 **살린다**:

```
┌──────────────────────────────────────────────────────┐
│ Layer 3: oxicode-mnemopi (엔진 내부 API — trait 아님)      │
│                                                        │
│  Mnemopi                                               │
│    .remember() / .recall() / .forget() / .update()    │
│    .sleep() / .consolidate() / .flush_extractions()   │
│    .banks() / .stats() / .diagnose()                  │
│                                                        │
│  → sleep, consolidate, banks, extract는 포트가 아닌    │
│    엔진의 고유 기능. oxicode-mnemopi 크레이트 내부에 캡슐화 │
└───────────────────────┬──────────────────────────────┘
                        │ 브리지 (oxicode-cli)
┌───────────────────────▼──────────────────────────────┐
│ Layer 2: oxicode-agent MemoryBackend (도구 계약 — 기존)    │
│                                                        │
│  put(content, kind, subject) → id                     │
│  search(query, k) → Vec<MemoryItem>                   │
│  list(subject) → Vec<MemoryItem>                      │
│  delete(id) → ()                                      │
│  ── 신규 (default 구현으로 additive) ──                 │
│  update(id, content?, importance?) → bool             │
│  recall_text(query, limit) → String   // 도구용 포맷   │
│  status() → MemoryStatus                              │
└───────────────────────┬──────────────────────────────┘
                        │ 사용
┌───────────────────────▼──────────────────────────────┐
│ Layer 1: oxicode-sdk ports (SDK 계약 — 기존, 유지)         │
│                                                        │
│  MemoryStore:    put(entry) / list(scope) /            │
│                  search(query, k, filter?)              │
│  EmbeddingProvider: embed(texts) → Vec<Vec<f32>>       │
│                                                        │
│  → 이 둘은 분리된 채로 둔다. recall 한 호출에 묶지 않는다│
│  → sleep/consolidate/banks는 포트에 추가하지 않는다     │
└──────────────────────────────────────────────────────┘
```

**핵심 원칙**:
- `MemoryStore`(저장/검색)와 `EmbeddingProvider`(벡터화)의 분리는 omp보다 깔끔하다 — 유지.
- sleep/consolidate/extract/banks는 `oxicode-mnemopi` 내부 API. SDK 포트나 `MemoryBackend` trait에 넣지 않는다.
- 세션 라이프사이클 훅(auto-recall, auto-retain, consolidate-on-dispose)은 omp의 `MemoryBackend` interface처럼 trait를 부풀리지 않고, oxicode-cli의 `AgentSession` + `CompactionHook`에 직접 와이어링한다.

### 2.3 MemoryBackend trait 확장 (additive)

```rust
// oxicode-agent/src/tools.rs — 기존 trait에 default 메서드 추가

pub trait MemoryBackend: Send + Sync + std::fmt::Debug {
    // ── 기존 (유지) ──
    fn put<'a>(&'a self, content: &'a str, kind: &'a str, subject: &'a str)
        -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;
    fn search<'a>(&'a self, query: &'a str, k: usize)
        -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>>;
    fn list<'a>(&'a self, subject: &'a str)
        -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>>;
    fn delete<'a>(&'a self, id: &'a str)
        -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>>;

    // ── 신규 (default = 기존 동작 유지, 하위 호환) ──

    /// 항목 업데이트 (content 및/또는 importance). 기본: put 재사용.
    fn update<'a>(&'a self, id: &'a str, content: Option<&'a str>, importance: Option<f32>)
        -> Pin<Box<dyn Future<Output = Result<bool, ToolError>> + Send + 'a>> {
        Box::pin(async { Err("update not supported".into()) })
    }

    /// 항목 무효화 (soft delete + 대체 ID). 기본: delete 호출.
    fn invalidate<'a>(&'a self, id: &'a str, replacement_id: Option<&'a str>)
        -> Pin<Box<dyn Future<Output = Result<bool, ToolError>> + Send + 'a>> {
        let _ = replacement_id;
        Box::pin(async { self.delete(id).await.map(|_| true) })
    }

    /// recall 결과를 도구용 텍스트로 포맷. 기본: search → format.
    fn recall_text<'a>(&'a self, query: &'a str, limit: usize)
        -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let items = self.search(query, limit).await?;
            Ok(format_memory_items(&items))
        })
    }
}
```

> 이 확장은 `MemoryBackend` 구현체(SqliteMemoryStore, MnemopiStore)에 **변경을 요구하지 않는다** — default 구현이 있으므로. `MnemopiMemoryBackend`만 override한다.

---

## 3. 임베딩 전략 — fastembed-rs 매핑

### 3.1 omp의 임베딩 아키텍처

omp는 fastembed(JS, ONNX 런타임 래퍼)를 **서브프로세스**(`embed-worker.ts`)에서 실행한다 — 메인 이벤트 루프 블로킹 방지.

### 3.2 Rust 매핑

| omp (JS) | oxicode (Rust) | 비고 |
|----------|-----------|------|
| `fastembed` (JS) | `fastembed-rs` (`fastembed` crate) | 동일 ONNX 모델, Rust 네이티브 |
| `embed-worker.ts` (서브프로세스) | `tokio::task::spawn_blocking` | Rust는 async 런타임에서 블로킹 방지가 더 간단 |
| `FlagEmbedding.init()` | `TextEmbedding::try_new(model)` | 모델 다운로드 + 캐시 |
| `BAAI/bge-base-en-v1.5` | 동일 (fastembed-rs 지원) | omp 기본 모델 |
| `intfloat/multilingual-e5-large` | 동일 (fastembed-rs 지원) | 다국어 variant |

**임베딩 제공자 우선순위** (omp `resolveEmbeddingProvider` 매핑):

```
1. 설정: mnemopi.embeddingApiUrl → RemoteEmbeddingProvider (OpenAI 호환 /v1/embeddings)
2. 환경: MNEMOPI_EMBEDDING_MODEL → 로컬 fastembed-rs
3. 기본: BAAI/bge-base-en-v1.5 (로컬, 384차원)
4. 비활성: MNEMOPI_NO_EMBEDDINGS=1 → FTS-only recall (임베딩 없이 동작)
```

### 3.3 의존성

```toml
# oxicode-mnemopi/Cargo.toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }  # FTS5 포함 (bundled)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["sync"] }
parking_lot = "0.12"
reqwest = { version = "0.12", features = ["json"] }       # 원격 임베딩 API

[features]
default = []                    # 원격 API 임베딩만 (경량)
local-embeddings = ["fastembed"] # 로컬 ONNX (+~50MB 바이너리)
local-llm = []                  # 로컬 LLM fact 추출 (별도 feature)
```

```toml
# oxicode-mnemopi/Cargo.toml [target.'cfg(feature = "local-embeddings")'.dependencies]
fastembed = "4"   # BAAI/bge-base-en-v1.5, multilingual-e5 등 지원
```

> **기본은 원격 API** (경량). `local-embeddings` feature 켜면 fastembed-rs가 ONNX 모델을 다운로드하여 로컬 추론. omp와 동일한 폴백 전략.

---

## 4. 기반 정정 — SQLite 상태

**Advisory 정정**: SQLite는 이미 oxicode-cli에 있다.

```
oxicode-cli/Cargo.toml:49
  rusqlite = { version = "0.31", features = ["bundled"] }
```

`oxicode-cli/src/store/memory_sqlite.rs`가 이미 `rusqlite::{Connection, params}`를 사용 중 (LIKE 검색).

**실제 갭**:
- oxicode-cli에는 rusqlite가 있지만, 신규 `oxicode-mnemopi` 크레이트는 **자체 의존성**이 필요 (oxicode-cli와 별개 크레이트이므로)
- `rusqlite`의 `bundled` feature에 **FTS5가 포함**되는지 확인 필요 — `rusqlite 0.31`의 bundled SQLite는 FTS5를 기본 활성화함 (SQLite 3.41+ bundled build)
- WAL 모드 + busy_timeout + foreign_keys PRAGMA는 기존 `SqliteMemoryStore::open`에서 이미 사용하는 패턴 재사용

---

## 5. 수정된 구현 단계

기존 설계 §6의 N3.1-N3.19를 3-단계로 재그룹화 (Advisory 권고 반영):

### Phase 1 — Foundation: SQLite + FTS5 + recall-by-text

**의존성**: `rusqlite` (bundled, FTS5 포함)
**포트 변경**: 없음. `MemoryStore::search`는 `&[f32]`(임베딩 벡터)를 받으므로 Phase 1과 무관 — 텍스트 검색은 엔진 내부(FTS5)에서 수행하고 `MnemopiMemoryBackend: MemoryBackend`의 `search(&str)`가 브리지. §7 D3 참조.
**산출물**:

```
oxicode-mnemopi/
├── db.rs           SQLite 핸들 + WAL + PRAGMA + spawn_blocking 패턴
├── schema.rs       init_schema (working_memory + FTS5 + 트리거)
├── types.rs        MemoryRow, RecallResult, Veracity
├── store.rs        remember / forget / update / get
├── recall.rs       FTS5 검색 + importance/recency 가중 (임베딩 없이)
├── vector_math.rs  cosine_similarity (Phase 2 대비)
└── lib.rs          Mnemopi 파사드 (remember/recall/forget/get만)
```

**검증 기준**:
- `cargo nextest run -p oxicode-mnemopi` — remember → recall(FTS) → forget 사이클
- recall이 임베딩 없이 FTS5 + importance + recency로 동작
- `MnemopiMemoryBackend: MemoryBackend` 브리지로 `memory_recall` 도구가 beam recall 사용

### Phase 2 — Vector: 임베딩 + 벡터 블렌딩

**의존성**: `fastembed` (feature `local-embeddings`), `reqwest` (원격 API)
**포트 변경**: `EmbeddingProvider` 두 구현체 (Remote + Local)
**산출물**:

```
oxicode-mnemopi/
├── embeddings.rs       EmbeddingProvider trait + Remote + Local(fastembed-rs)
├── vector_index.rs     build_exact_index + search_exact (brute-force top-k)
├── beam/helpers.rs     FTS + vec 검색 통합 + 임베딩 스케줄링
├── beam/recall.rs      ← 확장: 6신호 하이브리드 스코어링 (기존 설계 §2.5 공식)
├── beam/mmr.rs         MMR 다양성
├── beam/query_intent.rs 질의 의도 분류 (가중치 동적 조정)
├── beam/synonyms.rs    동의어 확장
├── beam/temporal.rs    시간 표현 추출 + Weibull 감쇠
├── query_cache.rs      LRU 512 캐시
├── extraction.rs       LLM fact 추출 (host → remote → heuristic)
├── entities.rs         정규식 엔티티 추출
└── content_sanitizer.rs 민감 정보 마스킹
```

**검증 기준**:
- 임베딩 기반 recall 정확도 (FTS-only 대비 향상)
- 원격 API ↔ 로컬 fastembed 전환 시 동일 결과
- extraction 파이프라인: 텍스트 → facts 추출 → 저장

### Phase 3 — Advanced: consolidation + graph + polyphonic

**의존성**: 없음 (Rust 표준 라이브러리 + 기존)
**산출물**:

```
oxicode-mnemopi/
├── beam/consolidate.rs     sleep (working → episodic 압축) + tier degradation
├── annotations.rs          memory ↔ annotation (트리플 스토어)
├── triples.rs              SPO 트리플
├── episodic_graph.rs       에피소드 그래프
├── polyphonic_recall.rs    다중 쿼리 팬아웃
├── veracity_consolidation.rs 신뢰도 통합
├── shmr.rs                 Semantic Hierarchical Memory Retrieval
├── patterns.ts → .rs       패턴 매칭
├── banks.rs                BankManager + 스코핑 (global/per-project/per-project-tagged)
└── session.rs              MnemopiSessionState (auto-recall/retain/consolidate/dispose)
```

**세션 와이어링 (oxicode-cli)**:
- `beforeAgentStartPrompt` → 시스템 프롬프트에 recall 블록 주입
- `maybeRetainOnAgentEnd` → 매 N턴 대화 요약 저장
- `CompactionHook` → compaction 시 memory 컨텍스트
- `dispose` → consolidate + DB close

**검증 기준**:
- 세션 종료 → sleep → episodic 압축 확인
- 다중 프로젝트 뱅크 분리 (per-project 스코핑)
- `/memory` 슬래시 명령 (status/clear/enqueue/stats/diagnose)

---

## 6. 기존 설계 수정 사항

기존 `11-mnemopi-backend.md`에 대한 타겟 수정:

| 기존 § | 수정 내용 | 이유 |
|--------|----------|------|
| §2.1 모듈 목록 | 12개 → 32개 확장 (§1.2 표 참조) | 20개 모듈 누락 |
| §2.5 recall.rs | `hybrid_recall` 단일 함수 → `score_candidate` + `recall_enhanced` + `mmr_rerank` 분리 | omp recall.ts가 단일 함수가 아님 |
| §2.7 embeddings.rs | `fastembed::TextEmbedding` → `fastembed` crate v4 | fastembed-rs API |
| §3 MnemopiMemoryBackend | 브릿지 30줄 → session.rs(662줄) + backend.ts(612줄) 분석 추가 | 세션 라이프사이클 누락 |
| §5 config.rs | `MnemopiConfig` → `MnemopiBackendConfig` 확장 (polyphonicRecall, enhancedRecall, proactiveLinking, injectionTokenLimit, recallMaxQueryChars 추가) | v16.1.21 신규 필드 |
| §6 N3.x | N3.1-N3.19 → Phase 1/2/3 재그룹화 (§5 참조) | 의존성 순서 + 임베딩 분리 |
| §7 위험 | `rusqlite 동기 + Mutex` → `spawn_blocking` 패턴 명시 | tokio 환경에서 SQLite 호출 |

---

## 7. 미결정 사항

| # | 항목 | 옵션 | 권고 |
|---|------|------|------|
| D1 | `oxicode-mnemopi` 크레이트 배치 | workspace 내 / 별도 저장소 | workspace 내 (oxicode-hashline 패턴) |
| D2 | 임베딩 기본 경로 | 원격 API / 로컬 fastembed | 원격 API (경량, 기본 feature off) |
| D3 | MemoryStore 포트 수정 | search에 filter 파라미터 추가 / 별도 trait | search 시그니처 유지, MemoryBackend에서 recall_text 추가 |
| D4 | 세션 상태 관리 | oxicode-cli AgentSession에 통합 / 별도 SessionState 구조체 | 별도 `MnemopiSessionState` (omp 패턴) |
| D5 | memory_reflect 도구 의미 변경 | recall → 합성 / 기존 요약 저장 유지 | recall → 합성 (omp 정합). 기존 summary 파라미터 deprecated |
| D6 | embed-worker 서브프로세스 | spawn_blocking / 별도 프로세스 | spawn_blocking (Rust는 JS보다 블로킹 우려 적음) |
