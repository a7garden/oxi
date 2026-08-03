# oxicode 프로덕션 준비도 업그레이드 설계

> **작성일**: 2026-05-06  
> **기준 버전**: v0.5.0  
> **목표**: 프로덕션 배포 가능 수준(종합 85+점) 도달  
> **관련 문서**: `upgrade-to-v0.6.md` (품질 개선), 본 문서 (프로덕션 준비도)

---

## 1. 현재 상태 평가 (2026-05-06 기준)

### 1.1 종합 점수

| 분야 | 점수 | 프로덕션 기준치 | 판정 |
|------|:----:|:-------------:|:----:|
| 아키텍처 | 84 | 80 | ✅ 합격 |
| 테스트 | 76 | 80 | ❌ 불합격 |
| 코드 품질 | 75 | 80 | ❌ 불합격 |
| 보안 | 70 | 85 | ❌ 불합격 |
| 퍼포먼스 | 72 | 75 | ❌ 불합격 |
| 프로덕션 준비도 | 60 | 85 | ❌ 불합격 |
| **종합** | **73** | **80** | **❌ 프로덕션 불가** |

### 1.2 블로킹 이슈 (프로덕션 배포 불가 사유)

| ID | 심각도 | 분야 | 이슈 | 영향 |
|----|:------:|------|------|------|
| B1 | 🔴 Critical | 프로덕션 | CI/CD 파이프라인 없음 | 빌드/테스트/배포 자동화 불가 |
| B2 | 🔴 Critical | 보안 | API 키 로깅 마스킹 없음 | API 키가 로그/에러 메시지에 평문 노출 |
| B3 | 🔴 Critical | 보안 | 동적 라이브러리 검증 없음 | 악의적 `.so`/`.dylib` 무조건 로딩 |
| B4 | 🔴 Critical | 프로덕션 | 설정 검증 없음 | 잘못된 설정으로 런타임 패닉 가능 |
| B5 | 🟠 High | 퍼포먼스 | parking_lot::RwLock 비동기 위험 | 데드락/기아 상태 가능 |
| B6 | 🟠 High | 테스트 | 핵심 모듈 테스트 없음 | agent.rs 710줄, agent_loop 494줄 테스트 0 |
| B7 | 🟠 High | 테스트 | 14개 깨진 테스트 방치 | 테스트 신뢰도 훼손 |
| B8 | 🟡 Medium | 코드 품질 | unwrap 1025개 | 런타임 패닉 가능 |
| B9 | 🟡 Medium | 프로덕션 | graceful shutdown 미흡 | 크래시 시 세션 손실 |
| B10 | 🟡 Medium | 보안 | 경로 순회 방지 일부 누락 | read/edit 도구 경로 검증 불완전 |

---

## 2. 목표 점수 (Phase 완료 후)

| 분야 | 현재 | Phase A | Phase B | Phase C | 목표 |
|------|:----:|:-------:|:-------:|:-------:|:----:|
| 아키텍처 | 84 | 84 | 86 | 88 | **88** |
| 테스트 | 76 | 80 | 84 | 86 | **86** |
| 코드 품질 | 75 | 82 | 85 | 88 | **88** |
| 보안 | 70 | 82 | 86 | 88 | **88** |
| 퍼포먼스 | 72 | 78 | 82 | 84 | **84** |
| 프로덕션 준비도 | 60 | 78 | 84 | 88 | **88** |
| **종합** | **73** | **81** | **85** | **87** | **87** |

> **Phase A 완료 = 프로덕션 베타 가능 (81점)**  
> **Phase B 완료 = 프로덕션 정식 가능 (85점)**  
> **Phase C 완료 = 프로덕션 안정 (87점)**

---

## Phase A: 블로킹 이슈 해결 (1주)

> 목표: Critical/High 이슈 전부 해결, 프로덕션 베타 배포 가능 상태

### A1. CI/CD 파이프라인 구축 [B1]

**사유**: CI 없이는 품질 보증이 불가능. 모든 작업의 선행 조건.

