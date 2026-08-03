# Rust 2024 Edition & Async Modernization

> **Status:** Complete ✅
> **Date:** 2026-06-07
> **Scope:** Workspace-wide (oxicode-ai, oxicode-agent, oxicode-sdk, oxicode-cli)

## Motivation

oxicode는 이미 `edition = "2024"`, `rust-version = "1.96"`으로 마이그레이션을 완료했다.
하지만 마이그레이션은 "컴파일 되게 만드는" 작업이었고, 에디션이 제공하는 **새 언어 기능**과
최신 러스트 생태계의 **관용 패턴**을 적극 활용하진 않았다.

이 설계는 세 가지 목표를 가진다:

1. **의존성 가지치기** — 더 이상 필요 없는 크레이트 제거
2. **런타임 오버헤드 제거** — 매크로 확장, 불필요한 Box 대체
3. **가독성·유지보수성 향상** — 2024 관용 패턴 도입

---

## 1. `#[async_trait]` → Native `async fn` in trait

### 배경

Rust 1.75 (2023-12) 부터 `async fn` in trait이 안정화되었다.
`#[async_trait]`은 trait 메서드의 리턴을 `Pin<Box<dyn Future + Send>>`로 박싱하는 매크로인데,
이제 컴파일러가 자동으로 처리한다.

### 현재 상태

| 메트릭 | 값 |
|---|---|
| 사용 파일 | 59 |
| `#[async_trait]` 발생 | 104 |
| 영향 크레이트 | oxicode-ai, oxicode-agent, oxicode-sdk, oxicode-cli |
| `async-trait` 의존 | 4개 Cargo.toml |

### 변경 계획

#### 1a. trait 정의에서 `#[async_trait]` 제거

```rust
// Before
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    async fn stream(/* ... */) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;
    fn name(&self) -> &str;
}

// After
pub trait Provider: Send + Sync + 'static {
    async fn stream(/* ... */) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;
    fn name(&self) -> &str;
}
```

trait 정의의 `async fn`은 그대로 유지된다. `#[async_trait]`만 제거.

#### 1b. `impl` 블록에서 `#[async_trait]` 제거

```rust
// Before
#[async_trait]
impl Provider for OpenAiProvider {
    async fn stream(/* ... */) -> Result<...> { /* ... */ }
    fn name(&self) -> &str { "openai" }
}

// After
impl Provider for OpenAiProvider {
    async fn stream(/* ... */) -> Result<...> { /* ... */ }
    fn name(&self) -> &str { "openai" }
}
```

#### 1c. `use async_trait::async_trait;` 제거

모든 파일에서 import 제거.

#### 1d. `Cargo.toml`에서 `async-trait` 제거

4개 크레이트 모두에서:
```toml
# 제거
async-trait = "0.1"
```

### Send 바운드 검증

`#[async_trait]`은 기본적으로 `+ Send`를 추가했다. native `async fn` in trait은
동적 디스패치 시(`dyn Trait`) 자동으로 `Send`를 요구하지 않으므로, 다음을 확인해야 한다:

- `dyn Provider` / `dyn AgentTool` 사용처가 이미 `+ Send` 바운드를 명시하는지 확인
- 현재 코드에서 `Box<dyn Provider>` → `Box<dyn Provider + Send + Sync>` 이미 명시되어 있음 ✅

### 영향받는 트레잇 목록

| 크레이트 | 트레잇 | async 메서드 수 |
|---|---|---|
| oxicode-ai | `Provider` | 1 |
| oxicode-agent | `AgentTool` | 1 |
| oxicode-sdk | `StateStore` | 4 |
| oxicode-sdk | `AuthProvider` | 5 |
| oxicode-sdk | `EventBus` | 2 |
| oxicode-sdk | `SkillLoader` | 1 |
| oxicode-sdk | `PersonaProvider` | 1 |
| oxicode-sdk | `MemoryStore` | 3 |
| oxicode-sdk | `CronScheduler` | 2 |
| oxicode-sdk | `ResourceMonitor` | 2 |
| oxicode-sdk | `AccessGate` | 1 |
| oxicode-sdk | `CapabilityResolver` | 1 |
| oxicode-ai | `ProviderResolver` (agent.rs) | 1 |
| oxicode-cli | `Extension` | 여러 |

