# 설계: TUI 출력 언어 정책 — UX 갭 해소

> 상태: 설계 v2 (구현 전 합의용, 리뷰 반영)
> 작성: 2026-06-17 (v1), 리뷰 2026-06-17 (v2)
> 선행: 기존 `Settings::output_languages` 구현 (v5, oxicode-cli/src/store/settings.rs)
> 후속: CHANGELOG.md 갱신 + AGENTS.md pitfalls 강화

## 0. 핵심 (TL;DR)

기존 `output_languages`는 **"기능은 있는데 발견되지도 끄지도 못하며 적용도 즉시 반영 안 되는"** 상태로 출시됐다. 본 설계는 다음 4가지를 바꾼다:

1. **마스터 토글 `language_policy_enabled` 추가, default false.** 사용자가 명시적으로 켜야 동작. "조용히 무시" 가능성 차단.
2. **오버레이 Esc 닫을 때 자동 적용.** 디스크 저장 + `rebuild_system_prompt()` 호출을 한 번에. `/reload` 수동 단계 제거.
3. **AGENTS.md에 의도된 비대칭을 명시.** `print`/RPC 미적용이 우연이 아니라 설계 결정임을 박제.
4. **`/settings` 슬래시 description 수정.** "Show current settings" → "Edit settings (theme, language, tools, ...)" (실제 동작은 편집 오버레이).

**한 문장 요약**: TUI 언어 정책을 "기본 OFF + 명시적 토글 + 오버레이 닫으면 즉시 적용"으로 다시 정의한다. print/RPC는 의도적으로 비대칭 — 손대지 않는다.

## 1. 배경: 현재 구현과 마주친 3가지 갭

### 1.1 현재 구현 (v5)

```
사용자 → /settings (오버레이 편집) → 디스크 저장 → /reload (수동) → 다음 턴부터 적용
                          ↓
                  output_languages HashMap<channel, lang>
                          ↓
                  language_directive() → 시스템 프롬프트 끝에 MUST 디렉티브 주입
                          ↓
                  build_compaction_instruction() → 요약기에 약한 형태의 힌트
```

위치:

| 컴포넌트 | 파일 |
|---|---|
| 데이터 정의 | `oxicode-cli/src/store/settings.rs:307` (`output_languages: HashMap<String, String>`) |
| 디렉티브 빌더 | `oxicode-cli/src/prompt/system_prompt.rs:227` (`language_directive()`) |
| TUI 소비 | `oxicode-cli/src/app/agent_session_runtime.rs:265, 275, 303, 322` |
| 라이브 갱신 | `oxicode-cli/src/app/agent_session.rs:594` (`rebuild_system_prompt()`) |
| 오버레이 UI | `oxicode-cli/src/tui/overlay/settings.rs:451–462` |
| 코어 채널 테이블 | `oxicode-cli/src/store/settings.rs:31–43` (`KNOWN_CHANNELS`) |
| 언어 코드 테이블 | `oxicode-cli/src/store/settings.rs:46–55` (`KNOWN_LANGS`, 8개: auto/en/ko/ja/zh/es/fr/de) |

### 1.2 UX 갭 3가지

| # | 갭 | 증상 |
|:-:|---|---|
| G1 | **전역 OFF 토글 부재** | 기능을 끄려면 4개 채널을 돌아다니며 각각 `auto`로 바꿔야 함. "기능을 끄고 싶다"는 의도 표현 불가. |
| G2 | **`/reload` 수동 필요** | 오버레이 닫아도 라이브 에이전트 미반영. 사용자가 두 단계 거쳐야 함. 매뉴얼 docstring도 명시. |
| G3 | **`/settings` description 오기** | `BUILTIN_SLASH_COMMANDS`에 `"Show current settings"`라고 적혀 있지만 실제 핸들러(`tui/slash.rs:161`)는 편집 오버레이를 염. 신규 사용자에게 혼란. |

