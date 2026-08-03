# Master Design: native-browser 부활 및 트라이앵글 정렬

> **Status:** Proposal
> **Scope:** `oxicode` (oxicode-agent, oxicode-sdk) · `oxibrowser` · `oxios`
> **Date:** 2026-06-15
> **Trigger:** oxios의 workspace-wide edition 2024 전환이 `--all-features`에서
> `oxicode-agent`의 27개 컴파일 에러를 드러냄.

---

## 0. TL;DR

`native-browser` feature가 **oxicode CI에서 단 한 번도 컴파일된 적 없어서**
edition 2024 수명 규칙에 맞지 않는 코드가 crates.io 0.32.0까지 배포되었다.
이 설계는 세 가지를 동시에 해결한다:

1. **버그 수정** — `BrowserTab`/`BrowserEngine`을 `#[async_trait]`로 전환
   (이미 의존성이고 sibling 4개가 쓰는 중)
2. **버전 정렬** — `oxibrowser-core` 0.14.1 → 0.15, oxicode 0.34.0 배포
3. **CI 경화** — `native-browser`를 매 빌드마다 컴파일하여 재부패 영구 차단

---

## 1. 현황 진단 (As-Is)

### 1.1 의존성 삼각형

```
oxios (0.32.0 사용)
  └─ oxicode-sdk 0.32.0 (crates.io)
       └─ oxicode-agent 0.32.0 (crates.io)
            └─ [native-browser] oxibrowser_backend.rs ← 27 errors

oxicode (local, 0.34.0 — 미배포)
  ├─ oxicode-sdk 0.34.0    → oxibrowser-core "0.14.1" ← STALE
  └─ oxicode-agent 0.34.0  → oxibrowser-core "0.15"   ← current

oxibrowser (0.15.0 배포중) — 수정 불필요
```

**문제 3종:**

| # | 문제 | 위치 |
|---|------|------|
| A | native-browser 코드가 edition 2024에서 컴파일 안 됨 | `oxicode-agent/.../oxibrowser_backend.rs` |
| B | oxicode-sdk의 oxibrowser-core가 0.14.1 (구버전) | `oxicode-sdk/Cargo.toml:22` |
| C | oxios가 oxicode 0.32.0에 고정 (로컬은 0.34.0) | `oxios/Cargo.toml` |

### 1.2 부패 메커니즘 — 왜 아무도 몰랐나

```
oxicode/.github/workflows/ci.yml
  → "native-browser" feature를 컴파일하는 스텝이 없음
  → oxicode-agent --features native-browser가 2년간 미검증
  → edition 2024 전환 시에도 default features만 검사
  → 부서진 코드가 그대로 0.32.0, 0.33.0, 0.34.0으로 버전업
```

**oxios 쪽에서만 `--all-features`를 돌리다 처음 발견.** 근본 원인은
oxicode의 CI 커버리지 구멍이다.

### 1.3 버그 상세 — edition 2024 async lifetime

**트레이트 정의** (`engine.rs`) — 30개 메서드 전부 동일 패턴:
```rust
pub trait BrowserTab: Send + Sync {
    fn goto<'a>(
        &'a self,
        url: &str,                    // ← 익명 수명 '1
    ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>>
              + Send + 'a>>;          // ← future는 'a(self)만 보장
}
```

**구현** (`oxibrowser_backend.rs`):
```rust
fn goto<'a>(&'a self, url: &str)
    -> Pin<Box<dyn Future<...> + Send + 'a>>
{
    Box::pin(async move {
        self.inner.goto(url).await    // ← async 블록이 url(&'1)을 캡처
    })                                //   하지만 future는 +'a만 — 모순!
}
```

**Edition 2024의 판정:** async 블록은 캡처하는 모든 참조의 수명을
future에 반영해야 한다. `url: '1`이 future의 `+'a`보다 짧을 수 있으므로
하드 에러. (2021에서는 캡처 추론이 관대해 — 사실 unsound하지만 — 통과.)

**추가 버그 2종 (같은 파일):**
- E0261 × 2: `tab_id(&'a self)`, `evaluate_await` 리턴타입에 선언되지 않은 `'a`
- E0271 × 1: `new_tab`에서 `Box<OxicodeTab>` → `Box<dyn BrowserTab>` coercion 실패

