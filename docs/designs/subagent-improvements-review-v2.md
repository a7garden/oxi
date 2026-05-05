# Subagent Tool 개선 설계서 v2 — 리뷰

## 🔴 Critical: P0 수정이 불완전 — 두 Agent 인스턴스 문제

### 발견

현재 oxi에는 **Agent가 두 곳에서 따로 생성**됩니다:

```
경로 A: main.rs → App::new() → Agent::new()       ← agent_1
경로 B: tui_interactive.rs → create_agent_session_from_services() → Agent::new()  ← agent_2
```

TUI 모드(`run_tui_interactive`)는 `app`에서 `settings`와 `model_id`만 가져오고,
`app.agent()`는 **완전히 버립니다**. 대신 `create_agent_session_from_services()`에서
새 Agent를 만듭니다.

### 설계의 문제

설계서 항목 0은 두 곳에 builtin 도구를 등록하라고 합니다:
- `App::new()` ← 경로 A용
- `create_agent_session_from_services()` ← 경로 B용

그런데 이렇게 하면:

| | Builtin 도구 | Extension 도구 |
|---|---|---|
| **경로 A** (print_mode) | ✅ 등록됨 | ✅ main.rs에서 등록 |
| **경로 B** (TUI) | ✅ 등록됨 | ❌ **extension 도구가 안 들어감** |

main.rs에서 `app.agent_tools().register_arc(tool)`로 extension을 App의 agent에 등록하지만,
TUI는 새 agent를 만들므로 extension 도구가 유실됩니다.

### 해결

ToolRegistry를 App 수준에서 관리하고, `create_agent_session_from_services()`에 전달:

```rust
// main.rs
let app = oxi::App::new(settings).await?;

// Register builtin + extension tools at App level
let tools = app.agent_tools();
let builtins = oxi_agent::ToolRegistry::with_builtins_cwd(cwd.clone());
for name in builtins.names() {
    if let Some(tool) = builtins.get(name) {
        tools.register_arc(tool);
    }
}
for tool in ext_registry.all_tools() {
    tools.register_arc(tool);
}

// TUI 모드: App의 ToolRegistry를 session에 전달
oxi::tui_interactive::run_tui_interactive(app).await?;
```

```rust
// create_agent_session_from_services()
// App.agent.tools()를 clone하여 새 agent에 설정
pub fn create_agent_session_from_services(
    options: CreateAgentSessionFromServicesOptions,
    tool_registry: Option<Arc<ToolRegistry>>,  // ← 추가
) -> Result<...> {
    let agent = Arc::new(Agent::new(provider, config));

    // 도구 등록: 전달받은 registry 또는 기본 builtins
    let tools = tool_registry.unwrap_or_else(|| {
        Arc::new(ToolRegistry::with_builtins_cwd(cwd.clone().into()))
    });
    for name in tools.names() {
        if let Some(tool) = tools.get(name) {
            agent.tools().register_arc(tool);
        }
    }
}
```

**이것이 항목 0의 올바른 수정.** 현재 설계는 이 문제를 다루지 않습니다.

---

## 🔴 Critical: 항목 3 — run_single_agent가 free function

### 문제

설계서:
> `run_single_agent()`에서 `Command::new(tool.get_binary())`

하지만 `run_single_agent`는 `&self`가 없는 **free function**입니다:

```rust
async fn run_single_agent(
    cwd: &Path,
    agents: &[AgentConfig],
    agent_name: &str,
    task: &str,
    ...
) -> SingleResult
```

`tool.get_binary()`를 호출할 방법이 없습니다.

### 해결

binary_path를 파라미터로 추가:

```rust
async fn run_single_agent(
    // ...기존 파라미터...
    binary_path: &Path,   // ← 추가
) -> SingleResult {
    // ...
    let mut cmd = Command::new(binary_path);
}

// 호출부 (impl AgentTool for SubagentTool)
let result = run_single_agent(
    &self.cwd, &agents, agent_name, task,
    &self.get_binary(),  // ← 여기서 get_binary() 호출
    ...
).await;
```

`run_parallel`에도 전파 필요.

---

## 🟡 Medium: 항목 5 — UsageStats가 AgentEvent::Usage보다 풍부

### 문제

`AgentEvent::Usage`는 두 필드만 있습니다:
```rust
Usage {
    input_tokens: usize,
    output_tokens: usize,
}
```

하지만 `UsageStats`는 6개 필드:
```rust
pub struct UsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,    // ← 항상 0
    pub cache_write: u64,    // ← 항상 0
    pub cost: f64,           // ← 항상 0.0
    pub turns: u32,
}
```

