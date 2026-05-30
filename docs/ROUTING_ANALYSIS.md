# 라우팅 시스템 비교 분석 보고서

> **작성일**: 2025-05-30  
> **대상**: oxi 라우팅 시스템 vs claude-code-router, oh-my-openagent, pi-model-router  
> **방법**: 각 리포지토리 클론 후 전체 소스코드 분석

---

## 1. 시스템 아키텍처 비교

### 1.1 claude-code-router (CCR)

**타입**: HTTP 리버스 프록시 (Node.js/Fastify)

```
Claude Code CLI → CCR 서버 → Provider API
                   │
                   ├─ router.ts: 시나리오 기반 라우팅
                   ├─ transformer/: 21개 양방향 포맷 변환기
                   ├─ tokenizer/: 3종 (tiktoken/HF/API)
                   └─ config.json: 프로바이더 + 라우터 설정
```

**라우팅 결정 로직** (`packages/core/src/utils/router.ts`):

```
우선순위 캐스케이드 (고정 순서):
  0. 명시적 "provider,model" 포맷 → 해당 모델
  1. 토큰 > longContextThreshold(60K) → longContext 모델
  2. <CCR-SUBAGENT-MODEL> 태그 → 태그에서 추출
  3. 모델명에 "claude"+"haiku" 포함 → background 모델
  4. tools에 web_search 포함 → webSearch 모델
  5. thinking 활성화 → think 모델
  6. 기본 → default 모델
```

**특징**:
- 사용자 메시지 내용을 **전혀 분석하지 않음** — 요청 메타데이터(모델명, 툴, 토큰수)만으로 판단
- 21개 트랜스포머로 Anthropic↔OpenAI↔Gemini 포맷 변환
- 프로젝트별 라우팅 오버라이드 (`~/.claude/projects/{project}/config.json`)
- 커스텀 라우터 JS 파일로 전체 결정 로직 교체 가능
- 폴백 체인: 시나리오별 fallback 배열 → 순차 시도

### 1.2 oh-my-openagent (OmO)

**타입**: OpenCode 플러그인 (TypeScript)

```
사용자 요청 → 에이전트 선택 → 모델 해결 → Provider API
               │                │
               ├─ 10개 내장 에이전트   ├─ 5단계 해결 파이프라인
               │  (Sisyphus, Oracle..) │  UI선택 → 유저설정 → 카테고리 → 폴백체인 → 시스템기본
               └─ 8개 카테고리         └─ 퍼지 매칭 + 프로바이더별 변환
```

**모델 해결 파이프라인** (`packages/model-core/src/model-resolution-pipeline.ts`):

```
Stage 1: UI에서 선택한 모델 (primary 에이전트만)
Stage 2: 유저 config 오버라이드
Stage 3: 카테고리 기본 모델 (퍼지 매칭)
Stage 4: 하드코딩 폴백 체인 (프로바이더 스코프 → 크로스 프로바이더)
Stage 5: 시스템 기본 모델
실패 → 에이전트 스킵
```

**특징**:
- 에이전트 = 라우팅 단위 — 각 에이전트가 고유한 폴백 체인 보유
- Sisyphus(코딩): claude-opus → kimi-k2.6 → gpt-5.5 → ...
- Librarian(검색): gpt-5.4-mini → qwen3.5 → minimax → ... (항상 저렴)
- 런타임 폴백: HTTP 429/500 감지 → 자동 다음 모델 + 60초 쿨다운
- 프로바이더별 동시성 제한 (anthropic=3, opencode=10)
- 에이전트별 모델 특화 프롬프트 (Claude용, GPT용, Gemini용 각각 다름)

### 1.3 pi-model-router

**타입**: pi 확장 (TypeScript)

```
사용자 요청 → decideRouting() → 폴백 체인 → Provider API
               │
               ├─ 핀 오버라이드?
               ├─ 커스텀 룰 매칭?
               ├─ 키워드 휴리스틱 (6가지 카테고리)
               ├─ 페이즈 끈적임 (phaseBias)
               ├─ LLM 분류기 (선택, 애매할 때만)
               └─ 컨텍스트/예산 오버라이드
```

**라우팅 결정 알고리즘** (`extensions/routing.ts`):

```
14단계 캐스케이드 + 6개 후처리:
  0. 핀 → 고정 티어
  1. 커스텀 룰 → 룰의 티어
  2. 적응형 임계값 계산 (phaseBias 조절)
  3. 명시적 high 힌트 → HIGH
  4. 명시적 low 힌트 → LOW
  5. 요약 키워드 → LOW
  6. 계획 키워드 / "why " / 단어수≥임계값 / 4+줄 → HIGH
  7. 구현 키워드 → MEDIUM
  8. 짧은 lookup (≤24단어, 툴결과 없음) → LOW
  9. 계획 페이즈 유지 → HIGH
  10. 활성 구현 감지 → MEDIUM
  11. 짧은 요청 (≤lowThreshold) → LOW
  12. 기본 → MEDIUM
  13. 예산 초과 시 high→medium 강등

후처리 (provider.ts):
  14. 컨텍스트 > largeContextThreshold → HIGH
  15. LLM 분류기 오버라이드 (설정 시, 핀/루ール/컨텍스트 제외)
  16. 예산 재확인
  17. Google thinking 연속성 보호
  18. 이미지 첨부 → 비전 지원 모델로 업그레이드
  19. 폴백 체인 실행 + 컨텍스트 자동 잘림
```