```
.github/
├── workflows/
│   ├── ci.yml              # PR/push 시 빌드 + 테스트 + 린트
│   ├── release.yml          # 태그-push 시 릴리즈 바이너리 빌드
│   └── security-audit.yml   # 매일 cargo audit 실행
└── dependabot.yml           # 의존성 자동 업데이트
```

#### ci.yml

```yaml
name: CI
on: [push, pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace -- -D warnings
      - run: cargo test --workspace
      - run: cargo build --release --workspace

  security:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install cargo-audit && cargo audit
```

#### release.yml (cross-platform 빌드)

```yaml
name: Release
on:
  push:
    tags: ['v*']

jobs:
  build:
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-apple-darwin
            os: macos-13
          - target: x86_64-pc-windows-msvc
            os: windows-latest
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release --target ${{ matrix.target }}
      - uses: actions/upload-artifact@v4
        with:
          name: oxicode-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/oxicode*
```

**의존성**: 없음 (최우선 작업)

---

### A2. API 키 보안 강화 [B2]

**사유**: API 키가 `tracing::info!`, `Debug`, `Display` 구현에서 평문 노출 가능.

#### A2.1 시크릿 래퍼 타입 도입

```rust
// oxicode-ai/src/secret.rs (NEW)
use std::fmt;

/// API 키 등 민감 정보를 감싸는 타입.
/// Debug/Display 구현에서 값을 마스킹한다.
#[derive(Clone)]
pub struct Secret<T> {
    inner: T,
}

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    /// 실제 값에 접근. 로깅/표시 목적이 아닌 경우에만 사용.
    pub fn expose(&self) -> &T {
        &self.inner
    }

    pub fn expose_owned(self) -> T {
        self.inner
    }
}

// Debug 마스킹
impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

// Display 마스킹
impl<T> fmt::Display for Secret<String> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.inner;
        if s.len() > 8 {
            write!(f, "{}...{}", &s[..4], &s[s.len()-4..])
        } else {
            f.write_str("[REDACTED]")
        }
    }
}

// Serialize/Derialize는 값 그대로 (저장/전송 목적)
impl serde::Serialize for Secret<String> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        serializer.serialize_str(&self.inner)
    }
}

impl<'de> serde::Deserialize<'de> for Secret<String> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        Ok(Self::new(String::deserialize(deserializer)?))
    }
}
```

#### A2.2 프로바이더에 적용

```rust
// oxicode-ai/src/providers/mod.rs
use crate::secret::Secret;

// Before: 모든 프로바이더에서
api_key: Option<String>,

// After:
api_key: Option<Secret<String>>,

// HTTP 헤더 생성 시에만 노출
.header("x-api-key", key.expose())
.header("Authorization", format!("Bearer {}", key.expose()))
```

#### A2.3 로깅 필터링

```rust
// oxicode-ai/src/providers/mod.rs
// 요청 로깅에서 헤더 마스킹
fn log_request_headers(headers: &HeaderMap) {
    for (name, value) in headers.iter() {
        let value_str = if is_sensitive_header(name.as_str()) {
            "[REDACTED]"
        } else {
            value.to_str().unwrap_or("<non-ascii>")
        };
        tracing::debug!("  {name}: {value_str}");
    }
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(),
        "authorization" | "x-api-key" | "x-goog-api-key" |
        "api-key" | "cookie" | "set-cookie"
    )
}
```

---

### A3. 동적 라이브러리 보안 [B3]

**사유**: `libloading::Library::new(path)`가 어떤 파일이든 로딩. 악의적 라이브러리 방어 필요.

#### A3.1 확장 매니페스트 검증

