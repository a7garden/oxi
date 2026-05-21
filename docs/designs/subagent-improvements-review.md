# Subagent Tool 개선 설계서 — 리뷰

## 치명적 누락: oxi에 builtin 도구 등록 코드가 없음

설계서를 작성하면서 `--tools` 전달 (항목 3)을 설계했지만, **현재 oxi는 Agent에 builtin 도구를 등록하는 코드가 전혀 없습니다.**

```
Agent::new() → ToolRegistry::new() (빈 상태)
App::new()   → Agent::new() 호출, 도구 등록 안 함
main.rs      → extension tools만 register_arc()
```

즉, **현재 oxi를 실행하면 에이전트가 사용할 수 있는 도구가 0개**입니다.
이것은 subagent 설계 이전에 해결해야 할 근본적인 문제입니다.

**해결:** `create_agent_session_from_services()` 또는 `App::new()`에서
`ToolRegistry::with_builtins_cwd(cwd)`를 사용하여 기본 도구를 등록해야 합니다.

---

## 항목별 리뷰

### 1. 프로세스 스폰 — ⚠️ 불완전

**문제:** `current_exe()`는 oxi-agent 라이브러리를 사용하는 모든 호스트 바이너리의 경로를 반환합니다. oxi-cli로 실행하면 정상 작동하지만, 다른 애플리케이션이 oxi-agent를 임베드하면 잘못된 바이너리를 스폰합니다.

**해결:** 실행 파일 경로를 설정 가능하게 해야 합니다:
```rust
pub struct SubagentTool {
    cwd: PathBuf,
    binary_path: Option<PathBuf>, // None이면 current_exe()
}
```

pi-mono의 `getPiInvocation()`도 같은 문제를 가지고 있으므로 이것은 개선입니다.

### 2. Usage 추적 — ❌ 근본적 설계 오류

**문제:** 설계서는 `print_mode.rs`의 `complete` 이벤트에 usage를 추가하라고 하지만:

1. `AgentEvent::Complete`의 실제 필드는 `content: String, stop_reason: String` 뿐. **Message나 usage 정보가 없음.**
2. `AgentEvent::Usage` 이벤트가 별도로 존재하지만 `event_to_json()`에서 `"unknown"`으로 처리됨
3. `print_mode.rs`의 `event_to_json()`에 아예 `Usage` 매치가 없음

**수정된 해결:**

`event_to_json()`에 `Usage` 이벤트 매치를 추가하는 것이 먼저:
```rust
AgentEvent::Usage { input_tokens, output_tokens } => json!({
    "type": "usage",
    "input_tokens": input_tokens,
    "output_tokens": output_tokens,
})
```

그 다음 subagent.rs에서 `usage` 이벤트를 누적 수집. `complete` 이벤트 수정은 불필요.

### 3. --tools 전달 — ❌ 선행 작업 누락

**문제 1:** oxi CLI에 `--tools` 인자가 존재하지 않음. 이것을 추가하려면:
- `cli.rs`: 인자 파싱
- `App::new()` 또는 `create_agent_session_from_services()`: 전달받은 도구 목록으로 ToolRegistry 필터링
- 하지만 **현재 builtin 도구 자체가 등록되지 않는 버그**가 있으므로 이것부터 고쳐야 함

**문제 2:** 설계서에서 "ToolRegistry 필터링"이라고만 하고 구체적인 구현 경로가 없음.
`ToolRegistry::with_builtins_cwd()`가 모든 도구를 등록하는데, 여기서 선택적으로 등록하려면:
```rust
pub fn with_selected_tools(cwd: PathBuf, tools: &[&str]) -> Self {
    // tools에 있는 것만 등록
}
```
또는 전체 등록 후 미사용 도구 제거.

**문제 3:** `--no-session`, `--append-system-prompt` 플래그도 oxi CLI에 존재하지 않을 가능성이 높음.
현재 subagent.rs가 이 인자들을 전달하지만 실제로 동작하는지 검증 필요.

### 4. Abort 처리 — ❌ 구현 불가능한 설계

**문제 1:** `start_kill()`은 SIGKILL을 보냅니다. SIGTERM이 아닙니다.
설계서는 "SIGTERM → 5초 → SIGKILL"이라고 하지만 `start_kill()`만으로는 SIGTERM을 보낼 수 없습니다.

Unix에서 SIGTERM을 보내려면:
```rust
#[cfg(unix)]
fn send_sigterm(pid: u32) -> std::io::Result<()> {
    let pid = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if pid == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}
```

**문제 2:** `tokio::select!` 안에서 `lines.next_line()`을 사용하면,
select가 취소될 때 `lines`의 소유권 문제가 발생합니다. 매 반복마다 `lines`를 빌려야 하므로:
```rust
// 이건 안 됨:
tokio::select! {
    line = lines.next_line() => { ... }  // lines가 moved
    _ = signal_rx => { ... }
}
// 다음 반복에서 lines를 사용할 수 없음
```