**LLM 분류기 프롬프트**:
```
You are a model router classifier. Categorize into:
- high: Architecture, design, planning, tradeoff analysis, ...
- medium: Implementation, multi-file edits, normal coding, ...
- low: Summaries, changelogs, formatting, quick transforms, ...

Current phase: ${phase}
Recent history: ${last4messages}
Latest user message: ${prompt}

Return:
Tier: [high|medium|low]
Reasoning: [one short sentence]
```

**키워드 카테고리**:

| 카테고리 | 키워드 | 대상 티어 |
|----------|--------|-----------|
| 명시적 High | best, deep, carefully, thoroughly, robust, comprehensive, step by step, think hard | high |
| 명시적 Low | fast, cheap, quick, brief, one sentence, one line, tiny | low |
| 요약 | summarize, summary, changelog, rewrite, reformat, format, rename, recap, tl;dr | low |
| 계획 | plan, planning, architecture, architect, design, tradeoff, research, investigate, root cause, analyze, migration, strategy, compare | high |
| 구현 | implement, code, fix, update, edit, write, refactor, add tests, patch, change, apply, continue, resume | medium |
| 조회 | where is, which file, show me, list, what files, find, grep | low (≤24단어 + 툴결과 없을 때만) |

**특징**:
- **키워드 기반** — 한국어 등 다국어 미지원
- 커스텀 룰: `matches: ["deploy", "production"] → tier: "high"`
- 세션 상태 영속: `pi.appendEntry('router-state', ...)` — 재시작 후 복원
- 페이즈 끈적임: `phaseBias` (0~1)로 이전 페이즈 관성 조절
- 컨텍스트 자동 잘림: 폴백 모델의 컨텍스트 윈도우에 맞춰 메시지 삭제
- 이미지 감지: 비전 미지원 모델 → 상위 티어로 자동 업그레이드
- 핀: `/router pin high|medium|low|auto`
- 90초 퍼스트프롬프트 워치독 (응답 없으면 폴백)

---

## 2. 기능 매트릭스

| 기능 | oxi (현재) | CCR | OmO | pi-model-router |
|------|-----------|-----|-----|-----------------|
| **사용자 메시지 분석** | ❌ | ❌ | ❌ | ✅ 키워드 |
| **시그널 기반 스코어링** | ✅ 구조/행동/예산/비전 | ❌ | ❌ | ❌ |
| **툴 타입 감지** | ❌ | ✅ web_search | ❌ | ✅ 툴 결과 카운트 |
| **컨텍스트 길이** | ✅ 토큰 추정 | ✅ tiktoken 정확 | ❌ | ✅ 토큰 임계값 |
| **비전 감지** | ✅ 이미지 시그널 | ❌ | ❌ | ✅ 이미지 업그레이드 |
| **비용 추적** | ✅ 누적 비용 | ❌ | ❌ | ✅ 세션 예산 |
| **커스텀 룰** | ❌ | ✅ custom-router.js | ✅ 카테고리 매핑 | ✅ rules 배열 |
| **LLM 분류기** | ❌ 스텁만 | ❌ | ❌ | ✅ 선택적 |
| **폴백 체인** | ✅ | ✅ 시나리오별 | ✅ 멀티레벨 | ✅ 티어별 |
| **핀/수동 오버라이드** | ❌ | ✅ provider,model 포맷 | ✅ 에이전트별 | ✅ /router pin |
| **세션 상태 영속** | ❌ | ✅ LRU 캐시 | ❌ | ✅ 엔트리 영속 |
| **페이즈 관성** | ❌ | ❌ | ❌ | ✅ phaseBias |
| **컨텍스트 자동 잘림** | ✅ | ❌ | ❌ | ✅ |
| **런타임 에러 폴백** | ❌ | ✅ handleFallback | ✅ 상태머신 | ✅ 체인 순차 |
| **동시성 제어** | ❌ | ❌ | ✅ 세마포어 | ❌ |
| **포맷 변환** | ❌ | ✅ 21개 트랜스포머 | ❌ | ❌ |

---

## 3. 각 시스템의 핵심 인사이트

### CCR에서 배울 점
- **시나리오 타입 분류**: default/background/think/longContext/webSearch — 요청의 목적에 따라 모델 지정
- **툴 타입으로 라우팅**: web_search 툴 → 전용 모델 (가장 확실한 신호)
- **명시적 오버라이드**: subagent 태그, custom-router.js
- **폴백 체인**: 시나리오별 독립 폴백 배열

