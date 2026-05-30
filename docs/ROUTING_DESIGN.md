# 라우팅 시스템 재설계

> **상태**: 구현 완료

## 결정 흐름

```
요청 → [핀 고정?] ─예→ 해당 티어
          │아니오
          ▼
      [시그널 융합 스코어링] → 점수 → 티어
          │
          ▼
      (선택적) LLM 분류기 → 애매할 때만
          │
          ▼
      후처리 → 컨텍스트 초과? 예산 초과?
          │
          ▼
      실행 → 실패? → 다음 모델
```

## 단계별 설명

### 1. 핀 고정 (0ms)

사용자가 수동으로 티어를 고정했을 때만.

```rust
if let Some(tier) = config.pin_tier {
    return (tier, DecisionMethod::PinOverride);
}
```

### 2. 시그널 융합 스코어링 (<1ms)

5개 시그널을 가중 평균:

| 시그널 | 가중치 | 측정 대상 |
|--------|--------|-----------|
| `StructuralSignal` | 0.25 | 메시지수, 툴밀도, 토큰수 |
| `BehavioralSignal` | 0.20 | 페이즈, 툴 사용 패턴 |
| `ContextBudgetSignal` | 0.15 | 토큰 압력, 비용 |
| `VisionSignal` | 0.10 | 이미지 |
| `MessageContentSignal` | 0.30 | 메시지 내용의 구조적 특성 |

**MessageContentSignal** — 언어가 독립적 (키워드 매칭 없음):
- 메시지 길이 (문자수)
- 줄 수
- 코드블록 유무 (```)
- 파일경로 수
- 기호 밀도 ({, }, (, ), =, ; 비율)
- 질문 형태 (?로 끝남)
- 단일 문장 여부

결정:
```
score ≥ 0.65 → High
score ≤ 0.35 → Low
그 외 → Medium
```

### 3. LLM 분류기 (선택, ~200ms)

score가 0.25~0.75일 때만, `classifier_model` 설정 시에만.

### 4. 후처리

- 컨텍스트 > 임계값 → High 강제 업그레이드
- 예산 초과 + High → Medium 강등

### 5. 런타임 폴백

실패 시 폴백 체인 순차 시도.

## 제거된 것 (언어 종속성)

- ❌ 커스텀 룰 (`matches → tier`) — 키워드 매칭이므로 제거
- ❌ 툴 시나리오 (`web_search → 전용 모델`) — 구조적 시그널로 대체 가능
- ❌ DecisionMethod::RuleMatch, DecisionMethod::ScenarioMatch

## 설정 스키마

```toml
[router]
enabled = true
default_profile = "auto"
classifier_model = "anthropic/claude-haiku-4"   # 선택적
context_upgrade_threshold = 100000
max_session_budget = 1.0
pin = "high"    # 또는 생략 (auto)
phase_bias = 0.5  # 0=즉시 전환, 1=끈적임

[router.profiles.auto]
high.model = "anthropic/claude-sonnet-4"
high.thinking = "high"
high.fallbacks = ["openai/gpt-4o"]

medium.model = "anthropic/claude-haiku-4"

low.model = "google/gemini-2.0-flash"

[router.weights]
structural = 0.25
behavioral = 0.20
context_budget = 0.15
vision = 0.10
message = 0.30
```

## DecisionMethod enum

```rust
pub enum DecisionMethod {
    Heuristic,       // 시그널 융합 스코어링
    LlmClassifier,   // LLM 분류기
    PinOverride,     // 핀 고정
    ContextUpgrade,  // 컨텍스트 초과 강제
    BudgetDowngrade, // 예산 초과 강제
}
```

## 기존 코드 변경 영향

| 파일 | 변경 |
|------|------|
| `router/signals.rs` | `MessageContentSignal` 추가 (130줄) |
| `router/scoring.rs` | 5번째 시그널 인자 추가 |
| `router/types.rs` | `DecisionMethod` 확장, `RouterConfig`에 `pin_tier`, `phase_bias` 추가 |
| `router/mod.rs` | `route()` → 4-tuple 반환, Layer 0 핀 체크, phase bias 적용 |
| `store/router_config.rs` | `message` 가중치, `pin_tier`, `phase_bias` 추가 |
| `router_integration.rs`, `main.rs` | `RouterConfig::with_pinning()` 사용 |

**건드리지 않음**: `StructuralSignal`, `BehavioralSignal`, `ContextBudgetSignal`, `VisionSignal` 본逻辑