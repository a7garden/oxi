# Subagent Tool 개선 설계서

## 개요

pi-mono의 서브에이전트 확장과 비교하여 현재 oxi 구현의 8가지 문제점을 개선하는 설계.

---

## 1. 프로세스 스폰: 현재 실행 파일 재사용

### 문제
`Command::new("oxi")`로 하드코딩 → PATH에 없거나 다른 이름으로 빌드되면 동작 안 함.

### 해결
```rust
use std::env::current_exe;

fn get_oxi invocation() -> PathBuf {
    // 현재 실행 파일 경로를 그대로 사용
    // oxi, oxi-debug, ./target/debug/oxi 등 모두 동작
    current_exe().unwrap_or_else(|_| PathBuf::from("oxi"))
}
```

pi-mono의 `getPiInvocation()`과 동일한 철학. 현재 프로세스가 곧 서브에이전트 실행 파일.

### 변경 파일
- `subagent.rs`: `run_single_agent()`에서 `Command::new("oxi")` → `Command::new(get_oxi_invocation())`

---

## 2. Usage/토큰/비용 추적

### 문제
`text_delta` 이벤트만 수집하고 `message_end`의 usage 데이터를 무시.
토큰 사용량, 비용, 컨텍스트 크기를 알 수 없음.

### 해결
oxi의 `--mode json` 출력을 확인하면 `message_end` (또는 `complete`) 이벤트에 usage 정보가 포함됨.
이를 파싱하여 `UsageStats`에 누적.

```rust
// print_mode.rs의 event_to_json이 출력하는 필드 확인 필요
// 현재 complete 이벤트는 단순히 {"type": "complete"}만 출력하므로
// print_mode.rs도 수정하여 usage 정보를 포함해야 함

// subagent.rs에서 파싱:
match event_type {
    "complete" => {
        result.usage.input_tokens += event["usage"]["input"].as_u64().unwrap_or(0);
        result.usage.output_tokens += event["usage"]["output"].as_u64().unwrap_or(0);
        // ...
    }
}
```

### 선행 작업
`print_mode.rs`의 `event_to_json()`에서 `complete` 이벤트에 usage 포함:
```rust
AgentEvent::Complete { message, .. } => serde_json::json!({
    "type": "complete",
    "usage": message.usage().map(|u| json!({
        "input": u.input_tokens,
        "output": u.output_tokens,
        "cache_read": u.cache_read_tokens,
        "cache_write": u.cache_write_tokens,
        "total_tokens": u.total_tokens,
        "cost": u.cost,
    }))
})
```

### 변경 파일
- `oxi-cli/src/print_mode.rs`: complete 이벤트에 usage 추가
- `oxi-agent/src/tools/subagent.rs`: JSON 파싱에서 usage 수집

---

## 3. 에이전트별 도구 제한 (--tools 전달)

### 문제
에이전트 정의에 `tools: read, grep, find, ls`가 있어도 스폰 시 전달하지 않음.
scout이 bash/write를 사용할 수 있게 됨 → 보안상 위험.

### 해결
```rust
if let Some(ref tools) = agent.tools {
    if !tools.is_empty() {
        args.push("--tools".to_string());
        args.push(tools.join(","));
    }
}
```

### 선행 작업
oxi CLI에 `--tools` 인자가 있는지 확인 필요. 없으면 추가.

```
oxi --mode json -p --tools read,grep,find,ls "Task: ..."
```

이 인자는 ToolRegistry에 등록할 도구를 필터링하는 역할.

### 변경 파일
- `oxi-cli/src/cli.rs`: `--tools` 인자 파싱 추가
- `oxi-cli/src/print_mode.rs`: 전달받은 도구 목록으로 ToolRegistry 필터링
- `oxi-agent/src/tools/subagent.rs`: args에 `--tools` 추가

---

## 4. Abort 처리

### 문제
`signal: Option<oneshot::Receiver<()>>`를 완전히 무시.
긴 체인/병렬 작업을 취소할 방법이 없음.

### 해결
```rust
// tokio::select!로 signal 대기와 프로세스 출력을 동시에 모니터링
tokio::select! {
    line = lines.next_line() => { /* 정상 처리 */ }
    _ = async {
        if let Some(mut rx) = signal {
            let _ = rx.await;
        } else {
            std::future::pending::<()>().await;
        }
    } => {
        // abort: SIGTERM → 5초 대기 → SIGKILL
        let _ = child.start_kill();
        tokio::time::sleep(Duration::from_secs(5)).await;
        if child.id().is_some() {
            let _ = child.start_kill(); // force kill
        }
    }
}
```

pi-mono의 패턴과 동일: SIGTERM → 5초 타임아웃 → SIGKILL

### 변경 파일
- `oxi-agent/src/tools/subagent.rs`: `run_single_agent()`에 select! 추가

---

## 5. 임시 파일 정리

### 문제
`std::env::temp_dir().join("oxi-subagent-{uuid}")`에 시스템 프롬프트를 쓰지만
삭제하지 않아 임시 파일이 누적됨.

### 해결
```rust
// Drop 기반 RAII 가드
struct TempDirGuard {
    path: PathBuf,
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// 사용
let tmp_dir = TempDirGuard {
    path: std::env::temp_dir().join(format!("oxi-subagent-{}", uuid::Uuid::new_v4())),
};
std::fs::create_dir_all(&tmp_dir.path)?;
// ... 사용 ...
// Drop이 자동으로 정리
```

