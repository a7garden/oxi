# 세부 설계 ③ — TTSR (Time-Traveling Stream Rules)

> 상태: 설계 v1 (구현 전 합의용)
> 작성: 2026-06-19
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `export/ttsr.ts` (`TtsrManager`), `capability/rule.ts` (`Rule`), `discovery/builtin-rules/`, `prompts/system/ttsr-interrupt.md`
> 후속: M3a 구현 → CHANGELOG.md + AGENTS.md "Rules" 섹션

---

## 0. 핵심 (TL;DR)

omp의 **TTSR**은 스트리밍 중 모델 출력이 **프로젝트 룰을 위반**하면 (예: Rust에서 `Box::leak` 작성) 즉시 스트림을 **중단**하고, 룰을 **시스템 리마인더로 주입**한 뒤 **같은 지점에서 재시도**한다. "컨텍스트 세금 없는 교정" — 매 턴 룰을 프롬프트에 넣지 않고도 위반 시에만 발동.

**oxi의 핵심 자산**: `agent_loop/streaming.rs`의 토큰 처리 루프(이미 delta마다
이벤트를 처리)에 TTSR 체크를 **인라인 주입**할 수 있다. 단, **`cancel_signal`과
`retry.rs`를 직접 재사용하는 것은 의미가 다르다**:
- `cancel_signal` / `external_stop`은 **"사용자가 중단을 원함(Ctrl+C)"** → 스트림
  종료 + 루프 탈출, **재시도 없음**(`streaming.rs:83-105, 158-178`에서
  `StopReason::Aborted` 마킹 후 리턴).
- `retry.rs`(`stream_with_retry`)는 **provider 일시적 오류(429/500/timeout)**
  재시도 — 지수 백오프, 컨텍스트 수정 없음.
- TTSR가 필요한 것은 **"룰 위반 → 부분 출력 폐기 → 룰 주입 후 동일 프롬프트
  재시도"** — 위 두 메커니즘 어느 쪽과도 다른 **새 제어 흐름**이다.

따라서 TTSR는 `cancel_signal`을 **과부하하지 않고**, 스트리밍 함수에 새 반환
타입(`StreamOutcome`)을 도입하여 루프 레벨에서 interrupt + retry를 처리한다
(§2.3). 기존 `cancel_signal` / `external_stop` / `retry.rs`는 **건드리지 않는다**.

### omp가 검증한 가치
- **강제 규칙** — 코딩 컨벤션·보안 정책·아키텍처 제약을 프로젝트별로 자동 강제.
- **compaction 생존** — 인터럽트 이력이 요약에 남아 "고쳐진" 상태가 유지 (스트림 중단 시 룰이 영구 적용).
- **토큰 절약** — 룰이 항상 프롬프트에 있지 않음. 위반 시에만 주입.
- **번들 기본 룰** — omp는 Rust/TS 룰 19개 번들 (`rs-box-leak`, `ts-no-any` 등).

---

## 1. omp 메커니즘

### 1.1 Rule 구조 (`capability/rule.ts`)

frontmatter + 본문 (omp는 Cursor `.mdc`/Windsurf `.md`/Cline 형식을 정규화):

```yaml
---
description: Never use Box::leak - it intentionally leaks memory
condition: "Box::leak"                          # 정규식 매칭 (스트림 텍스트)
scope: "tool:edit(*.rs), tool:write(*.rs)"      # 발동 범위
astCondition: "Box::leak($$$)"                  # ast-grep 패턴 (**후순위**, edit/write만 — 본 로드맵은 정규식 condition만)
interruptMode: prose-only                       # never|prose-only|tool-only|always
---
본문 (룰 설명 + 대안) — 위반 시 시스템 리마인더로 주입
```

### 1.2 TtsrManager (`export/ttsr.ts`)

