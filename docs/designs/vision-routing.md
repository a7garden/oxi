# Design: Vision-Aware Model Routing

> 이미지 처리 요청을 비전 지원 모델로 자동 라우팅
> Status: **Draft** | Date: 2026-05-25

## 0. 문제

현재 라우터는 메시지 구조( StructuralSignal ), 행동 패턴( BehavioralSignal ),
토큰 예산( ContextBudgetSignal )만 고려하여 티어를 결정합니다.

browse 도구의 스크린샷, 사용자의 이미지 첨부 등 **이미지 ContentBlock이 포함된
요청**이 비전 미지원 모델(예: claude-haiku-3.5-text, deepseek-chat)로 라우팅되면:

1. **에러 발생** — provider가 이미지 블록을 거부하거나 무시
2. **정보 손실** — 이미지가 무시되고 텍스트만 처리
3. **Fallback 낭비** — 불필요한 fallback chain 실행

## 1. 현재 아키텍처

```
Context (messages)
      │
      ├── StructuralSignal   ← 메시지/도구 수, 토큰 추정
      ├── BehavioralSignal   ← 대화 단계, 최근 도구 패턴
      └── ContextBudgetSignal ← 토큰 예산, 비용
              │
              ▼
      compute_score() ──► RoutingScore ──► RouterTier { High | Medium | Low }
                                                │
                                                ▼
                                      RoutedTierConfig.model
                                      (항상 동일 모델, vision 고려 없음)
```

## 2. 설계

### 2.1 VisionSignal 추가

```rust
// oxicode-ai/src/router/signals.rs

/// Signals derived from multimodal content in the conversation.
#[derive(Debug, Clone, Default)]
pub struct VisionSignal {
    /// Number of image content blocks in the last N messages.
    pub recent_image_count: usize,
    /// Whether the *latest* user turn contains an image.
    pub has_image_in_latest_turn: bool,
    /// Tool names that recently produced image blocks (e.g. "browse").
    pub image_producing_tools: Vec<String>,
}
```

#### 추출 로직

```rust
impl VisionSignal {
    /// Extract vision signals from the last `window` messages.
    pub fn extract(messages: &[Message], window: usize) -> Self {
        let start = messages.len().saturating_sub(window);
        let recent = &messages[start..];

        let mut signal = Self::default();

        // 1. Check latest user turn for images
        if let Some(Message::User(u)) = messages.last() {
            if let MessageContent::Blocks(blocks) = &u.content {
                for block in blocks {
                    if matches!(block, ContentBlock::Image(_)) {
                        signal.has_image_in_latest_turn = true;
                        signal.recent_image_count += 1;
                    }
                }
            }
        }

        // 2. Scan recent tool results for image blocks
        for msg in recent {
            if let Message::ToolResult(tr) = msg {
                for block in &tr.content {
                    if matches!(block, ContentBlock::Image(_)) {
                        signal.recent_image_count += 1;
                        if !signal.image_producing_tools.contains(&tr.tool_name) {
                            signal.image_producing_tools.push(tr.tool_name.clone());
                        }
                    }
                }
            }
        }

        signal
    }

    /// Returns true if the current turn requires vision capability.
    pub fn requires_vision(&self) -> bool {
        self.has_image_in_latest_turn || self.recent_image_count > 0
    }
}
```

### 2.2 ScoringWeights 확장

```rust
// oxicode-ai/src/router/types.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringWeights {
    pub structural: f64,      // 0.35 (기존 0.40 → 0.05 감소)
    pub behavioral: f64,      // 0.35 (유지)
    pub context_budget: f64,  // 0.20 (기존 0.25 → 0.05 감소)
    #[serde(default = "default_vision")]
    pub vision: f64,          // 0.10 (신규)
}

fn default_vision() -> f64 { 0.10 }
```

> 비전 가중치는 기본 0.10으로 낮게 설정.
> 비전 신호가 없으면 0점이므로 점수에 영향 없음.
> 비전 신호가 있으면 0.10만큼 상향 → 티어 승격 가능.

### 2.3 scoring에 VisionSignal 통합