### OmO에서 배울 점
- **폴백 체인의 정교함**: `FallbackEntry { providers[], model, variant, thinking, reasoningEffort }` — 모델뿐 아니라 variant/thinking까지 체인에 포함
- **런타임 폴백**: HTTP 에러 코드 자동 감지 + 쿨다운 + 최대 시도 횟수 + 90초 워치독
- **동시성 제어**: 비싼 프로바이더(anthropic=3) vs 저렴한 프로바이더(opencode=10)
- **퍼지 매칭**: `claude-opus-4-7` ≈ `claude-opus-4.7` — 버전 포맷 차이 흡수

### pi-model-router에서 배울 점
- **키워드 기반 메시지 분석** (하지만 우리는 **언어 독립적**으로 갈 것)
- **커스텀 룰**: matches→tier 매핑으로 프로젝트별 특화
- **페이즈 관성**: phaseBias로 안정적인 티어 전환
- **LLM 분류기**: 애매한 케이스만 LLM 호출 (비용 최소화)
- **세션 영속**: append-only 엔트리 + 스냅샷 중복 제거
- **이미지 → 비전 모델 업그레이드**: 현재 티어 모델이 비전 미지원 시 상위 티어로

---

## 4. oxi 현재 시스템의 강점과 갭

### 강점 (그대로 유지)
- ✅ 4개 시그널 (structural, behavioral, context/budget, vision)
- ✅ sigmoid 기반 스코어 정규화
- ✅ 비전 감지 및 자동 업그레이드
- ✅ 비용 추적 및 예산 강등
- ✅ 프로바이더 폴백 체인

### 갭 (채워야 할 것)

| 갭 | 심각도 | 해결책 |
|----|--------|--------|
| **사용자 메시지 분석 없음** | 🔴 치명 | 구조적 시그널 + LLM 분류기 (키워드 아님) |
| **커스텀 룰 없음** | 🟡 중간 | `rules: Vec<RoutingRule>` 설정 추가 |
| **핀/수동 오버라이드 없음** | 🟡 중간 | `pin: Option<RouterTier>` 상태 추가 |
| **툴 타입 감지 없음** | 🟡 중간 | web_search 등 툴별 시나리오 분류 |
| **페이즈 관성 없음** | 🟢 경미 | phaseBias 파라미터 추가 |
| **세션 상태 미영속** | 🟢 경미 | RouterState 세션 엔트리 저장 |
| **런타임 에러 폴백 없음** | 🟢 경미 | HTTP 에러 → 다음 폴백 모델 |

---

## 5. 개선 방안 (언어 독립적)

### 원칙
- **키워드 매칭 사용 안 함** — 언어 종속적, 유지보수 불가
- **구조적 시그널**로 확실한 것은 판단, **LLM 분류기**로 애매한 것은 의미 파악
- 기존 4개 시그널(structural/behavioral/budget/vision)은 그대로 유지

### Phase 1: HeuristicClassifier 통합 (2시간)

기존 시그널 점수(40%) + 구조적 메시지 분석 점수(60%) 가중 평균:

```rust
// 메시지 분석 시그널 (언어 독립적)
- 메시지 길이 (문자수)
- 줄 수
- 코드블록 유무 (```)
- 파일경로 참조 (src/foo.rs 패턴)
- 기호 밀도 ({}, (), =, ; 비율)
- 질문 형태 (?로 끝남)
- 단일 문장 여부 (≤3단어, 줄바꿈 없음)
```

### Phase 2: 커스텀 룰 엔진 (2시간)

```toml
[[router.rules]]
matches = ["deploy", "production"]
tier = "high"

[[router.rules]]  
matches = ["summarize", "changelog"]
tier = "low"
```

### Phase 3: 툴 타입 시나리오 (3시간)

```rust
enum RouterScenario {
    Default,
    Background,   // haiku 급 요청
    Thinking,     // thinking 모드
    LongContext,  // 토큰 > 임계값
    WebSearch,    // web_search 툴
    Vision,       // 이미지 포함
}
```

### Phase 4: 핀 + 런타임 폴백 (2시간)

```rust
// 핀
pin: Option<RouterTier>

// 런타임 폴백
retry_on_errors: [429, 500, 502, 503, 504]
max_fallback_attempts: 3
cooldown_seconds: 60
```

### Phase 5: LLM 분류기 연결 (4시간)

애매한 구간(0.25~0.75)만 LLM 호출:
```rust
if heuristic_score > 0.25 && heuristic_score < 0.75 {
    llm_classify(input, heuristic_score).await
}
```

---

## 6. 결론

pi-model-router가 **메시지 분석** 측면에서 가장 정교하지만, 키워드 기반이라 언어 종속적이다.
CCR은 **메타데이터 기반**으로 가장 단순하지만 메시지 내용을 완전히 무시한다.
OmO는 **에이전트 중심**으로 라우팅과 에이전트가 동일하다.

**oxi의 최적 전략**:
기존 시그널 아키텍처(이미 vision, budget, structural을 갖춤)에
**언어 독립적인 구조적 메시지 분석**을 추가하고,
애매한 케이스는 **LLM 분류기**가 의미를 파악하는 하이브리드 방식.
