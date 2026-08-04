# Claude Code 호환 훅 시스템 — Design

> **Tier 1 — Architectural design.** 대상: SDK port layer + AgentBuilder middleware +
> cli settings + cli lifecycle call-sites.
> 배경: oxicode는 Rust 수준의 `AgentHooks`(`oxicode-agent`)를 가지고 있지만, 사용자가
> 설정 파일에서 이벤트→셸 커맨드를 매핑하는 **Claude Code/omp 스타일의 사용자 훅**은 없다.

## Problem

Claude Code / omp는 사용자가 `settings.json`에 이벤트(`PreToolUse`, `PostToolUse`,
`Stop`, `SessionStart` 등)와 셸 커맨드를 매핑하는 **사용자 설정 훅**을 제공한다. oxicode에는
이것이 없다. oxicode는 이미 `AgentHooks`(`oxicode-agent/src/config.rs`)라는 Rust 수준의
훅 슬롯을 가지고 있지만, 이는 **프로그래밍 훅**이지 사용자가 설정 파일로 꽂는
**이벤트→커맨드 설정 훅**이 아니다.

본 설계는 Claude Code 호환의 **사용자 설정 훅 시스템**을 oxicode에 도입한다. 단, oxicode가
**SDK를 통해 다른 제품(oxios)에 노출되는** 아키텍처이므로, 훅 실행 엔진과 계약을 SDK 포트로
올려 모든 제품이 재사용할 수 있게 한다.

## Decisions (approved during brainstorming)

| 결정 | 선택 | 근거 |
|---|---|---|
| 이벤트 범위 | **Claude Code 호환 7개 전체** | PreToolUse, PostToolUse, Stop, SubagentStop, SessionStart, SessionEnd, Notification |
| IO 계약 | **Claude Code 호환** (stdin JSON + exit code) | exit 2 = block, stdout JSON = 결과 수정. 스크립트 재사용 가능 |
| 실패 정책 | **Fail-open** (Claude Code 기본) | nonzero exit ≠ 2 / 타임아웃 시 에러 로그만, exit 2만 차단 |
| 프로젝트 훅 보안 | **첫 실행 승인 게이트** | 프로젝트 `.oxicode/settings.toml` 훅은 첫 실행 시 승인, `~/.oxicode`에 캐싱. 글로벌은 항상 신뢰 |
| 아키텍처 | **B+: SDK-native via `HookRunner` port** | 엔진+계약을 SDK 포트로, cli는 설정 로딩+생명주기 발화만. oxios 재사용 가능 |
| matcher 문법 | **Glob** (`bash\|write`) | OR 표현 가능, ReDoS 위험 없음, Claude Code 호환 |

## Architecture (3 layers)

```
oxicode-sdk  (계약 + 엔진 + 도구 훅 자동 와이어링)
├── ports/mod.rs ............ PortRegistry 에 hooks 필드 추가 (port #16)
├── ports/hooks.rs .......... HookEvent, HookContext, HookOutcome, HookSpec, HookRunner trait + NoopHookRunner
├── ports/fs/hook_runner.rs . CommandHookRunner (셸 커맨드 실행 엔진 — 레퍼런스 구현)
├── ports/inmem/hook.rs ..... InMemoryHookRunner (테스트/헤드리스용)
├── middleware/hook.rs ...... HookMiddleware (Pre/PostToolUse → 기존 MiddlewarePipeline)
└── agent_builder.rs ........ with_port_hooks() — ports.hooks 를 HookMiddleware 로 compose

oxicode-cli  (설정 로딩 + 생명주기 발화 지점)
├── store/settings.rs ....... [[hooks]] 스키마 + HookConfig
├── store/hook_approval.rs .. 첫 실행 승인 게이트 + ~/.oxicode/hooks_approved.toml 캐시
├── bootstrap.rs ............ settings → CommandHookRunner 빌드 → with_port_hooks()
└── 생명주기 발화:
    ├── app/agent_session.rs .... SessionStart (세션 생성) / SessionEnd (teardown)
    ├── subagent 툴 결과 ........ SubagentStop
    └── 권한요청/ask 흐름 ........ Notification

oxios  (자체 생명주기에서 동일 HookRunner 호출 + 자체 훅 스펙)
```