```rust
// oxicode-cli/src/extensions/loading.rs (NEW)

/// 확장 바이너리 로딩 전 검증 수행
pub fn validate_extension(path: &Path) -> Result<ValidatedExtension, ExtensionError> {
    // 1. 파일 존재 확인
    if !path.exists() {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "File not found".into(),
        });
    }

    // 2. 파일 크기 확인 (0바이트 또는 과도하게 큰 파일 차단)
    let metadata = std::fs::metadata(path)?;
    if metadata.len() == 0 {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "Empty file".into(),
        });
    }
    if metadata.len() > 100 * 1024 * 1024 {
        // 100MB 제한
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "File too large (>100MB)".into(),
        });
    }

    // 3. 매니페스트 파일(.oxicode-extension.json) 존재 확인
    let manifest_path = path.with_extension("oxicode-extension.json");
    if !manifest_path.exists() {
        return Err(ExtensionError::LoadFailed {
            name: path.display().to_string(),
            reason: "Missing manifest file (.oxicode-extension.json)".into(),
        });
    }

    // 4. 매니페스트 파싱 및 검증
    let manifest: ExtensionManifest = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)?
    ).map_err(|e| ExtensionError::LoadFailed {
        name: path.display().to_string(),
        reason: format!("Invalid manifest: {e}"),
    })?;

    // 5. 권한 검증 (요청된 권한이 허용 목록에 있는지)
    validate_permissions(&manifest)?;

    // 6. 버전 호환성 검증
    validate_version(&manifest)?;

    // 7. 파일 해시 체크섬 기록 (최초 로딩 시)
    let checksum = sha256_file(path)?;

    Ok(ValidatedExtension {
        path: path.to_path_buf(),
        manifest,
        checksum,
    })
}
```

#### A3.2 확장 샌드박싱 강화

```rust
// oxicode-cli/src/extensions/registry.rs

impl ExtensionRegistry {
    pub fn load_extension(&mut self, path: &Path) -> Result<ExtensionId, ExtensionError> {
        // 검증 통과한 확장만 로딩
        let validated = validate_extension(path)?;

        // 기존 panic 캐치 유지
        let result = std::panic::catch_unwind(|| {
            self.load_validated(validated)
        });

        match result {
            Ok(Ok(id)) => {
                tracing::info!("Extension loaded: {id}");
                Ok(id)
            }
            Ok(Err(e)) => {
                tracing::error!("Extension load failed: {e}");
                Err(e)
            }
            Err(panic_info) => {
                tracing::error!("Extension panicked during load: {panic_info:?}");
                Err(ExtensionError::LoadFailed {
                    name: path.display().to_string(),
                    reason: "Extension panicked".into(),
                })
            }
        }
    }
}
```

---

### A4. 설정 검증 시스템 [B4]

**사유**: `temperature: -5.0`, `max_tokens: 0` 등 잘못된 설정이 런타임까지 감지 안 됨.

```rust
// oxicode-cli/src/settings_validation.rs (NEW)

/// 설정 검증 결과
#[derive(Debug)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

#[derive(Debug)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ValidationWarning {
    pub field: String,
    pub message: String,
}

impl Settings {
    /// 현재 설정의 유효성을 검증한다.
    /// 애플리케이션 시작 시 호출.
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport {
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        // temperature: [0.0, 2.0]
        if let Some(temp) = self.default_temperature {
            if !(0.0..=2.0).contains(&temp) {
                report.errors.push(ValidationError {
                    field: "default_temperature".into(),
                    message: format!("Must be between 0.0 and 2.0, got {temp}"),
                });
            }
        }

        // max_tokens: [1, 128000]
        if let Some(tokens) = self.max_response_tokens {
            if tokens == 0 {
                report.errors.push(ValidationError {
                    field: "max_response_tokens".into(),
                    message: "Must be at least 1".into(),
                });
            } else if tokens > 128_000 {
                report.warnings.push(ValidationWarning {
                    field: "max_response_tokens".into(),
                    message: format!("Very high value ({tokens}). Most models don't support this."),
                });
            }
        }

        // tool_timeout_seconds: [1, 3600]
        if let Some(timeout) = self.tool_timeout_seconds {
            if timeout == 0 {
                report.errors.push(ValidationError {
                    field: "tool_timeout_seconds".into(),
                    message: "Must be at least 1 second".into(),
                });
            } else if timeout > 3600 {
                report.warnings.push(ValidationWarning {
                    field: "tool_timeout_seconds".into(),
                    message: format!("Very long timeout ({timeout}s). Tools may hang."),
                });
            }
        }

        // thinking_level 유효성
        if let Some(ref level) = self.thinking_level {
            if !["none", "minimal", "standard", "thorough"].contains(&level.as_str()) {
                report.errors.push(ValidationError {
                    field: "thinking_level".into(),
                    message: format!(
                        "Must be one of: none, minimal, standard, thorough. Got: {level}"
                    ),
                });
            }
        }

        // default_model 형식 (provider/model 또는 provider/org/model)
        if let Some(ref model) = self.default_model {
            if !model.contains('/') {
                report.warnings.push(ValidationWarning {
                    field: "default_model".into(),
                    message: format!(
                        "Expected format 'provider/model', got: {model}"
                    ),
                });
            }
        }

        report
    }
}
```