```
checkDelta(delta, ctx) -> Rule[]        // 소스(text/thinking/tool)별 격리 버퍼에 delta 추가 후 매칭
checkSnapshot(snapshot, ctx) -> Rule[]   // 도구가 정규화한 스냅샷으로 버퍼 교체 후 매칭
checkAstSnapshot(snapshot, ctx) -> Rule[]  // [**후순위**] ast-grep 매칭 (edit/write만, 언어 추론) — 본 로드맵 범위 외

TtsrMatchContext { source: "text"|"thinking"|"tool", filePaths?, toolName? }
InjectionRecord { lastInjectedAt: turn }   // 반복 게이팅 (같은 턴/인접 턴 중복 주입 방지)
```

- **소스별 격리 버퍼** — assistant prose / thinking / tool argument가 섞이지 않음.
- **반복 게이팅** — `messageCount` 기준. 룰 주입 후 일정 턴 동안 재발동 억제.
- **interruptMode** — 글로벌 설정 + 룰별 오버라이드.

### 1.3 인터럽트 시퀀스

```
1. 스트리밍 중 checkDelta 매치 → Rule 발견
2. 스트림 abort (omp는 자체, oxi는 `StreamOutcome::RuleInterrupt` 반환)
3. ttsr-interrupt.md 템플릿으로 시스템 메시지 주입:
   "<system-interrupt reason="rule_violation" rule="{{name}}" path="{{path}}">
    Your output was interrupted because it violated a user-defined rule.
    You MUST comply: {{content}}"
4. 같은 요청 재시도 (히스토리에 인터럽트 표식 보존)
5. InjectionRecord 갱신 → 같은 룰 즉시 재발동 억제
```

### 1.4 룰 discovery (`discovery/`)

- `.oxi/rules/*.mdc`, `.cursorrules`, `.clinerules`, `AGENTS.md` 섹션 → `Rule` 정규화.
- 번들 기본 룰 (`builtin-rules/`) — omp는 19개 (Rust 7 + TS 12). 최하위 우선순위, `ttsr.builtinRules`로 게이트.

---

## 2. oxi화 설계

### 2.1 새 포트: `RuleRegistry` (포트 13)

`oxi-sdk/src/ports/mod.rs`:

```rust
// Port 13 — RuleRegistry: TTSR + always-apply 룰 소스.

#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub content: String,                          // 본문 (시스템 리마인더로 주입)
    pub description: Option<String>,
    pub condition: Vec<regex::Regex>,             // 정규식 매칭
    pub scope: Vec<ScopeToken>,                    // text/thinking/tool:edit(*.rs)
    pub interrupt_mode: InterruptMode,
    pub globs: Vec<String>,
    pub always_apply: bool,                        // 매 턴 시스템 프롬프트에 항상 포함
    pub source: RuleSource,
}

#[derive(Debug, Clone)]
pub enum ScopeToken { Text, Thinking, Tool { name: String, globs: Vec<String> } }

#[derive(Debug, Clone, Copy)]
pub enum InterruptMode { Never, ProseOnly, ToolOnly, Always }

#[derive(Debug, Clone)]
pub enum RuleSource { BuiltinDefaults, Project, User, Omfg }   // omfg = 모델이 생성한 룰

pub trait RuleRegistry: Send + Sync + 'static {
    fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>>;
    /// 인터럽트 이력 — compaction 생존용. turn 기준 반복 게이팅.
    fn mark_injected(&self, name: &str, turn: u64);
    fn injected_records(&self) -> Vec<(String, u64)>;
    fn restore(&self, records: Vec<(String, u64)>);
}

#[derive(Default)]
pub struct NoopRuleRegistry;
impl RuleRegistry for NoopRuleRegistry {
    fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>> {
        Box::pin(async { vec![] })                                   // 룰 없음 = TTSR 비활성
    }
    // mark/restore noop
}
```

### 2.2 TtsrEngine (`oxi-agent/src/agent_loop/ttsr.rs`) — 신규

omp `TtsrManager` 포팅. **포트가 아닌 에이전트 루프 내부 컴포넌트** (스트림 훅 필요).