**핵심 통찰:** SDK에 이미 **미들웨어 파이프라인 → `build_hooks` → 단일 `set_hooks`** 경로가
있다(`agent_builder.rs:558-611`). 이 파이프라인은 `before_tool_call`/`after_tool_call`과
거부 단축회로(`BeforeToolCallResult { block: true }`)를 이미 처리한다. 따라서 Pre/PostToolUse는
**`HookMiddleware` 하나를 파이프라인에 추가**하는 것만으로 기존 경로를 그대로 탄다 —
oxicode-agent 수정 0.

## Core Types (SDK `ports/hooks.rs`)

```rust
/// 훅이 발화하는 7개 이벤트 (Claude Code 호환).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    Stop,
    SubagentStop,
    SessionStart,
    SessionEnd,
    Notification,
}

/// stdin JSON 으로 직렬화되어 스크립트에 전달되는 컨텍스트.
/// Claude Code 페이로드 필드와 호환되도록 설계.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    pub event: HookEvent,
    pub tool_name: Option<String>,
    pub tool_args: Option<serde_json::Value>,
    pub tool_result: Option<String>,
    pub session_id: Option<String>,
    pub session_cwd: Option<PathBuf>,
    /// Claude Code 호환을 위한 원본 페이로드 (확장용).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// 훅 실행 결과. `block` 은 exit code 2 에 해당하며, **"후킹된 기본 동작을 차단"**
/// 의미로 이벤트에 관계없이 통일된다:
/// - PreToolUse → block = 도구 실행 차단
/// - Stop → block = 정지 차단 (에이전트 계속 실행)
/// - 그 외 이벤트 → block 은 의미 없음 (알림 전용)
#[derive(Debug, Clone, Default)]
pub struct HookOutcome {
    /// exit code 2 → true. 후킹된 기본 동작(pre-tool / stop)을 차단.
    pub block: bool,
    pub reason: Option<String>,
    /// PostToolUse 에서 도구 결과를 이 값으로 치환.
    pub override_content: Option<String>,
}

/// settings 의 `[[hooks]]` 한 줄에 대응하는 훅 정의.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    pub event: HookEvent,
    /// 툴 이름 glob 매칭 (예: "bash|write"). 생략 시 해당 이벤트 전체 매칭.
    #[serde(default)]
    pub matcher: Option<String>,
    /// 실행할 셸 명령 (sh -c).
    pub command: String,
    /// 타임아웃 (초). 미지정 시 기본값(60s).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// 포트 #16: 훅 실행 계약. SDK 가 trait + noop 정의, 제품이 구현.
#[async_trait]
pub trait HookRunner: Send + Sync + 'static {
    /// 해당 이벤트에 매칭되는 모든 훅을 실행하고 통합 결과 반환.
    async fn run(&self, event: HookEvent, ctx: &HookContext) -> HookOutcome;
}
```

## CommandHookRunner (SDK `ports/fs/hook_runner.rs`)

제품 비의존적 셸 커맨드 실행 엔진 — SDK 레퍼런스 구현. cli 와 oxios 모두 재사용.

```rust
pub struct CommandHookRunner {
    specs: Vec<HookSpec>,
    // matcher 컴파일 캐시. 각 spec 의 matcher("bash|write")를 pipe 로 분할해
    // 각각 globset::Glob 으로 컴파일, 하나의 globset::GlobSet 으로 묶음.
    matchers: Vec<HookSpecMatcher>,
}

impl CommandHookRunner {
    /// `matcher` 가 None 이면 전체 매칭(wildcard). `"bash|write"` 는 pipe 분할 →
    /// 2개의 exact glob. 컴파일 실패(잘못된 glob 문법) 시 `HookConfigError`.
    pub fn new(specs: Vec<HookSpec>) -> Result<Self, HookConfigError> { /* globset 컴파일 */ }
}

#[async_trait]
impl HookRunner for CommandHookRunner {
    async fn run(&self, event: HookEvent, ctx: &HookContext) -> HookOutcome {
        // 1. event + matcher(ctx.tool_name) 에 매칭되는 spec 들 수집
        // 2. 매칭된 훅들을 순차 실행:
        //    - stdin 에 ctx 를 JSON 직렬화하여 pipe
        //    - sh -c "<command>"  (shell=true)
        //    - timeout 까지 대기
        //    - exit code:
        //        0   → 통과
        //        2   → block (PreToolUse) — 즉시 반환, 이후 훅 스킵
        //        _   → fail-open: 로깅 후 통과
        //    - stdout(JSON) 파싱 → override_content / reason 등 추출
        // 3. 모든 훅의 결과를 merge (block 이 하나라도 true 면 block)
    }
}
```