부가 갭 (본 설계의 손이 닿지 않는 의도된 비대칭):

| # | 갭 | 의도 |
|:-:|---|---|
| G4 | `print`/`RPC`는 정책 무시 | TUI-only가 의도. caller가 결정. |

### 1.3 채널 추상화의 본질적 한계 (변경 안 함)

채널은 **시스템 프롬프트의 디렉티브 문자열**이지, **모델이 자기 출력을 카테고리화하는 메커니즘이 아니다.** 즉:

> "코드 주석은 영어로, 응답은 한국어로" 라고 지시해도 — 모델이 본문과 주석을 정확히 구분하지 못하면 디렉티브가 무용지물.

채널이 많을수록:
- 시스템 프롬프트 길이 증가 → "lost in the middle" 효과 증가
- 채널 경계 모호성으로 모델이 잘못 분류할 가능성 증가

따라서 **코어 채널은 최소 집합으로 유지**하고, 사용자 워크플로우별 채널은 extension map(`settings.toml`에서 사용자 추가)으로 흡수한다.

## 2. 설계 원칙

| # | 원칙 | 구현 |
|:-:|---|---|
| 1 | **TUI-only는 우연이 아니라 의도** | `print`/`RPC`는 비대칭 적용. AGENTS.md에 박제. |
| 2 | **Opt-in 기본값** | `language_policy_enabled` default `false`. 사용자가 명시적으로 켜야 동작. |
| 3 | **단일 토글 지점** | `/settings` 오버레이의 `language_policy` 항목 하나. 채널별 ON/OFF는 채널 값을 `auto`로 두는 것. |
| 4 | **닫는다 = 적용한다** | 오버레이 Esc는 디스크 저장 + `rebuild_system_prompt()` 동시 호출. `/reload` 백업 경로 유지. |
| 5 | **채널은 최소 코어 + extension map** | `KNOWN_CHANNELS` 4개 고정. 사용자가 `settings.toml`에서 임의 추가 가능. |
| 6 | **강한 기본값이지 하드 보증이 아니다** | 디렉티브는 prompt-level. 모델이 가끔 위반할 수 있음. 100% 강제는 별도 레이어 필요 (out of scope). |

## 3. 데이터 모델

### 3.1 신규 필드

```rust
// oxicode-cli/src/store/settings.rs (Settings 구조체)
/// Master switch for the TUI output language policy.
///
/// **Default: false (opt-in).** Even with non-empty `output_languages`,
/// the policy is not injected into the system prompt unless this is
/// `true`. New users start with the policy OFF; the feature is only
/// discovered through the `/settings` overlay.
#[serde(default = "default_false")]   // v6 migration: false on first load
pub language_policy_enabled: bool,
```

`default_false()` 헬퍼는 이미 정의되어 있음 (`settings.rs:340`).

### 3.2 변경되지 않는 필드

- `output_languages: HashMap<String, String>` — 그대로
- `KNOWN_CHANNELS: &[(&str, &str)]` — 4개 (`response`, `code_comment`, `documentation`, `commit_message`)
- `KNOWN_LANGS: &[(&str, &str)]` — 8개 (`auto`, `en`, `ko`, `ja`, `zh`, `es`, `fr`, `de`)

### 3.3 채널 선정 근거 (코어 4개)

LLM 에이전트가 생성하는 텍스트 카테고리 중 "같은 응답 안에서 다른 언어 컨벤션이 자연스러운가" 기준:

