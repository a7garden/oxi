# oxicode 리팩터링 설계 문서

> **버전**: 0.23.0  
> **작성일**: 2025-05-30  
> **상태**: 설계 완료, 구현 대기

---

## 1. 개요

v0.23.0 기준 빌드/테스트/린트는 모두 통과하지만, 코드베이스에 아키텍처 레거시와 미구현 스텁이 존재한다.
본 문서는 두 가지 카테고리로 나누어 정리한다:

| 카테고리 | 수량 | 리스크 |
|----------|------|--------|
| 아키텍처 레거시 (리팩터링) | 2건 | 중복·복잡도 증가 |
| 미래 미구현 (설계·구현 필요) | 1건 | 기능 누락 |

---

## 2. 아키텍처 레거시 리팩터링

### 2.1 `build_system_prompt` 중복 제거

#### 현황

같은 목적의 함수가 두 파일에 존재한다. 둘 다 `prompt::system_prompt::build_system_prompt`에 위임하되,
서로 다른 옵션(`skills` vs `tool_snippets`)을 전달한다.

| 위치 | 시그니처 | 전달 옵션 | 호출처 |
|------|----------|-----------|--------|
| `oxicode-cli/src/lib.rs:221` | `fn build_system_prompt(thinking_level, skill_contents: &[String])` | `custom_prompt`, `skills`, `cwd` | `App` (TUI 모드) |
| `oxicode-cli/src/app/agent_session_runtime.rs:747` | `fn build_system_prompt(thinking_level: ThinkingLevel)` | `custom_prompt`, `selected_tools`, `tool_snippets`, `cwd` | `AgentSession` (에이전트 루프) |

#### 문제점

1. **중복 코드** — `thinking_level` → `custom_prompt` 매핑 로직이 두 파일에 완전히 동일하게 복제됨 (~30줄 × 2)
2. **DRY 위반** — 새 thinking level 추가 시 두 곳 모두 수정 필요
3. **의존성 혼란** — 어느 빌더가 어떤 옵션을 넘기는지 파악 어려움

#### 설계: 통합 `build_system_prompt`

`oxicode-cli/src/prompt/system_prompt.rs`에 이미 `BuildSystemPromptOptions`와 `build_system_prompt()`가 존재한다.
중간 래퍼 함수들을 제거하고, 호출처에서 직접 `BuildSystemPromptOptions`를 빌드하도록 변경한다.

```
┌──────────────────────────────────────────────────┐
│ prompt::system_prompt::build_system_prompt(opts) │  ← 단일 소스 of truth
└───────────┬──────────────────────┬───────────────┘
            │                      │
    ┌───────▼───────┐      ┌──────▼────────┐
    │ App (lib.rs)  │      │ AgentSession  │
    │ skills 전달    │      │ tools 전달     │
    └───────────────┘      └───────────────┘
```

#### 구현 계획

1. **`prompt/system_prompt.rs`**에 `thinking_level_to_custom_prompt()` 유틸리티 추가:

   ```rust
   /// Convert a ThinkingLevel to its default custom prompt string.
   pub fn thinking_level_prompt(level: ThinkingLevel) -> Option<String> {
       match level {
           ThinkingLevel::Off => Some("You are a helpful AI assistant...".into()),
           ThinkingLevel::Minimal => Some("...".into()),
           // ...
       }
   }
   ```

2. **`lib.rs`**의 `build_system_prompt` 삭제 → 호출처를 다음으로 교체:

   ```rust
   let options = BuildSystemPromptOptions {
       custom_prompt: thinking_level_prompt(settings.thinking_level),
       skills: loaded_skills,
       cwd: std::env::current_dir()...,
       ..Default::default()
   };
   let prompt = prompt::system_prompt::build_system_prompt(&options);
   ```

3. **`agent_session_runtime.rs`**의 `build_system_prompt` 삭제 → 동일 패턴 적용

4. 테스트: 기존 `build_system_prompt` 테스트를 `prompt/system_prompt.rs`로 이동

#### 영향 범위

- `oxicode-cli/src/lib.rs` (~50줄 삭제)
- `oxicode-cli/src/app/agent_session_runtime.rs` (~60줄 삭제)
- `oxicode-cli/src/prompt/system_prompt.rs` (~15줄 추가)
- 기능 변경 없음 — 동일 입력에 동일 출력

#### 위험도: **낮음**

두 함수가 이미 같은 최종 함수에 위임하므로, 래퍼 제거는 기계적 리팩터링이다.