### 1.4 치명적 아이러니 — 이미 해결책이 프로젝트 안에 있다

```toml
# oxicode-agent/Cargo.toml:33
async-trait = "0.1"     # ← 이미 의존성!
```
```rust
// 같은 browse/ 모듈의 sibling 트레이트 4개가 이미 #[async_trait] 사용:
//   browse_tool.rs:53          impl AgentTool for BrowseTool
//   browse_session_tool.rs:102 impl AgentTool for BrowseSessionTool
//   browse_extract_tool.rs:52  impl AgentTool for BrowseExtractTool
//   browse_script_tool.rs:453  impl AgentTool for BrowseScriptTool
```

`BrowserTab`/`BrowserEngine`만 **유일하게** 수동 패턴을 고집하고 있다.
일관성 측면에서도 전환이 정답.

---

## 2. 설계 (To-Be)

### 2.1 해결책 선정 — `#[async_trait]` 전환

**후보 3종 비교:**

| 후보 | 방식 | 장점 | 단점 |
|------|------|------|------|
| **A. `#[async_trait]`** ✅ | 매크로로 `async fn` → 자동 박싱 | 이미 dep, sibling 4개 사용 중, 최소 변경, object-safe | 런타임 1회 alloc/호출 (무시 가능) |
| B. 수명 직접 수정 | `url: &str` → `url: &'a str` | 새 dep 없음 | 트레이트 API 변경, 30곳 전부 수동, MockTab도 수정, 여전히 장황 |
| C. native `async fn in trait` + `trait_variant` | 1.75+ 안정 | 가장 모던, 제로 alloc | 새 dep(`trait_variant`), `dyn` 호환 복잡, 큰 리팩터 |

**선택: A.** 이유:
1. **이미 프로젝트에 있다** — 새 의존성/패턴 도입 비용 제로
2. **일관성** — sibling 트레이트와 동일 패턴으로 통일
3. **최소 mechanical change** — 30개 시그니처를 `async fn`으로 단순화
4. **Object-safe 보장** — `dyn BrowserTab`/`dyn BrowserEngine` 그대로 동작
  (`Arc<dyn BrowserEngine>`, `Box<dyn BrowserTab>` 소비처 변경 없음)

### 2.2 변환 후 모습

**Before (engine.rs):**
```rust
pub trait BrowserTab: Send + Sync {
    fn goto<'a>(
        &'a self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>>;

    fn hover<'a>(
        &'a self,
        selector: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(r#"... dispatchEvent ..."#);
        Box::pin(async move { self.evaluate(&js).await.map(|_| ()) })
    }
    // ... 28 more methods, each ~6 lines of boilerplate
}
```

**After:**
```rust
#[async_trait]
pub trait BrowserTab: Send + Sync {
    async fn goto(&self, url: &str) -> Result<PageContent, BrowserError>;
    async fn click(&self, selector: &str) -> Result<(), BrowserError>;
    async fn type_(&self, selector: &str, text: &str) -> Result<(), BrowserError>;
    // ... required methods

    // Default methods — clean async:
    async fn hover(&self, selector: &str) -> Result<(), BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(r#"... dispatchEvent ..."#);
        self.evaluate(&js).await.map(|_| ())
    }
    async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), BrowserError> {
        let js = format!("window.scrollBy({}, {})", delta_x, delta_y);
        self.evaluate(&js).await.map(|_| ())
    }
    // ... defaults
}
```

**Before (oxibrowser_backend.rs impl):**
```rust
impl BrowserTabTrait for OxicodeTab {
    fn goto<'a>(&'a self, url: &str)
        -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>>
    {
        Box::pin(async move {
            let page = self.inner.goto(url).await
                .map_err(|e| BrowserError::Navigation(e.to_string()))?;
            Ok(browse_result_to_page_content(page))
        })
    }
}
```

