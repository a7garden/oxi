# 세부 설계 ⑦ — Compaction 다중 모드 (snapcompact + inline imaging)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §6·§12 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `packages/snapcompact/src/snapcompact.ts` (1,554줄), `session/snapcompact-inline.ts` (542줄), `session/compact-modes.ts` (105줄)
> oxicode 기반: `oxicode-ai/src/compaction.rs` (1,288줄, `Compactor` trait, `LlmCompactor`, `CompactionManager`)
> 후속: N2 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

oxicode는 현재 **LLM 기반 단일 compaction**만 지원한다 (`LlmCompactor`). omp는 세 가지 전략을 제공한다:

1. **context-full (soft)** — LLM 요약 (oxicode가 이미 구현). omp의 `soft` 모드 대응.
2. **snapcompact** — 대화 이력을 **비트맵 PNG 프레임**으로 아카이빙. LLM 호출 없이 결정론적. 비전 모델이 이미지를 읽어 역사를 복원.
3. **inline imaging** — 요청 변환 단계에서 **큰 tool result를 PNG로 치환**. snapcompact의 인라인 버전.

**oxicode의 핵심 자산**: `Compactor` trait이 이미 확장 가능하게 설계됨. `SnapcompactCompactor`를 새 구현체로 추가하면 된다. **omp의 snapcompact 네이티브 렌더러가 이미 Rust**(`crates/pi-natives/src/snapcompact.rs`) — 이식 비용이 낮다.

### omp가 검증한 가치
- **토큰 절약** — 큰 tool result(3,000+ 토큰)를 PNG(수백 토큰)로 치환 → 컨텍스트 비용 60-90% 절감.
- **정보 보존** — LLM 요약의 정보 손실 없이, 원본을 이미지로 보존.
- **결정론적** — snapcompact는 LLM 호출 없이 로컬 렌더링. 지연/비용/키 불필요.
- **provider 인식** — Anthropic/Google/OpenAI 각각 최적화된 프레임 형상(해상도, 셀 크기, 잉크).

---

## 1. omp 메커니즘

### 1.1 세 가지 compaction 모드 (`session/compact-modes.ts`)

```typescript
type CompactMode = "soft" | "remote" | "snapcompact";

interface CompactionOverride {
    strategy?: "context-full" | "snapcompact";
    remoteEnabled?: boolean;
}

const COMPACT_MODES = [
    { name: "soft",        description: "LLM summary of old messages",          overrides: { strategy: "context-full" } },
    { name: "remote",      description: "Remote compaction endpoint",            overrides: { strategy: "context-full", remoteEnabled: true }, requiresRemote: true },
    { name: "snapcompact", description: "Archive as bitmap images (no LLM)",     overrides: { strategy: "snapcompact" }, rejectsFocus: true },
];
```

### 1.2 snapcompact — 비트맵 아카이빙 (`packages/snapcompact/`)

```
대화 이력 (텍스트)
  → 직렬화 (message → text)
  → 픽셀 폰트 렌더링 (renderSnapcompactPng — Rust 네이티브)
  → PNG 프레임들 (frameSize px, 높이는 텍스트 행에 맞춤)
  → 프레임을 compaction entry의 preserveData에 저장
  → 매 컨텍스트 재구축 시 프레임을 이미지로 재첨부
```

**프레임 형상 (provider 인식)** (`snapcompact.ts:59-103`):

| Provider | 형상 | 폰트 | 셀 | 잉크 | 프레임 크기 | 비고 |
|---|---|---|---|---|---|---|
| Anthropic | `11on16-bw` | 8x13 | 11px advance | 흑백 | 1932px (opus 4.7+) | letter-spacing 추가 |
| Google | `8on22-bw` | 8x13 | 22px pitch | 흑백 | 2048px | line spacing 추가 |
| OpenAI | `8on22-bw` | 8x13 | 22px pitch | 흑백 | 1568px | patch billing area-proportional |
| Unknown | `11on16-bw` | (Anthropic 폴백) | | | | |

> **근거**: SQuAD prose evals + toolbench (real search/read/find output). `6x12-dim` 등 조밀 셀은 OCR 한계(16px/char) 아래로 떨어져 f1 .351 (사실상 기권).

### 1.3 inline imaging — 요청 내 변환 (`session/snapcompact-inline.ts`)