RAII 패턴으로 panic/unwind 시에도 정리 보장.

### 변경 파일
- `oxi-agent/src/tools/subagent.rs`: `TempDirGuard` 추가

---

## 6. 프로젝트 에이전트 디렉토리 순회

### 문제
`cwd/.oxi/agents/`만 확인. git worktree나 하위 디렉토리에서 실행하면
루트 프로젝트의 `.oxi/agents/`를 찾지 못함.

pi-mono는 `findNearestProjectAgentsDir()`로 상위 디렉토리를 순회.

### 해결
```rust
fn find_project_agents_dir(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let candidate = current.join(".oxi").join("agents");
        if candidate.is_dir() {
            return Some(candidate);
        }
        // .git 디렉토리를 만나면 여기가 프로젝트 루트라고 간주하고 중단
        if current.join(".git").is_dir() {
            // .git이 있지만 .oxi/agents가 없으면 한 단계 더 확인하지 않음
            // (대부분의 경우 프로젝트 루트에 .oxi/agents가 있음)
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}
```

pi-mono는 무한 순회지만, oxi는 `.git` 기준으로 루트를 판단하는 것이 더 정확.
(`.git`이 있는 곳이 프로젝트 루트)

### 변경 파일
- `oxi-agent/src/tools/subagent.rs`: `discover_agents()` 수정

---

## 7. 스트리밍 진행 상황

### 문제
현재는 서브에이전트가 완전히 완료된 후에야 결과를 반환.
병렬에서는 어떤 작업이 진행 중인지, 체인에서는 어느 스텝인지 알 수 없음.

### 해결
`AgentTool::on_progress()` 콜백을 활용하여 실시간 업데이트 전달.

```rust
// AgentTool trait에 이미 있음:
fn on_progress(&self, _callback: ProgressCallback) {}

// SubagentTool에서 활용:
fn on_progress(&self, callback: ProgressCallback) {
    self.progress_callback = Some(callback);
}

// run_single_agent 실행 중:
if let Some(ref cb) = self.progress_callback {
    cb(format!("⏳ [{}] running...", agent_name));
}

// 병렬 모드:
cb(format!("⏳ Parallel: {}/{} done, {} running", done, total, running));
```

TUI에서는 이 콜백을 받아 진행 상태를 실시간 표시.

### 변경 파일
- `oxi-agent/src/tools/subagent.rs`: `on_progress()` 구현, `run_single_agent`에 콜백 전달

---

## 8. TUI 렌더링

### 문제
pi-mono는 전용 `renderCall`/`renderResult`로 서브에이전트 호출과 결과를
테마 색상으로 예쁘게 렌더링. oxi는 기본 텍스트만 출력.

### 해결
현재 oxi의 TUI 렌더링 시스템을 확인 후, 서브에이전트 전용 렌더링 추가.

pi-mono의 렌더링 설계:
- **호출**: `subagent scout [user]` + 작업 미리보기
- **병렬**: `subagent parallel (3 tasks)` + 각 에이전트 나열
- **체인**: `subagent chain (3 steps)` + 각 스텝 나열
- **결과 축소**: ✓/✗ 아이콘 + 마지막 5개 항목 + usage 통계
- **결과 확대**: 전체 작업 + 도구 호출 + 마크다운 출력

oxi에서는 `AgentToolResult`의 `metadata` 필드에 구조화된 데이터를 담고,
TUI 측에서 이를 읽어 렌더링.

### 변경 파일
- `oxi-agent/src/tools/subagent.rs`: metadata에 렌더링 정보 포함
- `oxi-tui/`: 서브에이전트 전용 렌더러 (별도 이슈로 분리 가능)

---

## 구현 우선순위

| 순서 | 항목 | 난이도 | 영향 |
|------|------|--------|------|
| 1 | 프로세스 스폰 수정 | ⭐ | Critical — 없으면 작동 안 함 |
| 2 | Abort 처리 | ⭐⭐ | Critical — 긴 작업 취소 불가 |
| 3 | 임시 파일 정리 | ⭐ | Medium — 리소스 누수 |
| 4 | 프로젝트 에이전트 순회 | ⭐ | Medium — 하위 디렉토리에서 실행 시 문제 |
| 5 | --tools 전달 | ⭐⭐ | Critical — 보안 (CLI 인자 추가 필요) |
| 6 | Usage 추적 | ⭐⭐⭐ | Medium — print_mode.rs 수정 필요 |
| 7 | 스트리밍 진행 상황 | ⭐⭐ | Nice-to-have |
| 8 | TUI 렌더링 | ⭐⭐⭐ | Nice-to-have — TUI 시스템 이해 필요 |

---

## 파일 변경 요약

```
oxi-agent/src/tools/subagent.rs   — 모든 개선 적용
oxi-cli/src/print_mode.rs         — complete 이벤트에 usage 추가
oxi-cli/src/cli.rs                — --tools 인자 파싱
oxi-cli/src/print_mode.rs         — --tools로 ToolRegistry 필터링
```

## 호환성 고려

- `--tools` CLI 인자는 선택 사항. 지정하지 않으면 기존처럼 모든 도구 사용.
- print_mode.rs의 complete 이벤트 확장은 JSON 소비자에게만 영향. 기존 필드 유지.
- `current_exe()`는 모든 플랫폼에서 동작 (Linux, macOS, Windows).