**After:**
```rust
#[async_trait]
impl BrowserTabTrait for OxicodeTab {
    async fn goto(&self, url: &str) -> Result<PageContent, BrowserError> {
        let page = self.inner.goto(url).await
            .map_err(|e| BrowserError::Navigation(e.to_string()))?;
        Ok(browse_result_to_page_content(page))
    }
}
```

**변경량 추정:**
- `engine.rs`: -~200 라인 (보일러플레이트 제거)
- `oxibrowser_backend.rs`: -~150 라인 + 27개 버그 자동 해결
- `tab_guard.rs` (MockTab): -~100 라인
- `browse_tool.rs` (MockEngine): -~30 라인
- **순 효과:** 약 480 라인 감소 + 버그 0 + 일관성 확보

### 2.3 버전 정렬

```
① oxibrowser-core 버전 통일
   oxicode-sdk/Cargo.toml:    "0.14.1" → "0.15"   (oxicode-agent과 일치)

② oxicode 0.35.0 배포 (bug fix = PATCH/MINOR)
   oxicode-agent 0.35.0  — async_trait 전환 + native-browser 수정
   oxicode-sdk   0.35.0  — oxibrowser-core 정렬 + oxicode-agent 0.35.0

③ oxios 업그레이드
   Cargo.toml: oxicode-sdk "0.32.0" → "0.35.0"
   검증: cargo build --workspace --all-features (드디어 통과)
```

> **버전 정책:** 트레이트 시그니처가 `Pin<Box<...>>` → `async fn`으로
> 바뀌는 것은 API breaking change이지만, `BrowserTab`/`BrowserEngine`의
> 외부 impl은 존재하지 않으므로 (모두 oxicode-agent 내부) downstream에는
> 실질적 영향이 없다. `#[async_trait]`가 내부적으로 동일한 `Pin<Box<...>>`
> 디슈가를 생성하므로 런타임 동작도 동일. 따라서 **MINOR** 업으로 충분
> (0.35.0). 단, 확신이 없으면 MAJOR(1.0.0)도 고려 — 하지만 과잉.

---

## 3. CI 경화 (재부팅 영구 차단)

### 3.1 oxicode CI — native-browser 검증 스텝 추가

```yaml
# .github/workflows/ci.yml (oxicode)
- name: Clippy (native-browser)
  run: cargo clippy -p oxicode-agent --features native-browser -- -D warnings

- name: Test (native-browser)
  run: cargo test -p oxicode-agent --features native-browser
```

> `oxibrowser_backend.rs`는 실제 헤드리스 브라우저를 띄우지 않으므로
> 컴파일 + 타입체크만으로도 버그를 잡는다. 런타임 테스트는 별도.

### 3.2 oxios CI — `--all-features` 금지 + 개별 feature 검증

이미 AGENTS.md에 명시됨. CI는 per-crate feature를 사용 (현행 유지).

---

## 4. 실행 계획 (Task Breakdown)

### Phase 1 — oxicode-agent 수정 (핵심, ~2h)

| Task | 파일 | 내용 |
|------|------|------|
| 1.1 | `engine.rs` | `#[async_trait]` 추가, 30개 메서드 → `async fn` |
| 1.2 | `oxibrowser_backend.rs` | impl 27개 → `async fn`, `BrowserTab` alias 정리 |
| 1.3 | `tab_guard.rs` | MockTab → `async fn` |
| 1.4 | `browse_tool.rs` | MockEngine → `async fn` |
| 1.5 | `helpers.rs` | `&dyn BrowserTab` 소비처 정리 (`extract_links`) |
| 1.6 | 검증 | `cargo build -p oxicode-agent --features native-browser` |

### Phase 2 — 버전 정렬 (~30min)

| Task | 파일 | 내용 |
|------|------|------|
| 2.1 | `oxicode-sdk/Cargo.toml` | `oxibrowser-core "0.14.1"` → `"0.15"` |
| 2.2 | `oxicode-sdk/Cargo.toml` | `oxicode-agent` path dep → 버전 일치 확인 |
| 2.3 | 검증 | `cargo build --workspace --features native-browser` (oxicode 전체) |

### Phase 3 — CI 경화 (~20min)