---

### 2.2 `resource_loader` + `resource_loader_compat` 합병

#### 현황

리소스 로딩이 두 파일에 분산되어 있다:

| 파일 | 크기 | 역할 |
|------|------|------|
| `resource_loader.rs` | 1,853줄 | 공개 API, 컨텍스트 파일, 중복 제거, compat 래핑 |
| `resource_loader_compat.rs` | 512줄 | 타입 정의, 로우레벨 `_impl` 함수, 중복 제거(복제) |

호출 관계:

```
외부 호출자
    │
    ▼
resource_loader.rs  ←─ 공개 함수들 (load_skills_from_dir, load_theme, ...)
    │
    │  super::resource_loader_compat::*_impl()
    ▼
resource_loader_compat.rs  ←─ 실제 파일 I/O 구현
```

#### 문제점

1. **불필요한 간접층** — `load_skills_from_dir()`이 `load_skills_from_dir_impl()`만 호출
2. **중복 제거 로직 2배** — `dedupe_skills/themes/prompts`가 두 파일에 각각 존재
3. **`#[allow(dead_code)]` 14개** — compat 분리로 인한 잔여
4. **이름 혼란** — "compat"이지만 실제 호환성 문제가 아닌 과거 리팩터링 잔여

#### 설계: 단일 파일로 합병

```
resource_loader.rs (통합)
├── 타입 정의 (ResourceType, Resource, Skill, Theme, Prompt, ...)
├── 로우레벨 I/O (기존 *_impl → 직접 구현)
├── 공개 API (load_skills_from_dir, load_theme, ...)
├── 중복 제거 (dedupe_* — 1세트만)
├── 컨텍스트 파일 (ContextFile)
└── 테스트
```

#### 구현 계획

**Phase 1: `_impl` 함수 인라인** (기계적)

1. `resource_loader_compat.rs`의 각 `*_impl` 함수를 `resource_loader.rs`로 이동
2. `*_impl` 접미사 제거 (예: `load_skills_from_dir_impl` → `load_skills_from_dir_inner`)
3. 기존 래퍼 함수에서 직접 호출하도록 변경
4. `resource_loader_compat.rs`에서 이동된 함수 삭제

**Phase 2: 중복 제거 로직 통합**

1. compat의 `dedupe_*` 제거 — resource_loader.rs의 것만 유지
2. 두 파일간 차이점 분석 후 하나로 통합

**Phase 3: 타입 이동**

1. `ResourceType`, `LoadResult`, `LoadError` 등을 `resource_loader.rs` 상단으로 이동
2. `resource_loader_compat.rs`의 `pub use` 제거
3. `mod.rs`에서 `resource_loader_compat` 모듈 등록 삭제

**Phase 4: 파일 삭제**

1. `resource_loader_compat.rs` 삭제
2. 모든 `#[allow(dead_code)]` 재검토

#### 영향 범위

- `oxicode-cli/src/storage/resource_loader.rs` — 512줄 흡수
- `oxicode-cli/src/storage/resource_loader_compat.rs` — 삭제 (512줄)
- 외부 크레이트 영향 없음 — 모두 `pub(crate)`

#### 위험도: **중간**

- 파일 I/O 로직이므로 각 Phase마다 테스트 스위트 확인 필수
- 리소스 로딩(skills, themes, extensions, prompts)이 모두 영향받음
- Phase별로 커밋 분리 권장

---

## 3. 미래 미구현: LLM 라우터 분류기

### 3.1 현황

| 항목 | 내용 |
|------|------|
| 파일 | `oxicode-ai/src/router/classifier.rs` (22줄) |
| 타입 | `LlmClassifier` |
| 상태 | 스텁 — `classify()` 호출 시 항상 `bail!()` |
| 설정 연결 | `RouterConfig.classifier_model` — 설정 파일에서 읽기 가능 |

현재 라우터는 규칙 기반으로만 동작한다:

```
사용자 입력
    │
    ▼
RouterConfig (규칙: high/medium/low)
    │
    ├── 기본 규칙: 컨텍스트 길이 → tier 매핑
    │
    └── [미구현] LlmClassifier: 입력 복잡도 → tier 매핑
```

관련 타입 (이미 구현됨):

- `oxicode-ai/src/router/types.rs` — `ClassifierType::LlmClassifier`, `RoutingDecision.classifier_confidence`
- `oxicode-store/src/router_config.rs` — `classifier_model` 필드, TOML 파싱
- `oxicode-cli/src/main.rs` — 설정에서 `classifier_model` 읽기