| 카테고리 | 채널 가치 | 채택 |
|---|:-:|---|
| 사용자 응답 | ✅ 매우 높음 | `response` |
| 코드 주석 | ✅ 높음 | `code_comment` |
| 문서 | ✅ 높음 | `documentation` |
| 커밋 메시지 | ✅ 높음 | `commit_message` |
| 에러 메시지 | ⚠️ 중간 | (채택 안 함 — `response`에 흡수) |
| 로그/디버그 출력 | ❌ 낮음 | (채택 안 함 — 디버깅 손상 위험) |
| 테스트 어설션 | ⚠️ 약함 | (채택 안 함 — 채널 경계 모호) |
| CLI/도구 출력 | ❌ 매우 낮음 | (채택 안 함 — 데이터 손상 위험) |
| 설정 파일 값 | ❌ 낮음 | (채택 안 함 — 식별자 영역) |
| 데이터 (JSON/SQL) | ❌ 매우 낮음 | (채택 안 함 — 문법 의존, 번역 불가) |

깃허브 워크플로우 특화 채널(`pr_description`, `issue_*`)은 extension map으로 흡수.

### 3.4 채널 경계 모호성 — 핵심 사례

| 사례 | 모델이 어떻게 분류하는가 |
|---|---|
| "이 함수의 시간 복잡도는 O(n)입니다" | 본문 — `code_comment`인가 `response`인가? |
| "다음과 같이 커밋하겠습니다: feat: ..." | 본문은 `response`, 내부 인용은 `commit_message`? |
| "`TypeError: undefined is not a function`" | 식별자 — 어떤 채널로도 번역 대상 아님 |
| "README.md를 업데이트했습니다. 내용은 다음과 같습니다: ..." | 본문 `response` + 인용 `documentation` 혼재 |

→ 모델이 위 사례들을 정확히 분리할 거라고 기대하지 않는다. **디렉티브는 강한 시그널이지 분류기가 아니다.**

## 4. 동작 모델

### 4.1 시그니처 변경

```rust
// oxicode-cli/src/prompt/system_prompt.rs

// before
pub fn language_directive(channels: &HashMap<String, String>) -> Option<String>

// after
pub fn language_directive(
    enabled: bool,                              // ← 신규 (마스터 게이트)
    channels: &HashMap<String, String>,
) -> Option<String> {
    if !enabled { return None; }                // ← 신규
    if channels.is_empty() { return None; }     // 기존
    // ... 기존 로직
}
```

동일 패턴이 `build_compaction_instruction()`에도 적용 — 마스터 OFF면 컴팩션 인스트럭션도 약한 형태 대신 `None`.

### 4.2 호출 사이트 변경

| 위치 | 변경 |
|---|---|
| `agent_session_runtime.rs:265` | `language_directive(settings.language_policy_enabled, &settings.output_languages)` |
| `agent_session_runtime.rs:275` | `build_compaction_instruction(settings.language_policy_enabled, &settings.output_languages)` |
| `agent_session_runtime.rs:303` | 동일 |
| `agent_session_runtime.rs:322` | 동일 |

### 4.3 in-memory / 디스크 동기화 (v2 결정: rebuild에서 fresh load)

문제: `persist_changes()`는 디스크 fresh load → 머지 → save. `rebuild_system_prompt()`는 in-memory `Arc<RwLock<Settings>>` 캐시에서 읽음. 두 경로가 어긋날 위험.

**v2 결정**: `rebuild_system_prompt()`을 호출하기 직전에 디스크에서 fresh load하여 in-memory 캐시를 교체한다. `persist_changes()`는 디스크만 저장한다.

```rust
// oxicode-cli/src/app/agent_session.rs — AgentSession::rebuild_system_prompt 확장

pub fn rebuild_system_prompt(&self) {
    // v2: 디스크 fresh load로 in-memory 캐시와 동기화
    let settings = crate::store::settings::Settings::load()
        .unwrap_or_default();
    let thinking = settings.thinking_level;
    let languages = settings.output_languages.clone();
    let enabled = settings.language_policy_enabled;
    *self.settings.write() = settings;       // ← 캐시 교체

    let prompt = crate::app::agent_session_runtime::build_system_prompt(
        thinking,
        enabled,                              // ← 신규
        &languages,
    );
    self.agent.set_system_prompt(prompt);
}
```