```
요청 직전 (transformProviderContext 훅)
  → 컨텍스트 내 큰 tool result 탐지 (MIN_TOOL_RESULT_TOKENS=3000)
  → savings gate: imageTokens <= textTokens * 0.9
  → 시스템 프롬프트 / tool result를 PNG 프레임으로 치환
  → stub 텍스트 + 사용자 노트 주입
  → 변환된 컨텍스트로 provider 호출
```

**치환 정책**:
- `MIN_TOOL_RESULT_TOKENS = 3000` — 이 미만은 래스터화하지 않음 (절약 불가).
- `SAVINGS_MARGIN = 0.9` — 이미지 토큰이 텍스트 토큰의 90% 이하일 때만 치환.
- `MAX_SYSTEM_PROMPT_FRAMES = 6` — 시스템 프롬프트는 최대 6프레임.
- provider 이미지 예산 초과 시 나머지는 텍스트 그대로 전송.

### 1.4 savings journal (`session/snapcompact-savings-journal.ts`)

```typescript
type SnapcompactSavingsSink = (
    savings: ReadonlyArray<{ toolCallId: string; savedTokens: number }>,
    model: Model,
) => void;
```

각 inline swap이 절약한 토큰을 append-only 저널에 기록. `/context` 명령이 총 절약량 표시.

---

## 2. oxicode 기존 분석

### 2.1 `oxicode-ai/src/compaction.rs`

```rust
pub trait Compactor: Send + Sync {
    fn compact<'a>(
        &'a self,
        messages: &'a [Message],
        config: &'a CompactionConfig,
    ) -> Pin<Box<dyn Future<Output = Result<CompactedContext, CompactionError>> + Send + 'a>>;
}

pub struct LlmCompactor { model: Model, provider: Arc<dyn Provider> }

pub struct CompactionManager {
    strategy: CompactionStrategy,
    context_window: usize,
    config: CompactionConfig,
    compactor: Arc<dyn Compactor>,
}

pub enum CompactionStrategy {
    None,
    Threshold { max_tokens: usize },
    Iteration { max_iterations: usize },
    Hybrid { max_tokens: usize, max_iterations: usize },
}
```

oxicode의 compaction은:
- LLM 요약만 (`LlmCompactor`).
- `CompactionConfig`: keep_recent, max_batch, target_ratio, summary_max_tokens, temperature, timeout.
- `CompactionManager`가 `should_compact` 판단 후 `compact` 호출.

### 2.2 갭

| omp 기능 | oxicode 상태 |
|---|---|
| snapcompact (비트맵 아카이빙) | ❌ 없음 |
| inline imaging (요청 내 변환) | ❌ 없음 |
| 다중 모드 (`/compact soft\|remote\|snapcompact`) | ❌ 단일 전략만 |
| provider 인식 프레임 형상 | ❌ 없음 |
| savings journal | ❌ 없음 |

---

## 3. oxicode화 설계

### 3.1 `Compactor` trait 확장

기존 trait은 유지하고 새 구현체 추가:

```rust
// oxicode-ai/src/compaction.rs — 기존 유지
pub trait Compactor: Send + Sync {
    fn compact<'a>(...) -> ...;
    fn name(&self) -> &str { "default" }
}

// 신규: SnapcompactCompactor
pub struct SnapcompactCompactor {
    renderer: Arc<dyn SnapcompactRenderer>,
}

pub trait SnapcompactRenderer: Send + Sync {
    /// 텍스트를 PNG 프레임들로 렌더. provider별 최적 형상 선택.
    fn render<'a>(
        &'a self,
        text: &'a str,
        shape: FrameShape,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<u8>>, CompactionError>> + Send + 'a>>;
}
```

### 3.2 프레임 형상 (`FrameShape`)