```rust
// oxicode-ai/src/router/scoring.rs

pub fn compute_score(
    structural: &StructuralSignal,
    behavioral: &BehavioralSignal,
    budget: &ContextBudgetSignal,
    vision: &VisionSignal,        // ← 추가
    weights: &ScoringWeights,
) -> f64 {
    let s_raw = structural.normalized();
    let b_raw = behavioral.normalized();
    let c_raw = budget.normalized();
    let v_raw = vision.normalized();    // ← 추가

    let s_sharp = sigmoid(s_raw, 0.5, 4.0);
    let b_sharp = sigmoid(b_raw, 0.5, 4.0);
    let c_sharp = sigmoid(c_raw, 0.5, 4.0);
    let v_sharp = sigmoid(v_raw, 0.5, 6.0);  // 더 날카로운 시그모이드

    let raw = weights.structural * s_sharp
            + weights.behavioral * b_sharp
            + weights.context_budget * c_sharp
            + weights.vision * v_sharp;       // ← 추가

    let total = weights.structural + weights.behavioral
              + weights.context_budget + weights.vision;

    if total > 0.0 {
        (raw / total).clamp(0.0, 1.0)
    } else {
        0.5
    }
}
```

### 2.4 VisionSignal 정규화

```rust
impl VisionSignal {
    /// Normalize to `[0, 1]` for scoring.
    /// - 0 images → 0.0
    /// - 1 image  → 0.7
    /// - 2+ images → 0.9~1.0
    pub fn normalized(&self) -> f64 {
        if !self.requires_vision() {
            return 0.0;
        }
        // 강한 신호: 이미지가 있으면 높은 점수
        let count_factor = match self.recent_image_count {
            0 => 0.0,
            1 => 0.7,
            _ => 0.9 + (self.recent_image_count as f64 * 0.02).min(0.1),
        };
        let latest_factor = if self.has_image_in_latest_turn { 0.1 } else { 0.0 };
        (count_factor + latest_factor).clamp(0.0, 1.0)
    }
}
```

### 2.5 Tier → 모델 선택에 Vision 필터 추가

핵심 변경: **라우터가 티어에 해당하는 모델을 선택할 때, 이미지가 있으면
비전을 지원하는 모델로 강제 전환**.

```rust
// oxicode-ai/src/router/mod.rs — RouterProvider::stream() 내부

// 2. Vision signal 추출
let vision = VisionSignal::extract(&context.messages, 10);

// 3. Route through pipeline (vision 가중치 포함)
let (score, tier, phase) = self.pipeline.write().route_with_vision(context, &vision);

// 4. Tier config 가져오기
let tier_config = self.profiles.read().tier_config(profile_name, tier).cloned();

// 5. ★ Vision 필터: 이미지가 있으면 비전 모델로 덮어쓰기
let tier_config = if vision.requires_vision() {
    self.ensure_vision_model(tier_config, tier)
} else {
    tier_config
};
```

```rust
impl RouterProvider {
    /// If the selected tier model doesn't support vision, find one that does.
    fn ensure_vision_model(
        &self,
        tier_config: Option<RoutedTierConfig>,
        tier: RouterTier,
    ) -> Option<RoutedTierConfig> {
        let config = tier_config?;

        // 1. 현재 모델이 비전을 지원하는지 확인
        if let Some(pm) = parse_tier_model(&config) {
            if let Some(model) = crate::lookup_model(&pm.provider, &pm.model_id) {
                if model.supports_vision() {
                    return Some(config); // 이미 비전 지원
                }
            }
        }

        // 2. fallback 목록에서 비전 지원 모델 찾기
        for fb in &config.fallbacks {
            if let Some(pm) = ProviderModel::parse(fb) {
                if let Some(model) = crate::lookup_model(&pm.provider, &pm.model_id) {
                    if model.supports_vision() {
                        tracing::info!(
                            "Vision override: {} → {} (vision-capable)",
                            config.model, fb
                        );
                        return Some(RoutedTierConfig {
                            model: fb.clone(),
                            thinking: config.thinking.clone(),
                            fallbacks: config.fallbacks.clone(),
                        });
                    }
                }
            }
        }

        // 3. 프로필의 다른 티어에서 비전 모델 찾기
        //    (예: Medium → High 티어의 비전 모델로 승격)
        let profiles = self.profiles.read();
        if let Some(default_profile) = profiles.default_profile() {
            for higher_tier in [RouterTier::High, RouterTier::Medium] {
                if higher_tier.rank() > tier.rank() {
                    if let Some(tc) = profiles.tier_config(&profiles.default_name, higher_tier) {
                        if let Some(pm) = parse_tier_model(tc) {
                            if let Some(model) = crate::lookup_model(&pm.provider, &pm.model_id) {
                                if model.supports_vision() {
                                    tracing::info!(
                                        "Vision upgrade: tier {:?} → {:?}, model {} → {}",
                                        tier, higher_tier, config.model, tc.model
                                    );
                                    return Some(tc.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. 비전 모델을 찾지 못함 → 경고 후 원래 설정 유지
        tracing::warn!(
            "Vision required but no vision-capable model found in tier {:?}. \
             Model {} may fail with image content.",
            tier, config.model
        );
        Some(config)
    }
}
```