#### 적용 지점

```rust
// oxicode-cli/src/main.rs — 시작 시 검증
fn main() {
    let settings = Settings::load().expect("Failed to load settings");
    let report = settings.validate();

    for err in &report.errors {
        tracing::error!("Config error: {} — {}", err.field, err.message);
    }
    for warn in &report.warnings {
        tracing::warn!("Config warning: {} — {}", warn.field, warn.message);
    }

    if !report.errors.is_empty() {
        eprintln!("❌ Configuration has {} error(s). Fix before proceeding.", report.errors.len());
        for err in &report.errors {
            eprintln!("   • {}: {}", err.field, err.message);
        }
        std::process::exit(1);
    }
    // ...
}
```

---

### A5. 깨진 테스트 수정/제거 [B7]

**사유**: 14개 `#[ignore]` 테스트가 테스트 스위트 신뢰도를 훼손.

| 파일 | ignore 수 | 조치 |
|------|:---------:|------|
| `clipboard_image.rs` | 3 | 제거 — 클립보드 이미지는 OS 의존적, CI 불가 |
| `image_convert.rs` | 6 | 수정 — `tempfile` 기반으로 재작성 |
| `frontmatter.rs` | 3 | 수정 — 파서 로직 재확인 후 수정 |
| `auto_compaction.rs` | 1 | 수정 — 실제 compaction 로직과 동기화 |
| `error_recovery.rs` | 1 | 수정 — 서킷 브레이커 상태와 동기화 |

**원칙**: 깨진 테스트는 48시간 내 수정 또는 제거. `#[ignore]` 방치 금지.

---

### A6. 핵심 모듈 테스트 추가 [B6]

**최소 테스트 목표** — 테스트 0인 핵심 파일에 대한 최소 커버리지:

```
oxicode-agent/src/agent.rs (710줄) — 15개 테스트 추가
├── test_agent_new_with_config
├── test_agent_switch_model_same_api
├── test_agent_switch_model_cross_api
├── test_agent_try_fallback_success
├── test_agent_try_fallback_all_fail
├── test_agent_circuit_breaker_opens
├── test_agent_state_snapshot
├── test_agent_compaction_trigger
├── test_agent_max_iterations
├── test_agent_tool_execution_sequential
├── test_agent_tool_execution_parallel
├── test_agent_cancellation_signal
├── test_agent_error_recovery_retry
├── test_agent_streaming_events_order
└── test_agent_context_update

oxicode-agent/src/agent_loop/mod.rs (494줄) — 10개 테스트 추가
├── test_run_single_turn_no_tools
├── test_run_multi_turn_with_tools
├── test_run_max_iterations_stop
├── test_run_steering_injection
├── test_run_follow_up_queue
├── test_run_model_switch_mid_conversation
├── test_run_compaction_during_loop
├── test_run_error_continues_after_retry
├── test_run_tool_timeout
└── test_run_cancellation
```

---

## Phase B: 안정성 강화 (1주)

> 목표: 런타임 안정성 확보, 에러 복구 체계화

### B1. parking_lot::RwLock 비동기 안전 점검 [B5]

**문제**: `parking_lot::RwLock`은 async-aware가 아님. 락을 잡은 상태로 `.await`하면 데드락 가능.

#### B1.1 위험 지점 식별

```bash
# 락 가드가 .await를 넘어가는지 정적 분석
grep -rn "\.read()\|\.write()" --include="*.rs" oxicode-*/src/ | \
  grep -A5 "\.await"
```

#### B1.2 해결 전략 (3단계)