```rust
pub struct TtsrEngine {
    rules: Arc<dyn RuleRegistry>,
    buffers: parking_lot::RwLock<HashMap<BufferKey, String>>,   // 소스별 격리
    settings: TtsrSettings,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct BufferKey { source: MatchSource, tool_name: Option<String> }
#[derive(Clone, Copy)] enum MatchSource { Text, Thinking, Tool }

pub struct TtsrMatchContext {
    pub source: MatchSource,
    pub file_paths: Vec<String>,
    pub tool_name: Option<String>,
}

impl TtsrEngine {
    pub fn new(rules: Arc<dyn RuleRegistry>, settings: TtsrSettings) -> Self { ... }

    /// 스트리밍 delta 추가 후 매칭 룰 반환.
    pub fn check_delta(&self, delta: &str, ctx: &TtsrMatchContext) -> Vec<Rule> {
        let key = self.buffer_key(ctx);
        let mut buffers = self.buffers.write();
        let buf = buffers.entry(key).or_default();
        buf.push_str(delta);
        self.match_buffer(buf, ctx)
    }

    /// 도구 정규화 스냅샷으로 버퍼 교체 후 매칭 (matcherDigest 패턴).
    pub fn check_snapshot(&self, snapshot: &str, ctx: &TtsrMatchContext) -> Vec<Rule> { ... }

    fn match_buffer(&self, buf: &str, ctx: &TtsrMatchContext) -> Vec<Rule> {
        // 1. scope 필터 (ctx.source/tool/glob 매칭)
        // 2. interrupt_mode 필터 (settings + 룰별 오버라이드)
        // 3. condition 정규식 매칭
        // 4. 반복 게이팅 (InjectionRecord.turn 확인)
        // 매칭된 룰 반환 (호출자가 abort + inject 수행)
    }
}

pub struct TtsrSettings {
    pub enabled: bool,
    pub interrupt_mode: InterruptMode,            // 기본 ProseOnly
    pub builtin_rules: bool,
    pub max_retries_per_turn: u32,                // 무한 루프 방지
}
```

> **본 로드맵은 정규식 매칭만**: `check_ast_snapshot`(`astCondition`)은 tree-sitter/ast-grep 의존이 필요하므로 **후순위**로 미룬다. 정규식은 false positive 가능성이 있지만(예: bash 출력에 패턴이 문자열로 등장) `interruptMode=prose-only` 기본값 + `scope` 필터로 완화한다. AST 정확 매칭이 필요해지면 별도 도입.

### 2.3 스트림 인터럽트 — 새 제어 흐름 (`streaming.rs` + `mod.rs`)

> **설계 원칙**: `cancel_signal` / `external_stop` / `retry.rs`를 **건드리지
> 않는다**. 이들은 각각 "사용자 중단"과 "provider 오류 재시도"라는 명확한
> 의미를 가지며, TTSR의 "룰 위반 → 주입 → 재시도"는 **세 번째 제어 흐름**이다.
> 세 제어 흐름을 혼용하면 의미가 오염된다.

#### 2.3.1 `StreamOutcome` — 스트리밍 함수 반환 타입 확장

현재 `stream_assistant_message`는 `Result<AssistantMessage, AgentError>`를
반환한다. TTSR 인터럽트를 전달하기 위해 반환 타입을 확장한다:

```rust
/// 스트리밍 결과. TTSR 비활성 시 항상 Complete 또는 Error.
pub enum StreamOutcome {
    /// 정상 완료.
    Complete(AssistantMessage),
    /// 사용자 중단 (cancel_signal/external_stop). 기존 동작 — 루프 탈출.
    Cancelled(AssistantMessage),
    /// TTSR 룰 위반 감지. 부분 메시지 + 위반 룰.
    /// 호출자(mod.rs)가 룰 주입 후 재시도한다.
    RuleInterrupt {
        partial: AssistantMessage,   // StopReason::Aborted 마킹
        rule: Rule,
    },
    /// 스트림 오류 (기존 AgentError 래핑).
    Error(AgentError),
}
```