### 효과

- **컴파일 시간:** 매크로 확장 오버헤드 제거
- **디버그 경험:** 원본 소스코드 그대로 디버깅 가능
- **의존성:** 외부 크레이트 1개 제거 × 4 = 빌드 그래프 간소화
- **에러 메시지:** 매크로 확장 관련 에러 메시지 사라짐

### 위험도: 낮음

- 기계적 치환. 동작 변경 없음.
- `cargo nextest run --workspace`로 풀 테스트 커버리지 보장.

---

## 2. `if let` let chains (Rust 2024 신기능)

### 배경

Rust 2024에서 `if let A && let B && expr` 형태의 연쇄 패턴 매칭이 가능해졌다.
이전에는 `if let` 뒤에 바로 패턴이 와야 했고, `&&`로 연결할 수 없었다.

### 적용 패턴

#### 2a. 중첩 `if let` → let chain

```rust
// Before
if let Some(config) = self.config.as_ref() {
    if let Some(key) = config.api_key.as_ref() {
        if !key.is_empty() {
            // ...
        }
    }
}

// After
if let Some(config) = self.config.as_ref()
    && let Some(key) = config.api_key.as_ref()
    && !key.is_empty()
{
    // ...
}
```

#### 2b. early-return + 중첩 `if let` → let chain

```rust
// Before
let tier_config = match read_tier_config() {
    Some(c) => c,
    None => return None,
};
if let Some(pm) = parse_tier_model(&tier_config) {
    // ...
}

// After
if let Some(tier_config) = read_tier_config()
    && let Some(pm) = parse_tier_model(&tier_config)
{
    // ...
}
```

### 식별 방법

```bash
# 중첩 if let 패턴 탐지
rg -n --type rust -U 'if let .+\{\s*\n\s*if let'
```

### 식별된 후보 (37개 이상)

| 파일 | 라인 | 패턴 |
|---|---|---|
| `scripts/generate-models.rs` | 393-394 | `if let Some(ap)` → `if let Some(sample)` |
| `oxicode-sdk/src/coordination/shared_memory.rs` | 89-90 | `if let Some(expected)` → `if let Some(entry)` |
| `oxicode-sdk/src/middleware/builtins.rs` | 201-202 | `if let Some(tracker)` → `if let Some(budget)` |
| `oxicode-tui/src/widgets/chat/state.rs` | 250-251 | `if let Some(idx)` → `if let ContentBlock::Text` |
| `oxicode-tui/src/widgets/chat/state.rs` | 334-335 | `if let Some(existing_idx)` → `if let Some(ContentBlock::ToolCall)` |
| `oxicode-tui/src/widgets/chat/state.rs` | 369-370 | `if let Some(ref mut s)` → `if let Some(ref id)` |
| `oxicode-tui/src/widgets/chat/state.rs` | 418-419 | `if let Some(ref mut s)` → `if let Some(ContentBlock::Thinking)` |
| `oxicode-ai/src/router/signals.rs` | 276-277 | `if let Message::User(u)` → `if let MessageContent::Blocks` |
| `oxicode-cli/src/store/settings.rs` | 750-751 | `if let Ok(mut settings)` → `if let Some((provider, model))` |
| `oxicode-cli/src/store/settings.rs` | 811-812 | `if let Some(model)` → `if let Some((provider, model_name))` |
| `oxicode-agent/src/proxy.rs` | 573-577 | 3중첩 `if let Some(state)` → `if let ContentState::Text` → `if let Some(block)` → `if let ContentBlock::Text` |
| `oxicode-agent/src/proxy.rs` | 590-594 | 3중첩 (Thinking) |
| `oxicode-agent/src/proxy.rs` | 607-613 | 3중첩 (ToolCall) |
| `oxicode-agent/src/proxy.rs` | 846-847 | 2중첩 (ToolCall) |
| `oxicode-cli/src/bootstrap.rs` | 51-52 | `if let Some(ref level_str)` → `if let Some(level)` |
| `oxicode-cli/src/store/session.rs` | 1121-1122 | `if let SessionEntryEnum::Label` → `if let Some(ref label)` |

### 3중첩 예시 — 가장 극적인 개선