```rust
#[derive(Debug, Clone)]
pub struct FrameShape {
    pub font: PixelFont,
    pub cell_width: u16,
    pub cell_height: u16,
    pub variant: InkVariant,
    pub line_repeat: u8,
    pub frame_size: u16,
    pub frame_token_estimate: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum PixelFont {
    Font5x8,
    Font8x8,
    Font6x12,
    Font8x13,
}

#[derive(Debug, Clone, Copy)]
pub enum InkVariant {
    BlackWhite,        // "bw"
    SentenceColored,   // "sent" — 문장 경계별 6색 순환
}

/// provider별 최적 형상 선택 (omp 계약 이식).
pub fn shape_for_provider(provider: &str) -> FrameShape {
    match provider {
        "anthropic" => FrameShape {
            font: PixelFont::Font8x13,
            cell_width: 11,
            cell_height: 16,
            variant: InkVariant::BlackWhite,
            line_repeat: 1,
            frame_size: 1932,
            frame_token_estimate: 1100,
        },
        "google" => FrameShape {
            font: PixelFont::Font8x13,
            cell_width: 8,
            cell_height: 22,
            variant: InkVariant::BlackWhite,
            line_repeat: 1,
            frame_size: 2048,
            frame_token_estimate: 1120,
        },
        "openai" => FrameShape {
            font: PixelFont::Font8x13,
            cell_width: 8,
            cell_height: 22,
            variant: InkVariant::BlackWhite,
            line_repeat: 1,
            frame_size: 1568,
            frame_token_estimate: 850,
        },
        _ => shape_for_provider("anthropic"),  // 폴백
    }
}
```

### 3.3 네이티브 렌더러 이식

omp의 `crates/pi-natives/src/snapcompact.rs`는 **이미 Rust**. 이를 독립 크레이트 또는 oxicode-ai 내 모듈로 이식:

```rust
// oxicode-ai/src/snapcompact/renderer.rs (또는 별도 oxicode-snapcompact 크레이트)

/// 픽셀 폰트로 텍스트를 PNG 프레임들로 렌더.
/// omp pi-natives/snapcompact.rs에서 직접 이식.
pub fn render_text_to_png_frames(
    text: &str,
    shape: &FrameShape,
) -> Result<Vec<Vec<u8>>, CompactionError> {
    // 1. 텍스트를 행으로 분할 (frame_size에 맞춘 래핑)
    let lines = wrap_text_to_width(text, shape.frame_size, shape.cell_width);
    
    // 2. 프레임별로 행 그룹화
    let rows_per_frame = (shape.frame_size / shape.cell_height) as usize;
    let frames: Vec<Vec<&str>> = lines.chunks(rows_per_frame).map(|c| c.to_vec()).collect();
    
    // 3. 각 프레임을 PNG로 렌더
    let mut png_frames = Vec::with_capacity(frames.len());
    for frame_lines in frames {
        let png = render_frame(&frame_lines, shape)?;
        png_frames.push(png);
    }
    
    Ok(png_frames)
}

fn render_frame(lines: &[&str], shape: &FrameShape) -> Result<Vec<u8>, CompactionError> {
    // 픽셀 폰트 비트맵 렌더 (omp pi-natives 계약)
    // - 폰트 글리프 조회 (bundled X.org misc fonts)
    // - 셀 피치에 맞춰 배치
    // - line_repeat 만큼 반복 (두 번째부터는 하이라이트 밴드)
    // - variant에 따라 잉크 적용 (bw=흑백, sent=문장별 색상)
    // - PNG 인코딩 (image 크레이트 또는 직접)
    todo!("omp pi-natives/snapcompact.rs 이식")
}
```

> **의존**: `image` 크레이트 (PNG 인코딩) 또는 omp의 직접 PNG 인코더 이식. 폰트 글리프는 `include_bytes!`로 번들.

### 3.4 SnapcompactCompactor 구현

```rust
#[async_trait]
impl Compactor for SnapcompactCompactor {
    fn name(&self) -> &str { "snapcompact" }
    
    async fn compact(
        &self,
        messages: &[Message],
        config: &CompactionConfig,
    ) -> Result<CompactedContext, CompactionError> {
        // 1. keep_recent 이후 메시지를 텍스트로 직렬화
        let keep = &messages[..config.keep_recent.min(messages.len())];
        let archive = &messages[config.keep_recent.min(messages.len())..];
        
        let serialized = serialize_messages(archive);
        
        // 2. provider별 형상 선택
        let shape = shape_for_provider(&self.provider_name);
        
        // 3. PNG 프레임 렌더
        let frames = self.renderer.render(&serialized, shape).await?;
        
        // 4. CompactedContext 반환 (프레임을 이미지 블록으로)
        Ok(CompactedContext::new(
            keep.to_vec(),
            archive.len(),
            CompactionMetadata::snapcompact(frames.len(), shape.frame_token_estimate),
        ).with_archive_frames(frames))
    }
}
```

### 3.5 inline imaging — 요청 변환 훅

`oxicode-ai/src/`에 컨텍스트 변환 훅 추가:

