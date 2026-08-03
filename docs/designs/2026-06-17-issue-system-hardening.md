# 설계: 로컬 issue 시스템 경화 (hardening) — 완성본

> 상태: 설계 v2 (구현 합의용 최종안)
> 작성: 2026-06-17 (v1 리뷰), 2026-06-17 (v2 — 결함 #13 발견·반영)
> 선행: `oxicode-cli/src/store/issues.rs` (1323줄), `oxicode-cli/src/tools/issue_tool.rs` (419줄)
> 후속: CHANGELOG.md + AGENTS.md Pitfalls 갱신 + 본 문서 §11 체크리스트

## 0. 핵심 (TL;DR)

issue 시스템의 **동시성 코어**(flock liveness + content-hash CAS + per-file queue)는
정답이다. **하지만 그 코어를 무력화하는 결함 하나(#13)를 v2에서 발견했다** — 에이전트가
실행될 때 `ToolContext.session_id`가 항상 `None`이라, `start`/`close`가 **빈 문자열을
owner로 기록**하고, 결과적으로 **소유권/락 기능이 에이전트 경로에서 완전히 우회**된다.
이게 v1에서 놓친 가장 심각한 결함이다.

본 설계는 13개 결함을 **5개 phase**로 나눠 해소한다. **P0(#13)가 다른 모든 것보다 우선**이다.

| Phase | 결함 | 한 줄 요약 |
|:-:|---|---|
| **P0** | 🟥🟥 **#13** | 에이전트 session 식별자 누락 수정 — `AgentConfig.session_id` + 프로세스 단일 liveness lock으로 소유권 복원 |
| **P1** | 🟥 #1 | `atomic_write` temp 파일 UUID 접미 → PID 재활용 충돌 제거 (issues + session 동시 수리) |
| **P2** | 🟥 #2 🟧 #3 #4 🟨 #9 #12 | CAS 자동 재시도 + `IssuePatch`(**소유권 검사 유지**) + `reopen` 액션 + no-op 쓰기 감지 |
| **P3** | 🟧 #5 #6 #7 | 스키마 description 정교화 + 본문/라벨 크기 상한 + github readOnly 명시 |
| **P4** | 🟨 #8 #10 #11 | `.alive/` orphan 수거(age-gated) + `top_free_priority` + unsafe flock helper 중앙화 |

**세 가지 확정 원칙**:

1. **코어는 건드리지 않는다.** flock + CAS + per-file queue 3계층은 그대로. 외피만.
2. **엄격함은 저장소(store), 회복은 도구(tool).** store는 `Conflict`를 원시 그대로 반환.
   도구만 bounded retry로 감싼다. TUI 패널은 conflict를 사용자에게 노출(기존 의도 유지).
3. **기존 동작/포맷 보존.** 소유권 정책(업데이트 시 assignee 한정)은 **유지**한다(v1의
   GitHub-시맨틱 완화 제안은 **철회** — 그건 hardening이 아니라 재설계). 파일 포맷·기존
   action·에러 변형 전부 유지. 새 것은 추가만.

---

## 1. 배경

### 1.1 현재 구조 (요약)

```
agent ── issue tool ──┐   (ctx.session_id 로 owner 식별)
TUI panel ────────────┼──► FileIssueStore (Arc<RwLock<Inner>>)
oxicode issue (CLI) ──────┘        │
                               ├─ .oxicode/issues/*.md  (마크다웝 + YAML frontmatter)
                               └─ .oxicode/issues/.alive/<sid>  (flock liveness)

동시성 3계층:
  L1  flock(2)        ─ 프로세스 사망 자동 감지 (kill -9/crash 안전)
  L2  content_hash    ─ CAS (DefaultHasher, edit 도구와 동일)
  L3  per-file queue  ─ in-process 직렬화 (file_mutation_queue)
```

### 1.2 결함 #13 — 핵심 발견 (v2 신규)

**증명 코드 경로**:

```
oxicode-agent/src/agent.rs:496  (그리고 :849)
    let loop_config = AgentLoopConfig {
        ...
        session_id: None,          // ◄── 항상 None (하드코딩)
        ...
    };
            │
            ▼
agent_loop/mod.rs:213-216
    ToolContext { ..., session_id: self.session_id.clone() }   // → None
            │
            ▼
issue_tool.rs:execute
    let session = ctx.session_id.clone().unwrap_or_default();  // → "" (빈 문자열)
            │
            ▼
store::start(id, "", hash)
    assigned_to = Assignment { session: "", ... }
            │
            ▼
이후 is_session_alive("") ─► .alive/"" 파일 없음 ─► 항상 false
            │
            ▼
다른 누구든 즉시 reclaim 성공 → 소유권 보호 완전 우회
```

**관련 사실들**:

| 위치 | 사실 | 의미 |
|---|---|---|
| `agent.rs:496,849` | `session_id: None` 하드코딩 | 에이전트는 자기 session id를 모름 |
| `config.rs:126` `AgentConfig` | `session_id` 필드 **부재** | 주입 통로가 없음 |
| `tui/app.rs:809` | flock을 `"tui"`(고정 문자열)로 획득 | TUI는 "tui" 식별자 |
| `tui/overlay/issues_panel/mod.rs:159` | `session_id()` → `"tui"` 하드코딩 | 패널도 "tui" |
| `main.rs:189` | CLI는 `&session`(Uuid)로 flock 획득 | CLI만 정상 |
| `lib.rs` (print/RPC 경로) | flock 획득 **없음** | print/RPC는 아예 락 없음 |

**결론**: TUI 안의 에이전트는 `"tui"` 락이 프로세스에 잡혀 있음에도, 자기 caller id로
`None→""`을 쓰므로 그 락과 **연결되지 않는다**. 두 에이전트/세션이 같은 이슈를 `start`
하면 둘 다 조용히 성공하고 마지막이 이긴다. **헤드라인 기능(다중 세션 소유권)이 주 사용
사례(자율 에이전트)에서 조용히 동작하지 않는 것**이다.

### 1.3 식별된 13개 결함 전체 목록

| # | 심각도 | 결함 | 현재 코드 위치 |
|:-:|:-:|---|---|
| **13** | 🟥🟥 | 에이전트 `session_id` 누락 → 소유권 우회 | `agent.rs:496,849`; `config.rs:126` |
| 1 | 🟥 | `atomic_write` temp `tmp.<pid>` → PID 재활용 충돌 | `issues.rs:226`, `session.rs:23` |
| 2 | 🟥 | `update` CAS에 cross-process retry 없음 → 에이전트 포기 | `issues.rs:838` + tool description |
| 3 | 🟥 | `update` 필드 의미 암묵적 (absent=keep / `[]`=clear 구분 불가) | `issue_tool.rs` schema |
| 4 | 🟧 | `reopen` 부재; `update{status:open}`이 `closed_at` 미삭제(잠재 버그) | `issues.rs:884` |
| 5 | 🟧 | `body`/`title`/`labels` 크기 제한 없음 → 디스크 채우기 | `issue_tool.rs::execute` |
| 6 | 🟧 | `status`가 list 필터와 update 변경값으로 이중 의미 | `issue_tool.rs` schema |
| 7 | 🟧 | `github` read-only인데 schema/description에 명시 없음 | `issue_tool.rs` schema |
| 8 | 🟨 | `.alive/` 좀비 파일 비정상 종료 시 누적 | `issues.rs::liveness` |
| 9 | 🟨 | `update` mutator `Send + 'static` → 외부 데이터 캡처 시 clone 강제 | `issues.rs:838` |
| 10 | 🟨 | `IssueSummary::top_priority` 의미 미문서화 | `issues.rs:660` |
| 11 | 🟨 | `unsafe libc::flock` 2곳에 산재 | `issues.rs:398, 421` |
| 12 | 🟨 | `update` 매번 `updated_at` 갱신 + cache invalidate (no-op 감지 없음) | `issues.rs:856` |

### 1.4 retry가 반드시 필요한 이유 (P2 핵심 통찰)

`update`는 `expected_hash=None`이면 **hash 게이트를 건너뛰고 last-writer-wins**다
(`issues.rs:846`의 `if let Some(expected)` 분기). 그래서:

> **`start`를 hash 없이 호출하면 cross-process에서 assignment가 증발한다.**

```
T=0  A.read()  → assigned_to=None
T=1  B.read()  → assigned_to=None   (아직 A가 안 썼음)
T=2  A.write() → assigned_to=A
T=3  B.write() → assigned_to=B  ← A의 assignment 증발 (B mutator가 None을 봄)
```

B의 mutator가 `is_session_alive(A)`를 검사하더라도, B가 읽은 시점(T=1)에 A의 assignment가
디스크에 없었으므로 검사 대상 자체가 없다. 따라서 **hash는 advisory가 아니라 필수**이며,
안전한 자동 회복 = **"fresh hash 재독 + N회 재시도"**뿐이다. (#13이 먼저 고쳐져야 `is_session_alive`
검사가 의미를 갖는다 — 그래서 P0가 우선이다.)

---

## 2. 설계 원칙 (재확정)

1. **코어 불변.** flock + CAS + per-file queue 3계층은 그대로.
2. **엄격함은 저장소, 회복은 도구.** store는 `Conflict`를 원시 반환; 도구만 bounded retry.
3. **소유권 정책 유지.** update/close는 assignee(또는 free 시 누구나) 한정. v1의 GitHub
   시맨틱 완화 제안은 **철회** — 동작 변경은 별도 future PR.
4. **프로세스 단일 liveness identity.** 한 프로세스는 하나의 ownership id로 하나의 flock을
   잡는다. TUI의 여러 AgentSession은 그 id를 공유한다(과제적 한계, §10 참조).
5. **새 의존성 금지.** `uuid`는 이미 `oxicode-cli` 의존성(v4). `libc` 유지.
6. **5 phase는 (P0를 제외하고) 서로 독립.** P1–P4는 임의 순서/병렬 병합 가능. P0는 먼저.
7. **호환성 보존.** 파일 포맷·기존 action·에러 변형 유지. 새 것은 **추가만**.

---

## 3. Phase 0 — 소유권 복원 (🟥🟥 #13) [최우선]

### 3.1 목표
에이전트가 `issue` 도구를 호출할 때, `ctx.session_id`가 **실제 flock 홀더와 일치하는**
진짜 식별자가 되도록 만든다.

### 3.2 oxicode-agent 변경 (식별자 주입 통로)

`oxicode-agent/src/config.rs` — `AgentConfig`에 필드 추가(가장 뒤, additive):

```rust
pub struct AgentConfig {
    // ... 기존 필드 ...
    /// 이 에이전트 실행을 식별하는 id. 도구의 `ToolContext.session_id`로 흘러들어가,
    /// 예를 들어 `issue` 도구가 ownership/liveness 판정에 사용한다.
    /// None이면 ToolContext.session_id = None (ownership 기능 비활성).
    #[serde(default)]
    pub session_id: Option<String>,
}
```

`oxicode-agent/src/agent.rs:496, 849` — 하드코딩 `None`을 config에서 읽도록 수정:

```rust
let loop_config = AgentLoopConfig {
    // ... 기존 ...
    session_id: self.config().config.session_id.clone(),   // ← None 대신
    // ...
};
```

> **API 영향**: additive. 기존 `AgentConfig` 생성지점은 모두 `..Default::default()`나
> 리터럴이므로, 필드 추가 시 컴파일 에러가 나는 곳은 점검해야 한다(oxicode-cli `lib.rs`,
> 테스트). `#[serde(default)]` 덕분에 역직렬화 호환 유지.

### 3.3 oxicode-cli 변경 — 프로세스 단일 liveness lock + identity

**핵심**: `App`이 **프로세스 수명 동안 하나의 flock을 잡고**, 그 id를 에이전트에 주입한다.
TUI의 별도 flock 획득은 제거하고 App이 통합 관리한다(같은 파일에 같은 프로세스가 두 번
`LOCK_EX NB`는 두 번째가 실패하므로 반드시 하나여야 한다).

`oxicode-cli/src/lib.rs` — `App` 필드 추가 + `from_oxicode`에서 생성:

```rust
pub struct App {
    // ... 기존 ...
    /// 프로세스 단일 ownership identity. issue 도구·패널·CLI가 공유.
    ownership_session_id: String,
    /// 이 프로세스가 잡은 liveness lock. App이 살아있는 동안 유지.
    _liveness_guard: Option<crate::store::issues::liveness::AliveGuard>,
}

impl App {
    pub async fn from_oxicode(oxicode: oxicode_sdk::Oxicode, settings: Settings) -> Result<Self> {
        // ... 기존 ...

        // ── 프로세스 단일 ownership identity + liveness lock ──
        // TUI는 "tui", 그 외(print/RPC)는 안정적인 process-scoped id.
        let ownership_id = if /* TUI 모드 플래그 */ {
            "tui".to_string()
        } else {
            format!("proc-{}-{}", std::process::id(), /* short session uuid */)
        };
        let liveness_guard = issue_store.as_ref().and_then(|store| {
            crate::store::issues::liveness::acquire(&store.issues_dir(), &ownership_id).ok()
        });

        // ── issue 도구에 identity 주입 (도구 자체는 ctx.session_id를 쓰므로
        //    AgentConfig.session_id → ToolContext.session_id 경로로 흘림) ──
        // AgentConfig에 session_id 설정은 agent 빌드 전에:
        //   config.session_id = Some(ownership_id.clone());
        // (AgentBuilder나 config 리터럴에 추가)

        // ... 기존 도구 등록 ...
        Ok(Self {
            // ... 기존 ...
            ownership_session_id: ownership_id,
            _liveness_guard: liveness_guard,
        })
    }

    /// 이 프로세스의 ownership id (패널·CLI 공유).
    pub fn ownership_session_id(&self) -> &str { &self.ownership_session_id }
}
```

> **TUI 모드 판별**: `from_oxicode` 시점에는 run-mode가 아직 확정되지 않을 수 있다. 두 가지
> 안: (a) `from_oxicode`에 `mode: RunMode` 인자 추가(명시적, 권장); (b) `App::from_oxicode`는
> 항상 process-scoped id를 쓰고 TUI 시작 시 "tui"로 재발급. **(a) 채택** — `from_oxicode(oxicode,
> settings, mode)`로 서명 변경. 호출지점(main.rs, bootstrap.rs) 수정.

### 3.4 TUI 경로 정리

`oxicode-cli/src/tui/app.rs:806-813`의 별도 flock 획득 **제거** (App이 이미 잡음). 단,
`run_tui_interactive_impl`은 App의 guard가 살아있는 동안에만 실행되므로 소유권 이전 주의:
guard는 `App` 안에 있고, TUI 루프도 같은 프로세스/같은 수명이므로 OK.

`oxicode-cli/src/tui/overlay/issues_panel/mod.rs:159`:
```rust
pub fn session_id() -> &'static str { "tui" }   // ← 제거/변경
```
패널의 ownership 연산은 이제 `app.ownership_session_id()`(=& "tui" in TUI mode)를
사용하도록 변경. **`session_id()` 정적 메서드 제거**, 호출지점(state.rs 등)을
`app.ownership_session_id()` 참조로 교체. (패널이 `App` 참조를 이미 가지고 있다면 직접;
아니면 `IssuesPanelOverlay`에 id를 주입.)

### 3.5 CLI 경로 (main.rs)

`main.rs:189`는 이미 `&session`(Uuid) + flock 획득을 한다. 이 경로는 **그대로 유지**
(별도 `oxicode issue` 서브커맨드이므로 App을 거치지 않음). 단, `App::from_oxicode` 도입으로
print/RPC도 이제 flock을 갖게 되므로, CLI 서브커맨드 경로와의 일관성은 유지됨.

### 3.6 도구 쪽 안전망 (이미 존재, 확인)

`issue_tool.rs`는 이미 `session.is_empty()`일 때 변형 액션을 거부한다:
```rust
if session.is_empty() {
    return Err("cannot start: no active session id in context".to_string());
}
```
P0 후에는 session이 비지 않으므로 이 분기는 사실상 도달 불가. **유지**(방어적).

### 3.7 테스트 (P0 핵심 — 통합)

```rust
#[tokio::test]
async fn agent_start_is_rejected_by_second_live_owner() {
    // P0 전: 두 agent가 같은 이슈 start → 둘 다 성공(버그).
    // P0 후: agent A(session_id="proc-A", flock 보유) start 성공;
    //        agent B(session_id="proc-B") start → Assigned 에러.
}

#[tokio::test]
async fn tool_context_carries_session_id() {
    // AgentConfig.session_id=Some("s1") → ToolContext.session_id == Some("s1").
}

#[test]
fn app_holds_single_liveness_lock() {
    // App 빌드 후 .alive/<ownership_id> 존재 + is_session_alive == true.
}
```

### 3.8 규모
~250줄. oxicode-agent(config.rs + agent.rs) ~30줄, oxicode-cli(lib.rs + tui/app.rs +
panel) ~220줄. **P1–P4보다 먼저 병합**.

---

## 4. Phase 1 — 데이터 무결성 기반 (🟥 #1)

### 4.1 문제
`atomic_write`가 `path.with_extension("tmp.<pid>")`를 쓴다. PID namespace 재활용(컨테이너)
이나 fork+exec에서 두 프로세스가 같은 PID로 같은 파일을 동시 쓰면 **temp가 겹쳐 한 쪽
write 손실**. `session.rs`도 동일 결함(코드 중복).

### 4.2 해결: 공유 util `store::fs_util`

새 파일 `oxicode-cli/src/store/fs_util.rs`:

```rust
//! Atomic write helpers shared across `store/*`.
//!
//! Temp file name = `<path>.tmp.<pid>.<uuid-simple>` — PID 재활용(컨테이너,
//! fork+exec)에서 충돌 안전. PID는 디버깅용, UUID가 유일성 보장.

use std::fs;
use std::io;
use std::path::Path;

/// UTF-8 내용을 path에 원자적 쓰기(temp + rename).
pub fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    atomic_write_bytes(path, content.as_bytes())
}

/// 바이트를 path에 원자적 쓰기(temp + rename).
pub fn atomic_write_bytes(path: &Path, content: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::write(&tmp, content)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp); // rename 실패 시 orphan 누출 방지
            Err(e)
        }
    }
}
```

### 4.3 마이그레이션

| 파일 | 변경 |
|---|---|
| `oxicode-cli/src/store/fs_util.rs` | **신규** |
| `oxicode-cli/src/store/mod.rs` | `pub mod fs_util;` |
| `oxicode-cli/src/store/issues.rs:223-228` | 로컬 `atomic_write` 삭제 → `use super::fs_util::atomic_write;` |
| `oxicode-cli/src/store/session.rs:22-28` | 동일 (PID 결함 동시 수리) |

> **내구성(durability)**: rename은 원자적이되 내구적이지 않다(fsync 필요). 기존 CLI
> 일관성 모델을 존중해 fsync는 **명시적 옵트인**으로 남김(필요시 `atomic_write_durable`
> 추가). P1 스코프 아님.

### 4.4 테스트
```rust
#[test] fn atomic_write_survives_concurrent_same_path() { /* 16 스레드 동시 쓰기 → 최종 내용이 그 중 하나와 정확 일치 */ }
#[test] fn temp_file_name_contains_uuid()               { /* `tmp.<pid>.<32hex>` 정규식 검증 */ }
#[test] fn rename_failure_does_not_leak_orphan()        { /* 읽기전용 dir에서 rename 실패 → temp 제거 */ }
```

### 4.5 규모
~120줄. P0/P2/P3/P4와 독립.

---

## 5. Phase 2 — CAS 재시도 + Patch + reopen + no-op (🟥 #2 🟧 #3 #4 🟨 #9 #12)

### 5.1 도구 레이어 `cas_retry` (#2)

store는 **엄격** 유지. **도구만** 재시도. §1.4 통찰에 따라: 첫 시도는 에이전트 hash(빠른
경로), conflict 시 fresh hash 재독 후 재시도.

`oxicode-cli/src/tools/issue_tool.rs`:

```rust
const MAX_CAS_ATTEMPTS: u32 = 4;

async fn cas_retry<T, F, Fut>(
    store: &FileIssueStore,
    id: u32,
    agent_hash: Option<String>,
    mut op: F,
) -> Result<T, IssueError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<T, IssueError>>,
{
    let mut hash = agent_hash;
    for attempt in 0..MAX_CAS_ATTEMPTS {
        match op(hash.clone()).await {
            Ok(v) => return Ok(v),
            Err(IssueError::Conflict { .. }) if attempt + 1 < MAX_CAS_ATTEMPTS => {
                tracing::debug!(id, attempt = attempt + 1, "issue CAS conflict, re-reading");
                hash = store.read(id).ok().map(|(_, h)| h);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(IssueError::Conflict { id })
}
```

> **바운드**: `cas_retry`는 `async fn`(spawn 아님)이라 호출자 태스크의 `Send` 요구가 자동
> 전파. `op: FnMut`는 매 시도 새 future 생성. store op는 이미 `Send + 'static`과 호환.

모든 변형 액션을 감싼다:
```rust
async fn start(&self, params: Value, session: &str) -> Result<String, String> {
    let id = require_u32(params.get("id"), "id")?;
    if session.is_empty() { return Err("cannot start: no active session id in context".into()); }
    let agent_hash = hash_param(params.get("content_hash"));
    let store = self.store.clone();
    let session = session.to_string();
    cas_retry(&store, id, agent_hash, |hash| {
        let store = store.clone();
        let session = session.clone();
        async move { store.start(id, &session, hash).await }
    })
    .await
    .map(|issue| format!("assigned issue #{} to session {}", issue.meta.id, session))
    .map_err(|e| e.to_string())
}
```
`release`/`close`/`link_session`/`update`/`reopen` 동일 패턴.

### 5.2 `IssuePatch` + `apply_patch` (#3 정밀화 기반 + #9 완화 + **소유권 유지**)

> **v1 정정**: v1에서 `apply_patch`가 소유권 검사를 빼자고 했다. **철회.** 현재 `update`
> 액션은 assignee 한정이며, hardening은 동작을 유지해야 한다. `apply_patch`는 `caller`를
> 받아 **기존과 동일하게** 소유권을 강제한다.

`oxicode-cli/src/store/issues.rs`:

```rust
/// `update` 변경 묶음. 모든 필드 `Option`: `None`=유지, `Some`=교체.
/// `labels`만 특수 — `Some(vec![])` = 전체 삭제.
#[derive(Debug, Clone, Default)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    pub labels: Option<Vec<String>>,
}

impl FileIssueStore {
    /// patch 적용(엄격 CAS). caller가 제공되면 기존 정책대로 소유권 강제.
    pub async fn apply_patch(
        &self,
        id: u32,
        patch: IssuePatch,
        caller: Option<String>,
        expected_hash: Option<String>,
    ) -> Result<Issue, IssueError> {
        self.update(id, expected_hash, move |mut issue| {
            // 기존 동작 유지: 다른 live owner가 있으면 거부.
            if let Some(caller) = caller.as_deref() {
                if let Some(ref a) = issue.meta.assigned_to
                    && !a.session.is_empty()
                    && a.session != caller
                {
                    return Err(IssueError::NotAssigned { id, caller: caller.to_string() });
                }
            }
            if let Some(t) = patch.title    { issue.meta.title = t; }
            if let Some(b) = patch.body     { issue.body = b; }
            if let Some(s) = patch.status {
                issue.meta.status = s;
                issue.meta.closed_at = match s {
                    Status::Closed => Some(chrono::Utc::now()),
                    Status::Open => None,   // #4 잠재 버그 수정: reopen 시 closed_at 정리
                };
            }
            if let Some(p) = patch.priority { issue.meta.priority = p; }
            if let Some(l) = patch.labels   { issue.meta.labels = l; }
            Ok(issue)
        })
        .await
    }
}
```

> 도구 `update` 액션은 이제 `apply_patch` + `cas_retry` 조합. 클라이언트는 patch struct
> 하나를 넘기므로 closure에 String 여러 개를 clone할 필요 없음(#9 완화, 90% 해소).

### 5.3 `reopen` 액션 (#4)

```rust
/// 닫힌 이슈 재개: status=Open, closed_at=None. (누구나 가능 — close 후엔 owner 없음.)
pub async fn reopen(
    &self,
    id: u32,
    expected_hash: Option<String>,
) -> Result<Issue, IssueError> {
    self.update(id, expected_hash, |mut issue| {
        issue.meta.status = Status::Open;
        issue.meta.closed_at = None;
        Ok(issue)
    })
    .await
}
```

도구 action enum에 `"reopen"` 추가. description: *"To resume a closed issue, call
`reopen`, then `start`."*

### 5.4 no-op 쓰기 감지 (#12) — 정확한 코드

`update` 내부(file-mutation-queue 진입 후)에 추가:

```rust
pub async fn update<F>(&self, id: u32, expected_hash: Option<String>, mutator: F)
    -> Result<Issue, IssueError>
where F: FnOnce(Issue) -> Result<Issue, IssueError> + Send + 'static
{
    let path = self.path_for_id(id).map_err(IssueError::Other)?;
    let store = self.clone();
    oxicode_agent::tools::file_mutation_queue::global_mutation_queue()
        .with_queue(&path, move || async move {
            let raw = fs::read_to_string(&path)?;
            if let Some(expected) = expected_hash.as_deref()
                && content_hash(&raw) != expected {
                return Err(IssueError::Conflict { id });
            }
            let before = parse_issue(&raw, Some(path.clone())).map_err(IssueError::Other)?;
            let before_updated_at = before.meta.updated_at;
            let before_bytes = serialize_issue(&before).map_err(IssueError::Other)?;
            let after = mutator(before)?;

            // no-op 감지: updated_at을 before와 동일하게 강제한 probe가 before와
            // 직렬화 동일하면, 실제 내용 변화가 없는 것 → 쓰기/타임스탬프/invalidate 스킵.
            let mut probe = after.clone();
            probe.meta.updated_at = before_updated_at;
            let probe_bytes = serialize_issue(&probe).map_err(IssueError::Other)?;
            if probe_bytes == before_bytes {
                return Ok(after.with_path(path)); // no-op
            }

            let mut final_issue = after;
            final_issue.meta.updated_at = Utc::now();
            let content = serialize_issue(&final_issue).map_err(IssueError::Other)?;
            super::fs_util::atomic_write(&path, &content)?;   // = P1의 fs_util
            store.invalidate();
            Ok(final_issue.with_path(path))
        })
        .await
}
```

> **정합성**: `before_bytes`/`probe_bytes`는 모두 `serialize_issue` 결과(serde_yaml 정규화
> 형태)로 비교하므로, 디스크 `raw`와의 key-order/whitespace 차이에 영향받지 않는다.

### 5.5 테스트
```rust
#[tokio::test] async fn cas_retry_recovers_from_concurrent_write() { /* 2 태스크 start → 일관된 단일 owner */ }
#[tokio::test] async fn reopen_clears_closed_at()                  { /* close→reopen → status==Open && closed_at==None (회귀 방지) */ }
#[tokio::test] async fn noop_update_does_not_bump_timestamp()      { /* 동일 patch 재적용 → updated_at·dir mtime 불변 */ }
#[tokio::test] async fn apply_patch_labels_clear_vs_keep()         { /* None=유지 / Some([])=삭제 / Some([x])=교체 */ }
#[tokio::test] async fn apply_patch_enforces_ownership()           { /* 다른 owner일 때 NotAssigned (기존 동작 유지 검증) */ }
```

### 5.6 규모
~400줄 (store +250, tool +150). P0/P1 후 권장(P0 identity가 있어야 ownership 검사가 의미).

---

## 6. Phase 3 — 스키마 정밀화 + 크기 상한 (🟧 #5 #6 #7)

### 6.1 스키마 description 정교화

단일 평면 스키마 + action discriminator 유지(JSON Schema `oneOf`는 LLM 도구 호환성
저하). description으로 시맨틱 완전 명시. `issue_tool.rs::parameters_schema`:

```rust
"action": { "type":"string",
    "enum":["list","read","create","update","reopen","start","release","close","link_session"],
    "description":"Issue operation. For `update`, every field is optional — omit to keep, provide to replace. Concurrent edits are auto-reconciled (up to 4 retries)." },
"title":      { "type":"string",  "description":"create: required. update: replaces title. Max 512 chars." },
"body":       { "type":"string",  "description":"create: optional (default empty). update: replaces body. Max 256 KiB." },
"priority":   { "type":"string","enum":["low","medium","high","critical"], "description":"create/update: new priority. list: filter." },
"labels":     { "type":"array","items":{"type":"string"}, "description":"create/update: REPLACES labels entirely. Omit to keep; pass [] to clear all. Max 32 labels, 64 chars each." },
"status":     { "type":"string","enum":["open","closed"], "description":"list: filter by status. update: new status (prefer `close`/`reopen` actions for clarity)." },
"label":      { "type":"string",  "description":"list: filter to issues with this label." },
"text":       { "type":"string",  "description":"list: case-insensitive substring filter on title." },
"content_hash":{"type":"string",  "description":"Hash from last `read`. ADVISORY: tool auto re-reads and retries on conflict, so a stale hash still succeeds." },
"github":     { "type":"object","readOnly":true, "description":"READ-ONLY. Populated by Phase 6 GitHub sync; cannot be set via this tool." }
```

도구 최상위 description에 관례 노트 추가:
> *"For `update`: every field optional — omit to keep, provide to replace. `labels: []`
> clears all. Prefer `close`/`reopen`/`start`/`release` over `update{status}`. Concurrent
> edits auto-reconciled."*

### 6.2 크기 상한 (#5) — 도구 레이어 방어

```rust
const MAX_TITLE_LEN: usize = 512;
const MAX_BODY_LEN: usize = 256 * 1024; // 256 KiB
const MAX_LABELS: usize = 32;
const MAX_LABEL_LEN: usize = 64;

fn validate_size(params: &Value, action: &str) -> Result<(), String> {
    if !matches!(action, "create" | "update") { return Ok(()); }
    if let Some(t) = params.get("title").and_then(|v| v.as_str()) {
        if t.chars().count() > MAX_TITLE_LEN { return Err(format!("title too long (max {MAX_TITLE_LEN} chars)")); }
    }
    if let Some(b) = params.get("body").and_then(|v| v.as_str()) {
        if b.len() > MAX_BODY_LEN { return Err(format!("body too large (max {MAX_BODY_LEN} bytes)")); }
    }
    if let Some(l) = params.get("labels").and_then(|v| v.as_array()) {
        if l.len() > MAX_LABELS { return Err(format!("too many labels (max {MAX_LABELS})")); }
        for item in l {
            if item.as_str().map(|s| s.chars().count()).unwrap_or(0) > MAX_LABEL_LEN {
                return Err(format!("label too long (max {MAX_LABEL_LEN} chars)"));
            }
        }
    }
    Ok(())
}
```

`execute`의 action dispatch 직전 호출.

### 6.3 테스트
```rust
#[test] fn rejects_oversize_body()      { /* 257KiB → 에러 */ }
#[test] fn rejects_too_many_labels()    { /* 33 labels → 에러 */ }
#[test] fn update_omits_keeps_labels()  { /* labels 없으면 유지 */ }
#[test] fn update_empty_clears_labels() { /* labels:[] → 전체 삭제 */ }
```

### 6.4 규모
~200줄. P0–P2 독립.

---

## 7. Phase 4 — 견고성 마무리 (🟨 #8 #10 #11)

### 7.1 `.alive/` orphan 수거 — age-gated (#8, TOCTOU 안전)

> **v2 개선**: v1의 순수 `is_session_alive`-then-unlink는 TOCTOU가 있다(체크 후 unlink
> 사이에 다른 프로세스가 acquire). **age threshold** 추가: 파일 mtime이 임계(기본 1시간)
>보다 오래된 dead 파일만 수거. 최근 활성 세션은 절대 건드리지 않음.

```rust
const ORPHAN_AGE_SECS: u64 = 3600; // 1시간

/// `.alive/`에서 죽은 orphan lock 파일 수거(best-effort, 멱등).
/// (1) 홀더가 있는(flock 걸린) 파일은 절대 안 지움.
/// (2) mtime이 ORPHAN_AGE_SECS 이내인 파일도 안 지움(TOCTOU 회피).
pub fn reap_orphans(issues_dir: &Path) -> std::io::Result<usize> {
    let dir = issues_dir.join(".alive");
    let rd = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in rd.flatten() {
        let name = entry.file_name();
        let sid = name.to_string_lossy();
        if is_session_alive(issues_dir, &sid) { continue; }            // (1)
        let mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
        if now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0) < ORPHAN_AGE_SECS {
            continue;                                                   // (2)
        }
        if fs::remove_file(entry.path()).is_ok() { removed += 1; }
    }
    Ok(removed)
}
```

호출 지점:
- `FileIssueStore::open` 시 1회(lazy, 에러는 warn 로그만).
- `start`가 dead owner reclaim 후 best-effort.
- `oxicode issue --reap` 수동 플래그(권장).

### 7.2 `top_free_priority` (#10)

```rust
/// 가장 행동 가능한 우선순위 = open && 미할당 이슈 중 최대. 없으면 None.
/// `top_priority`(전체 open 최대)와 구분 — 전자가 "지금 손댈 것" 신호.
pub fn top_free_priority(&self) -> Option<Priority> { ... }
```

`Cache`에 `top_free_priority` 필드 추가(`refresh_if_stale`에서 `assigned_to.is_none()`
조건으로 계산). 기존 인디케이터 동작은 유지(호환); 새 필드는 **노출만**.

### 7.3 unsafe flock helper 중앙화 (#11)

```rust
/// SAFETY: caller must pass a valid, owned fd.
unsafe fn try_flock_exclusive(fd: i32) -> std::io::Result<()> {
    if libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0 { Ok(()) }
    else { Err(std::io::Error::last_os_error()) }
}
/// SAFETY: caller must pass a valid, owned fd.
unsafe fn probe_flock_shared(fd: i32) -> std::io::Result<()> {
    if libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) == 0 {
        libc::flock(fd, libc::LOCK_UN); Ok(())
    } else { Err(std::io::Error::last_os_error()) }
}
```

> **의존성**: `fs2`/`rustix` 추가 **안 함**(최소 의존성 철학). helper 추출 + SAFETY 주석으로
> unsafe 표면 명확화(2곳 산재 → 2개 명명 함수).

### 7.4 테스트
```rust
#[test] fn reap_skips_recent_dead_files()      { /* mtime < 1h인 dead 파일 → 보존 */ }
#[test] fn reap_removes_old_dead_keeps_alive() { /* old dead 제거, alive 보존 */ }
#[test] fn reap_idempotent()                   { /* 빈/없는 디렉토리 → Ok(0) */ }
#[test] fn reap_safe_during_live_lock()        { /* live holder 파일 → is_alive 여전 true */ }
```

### 7.5 규모
~170줄. 완전 독립.

---

## 8. 동시성 모델 — 재검증 (P0–P4 후 전체 그림)

```
agent: issue { action:"update", id, body, content_hash }
  │   (P0: ctx.session_id = "proc-…" 또는 "tui" — 진짜 flock 홀더와 일치)
  ▼
issue_tool::execute
  ├─ validate_size()                  (P3) 크기 상한 조기 거부
  └─ cas_retry(store, id, agent_hash, |hash| store.apply_patch(id, patch, caller, hash))  (P2)
        │  시도 1: agent_hash(있으면) — 빠른 경로
        │  conflict ─► fresh hash 재독 ─► 시도 2..4
        ▼
store::apply_patch → store::update    (엄격 CAS, 소유권 강제[기존 동작], no-op 감지)  (P2)
  ├─ file_mutation_queue (in-process 직렬화)
  ├─ content_hash CAS (cross-process)
  ├─ 소유권 검사 (caller != owner → NotAssigned)         (P0 identity로 의미 부여)
  ├─ mutator 적용
  ├─ no-op? ─► skip write/timestamp/invalidate            (P2 #12)
  └─ fs_util::atomic_write (UUID temp)                    (P1)

liveness:
  App holds flock under ownership_session_id for process lifetime  (P0)
  .alive/ reaped age-gated on open + manual --reap                  (P4)
```

**3계층(L1/L2/L3) 불변.** 외피만 개선. **#13 수정으로 L1 flock이 에이전트 경로에서도
실제로 작동**하게 된다.

---

## 9. 의존성 & 호환성

| 항목 | 결정 |
|---|---|
| 새 crate 의존성 | **없음**. `uuid`(이미 v4), `libc` 유지. |
| oxicode-agent API | `AgentConfig.session_id` **추가**(additive, `#[serde(default)]`). `from_oxicode` 서명 `+ mode` 인자. |
| 파일 포맷 | **변경 없음**. 동일 마크다운 + YAML frontmatter. |
| 스키마 breaking | **없음**. action enum에 `reopen` **추가만**. 기존 필드 유지. |
| 에러 변형 | **유지**. `Conflict`/`Assigned`/`NotAssigned`/`NotFound`. |
| 소유권 정책 | **유지**(assignee 한정). v1 완화 제안 철회. |
| TUI 패널 | `session_id()` 정적 메서드 제거 → `app.ownership_session_id()` 사용. 패널 `reopen` 키(`o`)는 별도 UX PR. |
| `oxicode issue` CLI | 기존 동작 유지; `reopen` 서브커맨드 + `--reap` 플래그 추가 권장. |

---

## 10. 리스크 & 트레이드오프

| 결정 | 리스크 | 완화 |
|---|---|---|
| 프로세스 단일 identity(P0) | 한 TUI의 여러 AgentSession이 소유권 공유(구별 불가) | v1 수용; 과제적 한계. per-session 소유권은 future |
| `from_oxicode`에 `mode` 인자 | 서명 변경 → 호출지점 수정 | main.rs/bootstrap.rs만(2곳) |
| TUI flock을 App으로 이관 | guard 수명 = App 수명 보장 필요 | App이 TUI 루프보다 오래 삶(이미 그럼) |
| retry를 도구에 한정 | CLI `oxicode issue`는 재시도 없음 | CLI에도 `cas_retry` 헬퍼 적용 권장 |
| `expected_hash` advisory | 엄격 게이트 원하는 호출자 | store `update`는 여전히 엄격; advisory는 도구 한정 |
| 소유권 정책 유지 | 비-assignee body 편집 불가 | 의도적(기존 동작); 완화는 별도 PR |
| `fsync` 제외 | rename 직후 정전 시 내구성 미보장 | 기존 모델 유지; `--durable` 옵션 여지 |
| `reopen` 누구나 가능 | 악의적/실수 reopen | 감사는 `sessions`/`updated_at`로 충분 |
| orphan 수거 unlink | TOCTOU | age threshold(1h) + 홀더 체크; startup/수동 한정 |
| #9 `'static` 미완해 | 잔여 clone | `IssuePatch`로 90% 해소; 잔여는 queue 재설계 시 |

---

## 11. 체크리스트 (구현 완료 기준)

- [ ] **P0**: `AgentConfig.session_id` 추가 + `agent.rs`에서 주입 + `App` 단일
      liveness lock + `from_oxicode(mode)` + 패널 identity 통일 + **통합 테스트 3개 통과**
- [ ] **P1**: `fs_util.rs` + issues/session 마이그레이션 + 충돌 테스트
- [ ] **P2**: `cas_retry` + `IssuePatch`/`apply_patch`(소유권 유지) + `reopen` + no-op 감지 + 회귀 테스트
- [ ] **P3**: 스키마 description + `validate_size` + github readOnly + 테스트
- [ ] **P4**: `reap_orphans`(age-gated) + open 시 호출 + `top_free_priority` + flock helper + 테스트
- [ ] 도구 description에 retry 정책 + reopen 워크플로우 명시
- [ ] `cargo fmt && cargo clippy --workspace -- -D warnings` clean
- [ ] `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` clean (P0가 config.rs 건드리므로)
- [ ] `cargo nextest run --workspace` 통과
- [ ] CHANGELOG.md 5항목(P0–P4) 추가
- [ ] AGENTS.md Pitfalls에 **ownership identity 모델(P0)** + **hash advisory/retry(P2)** 추가
- [ ] (권장) `oxicode issue` CLI에 `reopen` 서브커맨드 + `--reap` 추가

---

## 12. 롤아웃 순서

```
P0 (단독, 최우선) ──► 병합 ──┐
                             ├──► P1, P2, P3, P4 (병렬 worktree 가능, 서로 독립)
                             │    단 P2는 P0 identity가 있어야 ownership 테스트가
                             │    의미를 갖으므로 P0 이후 권장.
                             ▼
                         CHANGELOG + AGENTS.md 갱신
```

- **P0는 반드시 먼저** — 나머지가 의존하는 identity 기반.
- P1–P4는 `worktree: true` 병렬 착수 가능(파일 충돌 최소).
- 각 PR ≤ 4000줄 pr-gate.