`oxicode-agent/src/proxy.rs:573-577`:

```rust
// Before (3중첩, 들여쓰기 24칸)
if let Some(state) = self.content_states.get_mut(&content_index) {
    if let ContentState::Text { text } = state {
        text.push_str(&new_text);
        if let Some(block) = self.partial.content.get_mut(content_index) {
            if let ContentBlock::Text(t) = block {
                t.content = text.clone();
            }
        }
    }
}

// After (let chain)
if let Some(state) = self.content_states.get_mut(&content_index)
    && let ContentState::Text { text } = state
{
    text.push_str(&new_text);
    if let Some(block) = self.partial.content.get_mut(content_index)
        && let ContentBlock::Text(t) = block
    {
        t.content = text.clone();
    }
}
```

### 위험도: 매우 낮음

- 순수 가독성 개선. 동작 변경 없음.
- 자동화하기 어려우므로 수동 적용.

---

## 3. RPIT `use<..>` 정리

### 배경

Rust 2024에서 `-> impl Trait` 리턴 타입이 모든 in-scope lifetime을 자동 capture한다.
이전에 lifetime을 capture하기 위해 사용하던 `Captures<>` 트릭이나 outlives 트릭이 불필요해졌다.

### 현재 상태

코드에 `Captures<>` 트릭이나 명시적 `use<..>` 사용이 없음. 기존 코드가 이미 깔끔함. ✅

### 향후 권장사항

새 코드 작성 시:
- `-> impl Trait + 'a` 같은 outlives 트릭 대신, lifetime이 필요하면 그냥 두면 됨 (자동 capture)
- 정말 capture를 피해야 할 때만 `use<>` 명시

---

## 4. `if let` temporary scope 변경 활용

### 배경

Rust 2024에서 `if let`의 임시값(scrutinee temporary)이 `else` 블록 진입 전에 drop된다.
이는 lock guard + `if let` 패턴에서 데드락을 예방한다.

### 현재 코드의 이점

```rust
// oxicode-agent/src/tools/browse/engine.rs
if let Some(entry) = self.entries.lock().get(tab_id) {
    // lock guard는 여기서 유지
} else {
    // Rust 2024에서는 lock guard가 이미 drop됨 → 안전
}
```

이 패턴은 이미 올바르게 동작하지만, 2024 이전에는 guard가 `else` 블록 내내 살아있어서
`else`에서 다시 lock을 잡으면 데드락이 발생할 수 있었다. 이제 그 위험이 원천 제거됨.

### 조치: 없음 (이미 안전)

이미 올바르게 작성되어 있음. 향후 lock + `if let` 패턴 사용 시 이점을 인지하고 활용.

---

## 5. `once_cell::sync::Lazy` → `std::sync::LazyLock`

### 배경

`std::sync::LazyLock`이 Rust 1.80에서 안정화되었다.
`once_cell::sync::Lazy`와 완전히 동일한 기능을 표준 라이브러리에서 제공한다.

### 현재 상태

| 위치 | 사용 | 대체 |
|---|---|---|
| `oxicode-ai/src/model_registry.rs:49` | `static STATIC_MODELS: Lazy<HashMap<...>>` | `LazyLock` |
| `oxicode-ai/src/model_registry.rs:855` | `static GLOBAL_REGISTRY: Lazy<ModelRegistry>` | `LazyLock` |
| `oxicode-ai/src/env_api_keys.rs:23` | `static VERTEX_ADC_CHECK: Lazy<bool>` | `LazyLock` |
| `oxicode-ai/src/providers/mod.rs:211` | `static CUSTOM_PROVIDERS: Lazy<RwLock<...>>` | `LazyLock` |

`oxicode-cli`는 이미 `std::sync::LazyLock`을 사용 중 (wasm.rs, changelog.rs, packages.rs). ✅

### 변경 계획

```rust
// Before
use once_cell::sync::Lazy;
static STATIC_MODELS: Lazy<HashMap<String, Model>> = Lazy::new(|| { ... });

// After
use std::sync::LazyLock;
static STATIC_MODELS: LazyLock<HashMap<String, Model>> = LazyLock::new(|| { ... });
```

`oxicode-ai/Cargo.toml`에서 `once_cell = "1"` 제거.