**패턴 1: 락 범위 축소** (대부분의 경우)
```rust
// Before: 락이 .await를 넘어감
let data = state.read().clone();  // 즉시 clone 후 락 해제
some_async_fn(&data).await;       // 안전

// After: 명시적 스코프
let data = {
    let guard = state.read();
    guard.clone()
    // guard 드롭 → 락 해제
};
some_async_fn(&data).await;       // 안전
```

**패턴 2: tokio::sync::RwLock** (장시간 유지 필요한 경우)
```rust
// 상태가 크고 clone 비용이 큰 경우만
use tokio::sync::RwLock;

// 읽기 락은 여러 곳에서 동시 유지 가능
let guard = state.read().await;
some_async_fn(&guard).await;  // OK — tokio::sync::RwLock은 async-aware
```

**패턴 3: atomic 단일 값** (단순 카운터/플래그)
```rust
// Before: RwLock<bool>
shutdown: parking_lot::RwLock<bool>,

// After: AtomicBool
shutdown: AtomicBool,
```

#### B1.3 변경 대상

| 크레이트 | RwLock 수 | 패턴1 (clone) | 패턴2 (tokio) | 패턴3 (atomic) |
|----------|:---------:|:------:|:------:|:------:|
| oxicode-ai | 0 | — | — | — |
| oxicode-agent | 15 | 12 | 2 | 1 |
| oxicode-tui | 0 | — | — | — |
| oxicode-cli | 59 | 45 | 10 | 4 |

---

### B2. Graceful Shutdown [B9]

**문제**: SIGINT/SIGTERM 시 진행 중인 세션이 손실될 수 있음.

#### B2.1 시그널 핸들러

```rust
// oxicode-cli/src/shutdown.rs (NEW)

use tokio::sync::broadcast;

/// Graceful shutdown 조정자
pub struct ShutdownCoordinator {
    tx: broadcast::Sender<ShutdownSignal>,
    state: Arc<AtomicU8>,  // 0=Running, 1=Draining, 2=Forced
}

#[derive(Debug, Clone)]
pub enum ShutdownSignal {
    /// SIGINT 수신. 진행 중인 작업 완료 후 종료.
    Graceful,
    /// 두 번째 SIGINT. 즉시 종료.
    Force,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(2);
        Self {
            tx,
            state: Arc::new(AtomicU8::new(0)),
        }
    }

    /// SIGINT/SIGTERM 리스너 시작
    pub fn listen(&self) {
        let tx_graceful = self.tx.clone();
        let tx_force = self.tx.clone();
        let state = self.state.clone();

        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            let prev = state.swap(1, Ordering::SeqCst);
            if prev == 1 {
                // 두 번째 Ctrl+C
                state.store(2, Ordering::SeqCst);
                let _ = tx_force.send(ShutdownSignal::Force);
                tracing::warn!("Force shutdown requested");
            } else {
                let _ = tx_graceful.send(ShutdownSignal::Graceful);
                tracing::info!("Graceful shutdown requested (Ctrl+C again to force)");
            }
        });
    }

    /// 구독자에게 shutdown 신호 전달
    pub fn subscribe(&self) -> broadcast::Receiver<ShutdownSignal> {
        self.tx.subscribe()
    }

    pub fn is_draining(&self) -> bool {
        self.state.load(Ordering::SeqCst) > 0
    }
}
```

#### B2.2 세션 자동 저장

```rust
// oxicode-cli/src/session.rs에 추가

impl SessionManager {
    /// 현재 세션을 비동기로 자동 저장하는 백그라운드 태스크
    pub fn start_auto_save(
        &self,
        session_id: SessionId,
        mut shutdown_rx: broadcast::Receiver<ShutdownSignal>,
    ) -> JoinHandle<()> {
        let state = self.state.clone();
        let save_path = self.save_path.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // 주기적 저장
                        if let Err(e) = save_session(&state, &save_path) {
                            tracing::warn!("Auto-save failed: {e}");
                        }
                    }
                    Ok(signal) = shutdown_rx.recv() => {
                        // Shutdown 신호 수신 → 즉시 저장
                        tracing::info!("Saving session before shutdown...");
                        if let Err(e) = save_session(&state, &save_path) {
                            tracing::error!("Final save failed: {e}");
                        }
                        if matches!(signal, ShutdownSignal::Graceful) {
                            // 진행 중인 도구 완료 대기 (최대 10초)
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            let _ = save_session(&state, &save_path);
                        }
                        break;
                    }
                }
            }
        })
    }
}
```