**실행 규칙:**
- `tokio::process::Command::new("sh").arg("-c").arg(&spec.command)` — 셸을 통해 실행.
- stdin = `serde_json::to_string(&ctx)`.
- 환경변수: `OXICODE_HOOK_EVENT`, `OXICODE_HOOK_TOOL_NAME`, `OXICODE_HOOK_SESSION_ID` 추가 (편의).
- 타임아웃: `tokio::time::timeout`. 미지정 기본 60s. 초과 시 kill + fail-open 로깅.
- 매칭된 훅이 여러 개면 **순차 실행**. 하나라도 block=true 면 즉시 중단 + block 반환.

## Event → Call-site Mapping

| 이벤트 | 메커니즘 | 호출 지점 | 차단/수정 |
|---|---|---|---|
| **PreToolUse** | `HookMiddleware` → `before_tool_call` | 도구 실행 직전 (`tool_exec.rs`) | ✅ exit 2 → `BeforeToolCallResult { block: true }` |
| **PostToolUse** | `HookMiddleware` → `after_tool_call` | 도구 실행 직후 | 결과 수정 (`override_content`) |
| **Stop** | `HookRunner.run` in should_stop 경로 | 에이전트 정지 결정 직전 | ✅ exit 2 = `block` → 정지 차단(계속 실행) |
| **SubagentStop** | `HookRunner.run` | subagent 툴 결과 처리 | — |
| **SessionStart** | `HookRunner.run` | cli 세션 생성 (`App::from_oxicode` / `AgentSession`) | — |
| **SessionEnd** | `HookRunner.run` | cli 세션 teardown | — |
| **Notification** | `HookRunner.run` | 권한요청 / `ask` 흐름 | — |

**도구 훅(Pre/PostToolUse) 자동 와이어링:** `AgentBuilder::with_port_hooks()`가
`oxicode.ports().hooks`(`HookRunner`)를 `HookMiddleware`로 감싸 `MiddlewarePipeline`에
추가. 기존 `build_hooks(pipeline, ...)` → `set_hooks` 경로를 그대로 타므로 audit/authorizer
미들웨어와 충돌 없이 compose 됨.

**세션/생명주기 훅:** 포트(`HookRunner`)는 SDK 가 정의하지만, **발화 지점은 각 제품이 소유**.
이는 포트 모델의 일관된 패턴 — SDK 는 계약을 정의하고 제품은 자기 생명주기에서 호출.
- cli: `App::from_oxicode`/`AgentSession` 생성 시 `SessionStart`, teardown 시 `SessionEnd`.
- oxios: 자체 세션 모델에서 동일 포트 호출.