### 2.6 RouterPipeline 업데이트

```rust
impl RouterPipeline {
    /// Route with vision awareness.
    pub fn route_with_vision(
        &mut self,
        context: &Context,
        vision: &VisionSignal,
    ) -> (f64, RouterTier, RouterPhase) {
        let structural = StructuralSignal::extract(&context.messages);
        let behavioral = BehavioralSignal::extract(&context.messages, &self.decision_history);
        let budget = ContextBudgetSignal::extract(
            structural.estimated_tokens,
            self.accumulated_cost,
            self.budget_limit,
            self.context_upgrade_threshold,
        );

        let raw_score = compute_score(
            &structural, &behavioral, &budget, vision, &self.weights,
        );
        self.last_score = raw_score;

        let score = RoutingScore(raw_score);
        let mut tier = score.to_tier(0.65, 0.35);

        // 기존 업그레이드/다운그레이드 로직
        if budget.should_upgrade_context() && tier != RouterTier::High {
            tier = RouterTier::High;
        }
        if budget.is_over_budget() && tier == RouterTier::High {
            tier = RouterTier::Medium;
        }

        (raw_score, tier, behavioral.phase)
    }
}
```

### 2.7 RoutingDecision에 vision 정보 추가

```rust
// oxicode-ai/src/router/types.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    // ... 기존 필드 ...

    /// Whether vision capability influenced this decision.
    #[serde(default)]
    pub is_vision_triggered: bool,

    /// Vision signal at the time of decision.
    #[serde(default)]
    pub vision_images: usize,
}
```

## 3. 설정 예시

### settings.toml

```toml
[router]
enabled = true
default_profile = "auto"

[router.weights]
structural = 0.35
behavioral = 0.35
context_budget = 0.20
vision = 0.10           # ← 새 가중치

[router.profiles.auto]
high.model = "anthropic/claude-sonnet-4"       # vision ✅
high.thinking = "medium"

medium.model = "anthropic/claude-sonnet-4"     # vision ✅ (기본값)
# 또는:
# medium.model = "anthropic/claude-haiku-4"    # vision ✅ (haiku-4도 지원)

low.model = "google/gemini-2.0-flash"          # vision ✅

# 모델이 비전을 지원하지 않으면 자동으로 fallback에서 찾음:
# [router.profiles.budget]
# high.model = "deepseek/deepseek-chat"         # vision ❌
# high.fallbacks = ["openai/gpt-4o-mini"]       # vision ✅ → 자동 선택
```

## 4. 라우팅 시나리오

### 시나리오 A: 스크린샷 후 분석

```
1. 사용자: "이 웹사이트 분석해줘"
2. 에이전트: browse(url="...", screenshot=true)
3. ToolResult: [Image(base64), Text("...")]
4. 라우터:
   ├── VisionSignal.extract() → recent_image_count=1, requires_vision=true
   ├── compute_score() → vision 가중치 0.10 추가 → score 상향
   ├── tier = Medium (기존) → ensure_vision_model() → claude-sonnet-4 (vision ✅)
   └── Decision: tier=Medium, model=claude-sonnet-4, is_vision_triggered=true
5. 에이전트: "스크린샷을 분석한 결과..." (비전 모델로 이미지 인식)
```

### 시나리오 B: 텍스트 전용 코딩

```
1. 사용자: "이 함수 리팩토링해줘"
2. VisionSignal.extract() → recent_image_count=0, requires_vision=false
3. compute_score() → vision=0.0 → 영향 없음
4. 기존 라우팅 로직 그대로 (low tier → gemini-flash 등)
```