이 접근은 다음 장점이 있다:
1. **책임 분리** — `persist_changes` = 디스크 저장, `rebuild_system_prompt` = 디스크 fresh load → 적용.
2. **결합도 ↓** — overlay가 `AgentSession`의 mutable API를 알 필요 없음.
3. **기존 `/reload`와 일관** — `app.rs:1540`의 `Settings::load()` 패턴과 동일.
4. **결정성** — in-memory가 stale일 가능성이 원천 차단됨.

`persist_changes()`는 기존대로 디스크 저장만 수행하며, 이후 `rebuild_system_prompt()`이 자동으로 in-memory를 갱신한다.

## 5. UX 흐름

### 5.1 신규 사용자 (default OFF)

```
$ oxicode
  → TUI 시작, language_policy_enabled = false
  → 모든 응답이 모델 자연스러운 언어 (정책 없음)

> /settings
  → 오버레이에서 "── Language (TUI) ─" 섹션 보임
  → 첫 줄: Toggle "language_policy" = OFF (디폴트)
  → 그 아래 4개 Choice는 회색 disabled
  → Toggle을 ON으로 변경
  → 채널 4개 활성화됨, 각각 순환 가능
  → Esc → "Settings saved and applied." 알림 + 즉시 다음 턴부터 적용
```

### 5.2 기존 v5 사용자 (마이그레이션)

```
v5 settings.toml (output_languages에 값이 있어도)
  ↓ migrate_to_v6()
v6 settings.toml — language_policy_enabled = false (default)
  → 효과적으로 OFF가 됨 (= 정책이 무력화)
  → CHANGELOG.md에 "v0.x: TUI 언어 정책 default OFF. 기존 사용자도 마이그레이션 시 OFF로 시작" 명시
  → 사용자가 /settings에서 켜야 동작
```

### 5.3 자동 적용 흐름

```
사용자가 오버레이에서:
  - Toggle "language_policy" ON        → changed = true (인메모리 overlay 상태)
  - Choice "language.response" ko      → changed = true (인메모리 overlay 상태)
  - Choice "language.commit_message" en → changed = true (인메모리 overlay 상태)

Esc
  ├─ self.changed == true:
  │   ├─ persist_changes()             // 디스크 저장
  │   ├─ rebuild_system_prompt()       // 디스크 fresh load → in-memory 교체 → 적용
  │   └─ 알림: "Settings saved and applied." (NotificationKind::Success)
  └─ self.changed == false:
      └─ 단순 Close (no-op)
```

### 5.4 `/reload` 백업 경로 유지

오버레이 밖에서 `settings.toml`을 직접 수정한 경우(예: 에디터로 편집), `/reload` 슬래시 명령어가 동일한 효과를 제공:

```rust
// tui/slash.rs (기존, 변경 없음)
"/reload" => {
    session.rebuild_system_prompt();  // 디스크 fresh load → in-memory 갱신 → 적용
    // ...
}
```

### 5.5 OFF 동작 — 채널 설정 보존

`language_policy_enabled = false`로 토글해도 채널별 설정(`output_languages` 맵)은 **삭제되지 않고 보존된다.** 사용자가 나중에 다시 ON으로 켜면 이전에 설정해둔 채널 매핑이 그대로 적용됨.

```toml
# OFF 상태에서도 디스크에 보존됨
[output_languages]
response = "ko"
code_comment = "en"

language_policy_enabled = false   # ← 마스터 게이트만 false
```

## 6. 채널 확장 패턴 (extension map)

### 6.1 사용자 추가 예시

```toml
[output_languages]
response = "ko"
code_comment = "en"
pr_description = "ko"           # ← 사용자 추가 (코어 아님)
release_notes = "ko"            # ← 사용자 추가 (코어 아님)
```

→ `language_directive()`가 phase 2에서 `KNOWN_CHANNELS` 외 키를 알파벳 순으로 디렉티브에 자동 포함 (`prompt/system_prompt.rs:268–282`).