> `oxicode-cli/Cargo.toml`에도 `once_cell = "1"`과 `lazy_static = "1.4"`가 선언되어 있으나,
> 코드에서 직접 사용하지 않음 (다른 의존 크레이트의 전이 의존). 제거해도 무방.

### 위험도: 매우 낮음

- `LazyLock`은 `once_cell::sync::Lazy`와 동일한 API (`new()`, `force()`, deref).
- 1:1 치환.

---

## 6. Tokio 생태계 최적화

### 6a. `features = ["full"]` → 필요 feature만 선택

현재 4개 크레이트 모두 `tokio = { version = "1", features = ["full"] }` 사용.

`full`은 16개 feature를 포함하며, 그 중 사용하지 않는 것들이 있다:

| Feature | oxicode-ai | oxicode-agent | oxicode-sdk | oxicode-cli |
|---|:---:|:---:|:---:|:---:|
| `rt-multi-thread` | ✅ | ✅ | ✅ | ✅ |
| `macros` | ✅ (`#[tokio::test]`) | ✅ | ✅ | ✅ |
| `sync` (channel, Mutex) | ✅ | ✅ | ✅ | ✅ |
| `time` (sleep, timeout) | ✅ | ✅ | ✅ | ✅ |
| `io-util` (AsyncWriteExt 등) | — | ✅ | ✅ | — |
| `io-std` | — | — | — | — |
| `fs` | — | ✅ | ✅ | ✅ |
| `process` (Command) | — | ✅ | — | — |
| `signal` | — | — | — | ✅ |
| `net` (TcpListener 등) | — | — | — | — ❌ |
| `parking_lot` (내부) | — | — | — | — |

**판단:** `full`을 유지하되, 빌드 시간 최적화가 필요한 시점에 세분화.
현재는 `full`이 주는 편의성이 더 큼. `net`, `io-std`, `parking_lot` 등 미사용 feature의
컴파일 오버헤드가 미미함 (tokio는 이미 워크스페이스에서 1번만 컴파일됨).

**결론:** **보류.** `full` 유지. 나중에 CI 빌드 시간 병목이 tokio에서 발생하면 그때 세분화.

### 6b. `std::sync::Mutex` → `parking_lot::Mutex` 통일

현재 두 가지가 혼재:

| 위치 | 타입 | 컨텍스트 |
|---|---|---|
| oxicode-sdk 대부분 | `parking_lot::Mutex` / `RwLock` | 비동기 컨텍스트에서 lock을 짧게 잡을 때 |
| oxicode-cli | `std::sync::Mutex` | UI 콜백, 테스트 |
| oxicode-agent 테스트 | `std::sync::Mutex` | 테스트에서 Arc<Mutex<Vec<_>>> |
| oxicode-agent | `tokio::sync::Mutex` | `.await` 경계에서 lock 유지 필요 시 |

AGENTS.md에 이미 다음이 명시되어 있음:

> Use `parking_lot::RwLock` instead of `std::sync::RwLock`. (But `parking_lot::MutexGuard` is `!Send` — drop the guard before any `.await` or use `tokio::sync::Mutex` instead.)

**판단:** 혼재가 원칙에 맞게 사용되고 있음:
- `std::sync::Mutex`: 테스트 코드, `!Send`가 문제되지 않는 단일 스레드 컨텍스트
- `parking_lot::Mutex/RwLock`: 프로덕션 코드의 동기 lock
- `tokio::sync::Mutex`: `.await`를 가로지르는 lock

**결론:** **현상 유지.** 원칙이 이미 명확함.

### 6c. `tokio-test` 제거 ✅ 확정

`oxicode-ai`와 `oxicode-agent`에 `tokio-test = "0.4"`가 선언되어 있으나, 코드 전체에서 **단 한 곳도 사용하지 않음**:

```bash
$ rg --type rust 'tokio_test' -l
# (결과 없음)
```

**조치:** `oxicode-ai/Cargo.toml`과 `oxicode-agent/Cargo.toml`에서 `tokio-test = "0.4"` 제거.

### 6d. `tokio::select!` 패턴 개선