```rust
/// provider 호출 직전 컨텍스트를 변환하는 훅.
/// 큰 tool result를 PNG로 치환하여 토큰 절약.
pub trait ContextTransformer: Send + Sync {
    fn transform<'a>(
        &'a self,
        context: &'a Context,
        model: &'a Model,
    ) -> Pin<Box<dyn Future<Output = Context> + Send + 'a>>;
}

pub struct SnapcompactInlineTransformer {
    renderer: Arc<dyn SnapcompactRenderer>,
    options: SnapcompactInlineOptions,
}

pub struct SnapcompactInlineOptions {
    pub render_system_prompt: SystemPromptMode,  // none | agents_md | all
    pub render_tool_results: bool,
    pub shape: Option<FrameShape>,  // None = auto (provider별)
}

pub enum SystemPromptMode {
    None,
    AgentsMd,    // <context> 블록만 치환
    All,         // 전체 시스템 프롬프트
}

#[async_trait]
impl ContextTransformer for SnapcompactInlineTransformer {
    async fn transform(&self, context: &Context, model: &Model) -> Context {
        let shape = self.options.shape.unwrap_or_else(|| shape_for_provider(&model.provider));
        let mut new_context = context.clone();
        
        // 1. 이미지 예산 확인
        let existing_images = count_images(&new_context);
        let budget = provider_image_budget(&model.provider);
        let remaining = budget.saturating_sub(existing_images);
        if remaining == 0 { return new_context; }  // 예산 소진
        
        // 2. 큰 tool result 탐지 + 치환
        if self.options.render_tool_results {
            new_context = self.replace_large_tool_results(new_context, shape, remaining).await;
        }
        
        // 3. 시스템 프롬프트 치환
        if self.options.render_system_prompt != SystemPromptMode::None {
            new_context = self.replace_system_prompt_sections(new_context, shape).await;
        }
        
        new_context
    }
}

const MIN_TOOL_RESULT_TOKENS: usize = 3000;
const SAVINGS_MARGIN: f64 = 0.9;
const MAX_SYSTEM_PROMPT_FRAMES: usize = 6;

fn passes_savings_gate(frames: usize, shape: &FrameShape, text_tokens: usize) -> bool {
    (frames * shape.frame_token_estimate) as f64 <= text_tokens as f64 * SAVINGS_MARGIN
}
```

### 3.6 CompactionManager 확장

```rust
pub struct CompactionManager {
    strategy: CompactionStrategy,
    context_window: usize,
    config: CompactionConfig,
    compactor: Arc<dyn Compactor>,
    /// inline imaging transformer (snapcompact). None = 비활성화.
    inline_transformer: Option<Arc<dyn ContextTransformer>>,
}

impl CompactionManager {
    /// provider 호출 전 컨텍스트 변환.
    pub fn transform_context(&self, context: &Context, model: &Model) -> Context {
        if let Some(transformer) = &self.inline_transformer {
            // block_on은 안전하지 않음 — async 런타임에서 호출
            // 실제로는 provider stream 호출 전 await 지점에서 변환
            todo!("async 통합")
        } else {
            context.clone()
        }
    }
}
```

---

## 4. 슬래시 명령

### `/compact` 명령 확장

`oxicode-cli/src/tui/slash/builtin/`에 compaction 명령 추가:

| 서브명령 | 동작 |
|---|---|
| `/compact` | 기본 (설정된 전략 — 보통 LLM 요약) |
| `/compact soft [focus]` | LLM 요약. `focus`는 요약 방향 힌트 |
| `/compact snapcompact` | 비트맵 아카이빙 (LLM 없음) |
| `/compact remote` | 원격 엔드포인트 (후순위) |

```rust
pub enum CompactMode {
    Soft,        // context-full (LLM)
    Snapcompact, // bitmap archive
    Remote,      // 원격 (후순위)
}
```

---

## 5. 설정

```rust
pub struct Settings {
    pub compaction_strategy: CompactionStrategyConfig,  // soft | snapcompact | hybrid
    pub snapcompact_enabled: bool,                       // 기본 false (실험적)
    pub snapcompact_inline_enabled: bool,                // inline imaging, 기본 false
    pub snapcompact_render_tool_results: bool,           // 기본 true (snapcompact 켜진 경우)
    pub snapcompact_render_system_prompt: SystemPromptMode,  // 기본 AgentsMd
    pub snapcompact_min_tool_result_tokens: usize,       // 기본 3000
}
```

