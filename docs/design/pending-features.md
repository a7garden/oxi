# 미구현 기능 설계서

> **대상**: oxicode-sdk + oxicode-cli  
> **목적**: oxios(에이전트 OS)에서 필요로 하는 미구현 기능의 구현 설계  
> **날짜**: 2026-06-01

---

## 목차

1. [SharedMemory / WorkQueue 영속화](#1-sharedmemory--workqueue-영속화)
2. [OTel Span 익스포트](#2-otel-span-익스포트)
3. [TUI Branch Switching](#3-tui-branch-switching)
4. [EventStore 삭제](#4-eventstore-삭제)
5. [PluginLoader 삭제](#5-pluginloader-삭제)

---

## 1. SharedMemory / WorkQueue 영속화

### 문제

oxios가 데몬으로 장시간 실행됨. 프로세스 재시작 시 SharedMemory, WorkQueue의 데이터가 전부 날아감.

### 설계

#### 접근: JSONL append-only 백엔드

세션 스토어(oxicode-store)에서 이미 검증된 JSONL 패턴을 재사용.

```
~/.oxicode/state/
├── shared_memory.jsonl      # SharedMemory 영속 로그
└── work_queue.jsonl          # WorkQueue 영속 로그
```

#### SharedMemory 백엔드

```rust
// oxicode-sdk/src/coordination/shared_memory.rs 에 추가

/// 영속화 백엔드 트레이트
pub trait MemoryBackend: Send + Sync {
    /// 엔트리 변경사항 기록
    fn persist(&self, event: &MemoryPersistEvent) -> anyhow::Result<()>;
    /// 전체 상태 복원
    fn restore(&self) -> anyhow::Result<HashMap<MemoryKey, MemoryEntry>>;
}

/// 영속 이벤트 (append-only)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum MemoryPersistEvent {
    Write { key: MemoryKey, entry: MemoryEntry },
    Delete { key: MemoryKey },
    Increment { key: MemoryKey, delta: i64, result_version: u64 },
}

/// 파일 기반 JSONL 백엔드
pub struct FileMemoryBackend {
    path: PathBuf,
}

impl MemoryBackend for FileMemoryBackend {
    fn persist(&self, event: &MemoryPersistEvent) -> anyhow::Result<()> {
        // 1. JSONL 한 줄 append (atomic write)
        // 2. 파일 크기가 임계치 초과 시 compaction
    }

    fn restore(&self) -> anyhow::Result<HashMap<MemoryKey, MemoryEntry>> {
        // 1. JSONL 파일 전체 읽기
        // 2. 이벤트를 순서대로 재생 (replay)
        // 3. 최종 상태 반환
    }
}
```

**SharedMemory 수정**:

```rust
pub struct SharedMemory {
    entries: RwLock<HashMap<MemoryKey, MemoryEntry>>,
    tx: broadcast::Sender<MemoryEvent>,
    backend: Option<Arc<dyn MemoryBackend>>,  // 추가
}

impl SharedMemory {
    pub fn new() -> Self { /* backend: None */ }
    
    pub fn with_backend(backend: Arc<dyn MemoryBackend>) -> anyhow::Result<Self> {
        // 1. backend.restore()로 초기 상태 복원
        // 2. 메모리 로드
    }
    
    pub fn write(...) {
        // 기존 로직 + self.backend.persist(Write{...}) 호출
    }
}
```

#### WorkQueue 백엔드

동일한 패턴:

```rust
pub trait QueueBackend: Send + Sync {
    fn persist(&self, event: &QueuePersistEvent) -> anyhow::Result<()>;
    fn restore(&self) -> anyhow::Result<Vec<WorkItem>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum QueuePersistEvent {
    Enqueued { item: WorkItem },
    Claimed { id: String, agent_id: String },
    Started { id: String },
    Completed { id: String, result: WorkResult },
    Retried { id: String },
    Cancelled { id: String },
}
```

#### Compaction 전략

JSONL이 무한히 커지는 것을 방지:

```
임계치: 10MB 또는 100,000 줄
초과 시: 현재 인메모리 상태를 스냅샷으로 저장 → 기존 JSONL 삭제 → 새 JSONL 시작
```

#### 설정

```toml
# ~/.oxicode/settings.toml
[persistence]
shared_memory_path = "~/.oxicode/state/shared_memory.jsonl"
work_queue_path = "~/.oxicode/state/work_queue.jsonl"
compaction_threshold_mb = 10
```

### 구현 범위

| 파일 | 변경 |
|------|------|
| `coordination/shared_memory.rs` | `MemoryBackend` trait, `FileMemoryBackend`, `with_backend()` |
| `coordination/work_queue.rs` | `QueueBackend` trait, `FileQueueBackend`, `with_backend()` |
| `coordination/mod.rs` | 새 타입 re-export |

### 테스트

- `test_file_backend_persist_and_restore`
- `test_file_backend_compaction`
- `test_concurrent_writes_to_backend`
- `test_restore_after_crash` (강제 종료 후 복원 시뮬레이션)

---

## 2. OTel Span 익스포트

### 문제

oxicode-sdk의 `Tracer`는 내부적으로 span을 생성하지만, 외부 모니터링 시스템(Jaeger, Zipkin, Datadog)으로 전송 불가.  
oxios에는 이미 `otel` feature와 `OtelConfig`가 정의되어 있지만, `init_telemetry_layers()`가 빈 Vec을 반환함.

### 설계

#### 접근: Tracer → OTel SpanBridge

oxicode-sdk의 자체 `Tracer`를 그대로 유지하면서, span 이벤트를 OTel으로 브릿지.

```
oxicode-sdk Tracer (내부)
  │
  ├── subscribe() → broadcast::Receiver<Span>
  │
  └── OtelSpanExporter (새로 추가)
        │
        ├── Span → opentelemetry::SpanData 변환
        └── OTLP gRPC exporter로 전송
```

#### 새 모듈: `observability/otel.rs`

```rust
// oxicode-sdk/src/observability/otel.rs
// #[cfg(feature = "otel")] 가드

use crate::Tracer;
use opentelemetry::trace::{SpanData, SpanKind as OtelSpanKind, Status};

/// OTel 익스포트 설정
pub struct OtelExportConfig {
    /// OTLP gRPC 엔드포인트 (예: "http://localhost:4317")
    pub endpoint: String,
    /// 서비스 이름
    pub service_name: String,
    /// 샘플링 비율 (0.0 ~ 1.0)
    pub sampling_ratio: f64,
}

/// Tracer의 span을 OTel로 익스포트
pub async fn start_otel_export(
    tracer: &Tracer,
    config: OtelExportConfig,
) -> anyhow::Result<OtelGuard> {
    // 1. OTLP gRPC exporter 생성
    // 2. TracerProvider 빌드
    // 3. tracer.subscribe()로 span 수신 루프 시작
    // 4. 수신된 span을 OTel SpanData로 변환 후 익스포트
    // 5. OtelGuard 반환 (drop 시 종료)
}

/// OTel 익스포트 종료 관리
pub struct OtelGuard { /* shutdown sender */ }

impl Drop for OtelGuard {
    fn drop(&mut self) {
        // graceful shutdown
    }
}
```

#### Span 변환 매핑

| oxicode-sdk | OTel |
|---------|------|
| `SpanKind::Agent` | `SpanKind::Internal` |
| `SpanKind::Tool` | `SpanKind::Client` |
| `SpanKind::Llm` | `SpanKind::Client` |
| `SpanKind::Internal` | `SpanKind::Internal` |
| `SpanStatus::Ok` | `Status::Ok` |
| `SpanStatus::Error` | `Status::error(msg)` |
| `Span.attributes` | OTel attributes |
| `Span.events` | OTel span events |
| `TraceId` (u64) | OTel TraceId (128-bit, 상위 64비트 0 패딩) |
| `SpanId` (u64) | OTel SpanId (64-bit) |

#### Cargo.toml (feature-gated)

```toml
# oxicode-sdk/Cargo.toml
[features]
otel = [
    "dep:opentelemetry",
    "dep:opentelemetry_sdk",
    "dep:opentelemetry-otlp",
]

[dependencies]
opentelemetry = { version = "0.27", optional = true }
opentelemetry_sdk = { version = "0.27", features = ["rt-tokio"], optional = true }
opentelemetry-otlp = { version = "0.27", optional = true }
```

### oxios 연동

oxios의 `telemetry_otel.rs`에서 oxicode-sdk의 `start_otel_export()`를 호출:

```rust
// oxios-kernel/src/telemetry_otel.rs
pub fn init_telemetry_layers(config: &OtelConfig) -> Result<Vec<...>> {
    if config.enabled {
        let sdk_tracer = global_tracer();  // 기존 oxios Tracer
        let guard = oxicode_sdk::start_otel_export(sdk_tracer, OtelExportConfig {
            endpoint: config.endpoint.clone(),
            service_name: config.service_name.clone(),
            sampling_ratio: config.sampling_ratio,
        }).await?;
        // guard를 앱 수명주기에 바인딩
    }
}
```

### 구현 범위

| 파일 | 변경 |
|------|------|
| `oxicode-sdk/Cargo.toml` | `otel` feature + opentelemetry 의존성 |
| `oxicode-sdk/src/observability/otel.rs` | 새 파일 — OtelExportConfig, start_otel_export, 변환 |
| `oxicode-sdk/src/observability/mod.rs` | `#[cfg(feature = "otel")] pub mod otel;` |
| `oxicode-sdk/src/lib.rs` | feature-gated re-export |

### 테스트

- `test_span_to_otel_conversion` (단위)
- `test_trace_id_padding` (u64 → 128-bit)
- `test_attributes_mapping`
- integration: mock OTLP collector에 span 전송

---

## 3. TUI Branch Switching

### 문제

oxicode-cli TUI에서 세션 트리 오버레이로 엔트리를 선택해도 아무 일이 일어나지 않음.  
pi에서는 `navigateTree()`로 세션을 특정 엔트리 시점으로 되감기 가능.

### pi의 동작 (참고)

```typescript
// pi: interactive-mode.ts
navigateTree: async (targetId, options) => {
    const result = await this.session.navigateTree(targetId, {
        summarize: options?.summarize,
        customInstructions: options?.customInstructions,
        replaceInstructions: options?.replaceInstructions,
        label: options?.label,
    });
    // 채팅 화면 초기화 후 선택한 시점의 메시지 다시 렌더
    this.chatContainer.clear();
    this.renderInitialMessages();
    this.showStatus("Navigated to selected point");
}
```

### 설계

#### 접근: SessionManager.navigate_to() + TUI 핸들러

```
사용자가 트리 오버레이에서 엔트리 선택
  → OverlayAction::NavigateToEntry { entry_id }
    → SessionManager::navigate_to(entry_id)
      → leaf_id를 entry_id로 변경
      → 선택한 시점 이후의 메시지만 표시
    → TUI 채팅 화면 다시 렌더
```

#### SessionManager 확장

```rust
// oxicode-store/src/session.rs 에 추가

impl SessionManager {
    /// 지정한 엔트리로 세션을 되감기.
    /// 
    /// leaf_id를 target_id로 설정하여, 이후 새 메시지가 
    /// 해당 엔트리의 자식으로 추가되도록 함.
    /// 기존 메시지는 삭제되지 않음 (append-only).
    pub fn navigate_to(&mut self, target_id: &str) -> Result<NavigateResult, String> {
        // 1. 엔트리 존재 확인
        let entry = self.get_entry(target_id)
            .ok_or_else(|| format!("Entry {} not found", target_id))?;
        
        // 2. leaf_id 갱신
        *self.leaf_id.write() = Some(target_id.to_string());
        
        // 3. 선택한 시점까지의 메시지 ID 수집
        let visible_ids = self.collect_ancestor_chain(target_id)?;
        
        Ok(NavigateResult {
            target_id: target_id.to_string(),
            visible_entry_ids: visible_ids,
        })
    }
    
    /// root → target_id까지의 조상 체인 수집
    fn collect_ancestor_chain(&self, target_id: &str) -> Result<Vec<String>, String> {
        let mut chain = Vec::new();
        let mut current = Some(target_id.to_string());
        while let Some(id) = current {
            chain.push(id.clone());
            let entry = self.get_entry(&id)
                .ok_or_else(|| format!("Entry {} not found", id))?;
            current = entry.parent_id.clone();
        }
        chain.reverse(); // root-first
        Ok(chain)
    }
}

pub struct NavigateResult {
    pub target_id: String,
    pub visible_entry_ids: Vec<String>,
}
```

#### TUI 핸들러 수정

```rust
// oxicode-cli/src/tui/handlers.rs

OverlayAction::NavigateToEntry { entry_id } => {
    state.overlay_state = None;
    
    if let Some(sm) = &mut state.session_manager {
        match sm.navigate_to(&entry_id) {
            Ok(result) => {
                // 1. 채팅 메시지 재구성
                state.chat_messages = rebuild_messages_from_chain(
                    sm, 
                    &result.visible_entry_ids
                );
                
                // 2. 채팅 화면 다시 렌더
                state.chat_scroll_to_bottom();
                
                // 3. 알림
                state.add_notification(
                    "Navigated to selected point".into(),
                    NotificationKind::Info,
                );
            }
            Err(e) => {
                state.add_notification(
                    format!("Navigation failed: {}", e),
                    NotificationKind::Error,
                );
            }
        }
    }
}
```

#### 메시지 재구성

```rust
/// 조상 체인의 엔트리 ID로부터 채팅에 표시할 메시지 재구성
fn rebuild_messages_from_chain(
    sm: &SessionManager,
    entry_ids: &[String],
) -> Vec<ChatMessage> {
    entry_ids.iter()
        .filter_map(|id| sm.get_entry(id))
        .filter_map(|entry| entry_to_chat_message(entry))
        .collect()
}
```

### 구현 범위

| 파일 | 변경 |
|------|------|
| `oxicode-store/src/session.rs` | `navigate_to()`, `collect_ancestor_chain()`, `NavigateResult` |
| `oxicode-cli/src/tui/handlers.rs` | `NavigateToEntry` 핸들러 구현 |
| `oxicode-cli/src/tui/mod.rs` 또는 `render.rs` | `rebuild_messages_from_chain()` |

### 테스트

- `test_navigate_to_entry` — leaf_id 변경 확인
- `test_navigate_to_nonexistent_entry` — 에러 처리
- `test_ancestor_chain_root_to_leaf` — 체인 수집
- `test_navigate_then_append` — 되감기 후 새 메시지가 올바른 부모 아래에 추가되는지

---

## 4. EventStore 삭제

### 근거

- oxios에서 **0 uses**
- AuditLog가 이미 유사한 역할 (append-only, 필터링, 구독)
- 이벤트 소싱이 필요한 경우 oxios-kernel에서 직접 구현하는 게 맞음

### 삭제 범위

| 파일 | 액션 |
|------|------|
| `oxicode-sdk/src/observability/event_store.rs` | **삭제** |
| `oxicode-sdk/src/observability/mod.rs` | `pub mod event_store` 및 re-export 제거 |
| `oxicode-sdk/src/lib.rs` | `EventStore`, `EventStoreConfig`, `EventQuery`, `StoredEvent` export 제거 |
| `oxicode-sdk/src/prelude.rs` | 동일 제거 |

### 영향

```bash
# oxios에서 EventStore 사용 없음 (이미 확인)
# oxicode-cli에서 EventStore 사용 없음
# 영향 없음
```

---

## 5. PluginLoader 삭제

### 근거

- oxios에서 **0 uses**
- oxicode-cli에 이미 자체 확장 시스템 있음 (native + WASM)
- JSON 매니페스트 기반 동적 미들웨어 로딩은 실제 사용자 없음

### 삭제 범위

| 파일 | 액션 |
|------|------|
| `oxicode-sdk/src/middleware/plugin.rs` | **삭제** |
| `oxicode-sdk/src/middleware/mod.rs` | `pub mod plugin` 및 `PluginLoader`, `PluginManifest` re-export 제거 |
| `oxicode-sdk/src/lib.rs` | export 제거 |
| `oxicode-sdk/src/prelude.rs` | export 제거 |

### 영향

oxios에서 PluginLoader 사용 없음. 영향 없음.

---

## 구현 우선순위

| 순서 | 항목 | 예상 공수 | 난이도 |
|------|------|----------|--------|
| 1 | EventStore 삭제 | 30분 | 낮음 |
| 2 | PluginLoader 삭제 | 30분 | 낮음 |
| 3 | TUI Branch Switching | 반나절 | 중간 |
| 4 | SharedMemory 영속화 | 1일 | 중간 |
| 5 | WorkQueue 영속화 | 반나절 | 중간 (SharedMemory와 동일 패턴) |
| 6 | OTel Span 익스포트 | 1~2일 | 높음 (새 의존성 + 비동기 브릿지) |

삭제 먼저 하고, TUI 브랜치 전환, 영속화, OTel 순서로 진행.