### 시나리오 C: 비전 미지원 모델이 티어에 설정된 경우

```
설정:
  low.model = "deepseek/deepseek-chat"   # vision ❌
  low.fallbacks = ["google/gemini-2.0-flash"]  # vision ✅

라우팅:
  1. tier = Low → model = "deepseek/deepseek-chat"
  2. vision.requires_vision() = true
  3. ensure_vision_model():
     ├── deepseek-chat → supports_vision() = false
     ├── fallback[0] = gemini-flash → supports_vision() = true
     └── → "google/gemini-2.0-flash"로 교체
  4. Decision: model=gemini-flash, is_vision_triggered=true
```

## 5. Model DB의 vision 메타데이터

이미 `Model.supports_vision()`이 구현되어 있음:

```rust
// oxicode-ai/src/types.rs
impl Model {
    pub fn supports_vision(&self) -> bool {
        self.input.contains(&InputModality::Image)
    }
}
```

model_db.rs의 각 모델 엔트리도 `input: &[InputModality::Text, InputModality::Image]`로
vision 지원 여부가 이미 정의되어 있으므로 추가 작업 불필요.

## 6. 파일 변경 목록

| 파일 | 변경 | 설명 |
|------|------|------|
| `oxicode-ai/src/router/signals.rs` | **수정** | `VisionSignal` 추가 |
| `oxicode-ai/src/router/scoring.rs` | **수정** | `compute_score()`에 vision 파라미터 추가 |
| `oxicode-ai/src/router/types.rs` | **수정** | `ScoringWeights.vision` 추가, `RoutingDecision`에 vision 필드 |
| `oxicode-ai/src/router/mod.rs` | **수정** | `RouterPipeline::route_with_vision()`, `ensure_vision_model()` |
| `oxicode-store/src/router_config.rs` | **수정** | `ScoringWeights.vision` 필드 파싱 |
| `oxicode-ai/src/router/profiles.rs` | 변경 없음 | |
| `oxicode-ai/src/router/classifier.rs` | 변경 없음 | |

## 7. 호환성

### 하위 호환

- `ScoringWeights.vision`은 `#[serde(default)]` → 기존 settings.toml 그대로 작동
- `VisionSignal`이 없으면 (0.0) 기존 scoring과 동일한 결과
- `RoutingDecision.is_vision_triggered`는 `#[serde(default)]` → 기존 세션 호환

### 기본 가중치 조정

```
기존: structural=0.40, behavioral=0.35, context_budget=0.25 (합=1.00)
변경: structural=0.35, behavioral=0.35, context_budget=0.20, vision=0.10 (합=1.00)
```

vision 가중치를 structural과 context_budget에서 0.05씩 가져옴.
vision 신호가 없으면 normalize에서 0점이 되므로 기존 동작과 사실상 동일.

## 8. 테스트 계획

```
oxicode-ai/src/router/signals.rs
├── test_vision_signal_no_images
├── test_vision_signal_user_image
├── test_vision_signal_tool_result_image
├── test_vision_signal_browse_screenshot
├── test_vision_signal_normalized_zero
├── test_vision_signal_normalized_single_image
└── test_vision_signal_normalized_multiple_images

oxicode-ai/src/router/scoring.rs
├── test_compute_score_with_vision_zero (기존 동작과 동일)
├── test_compute_score_with_vision_present (score 상향)
└── test_compute_score_vision_weights_zero (vision=0.0이면 영향 없음)

oxicode-ai/src/router/mod.rs
├── test_ensure_vision_model_already_supports
├── test_ensure_vision_model_fallback_used
├── test_ensure_vision_model_tier_upgrade
└── test_ensure_vision_model_no_vision_model_warns

oxicode-store/src/router_config.rs
└── test_parse_weights_with_vision_field
```

## 9. 구현 순서

```
Step 1: signals.rs — VisionSignal 구조체 + extract + normalized + 테스트
Step 2: types.rs — ScoringWeights.vision + RoutingDecision vision 필드
Step 3: scoring.rs — compute_score()에 vision 추가 + 테스트
Step 4: mod.rs — route_with_vision + ensure_vision_model + 테스트
Step 5: router_config.rs — vision 가중치 파싱
Step 6: 통합 테스트
```

예상 기간: **1-2일**