> **하위 호환**: TTSR 비활성 시(`ttsr_engine == None`) 스트리밍 함수는
> `Complete` / `Cancelled` / `Error`만 반환 — 기존 동작과 동일. `RuleInterrupt`는
> TTSR 활성 시에만 발생.

#### 2.3.2 스트리밍 루프 내 TTSR 체크

`streaming.rs`의 `ProviderEvent::Delta` 처리 경로에 인라인 체크 추가:

```rust
ProviderEvent::Delta(delta) => {
    accumulated.push_str(&delta.text);
    event_count += 1;

    // ── TTSR 체크 (enabled 시만; 기존 cancel 체크와 별개) ──
    if let Some(ttsr) = &ttsr_engine {
        let ctx = TtsrMatchContext {
            source: MatchSource::Text,
            file_paths: active_file_paths.clone(),
            tool_name: None,
        };
        if let Some(violation) = ttsr.check_delta(&delta.text, &ctx).into_iter().next() {
            // 부분 메시지를 Aborted로 마킹하여 컨텍스트에 유지
            // (모델이 재시도 시 자신이 하던 작업을 본다 → 동일 위반 방지)
            let mut partial = build_partial_message(&accumulated);
            partial.stop_reason = StopReason::Aborted;
            ttsr.rules.mark_injected(&violation.name, current_turn);
            return StreamOutcome::RuleInterrupt { partial, rule: violation };
        }
    }
    // ── 기존 정규 토큰 출력 (변경 없음) ──
    emit(AgentEvent::Delta { text: delta.text, ... });
}
```

> **주의**: `check_delta`는 `parking_lot::RwLock` write를 잡는다 — 짧은 메모리
> 연산이므로 `.await` 없이 동기 처리. `streaming.rs`의 기존 `is_cancelled()`
> 체크(`cancel_signal` 기반)는 그대로 유지 — TTSR 체크와 독립적으로 동작.

#### 2.3.3 에이전트 루프에서의 interrupt 처리 (`mod.rs`)

```rust
// agent_loop/mod.rs 의 메인 루프
loop {
    let outcome = stream_assistant_message(...).await;

    match outcome {
        StreamOutcome::Complete(msg) => {
            // 기존: tool call 처리 또는 루프 종료
            ttsr_retry_count = 0;  // 턴 카운터 리셋
            ... // 기존 로직
        }

        StreamOutcome::Cancelled(msg) => {
            // 기존: 사용자 중단 → 루프 탈출
            break;
        }

        StreamOutcome::RuleInterrupt { partial, rule } => {
            // ── TTSR 전용 제어 흜름 (새) ──
            // 1. 무한 루프 가드
            ttsr_retry_count += 1;
            if ttsr_retry_count > ttsr_settings.max_retries_per_turn {
                emit(AgentEvent::Error {
                    message: format!(
                        "Rule '{}' violated {} times in one turn; giving up. \
                         Please review the rule or your approach.",
                        rule.name, ttsr_retry_count
                    ),
                    session_id: ...,
                });
                break;
            }

            // 2. 부분 메시지를 컨텍스트에 추가 (모델이 중단 지점을 인지)
            messages.push(Message::Assistant(partial));

            // 3. 인터럽트 메시지 주입 (§2.4 템플릿)
            let interrupt_msg = render_interrupt(&rule, &active_path);
            messages.push(Message::System(interrupt_msg));

            // 4. 재시도 — 같은 루프의 다음 반복으로 (새 API 호출)
            //    cancel_signal 건드리지 않음, external_stop 건드리지 않음
            emit(AgentEvent::TtsrInterrupt {
                rule_name: rule.name.clone(),
                attempt: ttsr_retry_count,
                session_id: ...,
            });
            continue;  // 루프의 다음 반복 → 동일 context + 주입된 룰로 재스트리밍
        }

        StreamOutcome::Error(e) => {
            // 기존: 오류 처리
            ...
        }
    }
}
```