**Stop / SubagentStop (설계에서 가장 얇은 부분):**
- `Stop`: `AgentHooks::should_stop_after_turn` 슬롯이 있으나, 이 슬롯은 이미 cli가
  should_stop_flag와 steering/follow-up을 위해 점유 중(`agent_session.rs:811`). HookRunner를
  기존 should_stop 클로저에 **체인**하여 통합 — HookRunner.run(Stop,...)이 `block: true`
  (exit 2)면 **정지를 차단** (에이전트 계속 실행). Claude Code 의 Stop 훅(exit 2 = don't stop)과 동일.
- `SubagentStop`: subagent 툴 완료 = 해당 툴의 `after_tool_call` 이벤트. 단순 매핑은
  `tool_name == "subagent"` 인 PostToolUse 와 동일. 별도 이벤트로 구분하려면 HookMiddleware가
  tool_name 기반으로 SubagentStop 도 발화하도록 분기. **구현 시 after_tool_call 경로에서
  tool_name == "subagent" 면 SubagentStop 추가 발화** 로 단순화.

## Config Schema (cli `store/settings.rs`)

```toml
# ~/.oxicode/settings.toml  (글로벌 — 항상 신뢰)
# 또는 .oxicode/settings.toml (프로젝트 — 첫 실행 승인 필요)

[[hooks]]
event = "PreToolUse"          # HookEvent (PascalCase)
matcher = "bash|write"        # 툴 이름 glob. 생략 = 전체 매칭
command = "echo pre >> /tmp/hook.log"
timeout_secs = 10

[[hooks]]
event = "SessionStart"
command = "notify-send 'oxicode started'"
```

```rust
// store/settings.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
}
```

`Settings` 구조체에 `#[serde(default)] pub hooks: Vec<HookSpec>` 추가 (serde-default,
version bump 불필요 — serde 가 unknown 키 무시 + default 로 backward-compat).

## Security — 첫 실행 승인 게이트 (cli `store/hook_approval.rs`)

프로젝트 `.oxicode/settings.toml` 훅은 저장소에 커밋되므로, 남의 저장소 클론 시 임의 명령
실행 위험. Claude Code 식 승인 게이트:

1. **글로벌 훅** (`~/.oxicode/settings.toml`): 항상 신뢰 (사용자 자신의 환경).
2. **프로젝트 훅** (`.oxicode/settings.toml`): 첫 실행 시 사용자에게 승인 프롬프트.
   - 승인 내역을 `~/.oxicode/hooks_approved.toml` 에 저장:
     ```toml
     ["<repo_abs_path>"]
     settings_hash = "<sha256 of .oxicode/settings.toml>"
     approved_at = "2026-08-04T00:00:00Z"
     ```
   - 다음 실행부터 settings_hash 가 일치하면 자동 통과.
   - hash 불일치(설정 변경) 시 재승인 요청.
3. **승인 거부 시**: 해당 프로젝트 훅 전체 비활성화 (에러 로깅, 비정상 종료 아님).

비인터랙티브 모드(print/RPC): 프로젝트 훅은 승인 캐시에 있을 때만 실행, 없으면 skip +
경고 (프롬프트 불가하므로).

## Failure Policy — Fail-open

| 상황 | 동작 |
|---|---|
| exit code 0 | 통과 |
| exit code **2** | **block** (PreToolUse) — 즉시 후속 훅 스킵, `HookOutcome { block: true }` |
| exit code ≠ 0, 2 | fail-open: `tracing::warn!` 로깅 후 통과 |
| 타임아웃 | 프로세스 kill, fail-open 로깅 후 통과 |
| 스크립트 없음/실행 불가 | fail-open 로깅 후 통과 |
| stdout JSON 파싱 실패 | stdout 을 raw content 로 간주하지 않고 무시 (exit code 만 신뢰) |

## Files to Add / Modify

| File | Change | Layer |
|---|---|---|
| `oxicode-sdk/src/ports/hooks.rs` | **NEW** — HookEvent, HookContext, HookOutcome, HookSpec, HookRunner trait + NoopHookRunner | SDK |
| `oxicode-sdk/src/ports/mod.rs` | PortRegistry 에 `hooks: Arc<dyn HookRunner>` 필드 + with_port_hooks 헬퍼. port #16 | SDK |
| `oxicode-sdk/src/ports/fs/hook_runner.rs` | **NEW** — CommandHookRunner (셸 커맨드 엔진) | SDK |
| `oxicode-sdk/src/ports/inmem/hook.rs` | **NEW** — InMemoryHookRunner (테스트용) | SDK |
| `oxicode-sdk/src/middleware/hook.rs` | **NEW** — HookMiddleware (Pre/PostToolUse → pipeline) | SDK |
| `oxicode-sdk/src/middleware/mod.rs` | HookMiddleware 등록 | SDK |
| `oxicode-sdk/src/agent_builder.rs` | `with_port_hooks()` — ports.hooks → HookMiddleware → pipeline compose | SDK |
| `oxicode-sdk/src/lib.rs` | hooks 모듈 re-export | SDK |
| `oxicode-cli/src/store/settings.rs` | `[[hooks]]` 스키마 + HookConfig, Settings 필드 | cli |
| `oxicode-cli/src/store/hook_approval.rs` | **NEW** — 승인 게이트 + 캐시 | cli |
| `oxicode-cli/src/bootstrap.rs` | settings → CommandHookRunner 빌드 → with_port_hooks(). SessionStart 발화 | cli |
| `oxicode-cli/src/app/agent_session.rs` | SessionStart/SessionEnd 발화, should_stop 에 HookRunner 체인 | cli |
| `oxicode-cli/src/store/mod.rs` | hook_approval 모듈 등록 | cli |

**oxicode-agent 수정: 없음.** (기존 AgentHooks 슬롯 + 미들웨어 파이프라인 재사용)

## Acceptance Criteria

1. `~/.oxicode/settings.toml` 의 `[[hooks]]` 가 정의한 대로 셸 커맨드가 실행된다.
2. `PreToolUse` 훅이 exit 2 반환 시 해당 도구 실행이 차단되고, reason 이 에이전트에게
   도구 에러로 전달된다.
3. `PostToolUse` 훅이 stdout JSON 으로 `override_content` 반환 시 도구 결과가 치환된다.
4. 훅 실패(nonzero ≠ 2)/타임아웃 시 fail-open — 도구는 정상 실행된다.
5. 프로젝트 `.oxicode/settings.toml` 훅은 첫 실행 시 승인 프롬프트가 뜨고, 승인 후
   `~/.oxicode/hooks_approved.toml` 에 캐싱된다.
6. settings 변경 시 hash 불일치로 재승인 요청.
7. `SessionStart`/`SessionEnd` 가 각각 세션 생성/종료 시 실행된다.
8. 글로벌 훅은 승인 없이 항상 실행된다.
9. `oxicode-sdk` 의 `HookRunner` 포트가 `PortRegistry` 에 등록되고, oxios 가 자체 생명주기에서
   호출할 수 있다 (with_port_hooks 또는 직접 ports.hooks 접근).
10. oxicode-agent crate 변경 없이 빌드 + 기존 테스트 통과.

## Test Strategy

- **CommandHookRunner 단위 (SDK):**
  - stdin JSON 직렬화가 Claude Code 페이로드와 호환되는지.
  - exit code 0/1/2 매핑 (통과/fail-open/block).
  - 타임아웃 처리 (kill + fail-open).
  - matcher glob 매칭 (`bash`, `bash|write`, 생략=전체, 비매칭).
  - 다중 매칭 훅 순차 실행 + block 즉시 중단.
- **HookMiddleware (SDK):**
  - block 단축회로가 `BeforeToolCallResult { block: true }` 로 변환되는지.
  - override_content 가 after_tool_call 결과에 반영되는지.
  - audit/authorizer 미들웨어와 동시 compose 시 순서 보존.
- **승인 게이트 (cli):**
  - 캐시 hit 시 자동 통과, hash 불일치 시 재승인.
  - 글로벌 훅은 승인 없이 실행.
  - 비인터랙티브 모드에서 미승인 프로젝트 훅 skip + 경고.
- **통합 (cli):**
  - 임시 settings + MockProvider 로 PreToolUse 차단 end-to-end.
  - SessionStart/SessionEnd 발화 검증.
  - should_stop 에 HookRunner 체인 검증.

## Out of Scope (v2+)

- **Webhook/remote 훅** — 로컬 셸 커맨드만. 원격 트리거는 별도 설계.
- **훅 스크립트 마켓플레이스** — ClawHub 스타일 공유는 별도.
- **필터 매칭 고급화** — tool_name glob 외에 args 기반 매칭은 v2.
- **`prepareNextTurn` / `transformContext` 훅** — pi-mono 에 있으나 별도 이슈.
- **동적 API 키 갱신 훅** (`getApiKey`) — 별도 이슈.