현재 `tokio::select!` 사용이 10곳 이상. 대부분 올바르게 작성되어 있으나,
Rust 2024의 `if let` temporary scope 변경과 결합하여 lock을 `select!` 안에서
안전하게 사용할 수 있게 됨.

**결론:** **현상 유지.** 개선 포인트가 발견되면 그때 적용.

---

## 7. 실행 계획

### Phase 1: `#[async_trait]` 제거 (P0)

**예상 소요:** 2-3시간 (기계적 치환 + 테스트)

1. `oxicode-ai` 부터 시작 (가장 독립적인 크레이트)
   - `trait_def.rs`: trait 정의에서 `#[async_trait]` 제거
   - 각 provider 구현체에서 `#[async_trait]` 제거
   - `multi_provider.rs`, `compaction.rs`, `provider_pool.rs` 등
   - `use async_trait::async_trait;` 전체 제거
   - `Cargo.toml`에서 `async-trait` 제거
   - `cargo clippy -p oxicode-ai -- -D warnings`
   - `cargo nextest run -p oxicode-ai`

2. `oxicode-agent`
   - `tools.rs`: `AgentTool` trait에서 `#[async_trait]` 제거
   - 17개 tool 구현체에서 제거
   - `mcp/`, `agent_loop/`, `proxy.rs` 등
   - 동일하게 clippy + test

3. `oxicode-sdk`
   - 11개 port trait에서 제거
   - noop 구현체, fs/ 구현체, inmem/ 구현체
   - `closure_tool.rs`, `kernel_bridge.rs`, `lifecycle/` 등

4. `oxicode-cli`
   - `agent_session.rs`, `extensions/wasm_tool.rs` 등
   - 의존하는 모든 upstream 크레이트가 완료된 후 작업

5. 최종 검증
   - `cargo clippy --workspace -- -D warnings`
   - `cargo nextest run --workspace`
   - `cargo fmt --all -- --check`

### Phase 2: let chains 적용 (P1)

**예상 소요:** 1-2시간 (수동 패턴 매칭)

PR 리뷰 또는 별도 브랜치에서 중첩 `if let` 패턴을 let chain으로 변환.
전체 검색으로 후보 식별 후 일괄 적용.

### Phase 3: `once_cell` → `std::sync::LazyLock` + 의존성 정리 (P2)

**예상 소요:** 30분

1. `oxicode-ai`: `once_cell::sync::Lazy` → `std::sync::LazyLock` (4곳)
2. `oxicode-ai/Cargo.toml`에서 `once_cell` 제거
3. `oxicode-cli/Cargo.toml`에서 `once_cell`, `lazy_static` 제거 (직접 미사용)
4. `oxicode-ai/Cargo.toml`, `oxicode-agent/Cargo.toml`에서 `tokio-test` 제거
5. `cargo clippy --workspace -- -D warnings` + `cargo nextest run --workspace`

---

## 위험도 평가

| 항목 | 위험도 | 이유 |
|---|---|---|
| `#[async_trait]` 제거 | 낮음 | 기계적 치환. `dyn Trait + Send` 이미 명시됨 |
| let chains | 매우 낮음 | 순수 가독성 개선. 동작 변경 없음 |
| `once_cell` → `LazyLock` | 매우 낮음 | 동일 API의 1:1 치환 |
| tokio feature 세분화 | 중간 | feature 누락으로 컴파일 에러 발생 가능 |
| tokio-test / once_cell / lazy_static 제거 | 낮음 | 사용처 없음 또는 교체 완료 |

---

## 성공 기준

- [x] `async-trait` 크레이트 의존 0개 (직접 의존)
- [x] `cargo clippy --workspace -- -D warnings` 통과
- [x] `cargo nextest run --workspace` 전체 통과 (2116/2116)
- [x] 중첩 `if let` 중 let chain으로 변환 가능한 곳 적용 (16곳 변환)
- [x] `cargo tree -p async-trait` → 직접 의존 없음 (extism/oxibrowser 전이 의존만)
- [x] `cargo tree -p once_cell` → 직접 의존 없음 (ahash 전이 의존만)
- [x] `cargo tree -p lazy_static` → 직접 의존 없음 (전이만)
- [x] `cargo tree -p tokio-test` → not found
- [x] `cargo fmt --all -- --check` 통과
