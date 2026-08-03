# Subagent Tool 개선 설계서 v3 — 리뷰

## 🔴 Critical: libc 의존성 누락

항목 4(Abort)에서 `libc::kill(pid, SIGTERM)`을 사용하지만,
`oxicode-agent/Cargo.toml`에 `libc`가 없습니다.

**현재 의존성:**
```toml
# oxicode-agent/Cargo.toml
[target.'cfg(unix)'.dependencies]
# libc = "0.2"   ← 없음
```

**해결:** `libc`를 Unix 타겟 의존성으로 추가:

```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

oxicode-cli에는 이미 `libc = "0.2"`가 있으므로 워크스페이스에 새로 추가되는 의존성은 아님.

---

## 🔴 Critical: 항목 0 — App::new() 시점에 cwd를 모름

설계서:
```rust
let app = oxicode::App::new(settings).await?;
let tools = app.agent_tools();
let builtins = oxicode_agent::ToolRegistry::with_builtins_cwd(cwd.clone());
```

그런데 `cwd`는 `main.rs`의 로컬 변수입니다. 현재 main.rs에서:
```rust
let prompt = args.prompt.join(" ");
let app = oxicode::App::new(settings).await?;
```

`std::env::current_dir()`를 main.rs에서 아직 호출하지 않습니다.
TUI 모드는 `tui_interactive.rs` 내부에서 `std::env::current_dir()`를 호출합니다.

**해결:** main.rs에서 cwd를 명시적으로 가져와야 합니다:

```rust
let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
let app = oxicode::App::new(settings).await?;

let builtin_registry = if let Some(ref tools_str) = args.tools {
    oxicode_agent::ToolRegistry::with_selected_tools(cwd.clone(), &names)
} else {
    oxicode_agent::ToolRegistry::with_builtins_cwd(cwd.clone())
};
```

미미한 문제이지만 설계에 명시되어야 합니다.

---

## 🟡 Medium: 항목 1 — --append-system-prompt 처리가 구체적이지 않음

설계서:
> `--append-system-prompt`는 main.rs에서 settings에 반영 후 App 생성 시 반영.

현재 `build_system_prompt()`는 `thinking_level`과 `skill_contents`만 받습니다.
`--append-system-prompt` 파일 내용을 추가하는 경로가 없습니다.

**해결 (구체화):**

```rust
// main.rs
let append_prompt = args.append_system_prompt.as_ref()
    .map(|p| std::fs::read_to_string(p))
    .transpose()?;

let app = oxicode::App::new_with_append(settings, append_prompt.as_deref()).await?;
```

또는 App 생성 후:
```rust
let app = oxicode::App::new(settings).await?;
if let Some(ref prompt_path) = args.append_prompt {
    let content = std::fs::read_to_string(prompt_path)?;
    let current = app.agent().config().system_prompt.clone().unwrap_or_default();
    app.agent().set_system_prompt(format!("{}\n\n{}", current, content));
}
```

두 번째 옵션이 더 간단하고 `Agent::set_system_prompt()`가 이미 존재합니다.

---

## 🟡 Medium: 항목 6 — TempDirGuard 에러 전파

`TempDirGuard::new()`가 `std::io::Result`를 반환하지만,
`run_single_agent`는 `Result<SingleResult, ToolError>`이고 `ToolError = String`.

설계서:
```rust
let tmp_dir = TempDirGuard::new("oxicode-subagent")?;
```

`?` 연산자가 `io::Error`를 `String`으로 변환하려면 `From` 구현이 필요.
현재 `From<io::Error> for String`이 없으므로 컴파일 안 됨.

**해결:**

```rust
let tmp_dir = TempDirGuard::new("oxicode-subagent")
    .map_err(|e| format!("Failed to create temp dir: {}", e))?;
```

---

## 🟡 Medium: 항목 5 — UsageStats 필드 제거는 breaking change

설계서가 `cache_read`, `cache_write`, `cost`를 UsageStats에서 제거하라고 합니다.
현재 이 필드들은 `pub`이므로 외부 코드에서 접근할 가능성이 있습니다.

**해결:** 제거 대신 `#[serde(skip_serializing_if = "...")]`로 직렬화에서 숨기거나,
그대로 두고 항상 0으로 문서화. 필드 제거는 minor 버전에서 breaking change.

권장: 그대로 두고 `turns`만 새로 추가. 나중에 `AgentEvent::Usage`가 확장되면 채우는 것으로.

---

## 🟡 Medium: 항목 4 — abort 후 stderr_handle 수집 안 함

abort 경로에서 `break`로 빠져나간 후 stderr_handle을 await하지 않습니다.
이것은 의도적(abort 시 stderr는 무시)이지만, stderr_handle을 drop하면
백그라운드 태스크가 취소되지 않고 좀비로 남을 수 있습니다.

**해결:** abort 경로에서도 stderr_handle을 await (timeout과 함께):

```rust
if aborted {
    // ...kill child...
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        stderr_handle
    ).await;
    break;
}
```

---

## 🟢 Minor

1. **항목 8:** 설계에서 `ProgressFn`을 새 타입으로 정의하지만, 이미 `tools.rs`에 `ProgressCallback = Arc<dyn Fn(String) + Send + Sync>`가 있습니다. 그대로 재사용하면 됨.

2. **항목 0:** 설계가 `lib.rs 변경 없음`이라고 하는데, 실제로 main.rs에서만 처리하므로 맞음. 정확함.

3. **항목 7:** `.git` 체크에서 `.git` 파일(git worktree의 파일)도 고려해야 할 수 있음.
   `current.join(".git").exists()`는 `.git` 파일도 true를 반환하므로 OK.
   (worktree에서 `.git`은 파일이고 내용은 `.git` 디렉토리 경로)

4. **항목 4:** `libc::kill`에 `use std::os::unix::process::ExitStatusExt`가 필요 없음.
   `child.id()`는 `Option<u32>`를 반환하고 `libc::kill`은 `pid_t`를 받음.
   타입 변환은 설계에 있는 대로 `pid as i32`로 OK.

---

## 최종 평가

| 항목 | 상태 | 비고 |
|------|------|------|
| 항목 0: ToolRegistry | ⚠️ | cwd 획득 누락, 본질은 OK |
| 항목 1: CLI 인자 | ⚠️ | --append-system-prompt 처리 구체화 필요 |
| 항목 2: --tools | ✅ | OK |
| 항목 3: 프로세스 스폰 | ✅ | OK |
| 항목 4: Abort | ❌ | libc 의존성 추가 필요 + stderr_handle 좀비 |
| 항목 5: Usage | ⚠️ | 필드 제거 대신 유지 권장 |
| 항목 6: 임시 파일 | ⚠️ | io::Error → ToolError 변환 누락 |
| 항목 7: 프로젝트 순회 | ✅ | OK |
| 항목 8: 스트리밍 | ⚠️ | ProgressFn → ProgressCallback 재사용 |

**v3는 v2의 근본적 문제(두 Agent 인스턴스, free function binary_path)를
모두 해결했습니다.** 남은 것은 사소한 누락뿐입니다:

1. `libc` 의존성 추가
2. `cwd` 획득
3. `--append-system-prompt` 구체적 처리
4. 에러 변환 (`io::Error → String`)
5. UsageStats 필드 유지

이 5가지만 보완하면 구현 가능한 설계입니다.