| Task | 파일 | 내용 |
|------|------|------|
| 3.1 | `oxicode/.github/workflows/ci.yml` | `--features native-browser` 스텝 추가 |
| 3.2 | `oxicode/AGENTS.md` | native-browser CI 의무화 명시 |

### Phase 4 — 배포 (~30min)

| Task | 내용 |
|------|------|
| 4.1 | oxicode 0.35.0 버전업 (`oxicode-agent`, `oxicode-sdk`) |
| 4.2 | `cargo test --workspace` (oxicode) 전체 통과 확인 |
| 4.3 | `cargo publish -p oxicode-agent` → `cargo publish -p oxicode-sdk` (의존순) |
| 4.4 | CHANGELOG 업데이트 |

### Phase 5 — oxios 업그레이드 (~20min)

| Task | 파일 | 내용 |
|------|------|------|
| 5.1 | `oxios/Cargo.toml` | `oxicode-sdk "0.32.0"` → `"0.35.0"` |
| 5.2 | `oxios/Cargo.lock` | 갱신 |
| 5.3 | 검증 | `cargo build --workspace --all-features` (oxios) — 드디어 clean |
| 5.4 | 검증 | oxios CI 게이트 (fmt + clippy + test) 전부 통과 |

---

## 5. 리스크 & 회피

| 리스크 | 확률 | 회피 |
|--------|------|------|
| `#[async_trait]` 전환 시 object-safety 손실 | 매우 낮 | async_trait은 원래 object-safe하도록 디슈가 |
| default 메서드(`hover` 등)의 JS 빌드 로직 오동작 | 낮음 | 로직 변경 없음, 시그니처만 단순화 |
| oxibrowser-core 0.14→0.15 API 변경 | 중간 | oxicode-agent은 이미 0.15 사용 중이므로 호환 확인됨 |
| oxios 0.32→0.35 사이 breaking change | 중간 | Phase 5에서 빌드/테스트로 검증 |

---

## 6. 검증 체크리스트 (Definition of Done)

- [ ] `cargo build -p oxicode-agent --features native-browser` (oxicode) — 0 errors
- [ ] `cargo clippy -p oxicode-agent --features native-browser -- -D warnings` (oxicode)
- [ ] `cargo test -p oxicode-agent --features native-browser` (oxicode) — 전부 통과
- [ ] `cargo build --workspace` (oxicode) — 0 warnings
- [ ] oxicode CI에 native-browser 스텝 추가됨
- [ ] oxicode-agent 0.35.0 + oxicode-sdk 0.35.0 crates.io 배포
- [ ] oxios `cargo build --workspace --all-features` — 0 errors (최초!)
- [ ] oxios CI (fmt + clippy + test) — 전부 통과

---

## 7. 열린 질문 — 해결 내역

1. **oxibrowser-core re-export 정책** ✅ **해결**
   oxicode-sdk은 `pub use oxibrowser_core::BrowserEvent`만 re-export한다.
   0.14.1 → 0.15 전환 후 `BrowserEvent` variant 구조는 동일
   (`NavigationStarted`, `WaitingForSelector`, `DocumentReady`,
   `ScreenshotCaptured` — `tab_id` 필드 포함). oxibrowser_backend.rs의
   `extract_event_tab_id` / `browse_progress_from_event` 매칭 로직은
   변경 없이 통과 (네이티브 브라우저 테스트 5개 green 확인).

2. **0.35.0 vs 1.0.0** ✅ **MINOR (0.35.0) 선택 — 정당화됨**
   트레이트 시그니처 변형은 기술적 breaking change이지만, 외부 impl이
   없으므로 downstream 영향이 없다. 5개 크레이트 전부 crates.io에
   0.35.0으로 게시 완료. downstream(oxios)에서 컴파일 + 테스트 통과 확인.

3. **oxios의 `native-browser` feature 활성화** ⚠️ **미해결 (후속 작업)**
   oxicode CI에 `clippy-native-browser` job이 추가되어 oxicode 측은 영구 차단됨.
   하지만 oxios의 자체 `oxios-kernel/native-browser` feature는
   oxios CI에서 별도로 검증되지 않는다. 후속으로 oxios CI에
   `cargo build -p oxios-kernel --features native-browser` 검증 스텝 추가 권장.
