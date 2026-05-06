# oxi 90점 달성 — 누락된 작업 분석

> **작성일**: 2026-05-06
> **전제**: production-readiness-upgrade-design.md + upgrade-to-v0.6.md 모두 완료 후
> **현실적 예측**: 두 설계 완료 시 ~82점. 90점 도달에는 추가 2-3주 필요.

---

## 82 → 90 점 갭 분석

두 설계를 모두 완료해도 남는 문제와 해결 방안:

---

## GAP 1: oxi-cli 분해 (아키텍처 +3점)

**현재**: oxi-cli 48,224줄, 70개 파일 — 단일 크레이트가 너무 큼
**목표**: 3개 크레이트로 분리

```
oxi-cli (48K줄) → 분해
├── oxi-cli        (~15K줄)  메인 진입점, CLI 파싱, 설정
├── oxi-session    (~12K줄)  세션 관리, JSONL, 분기, 내보내기
└── oxi-ext        (~8K줄)   확장 시스템, 패키지 매니저, 리소스 로더
```

### 분해 기준

| 새 크레이트 | 이동할 파일 | 의존성 |
|------------|-----------|--------|
| `oxi-session` | session.rs, session_navigation.rs, export.rs, branch_summarization.rs, auto_compaction.rs, compaction_utils.rs | oxi-ai |
| `oxi-ext` | extensions/*, packages.rs, resource_loader.rs, resource_loader_compat.rs | oxi-agent |
| `oxi-cli` | main.rs, lib.rs, settings.rs, keybindings.rs, theme.rs, auth_storage.rs, agent_session*.rs, rpc_mode.rs, model_registry.rs, model_resolver.rs, error_recovery.rs, templates.rs, footer_data.rs, skills/*, tui/* | oxi-session, oxi-ext |

### 워크스페이스 변경

```toml
[workspace]
resolver = "2"
members = [
    "oxi-ai",
    "oxi-agent",
    "oxi-tui",
    "oxi-session",   # NEW
    "oxi-ext",       # NEW
    "oxi-cli",
]
```

### 작업량: ~3일 (파일 이동 + import 수정 + 빌드 수정)

---

## GAP 2: Agent/AgentLoop 통합 (아키텍처 +2점)

**현재**: Agent(710줄) + AgentLoop(494줄) 이원화
**목표**: Agent = thin wrapper over AgentLoop

```rust
// oxi-agent/src/agent.rs (710줄 → ~120줄)

/// Agent는 AgentLoop의 편의 래퍼.
/// mpsc 채널 기반 인터페이스를 제공한다.
pub struct Agent {
    inner: AgentLoop,
    state: SharedState,
}

impl Agent {
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
        let state = SharedState::new();
        let inner = AgentLoop::new(provider, config.into(), ToolRegistry::new(), state.clone());
        Self { inner, state }
    }

    /// AgentLoop::run()을 mpsc 채널로 브릿지
    pub async fn run_with_channel(
        &self,
        prompt: String,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        self.inner.run(prompt, |event| {
            let _ = tx.blocking_send(event);
        }).await
    }

    // switch_model, try_fallback 등은 AgentLoop에 위임
    pub async fn switch_model(&self, model_id: &str) -> Result<()> {
        self.inner.switch_model(model_id).await
    }
}
```

### 선행 작업
- AgentLoop에 `switch_model()`, `try_fallback()` 메서드 이식
- `stream_with_retry` 통합 (mpsc::Sender → EmitFn 어댑터)

### 작업량: ~2일

---

## GAP 3: unwrap 전면 제거 (코드 품질 +4점)

**현재**: 1024개. 설계에서 ~50개만 수정 계획.
**목표**: 프로덕션 코드 0개. 테스트 코드만 허용.

### 3단계 제거 계획

#### Step 1: Infallible unwrap → expect() (350개, 1일)

```rust
// 자동 치환 가능
"application/json".parse().unwrap()
→ "application/json".parse().expect("valid MIME type")

HeaderName::from_static("content-type")
→ 그대로 (이미 infallible)
```

#### Step 2: Risky unwrap → Result 전파 (200개, 2일)

```rust
// env::var
std::env::var("KEY").unwrap()
→ std::env::var("KEY").map_err(|_| ConfigError::MissingEnv("KEY"))?

// 인덱스 접근
items[idx].unwrap()
→ items.get(idx).ok_or(Error::IndexOutOfBounds)?

// 파싱
value.parse().unwrap()
→ value.parse().map_err(|e| Error::Parse(e))?
```

#### Step 3: 남은 것 수동 분석 (474개, 3일)

각 케이스별로:
- `if let` / `match`으로 변경
- `unwrap_or_default()` / `unwrap_or(fallback)`
- `context("why this should exist")?`

### 검증

```rust
// 각 크레이트 lib.rs
#![deny(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]
```

→ CI에서 자동 차단. 새로운 unwrap 추가 불가.

### 작업량: ~6일 (가장 오래 걸림)

---

## GAP 4: 실증적 안정성 (프로덕션 준비도 +10점)

**이것이 82→90의 핵심. 코드가 아니라 운영 경험.**

### 4.1 스트레스 테스트 스위트

```
tests/stress/
├── long_session.rs       — 1000턴 대화 후 세션 무결성
├── concurrent_tools.rs   — 10개 도구 동시 실행
├── memory_leak.rs        — 100회 대화 후 메모리 증가 측정
├── network_failure.rs    — 중간에 네트워크 끊기 시 복구
└── rapid_model_switch.rs — 50회 모델 전환 시 상태 일관성
```

### 4.2 크로스 플랫폼 CI 행렬

```yaml
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, macos-13, windows-latest]
    rust: [stable, 1.75.0]  # MSRV
```

### 4.3 벤치마크 회귀 감지

```yaml
# CI에 추가
- name: Benchmark
  run: |
    cargo bench --workspace
    # 이전 결과와 비교하여 10% 이상 성능 저하 시 경고
```

### 4.4 cargo audit 자동화

```yaml
# 매일 실행
- cron: '0 6 * * *'
- run: cargo install cargo-audit && cargo audit
```

### 4.5 실제 배포 파이프라인 검증

- Homebrew formula 자동 생성
- release 태그 시 checksum 파일 생성
- 자동 업데이트 체크 (`reqwest` 이미 있음)

### 작업량: ~4일

---

## GAP 5: Extension 트레이트 분리 (아키텍처 +2점)

**현재**: 68개 메서드를 가진 "god trait"

```rust
// Before: 하나의 거대한 트레이트
trait Extension {
    fn on_load() {}
    fn on_unload() {}
    fn register_tools() {}
    fn register_commands() {}
    fn on_session_start() {}
    fn on_session_end() {}
    fn on_tool_call() {}
    fn on_tool_result() {}
    fn on_provider_request() {}
    fn on_text_delta() {}
    fn on_error() {}
    // ... 50+ more methods with default impl
}

// After: 관심사별 분리
trait Extension: ExtensionLifecycle + Any {}

trait ExtensionLifecycle {
    fn on_load(&self, _ctx: &ExtensionContext) {}
    fn on_unload(&self) {}
}

trait ToolProvider {
    fn register_tools(&self) -> Vec<Arc<dyn AgentTool>> { vec![] }
}

trait SessionHooks {
    fn on_session_start(&self, _session: &Session) {}
    fn on_session_end(&self, _session: &Session) {}
}

trait StreamHooks {
    fn on_text_delta(&self, _delta: &str) {}
    fn on_thinking_delta(&self, _delta: &str) {}
}

trait ErrorHooks {
    fn on_error(&self, _error: &AgentError) {}
}
```

### 작업량: ~2일

---

## 종합: 90점 도달 로드맵

```
현재 (73점)
  │
  ├── Phase A (본 설계, 1주) → 81점
  ├── Phase 1-2 (v0.6 설계, 2주) → 84점
  ├── Phase B (본 설계, 1주) → 85점
  │
  │  ── 여기까지가 기존 두 설계의 한계 (~85) ──
  │
  ├── GAP 1: oxi-cli 분해 (3일) → 86점
  ├── GAP 2: Agent 통합 (2일) → 87점
  ├── GAP 3: unwrap 전면 제거 (6일) → 88점
  ├── GAP 4: 실증 안정성 (4일) → 89점
  ├── GAP 5: Extension 분리 (2일) → 89점
  │
  └── Phase 3-5 (v0.6 설계, 3주) + 문서화 → 90-91점
```

### 총 일정

| 단계 | 기간 | 점수 |
|------|------|:----:|
| 기존 설계 (Phase A+B + v0.6 Phase 1-2) | 4주 | 85 |
| GAP 1-3 (구조 개선 + unwrap) | 2주 | 88 |
| GAP 4-5 (실증 + Extension) | 1.5주 | 89 |
| v0.6 Phase 3-5 (문서화 + 마일리지) | 3주 | 90-91 |
| **총계** | **~10주** | **90+** |

---

## 솔직한 결론

| 질문 | 답변 |
|------|------|
| 현재 설계만으로 90점? | ❌ **82점이 한계** |
| 추가 GAP 작업까지 하면? | ✅ **10주 후 90-91점 가능** |
| 가장 비용 대비 효율적인 작업? | **GAP 3 (unwrap 제거) — 코드 품질 +4, 총점 +3** |
| 가장 어려운 작업? | **GAP 4 (실증 안정성) — 코드가 아닌 운영 경험 필요** |
| 85점에서 멈추면? | 프로덕션 베타로는 충분. 정식은 아님. |

### 권장 전략

```
1단계: 기존 설계 실행 → 85점 → 베타 배포
2단계: 사용자 피드백 수집 + GAP 1-3 실행 → 88점
3단계: GAP 4-5 + 문서화 → 90점 → 정식 릴리즈
```

**베타를 먼저 내고 실제 사용 데이터로 방향을 잡는 것이 10주 동안 코드만 고치는 것보다 현명합니다.**