> **핵심**: `continue`로 루프를 다시 도는 것은 **기존 provider-error retry**
> (`retry.rs`)와 다르다. `retry.rs`는 같은 요청을 백오프 후 재전송하지만,
> TTSR retry는 **컨텍스트에 메시지 2개(부분 출력 + 룰 주입)를 추가한 새 요청**이다.
> 이것이 별도 제어 흐름이 필요한 이유다.

### 2.4 인터럽트 템플릿 (`oxi-agent/src/prompts/ttsr-interrupt.md`)

omp `ttsr-interrupt.md` 번역:

```
<system-interrupt reason="rule_violation" rule="{{name}}" path="{{path}}">
Your output was interrupted because it violated a project rule.
This is the coding agent enforcing project rules — comply with the following:

{{content}}
</system-interrupt>
```

### 2.5 compaction 생존 (`compaction.rs`)

인터럽트 이력이 요약 시 손실되면, 같은 룰이 compaction 후 재위반 → 무한 재시도 가능. omp는 인터럽트 표식을 요약에 보존.

`compaction.rs`에 훅 추가:
```rust
// 요약 프롬프트에 "이미 적용된 룰" 목록 포함
let injected_rules = rule_registry.injected_records();
if !injected_rules.is_empty() {
    summary_instruction.push_str(&format!(
        "\n\n다음 룰들이 이 세션에서 위반되어 이미 교정되었습니다 (재위반 금지):\n{}",
        injected_rules.iter().map(|(n, _)| format!("- {}", n)).collect::<Vec<_>>().join("\n")
    ));
}
```

### 2.6 룰 discovery (`oxi-cli/src/discovery/rules.rs`)

omp `discovery/` 포팅:
- `.oxi/rules/*.mdc` — frontmatter YAML + 본문.
- `.cursorrules`, `.clinerules` — 레거시 형식 정규화.
- `AGENTS.md`의 "Rules" 섹션 (있다면).
- 번들 기본 룰 — omp 19개 중 Rust 관련 7개를 oxi 컨텍스트로 이식 (`rs-box-leak`, `rs-parking-lot`, `rs-result-type`, `rs-lazylock`, `rs-future-prelude`, `rs-match-ergonomics` 등은 oxi AGENTS.md 관례와 정합).

### 2.7 ToolContext / AgentConfig 확장