설계서는 `Usage` 이벤트를 활용하라고 하지만, cache_read, cache_write, cost는
**절대 채워지지 않습니다.**

### 해결

둘 중 하나:

**옵션 A:** UsageStats에서 채울 수 없는 필드를 제거:
```rust
pub struct UsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u32,
}
```

**옵션 B:** AgentEvent::Usage에 필드를 확장 (더 큰 변경):
```rust
Usage {
    input_tokens: usize,
    output_tokens: usize,
    cache_read: usize,
    cache_write: usize,
}
```

이건 oxi-ai provider 레벨에서 cache 정보를 전달해야 하므로 큰 작업.

**추천:** 옵션 A. 지금은 사용 가능한 필드만 추적하고, 나중에 provider가
cache 정보를 제공하면 확장.

---

## 🟡 Medium: 항목 4 — Abort 후 5초 무조건 대기

### 문제

```rust
unsafe { libc::kill(pid as i32, libc::SIGTERM); }
tokio::time::sleep(Duration::from_secs(5)).await;
let _ = child.start_kill();
```

자식이 1초 안에 종료되도 5초를 기다립니다.

### 해결

```rust
#[cfg(unix)]
{
    if let Some(pid) = child.id() {
        unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    }
    // 자식이 5초 안에 종료되면 바로 진행
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    tokio::select! {
        _ = &mut deadline => {
            let _ = child.start_kill(); // SIGKILL
        }
        status = child.wait() => {
            // 정상 종료됨
            if let Ok(s) = status { result.exit_code = s.code().unwrap_or(1); }
            return result;
        }
    }
}
```

---

## 🟡 Medium: 항목 1 — print_mode.rs와 run_single_prompt의 이원화

### 문제

현재 main.rs의 single prompt 모드는 `app.run_interactive()`를 사용하고,
print_mode.rs는 별도의 `run_print_mode()` 함수로 되어 있습니다.

설계는 `--mode json`을 print_mode.rs로 라우팅하라고 하지만,
print_mode.rs는 `app.agent()`를 사용합니다 (경로 A).
반면 TUI 모드는 경로 B.

두 경로의 Agent가 다르므로, 도구 등록도 따로 해야 합니다.
설계가 이 이원화를 명시적으로 다루지 않습니다.

### 해결

"누가 Agent를 소유하는가"를 명확히 하는 것이 선결 과제.

현재 구조:
```
main.rs
  ├─ TUI 모드 → create_agent_session_from_services() → agent_2
  └─ print 모드 → app.agent() → agent_1
```

이 구조를 유지하려면 두 경로 모두에 동일한 ToolRegistry를 전달해야 합니다.
또는 App이 ToolRegistry를 직접 관리하고 session이 이를 clone하게 만들어야 합니다.

---

## 🟢 Minor

1. **항목 7 (.git 순회):** 파일시스템 루트에서 `current.parent()?`가 None을 반환하므로 무한 루프는 안 됨. OK.

2. **항목 6 (TempDirGuard):** `remove_dir_all`이 실패해도 `let _`로 무시하므로 OK. 다만 Windows에서 다른 프로세스가 파일을 잡고 있으면 삭제 실패 가능. 큰 문제는 아님.

3. **항목 8 (스트리밍):** `ProgressFn`을 `run_parallel`의 각 태스크에 전달하는 방법이 설계에 명시되지 않음. 각 태스크가 Arc<ProgressFn>을 clone해서 가지면 됨.

---

## 최종 평가

| 항목 | 상태 | 비고 |
|------|------|------|
| P0: Builtin 도구 등록 | ❌ 불완전 | 두 Agent 인스턴스 문제 미해결. extension 도구 TUI 유실 |
| 항목 1: CLI 인자 | ⚠️ | print_mode 경로와의 이원화 미해결 |
| 항목 2: --tools 필터링 | ⚠️ | P0 수정 후 재검증 필요 |
| 항목 3: 프로세스 스폰 | ❌ | free function에서 get_binary() 호출 불가 |
| 항목 4: Abort | ⚠️ | 5초 무조건 대기 → 조건부 대기로 개선 필요 |
| 항목 5: Usage | ⚠️ | UsageStats 6개 필드 중 3개만 채워짐 |
| 항목 6: 임시 파일 | ✅ | OK |
| 항목 7: 프로젝트 순회 | ✅ | OK |
| 항목 8: 스트리밍 | ⚠️ | 병렬 모드 진행 상황 머지 방법 미명시 |

**다음 단계:** 두 Agent 인스턴스 문제를 먼저 해결하는 설계를 추가해야 합니다.
이것이 모든 항목의 전제 조건입니다.