### 6.2 의도된 제약

- `validate_output_languages()`는 채널 키를 검증하지 **않는다.** (settings.rs:556–571) 사용자가 자유롭게 추가 가능.
- 단, `language_policy_enabled = false`면 디렉티브가 아예 주입되지 않으므로 채널 추가가 무의미. ON 상태에서만 효과.

## 7. 비대칭: print/RPC는 의도적으로 미적용

### 7.1 의도

`oxicode --print`와 RPC 모드는 **프로그래매틱/스크립터블 인터페이스**다. 언어 결정성은 caller의 책임이다. caller가 프롬프트를 직접 제어하고, 미리 번역하거나 어떤 언어로든 라우팅할 수 있다.

TUI는 **대화형 표면**이며, 이 정책이 가치를 발휘하는 자리다.

### 7.2 "fix"하지 말 것

**`oxicode-cli/src/lib.rs::build_system_prompt`에 디렉티브를 주입하지 말 것** (구현 충동이 들 수 있음). print/RPC에 정책이 필요해지면 **명시적 opt-in**(CLI flag 또는 추가 config field)을 추가하라. 암묵적으로 만들지 말 것.

### 7.3 단일 확장 지점 (참고용)

향후 caller가 opt-in을 원할 경우, 주입 지점은 단 한 곳:

```rust
// oxicode-cli/src/lib.rs::build_system_prompt (line 169)
// 현재 language_directive 호출 없음 — 의도된 비대칭
```

## 8. 마이그레이션

### 8.1 버전

`SETTINGS_VERSION: u32 = 5 → 6`

### 8.2 동작

```rust
// oxicode-cli/src/store/settings.rs::migrate (확장)

match current_version {
    5 => {
        // v5 → v6: language_policy_enabled 추가, default false.
        // #[serde(default = "default_false")]이 누락 시 자동으로 false 처리.
        // 별도 코드 불필요 (data-only migration).
        settings.version = 6;
        tracing::info!(
            "Migrated settings from version 5 to {} (language_policy_enabled defaults to OFF; \
             see docs/designs/2026-06-17-tui-language-policy.md)",
            settings.version
        );
    }
    // ... 기존 v4 → v5 경로는 그대로 유지
}
```

### 8.3 기존 사용자 영향

`output_languages`에 값을 설정해둔 v5 사용자도 v6로 마이그레이션 시 `language_policy_enabled = false`로 시작. **기존 동작(언어 정책 미적용)과 결과적으로 동일하지만, 사용자가 채널을 설정해두었다는 사실은 디스크에 보존됨.** 사용자가 `/settings`에서 토글을 켜면 즉시 이전 설정이 적용됨.

### 8.4 CHANGELOG 항목 (필수)

```markdown
## v0.x — TUI 언어 정책 default OFF로 변경

`output_languages`를 설정한 사용자는 `/settings` 오버레이에서
"language_policy" 토글을 켜야 동작합니다. 이전 버전에서는 디폴트로
ON이었으나(빈 맵이 아니면 자동 활성화), 이번 변경으로 명시적 opt-in
방식으로 전환합니다. 기존 채널별 설정은 보존됩니다.
```

## 9. 변경 파일 목록