```rust
pub struct AgentConfig {
    // ...
    pub ttsr_settings: Option<TtsrSettings>,         // None = TTSR 비활성
}

pub struct AgentState {
    // ... (기존 필드 전부 변경 없음)
    pub ttsr_engine: Option<Arc<TtsrEngine>>,        // 신규 — None = TTSR 비활성
    pub ttsr_retry_count: u32,                       // 신규 — 턴별 재시도 카운터 (턴 시작 시 0 리셋)
    // cancel_signal / external_stop: 기존 그대로 — TTSR가 건드리지 않음
}

---

## 3. 설정 & 롤아웃

```rust
// settings.rs
pub struct Settings {
    pub ttsr_enabled: bool,                          // 기본 false (점진적 강화)
    pub ttsr_interrupt_mode: InterruptMode,          // 기본 ProseOnly
    pub ttsr_builtin_rules: bool,                    // 기본 true (enabled 시)
}
```

| 단계 | ttsr_enabled 기본 | 비고 |
|---|:-:|---|
| M3a.0 | false | 구현만. 개발자가 설정으로 테스트. |
| M3a.1 | false + 옵션 | 얼리 어답터. |
| M3a.2 | (데이터 후) | 안정성 검증 후 true 전환 검토. |

> **무한 루프 방지**: `max_retries_per_turn` (기본 3). 같은 턴에 같은 룰이 3회 발동하면 더 이상 인터럽트하지 않고 사용자에게 알림.

---

## 4. 의존성 & 마일스톤 (M3a)

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| M3a.1 | `RuleRegistry` 포트 + `Noop` + `Rule`/`ScopeToken`/`InterruptMode` 타입 | — |
| M3a.2 | `TtsrEngine` (정규식 매칭, 소스별 버퍼, 반복 게이팅) | M3a.1 |
| M3a.3 | `StreamOutcome` 반환 타입 + 스트리밍 루프 TTSR 체크 + mod.rs interrupt/retry 제어 흐름 | M3a.2 |
| M3a.4 | `ttsr-interrupt.md` 템플릿 + compaction 생존 훅 | M3a.3 |
| M3a.5 | `discovery/rules.rs` (`.oxi/rules/*.mdc` + 레거시 정규화) | M3a.1 |
| M3a.6 | 번들 기본 룰 (Rust 7개 이식) | M3a.5 |
| M3a.7 | settings (`ttsr_enabled` 등) + 시스템 프롬프트 | M3a.6 |

> **M1과 병렬 가능**: TTSR은 agent_loop만 건드리고 edit/read 도구는 건드리지 않음. ① Hashline과 독립.

---

## 5. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| `interruptMode` 기본값 | 🟡 `ProseOnly` 제안 | 도구 출력(특히 bash echo)의 false positive 회피. 룰별 오버라이드 허용. **도입 전 측정 권장**: prose-only + scope 필터로 실제 false positive율을 omp 룰 7개로 벤치마크 (코드 펜스 내 패턴 설명 등 경계 케이스 포함). 측정 결과가 부적절하면 AST 매칭(M3a 이후)으로 전환 검토. |
| `cancel_signal` 과부하 위험 | 🟢 **해결됨** | 리뷰에서 확인: `cancel_signal`은 "사용자 중단 → 루프 탈출" 의미. TTSR는 별도 `StreamOutcome::RuleInterrupt` 반환 타입으로 분리 (§2.3). 기존 `cancel_signal`/`retry.rs` 건드리지 않음. |
| 중단 토큰 비용 (제공자별 과금) | 🔴 확인 필요 | 중단된 응답의 부분 토큰이 과금되는지. omp는 감수. 주요 제공자 정책 조사 |
| astCondition (ast-grep) | 🟢 후순위 (본 로드맵 범위 외) | 정규식은 false positive 가능. AST는 정확하지만 tree-sitter 의존. 본 로드맵 후 별도 검토 |
| 무한 재시도 | 🟢 `max_retries_per_turn` | 같은 룰 3회 시 사용자 알림 |
| 룰 임포트 다중 포맷 | 🟢 `.oxi/rules/*.mdc` 우선 | Cursor/Cline/Copilot는 후속 |
| 번들 룰과 oxi AGENTS.md 관례 충돌 | 🟡 검토 | `rs-parking-lot` 등은 oxi AGENTS.md와 일치 — 검증 후 번들 |
| omfg (모델 생성 룰) | 🔴 별도 | omp는 모델이 룰을 생성/수정. oxi는 M3a 범위 외 (도구 추가 필요) |

---

## 6. 부록: omp → oxi 매핑

| omp 파일 | oxi 위치 |
|---|---|
| `export/ttsr.ts` (`TtsrManager`) | `oxi-agent/src/agent_loop/ttsr.rs` (`TtsrEngine`) |
| `capability/rule.ts` (`Rule`) | `oxi-sdk/src/ports/mod.rs` (Rule 타입) |
| `capability/rule-buckets.ts` | `oxi-cli/src/discovery/rules.rs` |
| `discovery/builtin-rules/*.md` | `oxi-cli/src/discovery/builtin_rules/` |
| `discovery/helpers.ts` | `oxi-cli/src/discovery/rules.rs` |
| `prompts/system/ttsr-interrupt.md` | `oxi-agent/src/prompts/ttsr-interrupt.md` |
| `modes/controllers/omfg-rule.ts` | (별도 — 모델 생성 룰, 범위 외) |