---

### B3. 에러 복구 체계화

**현재 상태**: 서킷 브레이커 + 재시도가 구현되어 있으나, 사용자에게 노출되는 에러 메시지가 불친절.

#### B3.1 사용자 친화적 에러 메시지

```rust
// oxicode-ai/src/error.rs — Display 개선

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::RateLimited { retry_after_secs } => {
                write!(f, "⚠️ API rate limit reached. Retry in {}s.", retry_after_secs.unwrap_or(30))
            }
            ProviderError::InvalidApiKey { provider } => {
                write!(f, "❌ Invalid API key for {provider}. Run: oxicode config set {provider}_api_key <YOUR_KEY>")
            }
            ProviderError::NetworkError { source } => {
                write!(f, "🔌 Network error: {source}. Check your internet connection.")
            }
            ProviderError::ContextOverflow { model, tokens, limit } => {
                write!(f, "📏 Context too long: {tokens} tokens (limit: {limit}) for {model}. Consider enabling auto-compaction.")
            }
            // ... 나머지
            _ => write!(f, "{self:?}"),
        }
    }
}
```

#### B3.2 에러 복구 가이드

```rust
// oxicode-agent/src/error_recovery.rs — 복구 액션 추가

pub enum RecoveryAction {
    /// 사용자에게 메시지 표시 후 재시도
    RetryWithMessage(String),
    /// 모델 전환 후 재시도
    SwitchModel { from: String, to: String },
    /// 컨텍스트 압축 후 재시도
    CompactContext,
    /// 복구 불가, 사용자 개입 필요
    UserIntervention(String),
}

impl RetryableError {
    pub fn suggest_recovery(&self) -> RecoveryAction {
        match self {
            Self::RateLimited { retry_after } => {
                RecoveryAction::RetryWithMessage(
                    format!("Rate limited. Waiting {retry_after}s before retry...")
                )
            }
            Self::ContextOverflow { .. } => RecoveryAction::CompactContext,
            Self::ModelNotAvailable { model } => RecoveryAction::SwitchModel {
                from: model.clone(),
                to: "fallback-model".into(),
            },
            Self::NetworkError { .. } => RecoveryAction::RetryWithMessage(
                "Network error. Retrying...".into()
            ),
            _ => RecoveryAction::UserIntervention(format!("{self}")),
        }
    }
}
```

---

### B4. 경로 보안 강화 [B10]

**현재 상태**: `read`, `edit`, `ls`는 `..` 방지가 있으나, `bash` 도구는 셸 명령으로 경로 제어가 불가.

#### B4.1 통합 경로 검증 유틸

```rust
// oxicode-agent/src/tools/path_security.rs (NEW)

/// 파일 접근 시 보안 검증
pub struct PathGuard {
    allowed_roots: Vec<PathBuf>,
}

impl PathGuard {
    pub fn new(cwd: &Path) -> Self {
        Self {
            allowed_roots: vec![cwd.to_path_buf()],
        }
    }

    /// 경로가 허용된 루트 내에 있는지 확인
    pub fn validate(&self, path: &Path) -> Result<PathBuf, PathSecurityError> {
        // 1. canonicalize로 실제 경로 확인 (심볼릭 링크 해소)
        let canonical = path.canonicalize().map_err(|_| {
            PathSecurityError::NotFound(path.to_path_buf())
        })?;

        // 2. 경로 순회 방지
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err(PathSecurityError::Traversal(path.to_path_buf()));
        }

        // 3. 허용된 루트 내부인지 확인
        let is_allowed = self.allowed_roots.iter().any(|root| {
            canonical.starts_with(root)
        });

        if !is_allowed {
            return Err(PathSecurityError::OutsideWorkspace(
                canonical,
                self.allowed_roots[0].clone(),
            ));
        }

        Ok(canonical)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathSecurityError {
    #[error("Path not found: {0}")]
    NotFound(PathBuf),
    #[error("Path traversal detected: {0}")]
    Traversal(PathBuf),
    #[error("Path outside workspace: {0} (root: {1})")]
    OutsideWorkspace(PathBuf, PathBuf),
}
```