**수정된 해결:** 읽기 루프를 별도 태스크로 분리하고 채널로 통신:
```rust
let (line_tx, mut line_rx) = mpsc::channel(100);
let reader = tokio::spawn(async move {
    while let Ok(Some(line)) = lines.next_line().await {
        let _ = line_tx.send(line).await;
    }
});

loop {
    tokio::select! {
        line = line_rx.recv() => { /* 처리 */ }
        _ = &mut signal_rx => {
            send_sigterm(child.id());
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = child.start_kill(); // SIGKILL
            break;
        }
    }
}
```

### 5. 임시 파일 정리 — ⚠️ 타이밍 문제

**문제:** `TempDirGuard`가 `Drop`에서 파일을 삭제하지만,
서브에이전트 프로세스가 CLI 인자로 전달된 파일을 언제 읽는지 보장할 수 없습니다.

`--append-system-prompt`는 시스템 프롬프트 구성 시점에 파일을 읽을 것으로 예상되지만,
프로세스 스폰 직후에 읽지 않으면 race condition이 발생합니다.

**해결:** TempDirGuard를 `run_single_agent` 전체 생명주기 동안 유지.
프로세스가 종료되면(성공/실패/abort) 그때 Drop으로 정리. 현재 설계와 동일하지만
명시적으로 "함수 스코프 내에서 유지"라고 문서화해야 합니다.

### 6. 프로젝트 에이전트 순회 — ❌ 코드에 버그

**문제:** 설계서의 코드가 의미 없는 빈 블록을 포함:
```rust
if current.join(".git").is_dir() {
    // 여기서 아무것도 안 함! 루프가 계속됨
}
```
주석은 ".git에서 중단"이라고 하지만 실제로는 중단하지 않습니다.

**수정된 해결:**
```rust
fn find_project_agents_dir(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let candidate = current.join(".oxi").join("agents");
        if candidate.is_dir() {
            return Some(candidate);
        }
        // .git이 있으면 프로젝트 루트. 더 이상 올라가지 않음
        if current.join(".git").exists() {
            return None;
        }
        current = current.parent()?;
    }
}
```

### 7. 스트리밍 진행 상황 — ⚠️ 콜백 전달 경로 누락

**문제:** `on_progress()` 콜백을 `SubagentTool`에 저장하지만,
`run_single_agent()`는 `&self`가 없는 free function입니다.
콜백을 어떻게 전달하는지 설계에 없음.

**해결:** `run_single_agent()`에 콜백 파라미터 추가:
```rust
async fn run_single_agent(
    // ...기존 파라미터...
    on_progress: Option<&dyn Fn(String)>,
) -> SingleResult
```

병렬 모드에서는 여러 에이전트의 진행 상황을 하나의 콜백으로 머지해야 함.

### 8. TUI 렌더링 — ⚠️ 구체성 부족

"oxi의 TUI 렌더링 시스템을 확인 후"는 설계가 아닌 TODO입니다.
최소한 metadata의 JSON 스키마를 정의해야 합니다.

---

## 우선순위 재조정

| 순서 | 항목 | 이유 |
|------|------|------|
| 0 | **Builtin 도구 등록 버그 수정** | 이것 없이는 oxi 자체가 동작하지 않음 |
| 1 | **CLI 인자 검증** (`--no-session`, `--append-system-prompt`) | subagent가 존재하지 않는 인자를 전달하고 있을 가능성 |
| 2 | 프로세스 스폰 (`current_exe`) | 설정 가능하게 |
| 3 | Abort (채널 기반 select!) | SIGTERM→SIGKILL 패턴 수정 |
| 4 | Usage (Usage 이벤트 매치 추가) | complete가 아닌 Usage 이벤트 활용 |
| 5 | 임시 파일 정리 (RAII) | 타이밍 명시 |
| 6 | 프로젝트 순회 (.git 경계) | 버그 수정 |
| 7 | --tools 전달 | 선행: 도구 등록 + CLI 인자 먼저 |
| 8 | 스트리밍 | 콜백 전달 경로 설계 필요 |
| 9 | TUI | 별도 설계서 필요 |

## 결론

**설계서의 8개 항목 중 3개는 구현 불가능하거나 근본적으로 잘못된 설계**이고,
**2개의 치명적 누락**이 발견되었습니다:

1. oxi에 builtin 도구 등록 코드가 없음 (subagent 이전의 근본 문제)
2. CLI 인자 검증 없이 존재하지 않을 수 있는 플래그를 사용

수정된 설계서를 작성하기 전에 먼저 두 가지를 해결해야 합니다.