| 파일 | 변경 |
|---|---|
| `AGENTS.md` | pitfalls 396 강화 (의도된 비대칭 + `/settings` description 함정 명시) |
| `CHANGELOG.md` | "TUI 언어 정책 default OFF" 항목 (파일 존재 확인은 구현 시) |
| `oxicode-cli/src/util/slash_commands.rs:120` | `/settings` description 수정 |
| `oxicode-cli/src/store/settings.rs` | `language_policy_enabled` 필드 + v5→v6 마이그레이션 + 테스트 6개 |
| `oxicode-cli/src/prompt/system_prompt.rs` | `language_directive(enabled, channels)` 시그니처 + 테스트 9개 (language_directive 6 + build_system_prompt 3) |
| `oxicode-cli/src/app/agent_session_runtime.rs` | 호출 사이트 4곳에 `enabled` 인자 전달 + `build_compaction_instruction` 시그니처 변경 + 테스트 4개 |
| `oxicode-cli/src/app/agent_session.rs` | `rebuild_system_prompt()`이 디스크 fresh load로 in-memory 교체 |
| `oxicode-cli/src/tui/overlay/settings.rs` | `language_policy` Toggle 추가, `SettingsItem::Choice`에 `disabled` 필드 추가, OFF 시 채널 4개 disabled (회색 + Enter/Space 차단 + "Enable language_policy first." notification), Esc 자동 rebuild, notification 메시지 |

## 10. 한계와 향후 과제

### 10.1 본 설계가 해결하지 않는 것

| 한계 | 영향 | out of scope 사유 |
|---|---|---|
| 채널 경계 모호성 | 모델이 출력을 잘못 분류할 수 있음 | prompt-level 디렉티브의 본질적 한계 |
| 100% 강제 | 도구 출력이 디렉티브 위반해도 통과 | 별도 레이어(tool output wrapping, response post-processing) 필요 |
| 채널 추가 시 lost in the middle | 채널 8개 이상 시 시스템 프롬프트가 길어져 정책이 묻힘 | 사용자가 채널을 늘리지 않을 것을 권장. extension map이 상한 역할 |
| print/RPC 비대칭 | 정책이 무시됨 | **의도된 결정** — 7장 참조 |

### 10.2 향후 과제 (별도 설계)

1. **응답 후처리 레이어**: 모델 출력 후 디렉티브 위반 패턴을 감지·교정. 정확도 트레이드오프.
2. **채널별 Confidence**: 모델이 "이건 code_comment입니다"라고 명시하면 강제력 강화 (structured output).
3. **글로벌 OFF 토글을 환경변수로도 노출**: `OXICODE_LANGUAGE_POLICY=off` 등 CI 환경 고려.

## 11. 테스트 전략

### 11.1 단위 테스트

| 파일 | 케이스 | 신규/기존 |
|---|---|---|
| `prompt/system_prompt.rs::tests` | `language_directive(enabled=false, _)` → None | **신규 (게이팅)** |
| | `language_directive(enabled=true, 빈 맵)` → None | 기존 |
| | `language_directive(enabled=true, 전부 auto)` → None | 기존 |
| | `language_directive(enabled=true, 부분 채널)` → 디렉티브 존재 | 기존 |
| | `language_directive(enabled=true, 미지 채널)` → 디렉티브에 포함 | 기존 |
| | `language_directive(enabled=true, 정렬)` → 디렉티브에 정렬된 키 | 기존 |
| | `build_system_prompt(enabled=false)` → 디렉티브 미포함 | **신규** |
| | `build_system_prompt(enabled=true, 부분 채널)` → 디렉티브 포함 | 기존 |
| | `build_system_prompt(enabled=true, 전부 auto)` → 디렉티브 미포함 | 기존 |
| `agent_session_runtime.rs::tests` | `build_compaction_instruction(enabled=false, _)` → None | **신규** |
| | `build_compaction_instruction(enabled=true, 빈 맵)` → None | 기존 |
| | `build_compaction_instruction(enabled=true, 전부 auto)` → None | 기존 |
| | `build_compaction_instruction(enabled=true, ko)` → 디렉티브 | 기존 |
| `store/settings.rs::tests` | `language_policy_enabled` 디폴트가 false | **신규** |
| | v5 → v6 마이그레이션 시 `language_policy_enabled`가 false | **신규** |
| | 라운드트립 (save & load) 시 보존 | 기존 + 신규 |

**총 16개 단위 테스트** (기존 8 + 신규 8). 모두 같은 PR에서 갱신/추가.

### 11.2 통합 테스트