#### B4.2 적용

```rust
// oxicode-agent/src/tools/read.rs
async fn execute(...) -> Result<AgentToolResult, ToolError> {
    let path = params["path"].as_str().unwrap_or(".");
    let guard = PathGuard::new(cwd);
    let validated_path = guard.validate(Path::new(path))?;
    // validated_path로 파일 읽기
}
```

---

## Phase C: 품질 마일리지 (1주)

> 목표: 장기적 유지보수성 확보

### C1. unwrap 점진적 제거 [B8]

**우선순위**: 프로덕션 경로(비테스트)의 위험한 unwrap부터 제거.

#### C1.1 clippy 설정으로 새로운 unwrap 방지

```rust
// 각 크레이트 lib.rs 상단
#![warn(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]
```

이것만으로 새로운 unwrap 추가를 사전 차단. 기존 1025개는 점진적으로.

#### C1.2 위험도별 분류

| 위험도 | 개수 | 조치 |
|:------:|:----:|------|
| 🔴 Crash | ~50 | Phase C에서 즉시 수정 (환경변수, 파일 I/O, 네트워크) |
| 🟠 Silent data loss | ~100 | Phase C에서 수정 (인덱스, 파싱) |
| 🟡 Infallible | ~350 | `expect("reason")`로 변경 |
| ✅ Test-only | ~525 | 그대로 유지 (allow 커버) |

---

### C2. 퍼포먼스 최적화

#### C2.1 불필요한 clone() 제거

```rust
// 자주 발견되는 안티패턴:
// 1. &str → String 변환이 불필요한 경우
fn process(text: &str) {
    do_something(text.to_string());  // ❌
    do_something_ref(text);          // ✅
}

// 2. 이미 소유권이 있는 값의 clone
fn handle(msg: Message) {
    process(msg.clone());  // ❌
    process(msg);          // ✅ (소유권 이전)
}

// 3. 순회 중 불필요한 할당
for item in &items {
    let name = item.name.to_string();  // ❌ 매 반복마다 할당
    process(&name);
    // 대안: &str로 처리 가능하면 그렇게
}
```

#### C2.2 문자열 빌더 최적화

```rust
// 이미 with_capacity를 사용하는 곳은 좋음 (10개소 확인됨)
// 하지만 아직 적용 안 된 곳:
//
// messages.rs의 to_xml_string() — 대량 문자열 조합
// → estimated_len 계산 후 String::with_capacity 사용

// Before
let mut result = String::new();
for block in blocks { result.push_str(&block.to_string()); }

// After
let estimated_len = blocks.iter().map(|b| b.estimated_len()).sum();
let mut result = String::with_capacity(estimated_len);
for block in blocks { block.write_to(&mut result); }  // 중간 String 할당 제거
```

#### C2.3 스트리밍 버퍼 풀

```rust
// oxicode-ai/src/providers/sse_parser.rs
// SSE 청크 처리 시 매번 새 버퍼 할당 대신 재사용

pub struct SseParser {
    buffer: String,      // 재사용 버퍼
    // ...
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(4096),  // 초기 용량
        }
    }

    pub fn parse_chunk(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.clear();  // 재사용
        // ...
    }
}
```

---

### C3. thiserror 버전 통일

```toml
# 현재 상태:
# oxicode-ai: thiserror = "2"
# oxicode-agent: thiserror = "1"

# [workspace.dependencies]로 통일
# Cargo.toml (workspace root)
[workspace.dependencies]
thiserror = "2"

# 각 크레이트 Cargo.toml:
thiserror = { workspace = true }
```

---

### C4. 누락된 핵심 문서