### 3.2 목표

사용자 메시지의 복잡도를 LLM으로 분류하여, 적절한 라우팅 티어(high/medium/low)를 결정한다.

```
┌─────────────────────────────────────────────┐
│           LlmClassifier                     │
│                                             │
│  입력: 사용자 메시지 + 컨텍스트 메타데이터      │
│  출력: 복잡도 스코어 (0.0 ~ 1.0)              │
│                                             │
│  분류 기준:                                  │
│  - 코드 생성/수정 여부                        │
│  - 다중 파일 참조 여부                        │
│  - 추론 깊이 요구도                           │
│  - 단순 질문 vs 복잡 작업                     │
└─────────────────────────────────────────────┘
```

### 3.3 아키텍처 설계

#### 분류 파이프라인

```
사용자 메시지
    │
    ▼
┌──────────────────┐    ┌─────────────────────┐
│ 1. 휴리스틱 필터   │───▶│ 2. LLM 분류 (선택)   │
│    (규칙 기반)     │    │    (classifier_model)│
└──────────────────┘    └──────────┬──────────┘
                                   │
                    ┌──────────────▼──────────────┐
                    │ 3. 라우팅 결정                │
                    │    score → tier 매핑         │
                    │    high:  ≥ 0.7             │
                    │    medium: 0.3 ~ 0.7        │
                    │    low:    ≤ 0.3            │
                    └─────────────────────────────┘
```

#### Phase 1: 휴리스틱 분류기 (LLM 없이)

LLM 호출 없이 메시지 특성만으로 1차 분류:

```rust
pub struct HeuristicClassifier {
    /// 컨텍스트 길이 기준
    context_threshold_high: usize,
    context_threshold_low: usize,
}

impl HeuristicClassifier {
    pub fn classify(&self, input: &ClassifierInput) -> f64 {
        let mut score = 0.0;
        
        // 1. 메시지 길이 가중치
        score += self.length_weight(input.message.len());
        
        // 2. 코드 블록 포함 여부
        if input.contains_code_blocks() { score += 0.15; }
        
        // 3. 파일 경로 참조 여부
        if input.contains_file_paths() { score += 0.1; }
        
        // 4. 멀티턴 컨텍스트 길이
        score += self.context_weight(input.context_tokens);
        
        // 5. 키워드 분석 ("수정해", "디버그", "설계" vs "안녕", "뭐야")
        score += self.keyword_weight(input.message);
        
        score.clamp(0.0, 1.0)
    }
}
```

#### Phase 2: LLM 분류기 (선택적)

`classifier_model`이 설정된 경우에만 활성화:

```rust
pub struct LlmClassifier {
    provider: Arc<dyn Provider>,
    model: Model,
    heuristic: HeuristicClassifier,
}

impl LlmClassifier {
    pub async fn classify(&self, input: &ClassifierInput) -> Result<f64> {
        // 1. 휴리스틱으로 빠른 분류
        let heuristic_score = self.heuristic.classify(input);
        
        // 2. 애매한 구간(0.3~0.7)만 LLM으로 정밀 분류
        if heuristic_score > 0.25 && heuristic_score < 0.75 {
            self.llm_classify(input, heuristic_score).await
        } else {
            Ok(heuristic_score)
        }
    }
    
    async fn llm_classify(&self, input: &ClassifierInput, hint: f64) -> Result<f64> {
        let prompt = format!(
            "Rate the complexity of this user request on a 0.0-1.0 scale.\n\
             Context: {} tokens, has_code: {}, has_paths: {}\n\
             Request: {}\n\
             Respond with only a number.",
            input.context_tokens,
            input.contains_code_blocks(),
            input.contains_file_paths(),
            input.message,
        );
        
        // 낮은 모델(haiku/flash) 사용으로 비용 최소화
        let response = self.provider.stream(&self.model, ...).await?;
        let score: f64 = response.trim().parse().unwrap_or(hint);
        Ok(score.clamp(0.0, 1.0))
    }
}
```

#### 입력 구조체

```rust
/// Classifier input — 라우팅 분류에 필요한 메타데이터
pub struct ClassifierInput {
    /// 사용자 메시지 텍스트
    pub message: String,
    /// 현재 컨텍스트 토큰 수
    pub context_tokens: usize,
    /// 대화 턴 수
    pub turn_count: usize,
    /// 사용 가능한 도구 목록
    pub available_tools: Vec<String>,
}
```