---

## 6. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N2.14 | `FrameShape` + `PixelFont` + `InkVariant` 타입 | — |
| N2.15 | `shape_for_provider` (provider별 형상 선택) | N2.14 |
| N2.16 | `SnapcompactRenderer` trait + 네이티브 렌더러 이식 (omp pi-natives) | N2.15 |
| N2.17 | `SnapcompactCompactor` (Compactor trait 구현체) | N2.16 |
| N2.18 | `CompactionMetadata::snapcompact` + archive_frames | N2.17 |
| N2.19 | `ContextTransformer` trait | — |
| N2.20 | `SnapcompactInlineTransformer` (inline imaging) | N2.16, N2.19 |
| N2.21 | savings gate + 이미지 예산 관리 | N2.20 |
| N2.22 | `CompactionManager` inline_transformer 통합 | N2.20 |
| N2.23 | `/compact soft\|snapcompact` 슬래시 명령 | N2.17 |
| N2.24 | savings journal (append-only) | N2.20 |
| N2.25 | provider 스트림 호출 전 transform 훅 | N2.22 |

> **독립성**: ⑦은 ⑤⑥과 독립. N2에서 병렬 진행 가능.
> **omp 자산**: snapcompact 네이티브 렌더러가 이미 Rust → 이식 비용 낮음.

---

## 7. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| 폰트 글리프 번들 (X.org misc fonts) | 🟡 라이선스 | MIT/공개 도메인. `include_bytes!`로 번들 |
| PNG 인코딩 의존 (`image` 크레이트) | 🟢 가벼움 | 또는 omp의 직접 인코더 이식 |
| 비전 모델 호환성 (이미지 읽기) | 🟠 확인 필요 | Anthropic Claude, GPT-4V, Gemini 지원. 비-vision 모델은 폴백 |
| inline imaging false positive (핵심 코드를 이미지로) | 🟠 위험 | MIN_TOOL_RESULT_TOKENS=3000 + savings gate로 완화 |
| snapcompact 품질 검증 | 🔴 평가 필요 | omp SQuAD evals 재현. 도구 출력 벤치 필요 |
| 원격 compaction (remote 모드) | 🔴 후순위 | 별도 엔드포인트. N2 범위 외 |
| 프레임 렌더 성능 | 🟢 로컬 | 결정론적, LLM 없음. 수백 ms |

---

## 8. 테스트 계획

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn shape_for_provider_anthropic() {
        let shape = shape_for_provider("anthropic");
        assert_eq!(shape.font, PixelFont::Font8x13);
        assert_eq!(shape.cell_width, 11);
        assert_eq!(shape.frame_size, 1932);
    }

    #[test]
    fn savings_gate_rejects_small_results() {
        let shape = shape_for_provider("openai");
        // 1000 토큰 텍스트, 2프레임 필요 → 2*850=1700 > 1000*0.9=900 → 거부
        assert!(!passes_savings_gate(2, &shape, 1000));
        // 10000 토큰 텍스트, 2프레임 → 1700 < 9000 → 통과
        assert!(passes_savings_gate(2, &shape, 10000));
    }

    #[test]
    fn snapcompact_compactor_produces_frames() {
        // 100메시지 → keep_recent=5 → 95메시지 아카이빙
        // 프레임 수 > 0 확인
    }
}
```

---

## 9. 부록: omp → oxicode 매핑

| omp 위치 | oxicode 위치 |
|---|---|
| `packages/snapcompact/src/snapcompact.ts` (1,554) | `oxicode-ai/src/snapcompact/mod.rs` (또는 별도 크레이트) |
| `packages/snapcompact/src/snapcompact.ts` (Shape) | `FrameShape` |
| `packages/snapcompact/src/snapcompact.ts` (shapeForProvider) | `shape_for_provider` |
| `crates/pi-natives/src/snapcompact.rs` (Rust 네이티브) | `oxicode-ai/src/snapcompact/renderer.rs` (직접 이식) |
| `session/snapcompact-inline.ts` (542) | `oxicode-ai/src/snapcompact/inline.rs` |
| `session/snapcompact-savings-journal.ts` | `oxicode-ai/src/snapcompact/savings.rs` |
| `session/compact-modes.ts` (105) | `oxicode-cli/src/tui/slash/builtin/compact.rs` |
| `LlmCompactor` (기존) | 유지 (soft 모드) |