| 문서 | 내용 | 우선순위 |
|------|------|:--------:|
| `SECURITY.md` | 보안 정책, 취약점 보고 가이드 | 🔴 |
| `CONTRIBUTING.md` | 기여 가이드, 코드 스타일, PR 프로세스 | 🟠 |
| `docs/deployment.md` | 설치/배포 가이드, 크로스 컴파일 | 🟠 |
| `docs/troubleshooting.md` | 일반적인 문제 해결 가이드 | 🟡 |

#### SECURITY.md

```markdown
# Security Policy

## Supported Versions
| Version | Supported |
|---------|-----------|
| 0.5.x   | ✅ |
| < 0.5   | ❌ |

## Reporting a Vulnerability
Email: security@oxicode.dev (예시)
Do NOT file a public issue for security vulnerabilities.

## Security Features
- API keys are never logged in plaintext
- Extension loading requires manifest validation
- Path traversal prevention on all file tools
- Dynamic library sandboxing with panic isolation
```

---

## 3. 실행 계획

```
Week 1 (Phase A): 블로킹 이슈 해결
├── Day 1:     A1 CI/CD 파이프라인 구축
├── Day 2:     A2 API 키 보안 (Secret<T> 타입 + 로깅 필터)
├── Day 3:     A3 동적 라이브러리 검증 + A4 설정 검증
├── Day 4:     A5 깨진 테스트 수정/제거 (14개)
├── Day 5:     A6 핵심 모듈 테스트 추가 (25개)
└── Day 6-7:   통합 검증 + Phase A 리뷰

Week 2 (Phase B): 안정성 강화
├── Day 1-2:   B1 parking_lot::RwLock 비동기 안전 점검
├── Day 3:     B2 Graceful shutdown + 세션 자동 저장
├── Day 4:     B3 에러 복구 체계화
├── Day 5:     B4 경로 보안 강화 (PathGuard)
└── Day 6-7:   통합 검증 + Phase B 리뷰

Week 3 (Phase C): 품질 마일리지
├── Day 1-2:   C1 위험한 unwrap 제거 (~50개) + clippy 설정
├── Day 3:     C2 퍼포먼스 최적화 (clone, 버퍼)
├── Day 4:     C3 thiserror 통일 + C4 문서 작성
└── Day 5:     최종 검증 + 점수 재평가
```

---

## 4. 파일 변경 규모

| Phase | 신규 파일 | 수정 파일 | 총 라인 (추정) |
|-------|:---------:|:---------:|:-------------:|
| A | ~8 | ~25 | ~2,500 |
| B | ~5 | ~20 | ~1,800 |
| C | ~4 | ~30 | ~1,200 |
| **총계** | **~17** | **~75** | **~5,500** |

---

## 5. v0.6 설계와의 관계

본 설계는 `upgrade-to-v0.6.md`와 **상호 보완** 관계입니다:

| 문서 | 초점 | 중복 |
|------|------|------|
| `upgrade-to-v0.6.md` | 코드 품질 (unwrap, 문서화, 아키텍처 정리) | A5 테스트 일부 겹침 |
| 본 문서 | 프로덕션 준비도 (CI/CD, 보안, 안정성) | B8 unwrap 일부 겹침 |

### 권장 실행 순서

```
Phase A (본 문서) → Phase 1 (v0.6) → Phase B (본 문서) → Phase 2-3 (v0.6) → Phase C (본 문서) → Phase 4-5 (v0.6)
```

> CI/CD와 보안이 먼저 확보되어야 나머지 작업이 안전하게 진행됨.

---

## 6. 리스크 관리

| 리스크 | 확률 | 영향 | 완화 |
|--------|:----:|:----:|------|
| Secret<T> 도입 후 API breaking change | 중 | 낮 | `#[non_exhaustive]` + expose() 명시적 |
| parking_lot → tokio::sync::RwLock 성능 저하 | 낮 | 중 | 벤치마크로 검증, 필요한 곳만 변경 |
| CI 파이프라인 macOS 빌드 실패 | 중 | 낮 | cross-compilation 사전 테스트 |
| graceful shutdown 중 세션 손상 | 낮 | 높음 | WAL 패턴 (append-only JSONL로 이미 부분 구현) |
| 확장 검증으로 기존 확장 로딩 실패 | 중 | 중 | 마이그레이션 가이드 + fallback 로딩 |