### 3.4 설정 스키마 (이미 구현됨)

```toml
# .oxicode/settings.toml 또는 ~/.oxicode/settings.toml

[router]
enabled = true
default_profile = "auto"
classifier_model = "anthropic/claude-haiku-4"   # 분류용 모델 (빠르고 저렴)

[router.profiles.auto]
high.model = "anthropic/claude-sonnet-4"
medium.model = "anthropic/claude-haiku-4"
low.model = "google/gemini-2.0-flash"
```

`classifier_model`이 설정되지 않으면 휴리스틱만 사용한다.

### 3.5 구현 계획

| Phase | 내용 | 예상 공수 | 파일 |
|-------|------|-----------|------|
| **Phase 1** | `HeuristicClassifier` 구현 | 2-3시간 | `oxicode-ai/src/router/classifier.rs` |
| **Phase 2** | `LlmClassifier` 구현 | 3-4시간 | 동일 파일 |
| **Phase 3** | `Router`에 통합 | 1-2시간 | `oxicode-ai/src/router/mod.rs` |
| **Phase 4** | 테스트 + 설정 연동 | 2시간 | `oxicode-ai/src/router/`, `oxicode-store/` |

#### Phase 1 상세

1. `HeuristicClassifier` 구조체 및 `classify()` 구현
2. `ClassifierInput` 타입 정의
3. 단위 테스트:
   - 단순 인사 → score < 0.2
   - 파일 수정 요청 → score > 0.5
   - 복잡한 리팩터링 요청 → score > 0.7
4. `ClassifierType::Heuristic` enum variant 추가 (이미 `Default` 존재)

#### Phase 2 상세

1. `LlmClassifier`에 `Provider` 주입
2. 프롬프트 템플릿 작성
3. 애매한 구간만 LLM 호출하는 최적화
4. 타임아웃 + 폴백 (LLM 실패 시 휴리스틱 결과 사용)

#### Phase 3 상세

1. `Router::route()`에서 `classifier_type`에 따라 분기
2. `LlmClassifier` 사용 시 `Provider` 해결 로직
3. `RoutingDecision`에 분류 결과 메타데이터 추가

#### Phase 4 상세

1. 통합 테스트: 전체 파이프라인 (설정 → 분류 → 라우팅 → 모델 선택)
2. 벤치마크: 휴리스틱 vs LLM 분류 정확도 비교
3. `classifier_model` 설정이 없을 때의 기본 동작 보장

### 3.6 비용 분석

| 시나리오 | 추가 비용 | 설명 |
|----------|-----------|------|
| `classifier_model` 미설정 | **없음** | 휴리스틱만 사용 |
| 애매하지 않은 요청 | **없음** | score가 0.3 이하 또는 0.7 이상이면 LLM 미호출 |
| 애매한 요청 | ~50 tokens/요청 | haiku/flash 급 모델로 분류 |

---

## 4. 구현 우선순위

| 순위 | 작업 | 위험도 | 공수 | 효과 |
|------|------|--------|------|------|
| **1** | `build_system_prompt` 중복 제거 | 낮음 | 1시간 | 110줄 감소, DRY 달성 |
| **2** | `resource_loader` 합병 Phase 1-3 | 중간 | 3시간 | 512줄 감소, 복잡도 감소 |
| **3** | `LlmClassifier` Phase 1 (휴리스틱) | 낮음 | 3시간 | 라우터 기본 분류 가능 |
| **4** | `LlmClassifier` Phase 2 (LLM) | 중간 | 4시간 | 정밀 라우팅 |
| **5** | `resource_loader` 합병 Phase 4 | 낮음 | 1시간 | 최종 정리 |

각 작업은 독립적으로 커밋 가능하며, 순서대로 진행하는 것을 권장한다.

---

## 5. 검증 기준

각 리팩터링/구현 완료 후 다음이 통과해야 한다:

```bash
cargo fmt --all -- --check          # 포맷
cargo clippy --workspace -- -D warnings  # 린트
cargo nextest run --workspace        # 전체 테스트
cargo build --release                # 릴리즈 빌드
```

추가로:
- `#[allow(dead_code)]` 수가 감소했는지 확인
- 기존 테스트가 모두 동일하게 통과하는지 확인
- 새 기능에 대한 테스트가 추가되었는지 확인