- `agent_session_full.rs`: OFF 상태에서 시스템 프롬프트 끝에 디렉티브가 **없음**을 검증
- ON 상태 + 채널 2개 설정 시 디렉티브가 **있음**을 검증
- Esc 닫기 흐름은 별도 TUI 통합 테스트 작성

### 11.3 회귀 테스트 영향

기존 테스트 중 시그니처 변경 영향:

| 파일 | 영향 | 처리 |
|---|---|---|
| `prompt/system_prompt.rs` (3개) | `build_system_prompt` 호출 옵션 변경 | `enabled` 인자 추가 |
| `agent_session_runtime.rs` (2개) | `build_compaction_instruction` 호출 | `enabled` 인자 추가 |
| `store/settings.rs` (6개) | 영향 없음 (필드 추가만) | 그대로 |

**테스트는 같은 PR에서 갱신**되어야 CI 통과.

## 12. 결정 요약 (요청된 결정에 대한 답)

| # | 결정 | 답 |
|:-:|---|---|
| 1 | AGENTS.md 강화 | ✅ 8.1 + 7.3 |
| 2 | `/settings` description 수정 | ✅ "Show current settings" → "Edit settings (...)" |
| 3 | 자동 적용 (Esc → rebuild) | ✅ 5.3 |
| 4 | `language_policy_enabled` 추가 | ✅ default false (opt-in) |
| 5 | 코어 채널 4개 유지 | ✅ `response`, `code_comment`, `documentation`, `commit_message` |
| 6 | `pr_description` 코어 추가 | ❌ extension map으로 흡수 |
| 7 | 채널 평가 (깃허브 제외) | ✅ 3.3 표 — 4개 sweet spot |
| 8 | in-memory 동기화 전략 | ✅ §4.3 — rebuild에서 fresh load |
| 9 | 채널 disabled UI | ✅ §9 — `Choice::disabled` 필드 추가 |
| 10 | disabled 시 Enter 동작 | ✅ §9 — "Enable language_policy first." notification |

## 13. 열린 질문 (구현 전 합의 필요)

없음. 본 설계는 구현 직전까지 합의 가능한 모든 결정을 포함한다.

## 14. Self-review

- **Placeholder scan**: §10.1 한계 표에 "out of scope 사유" 명시. "TBD"/"TODO" 없음.
- **Internal consistency**: §4.1 시그니처 ↔ §4.2 호출 사이트 ↔ §9 변경 파일 일치. §5 UX 흐름 ↔ §4.3 동기화 메커니즘 일치. v2 결정(rebuild_fresh)이 §4.3 + §5.3 양쪽에 일관 적용.
- **Scope check**: 단일 설정 정책 변경에 집중. 향후 과제(§10.2)는 별도 설계로 분리.
- **Ambiguity check**: "마이그레이션 시 OFF"의 정확한 동작을 §8.2 코드와 §8.3 영향으로 명시. "강한 기본값이지 하드 보증이 아니다"를 §1.3 + §10.1에 반복.

### 14.1 v2 리뷰 반영 사항

리뷰에서 발견된 결함 5개를 v2에서 정정:

| ID | 결함 | 정정 |
|---|---|---|
| C1 | §4.3 `app.sync_settings_from_disk()` API 부재 | `rebuild_system_prompt`에서 fresh load하도록 변경 |
| C2 | §11.3 테스트 14개 오기 | 16개 (기존 8 + 신규 8)로 정정, 시그니처 영향 표 추가 |
| C3 | §9 `agent_session_runtime.rs::tests` 누락 | 명시적 추가 |
| M1 | in-memory 동기화 전략 미정 | §4.3 v2 결정 (rebuild_fresh) 채택 |
| M2 | disabled UI 표현 미정 | §9 `SettingsItem::Choice::disabled` 채택 |
| M3 | disabled 시 Enter 동작 미정 | §9 "Enable language_policy first." notification |
