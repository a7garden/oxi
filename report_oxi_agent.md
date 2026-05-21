# oxi-agent 심층 코드 분석 보고서

**분석 대상:** `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/` (약 17,879줄, 50개 파일)  
**분석 일자:** 2026-05-14  
**분석자:** AI Code Analyst  

---

## 목차

1. [아키텍처 개요](#1-아키텍처-개요)
2. [에이전트 루프 상태 머신](#2-에이전트-루프-상태-머신)
3. [도구 시스템 설계](#3-도구-시스템-설계)
4. [개별 도구 구현 분석](#4-개별-도구-구현-분석)
5. [재시도 및 스트리밍 재시도 로직](#5-재시도-및-스트리밍-재시도-로직)
6. [상태 관리 및 동시성](#6-상태-관리-및-동시성)
7. [MCP 통합 품질](#7-mcp-통합-품질)
8. [파일 뮤테이션 큐 및 편집 충돌 해결](#8-파일-뮤테이션-큐-및-편집-충돌-해결)
9. [경로 보안 및 샌드박싱](#9-경로-보안-및-샌드박싱)
10. [컴팩션 전략 효과성](#10-컴팩션-전략-효과성)
11. [프록시 구성 처리](#11-프록시-구성-처리)
12. [종합 평가 및 권장사항 요약](#12-종합-평가-및-권장사항-요약)

---

## 1. 아키텍처 개요

oxi-agent는 **2-레벨 에이전트 루프 아키텍처**를 구현합니다:

```
Agent (agent.rs)
  └── AgentLoop (agent_loop/mod.rs)
        ├── 내부 루프: 도구 호출 + 스티어링
        │     ├── stream_assistant_response (streaming.rs)
        │     ├── execute_tool_calls (tool_exec.rs)
        │     └── should_stop_after_turn (helpers.rs)
        └── 외부 루프: 팔로우업 메시지
              └── follow_up_queue
```

**강점:**
- `Agent`와 `AgentLoop`의 분리를 통해 상태 격리가 잘 됨
- 이벤트 기반 아키텍처(`AgentEvent`)로 UI/TUI 연동이 유연함
- `parking_lot::RwLock` 기반의 락-프리 읽기 최적화

**약점:**
- `Agent`의 `run_with_channel_inner()`이 `AgentLoop`에 **새 SharedState를 생성하여 전달**하고, 완료 후 다시 복사해오는 방식이 비효율적 (`agent.rs:265-272`)
- `AgentLoopConfig`가 `Clone`이 필요하지만 일부 필드는 불필요한 복사 발생

---

## 2. 에이전트 루프 상태 머신

### 파일: `agent_loop/mod.rs`

에이전트 루프는 명시적 상태 머신 없이 `loop`/`while` 중첩으로 구현되어 있습니다:

```
run_loop()
  loop {                                    ← 외부 루프 (팔로우업)
    while has_more_tool_calls || pending {  ← 내부 루프
      1. pending 메시지 주입
      2. maybe_compact()
      3. stream_assistant_response()
      4. 에러 시 재시도 판별
      5. 도구 호출 실행
      6. should_stop_after_turn 체크
      7. 스티어링 큐 drain
    }
    follow_up_queue drain
    break
  }
```

### 이슈 #1 — `first_turn` 플래그 로직 불필요한 복잡성
**심각도:** Low  
**위치:** `agent_loop/mod.rs:199-210`

```rust
if !first_turn {
    turn_number += 1;
    emit(AgentEvent::TurnStart { turn_number });
} else {
    first_turn = false;
    turn_number = 1;
    emit(AgentEvent::TurnStart { turn_number });
}
```

양쪽 분기 모두 `emit(TurnStart)`를 호출하므로, 단순히 `turn_number += 1` 후 emit하는 것으로 충분합니다. `first_turn` 플래그는 0-based → 1-based 변환을 위해 존재하지만, `turn_number`를 0으로 시작시켜 루프 상단에서 증가시키는 방식이 더 깔끔합니다.

**개선 제안:**
```rust
turn_number += 1;
emit(AgentEvent::TurnStart { turn_number });
```

### 이슈 #2 — `is_retryable_error` 정규식이 과도하게 넓음
**심각도:** Medium  
**위치:** `agent_loop/retry.rs:55-66`

```rust
r"(?i)overloaded|provider.?returned.?error|rate.?limit|too many requests\
 |429|500|502|503|504|service.?unavailable|server.?error|internal.?error\
 |network.?error|connection.?error|connection.?refused|connection.?lost\
 |other side closed|fetch failed|upstream.?connect|reset before headers\
 |socket hang up|ended without|http2 request did not get a response\
 |timed? out|timeout|terminated|retry delay"
```

`"terminated"`와 `"internal.?error"` 패턴은 정상적인 종료나 논리적 오류(예: 컨텍스트 길이 초과)도 재시도 대상으로 분류할 수 있습니다. 특히 `"internal.?error"`는 "internal error: context length exceeded" 같은 비-재시도 오류와도 매칭됩니다.

**개선 제안:** `terminated` 제거, `internal.?error` 대신 `internal.?server.?error`와 같이 더 구체적인 패턴 사용. 또한 오류 응답에 포함된 HTTP 상태 코드를 직접 파싱하여 매칭 정확도 향상.

### 이슈 #3 — `handle_retryable_error`의 취소 대기 경쟁 조건
**심각도:** Medium  
**위치:** `agent_loop/retry.rs:106-116`

```rust
tokio::select! {
    _ = tokio::time::sleep(...) => {}
    _ = tokio::task::yield_now() => {
        if loop_ref.auto_retry_cancel.load(Ordering::SeqCst) { ... }
    }
}
```

`tokio::task::yield_now()`는 한 번만 실행되며 즉시 반환되는 future입니다. 취소 신호를 폴링하려는 의도라면, `yield_now()` 대신 `tokio::sync::watch` 또는 `tokio::time::sleep` 내에서 주기적으로 플래그를 확인하는 방식이 필요합니다. 현재 구현은 사실상 yield 후 즉시 체크하는 1회성 검사이며, sleep 이후에도 취소 플래그를 다시 체크합니다.

**개선 제안:** `yield_now` 분기 대신 `tokio::sync::Notify` 또는 단순히 sleep 전후로 플래그를 체크하는 구조로 변경.

### 이슈 #4 — `should_stop_after_turn`가 반복 카운트를 매번 재계산
**심각도:** Low  
**위치:** `agent_loop/helpers.rs:37-44`

```rust
let current_iteration = _messages.iter()
    .filter(|m| matches!(m, oxi_ai::Message::Assistant(_)))
    .count();
if current_iteration >= max_iterations {
    return true;
}
```

메시지 리스트가 길어질 경우 매 턴마다 O(n) 순회가 발생합니다. `AgentLoop`에 이미 `turn_number`가 있으므로 이를 직접 전달하는 것이 효율적입니다.

**개선 제안:** `turn_number`를 파라미터로 받아 `turn_number >= max_iterations`로 단순 비교.

---

## 3. 도구 시스템 설계

### 파일: `tools.rs`

**강점:**
- `AgentTool` 트레이트가 깔끔하게 추상화됨 (`name`, `label`, `description`, `parameters_schema`, `execute`, `on_progress`)
- `ToolRegistry`가 `Arc<dyn AgentTool>` 기반으로 스레드 안전
- `with_builtins_cwd()`에서 비활성화 도구 목록 지원

### 이슈 #5 — `ToolRegistry::with_builtins_cwd`의 `OnceCell` 캐시 공유 문제
**심각도:** Medium  
**위치:** `tools.rs:217-225`

```rust
let cache_once: std::cell::OnceCell<Arc<search_cache::SearchCache>> = std::cell::OnceCell::new();
// ...
Box::new(web_search::WebSearchTool::new(
    cache_once.get_or_init(|| Arc::new(search_cache::SearchCache::new())).clone()
)),
Box::new(search_cache::GetSearchResultsTool::new(
    cache_once.get_or_init(|| Arc::new(search_cache::SearchCache::new())).clone()
)),
Box::new(github::GitHubTool::new(
    cache_once.get_or_init(|| Arc::new(search_cache::SearchCache::new())).clone()
)),
```

`OnceCell`이 함수 스코프에 있으므로 `get_or_init`이 여러 번 호출되어도 동일 인스턴스를 보장합니다. 그러나 `ToolRegistry::with_builtins_cwd()`가 여러 번 호출되면 매번 새로운 `SearchCache`가 생성됩니다. 서로 다른 레지스트리 간 캐시 공유가 되지 않아 검색 결과 재사용이 불가합니다.

**개선 제안:** `SearchCache`를 싱글톤(`OnceLock`)으로 관리하거나, `ToolRegistry` 빌더에서 캐시를 주입받는 방식으로 변경.

### 이슈 #6 — `AgentTool::to_definition`의 스키마 변환 손실
**심각도:** Low  
**위치:** `tools.rs:99-105`

```rust
fn to_definition(&self) -> ToolDefinition {
    ToolDefinition {
        name: self.name().to_string(),
        description: self.description().to_string(),
        input_schema: serde_json::from_value(self.parameters_schema()).unwrap_or_default(),
    }
}
```

`parameters_schema()`가 `serde_json::Value`를 반환하고, 이를 `HashMap<String, Value>`로 역직렬화합니다. JSON 스키마의 최상위가 `{"type": "object", "properties": {...}}` 형태가 아닌 경우 손실이 발생할 수 있습니다.

**개선 제안:** `ToolDefinition`의 `input_schema`를 `serde_json::Value`로 변경하여 손실 없이 저장.

### 이슈 #7 — `execute_tools_parallel`에서 `EmitFn`의 `Clone` 요구
**심각도:** Medium  
**위치:** `agent_loop/tool_exec.rs:123-137`

병렬 실행에서 `emit`을 `Arc`로 감싸서 클론하지만, `EmitFn` 타입이 `Arc<dyn Fn(AgentEvent) + Send + Sync>`이므로 이미 `Arc`입니다. 그러나 `execute_prepared_tool_call_static`에 전달할 때 `emit_clone.clone()`을 수행하며, 이는 `Arc`의 clone이므로 문제는 없습니다. 하지만 비동기 클로저 내에서 `self` 참조를 캡처하는 도구가 있는 경우 안전하지 않을 수 있습니다.

**개선 제안:** `Send + Sync + 'static` 바운드를 명시적으로 문서화하고, 도구의 `execute` 메서드가 `&self` 대신 필요한 데이터만 캡처하도록 가이드.

---

## 4. 개별 도구 구현 분석

### 4.1 Bash 도구 (`tools/bash.rs`)

### 이슈 #8 — 환경 변수 주입 시 보안 검증 누락
**심각도:** High  
**위치:** `tools/bash.rs:159-164`

```rust
if let Some(env_map) = env {
    for (key, val) in env_map {
        if let Some(val_str) = val.as_str() {
            cmd.env(key, val_str);
        }
    }
}
```

`PATH`, `HOME`, `LD_PRELOAD` 등 위험한 환경 변수를 LLM이 임의로 설정할 수 있습니다. MCP 서버(`mcp/client.rs:31`)에는 `BLOCKED_ENV_VARS`가 있지만, Bash 도구에는 없습니다.

**개선 제안:**
```rust
const BLOCKED_ENV: &[&str] = &["LD_PRELOAD", "LD_LIBRARY_PATH", "DYLD_INSERT_LIBRARIES", "PATH"];
if !BLOCKED_ENV.iter().any(|b| key.to_uppercase() == *b) {
    cmd.env(key, val_str);
}
```

### 이슈 #9 — 프로세스 그룹 킬이 macOS에서不完全할 수 있음
**심각도:** Medium  
**위치:** `tools/bash.rs:126-129`

```rust
.process_group(0);
```

`process_group(0)`은 새 프로세스 그룹을 생성하지만, 타임아웃 시 `child.kill()`만 호출하고 프로세스 그룹 전체(`kill -PGID`)를 죽이지 않습니다. 자식 프로세스(예: `sh -c "sleep 100 & sleep 100"`)가 좀비로 남을 수 있습니다.

**개선 제안:**
```rust
// 타임아웃 시:
#[cfg(unix)]
{
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
}
```

### 4.2 Read 도구 (`tools/read.rs`)

### 이슈 #10 — 대용량 파일 메모리 과다 사용
**심각도:** Medium  
**위치:** `tools/read.rs:151-165`

파일 전체를 메모리에 읽어들인 후 offset/limit을 적용합니다:

```rust
let mut content = String::from_utf8_lossy(&detect_buf[..n]).into_owned();
let mut buffer = vec![0u8; 8192];
loop {
    let n = file.read(&mut buffer).await?;
    if n == 0 { break; }
    content.push_str(&String::from_utf8_lossy(&buffer[..n]));
}
```

10GB 로그 파일에 offset=1, limit=10을 지정해도 전체 파일을 읽습니다.

**개선 제안:** offset/limit이 지정된 경우 줄 단위로 읽으면서 해당 범위만 버퍼링. 또는 파일 크기 상한(예: 100MB)을 두고 초과 시 에러 반환.

### 4.3 Write 도구 (`tools/write.rs`)

### 이슈 #11 — `write_file_impl`이 에러 발생 시에도 성공 결과 반환
**심각도:** Low  
**위치:** `tools/write.rs:196-199`

```rust
match Self::write_file_impl(path, content, append).await {
    Ok(msg) => Ok(AgentToolResult::success(msg)),
    Err(e) => Ok(AgentToolResult::error(e)),
}
```

`write_file_impl`이 `Err`를 반환하면 `AgentToolResult::error`로 변환되어 도구 호출 자체는 성공합니다. 이는 의도된 설계일 수 있으나, 호출자가 `Err`와 `Ok(AgentToolResult::error)`를 구분할 수 없습니다.

**개선 제안:** 일관성을 위해 현재 방식을 유지하되, 문서화 명확화.

### 4.4 Edit 도구 (`tools/edit.rs`)

### 이슈 #12 — 다중 편집 시 첫 번째 매치만 찾음
**심각도:** Medium  
**위치:** `tools/edit_diff.rs:96-99`

```rust
let start = content.find(&edit.old_text).ok_or_else(|| EditDiffError {
    message: "Text to replace not found in file...".to_string(),
})?;
```

`str::find`는 첫 번째 매치만 반환합니다. 동일한 `old_text`가 파일에 여러 번 나타나면 항상 첫 번째 항목만 교체됩니다. LLM이 의도한 위치가 아닐 수 있습니다.

**개선 제안:** 고유성 검증을 추가: `matches.len() == 1`인지 확인하고, 2개 이상이면 "ambiguous match" 에러 반환.

### 이슈 #13 — 편집 충돌 감지가 교체 후에만 동작
**심각도:** Low  
**위치:** `tools/edit_diff.rs:103-109`

역방향 정렬 후 `replace_range`로 교체하므로 overlap 검사는 정확합니다. 그러나 동일한 `old_text`에 대한 여러 편집이 있을 경우, 첫 번째 편집이 적용된 후 두 번째 편집의 위치가 틀어질 수 있는 문제는 "각 편집이 원본 파일에 대해 매치된다"는 점에서 해결됩니다. (정방향 설계 의도와 일치)

### 4.5 Grep 도구 (`tools/grep.rs`)

### 이슈 #14 — 재귀 디렉토리 순회 시 심볼릭 링크 순환 위험
**심각도:** High  
**위치:** `tools/grep.rs:157-176`

```rust
async fn grep_walk(...) {
    // ... 
    let mut entries = fs::read_dir(current).await...;
    while let Some(entry) = entries.next_entry().await... {
        // 숨김 파일 제외, node_modules 등 제외
        Box::pin(Self::grep_walk(...)).await?;
    }
}
```

심볼릭 링크를 따라가며 순회하므로 순환 심볼릭 링크(예: `a/b -> a`)가 있으면 무한 루프에 빠집니다. `find` 도구에도 동일한 문제가 있습니다.

**개선 제안:** 방문한 inode 집합(`HashSet<u64>`)을 유지하거나, 기본적으로 심볼릭 링크를 따라가지 않도록 변경.

### 4.6 Subagent 도구 (`tools/subagent.rs`)

### 이슈 #15 — 서브에이전트 프로세스의 임시 디렉토리 누수
**심각도:** Medium  
**위치:** `tools/subagent.rs:210-218`

```rust
fn create_system_prompt_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path)...;
    Ok(path)
}
```

"no RAII — let OS clean up" 주석이 있듯이 임시 디렉토리가 프로세스 종료까지 정리되지 않습니다. 장기 실행 에이전트에서 수천 개의 임시 디렉토리가 누적될 수 있습니다.

**개선 제안:** 서브에이전트 완료 후 `std::fs::remove_dir_all` 호출, 또는 `tempfile::TempDir` 사용.

### 이슈 #16 — 체인 모드에서 이전 출력 무한 대체
**심각도:** Low  
**위치:** `tools/subagent.rs:382`

```rust
let task = step.task.replace("{previous}", &previous_output);
```

`previous_output`에 `{previous}` 문자열이 포함된 경우, 치환 후 길이가 기하급수적으로 증가할 수 있습니다(ReDoS와 유사). 또한 `replace`는 모든 발생을 치환하므로 여러 `{previous}` 플레이스홀더가 있을 때 의도치 않은 동작이 발생할 수 있습니다.

**개선 제안:** 최초 1회만 치환(`replacen`)하고, `previous_output` 길이 상한 설정.

### 4.7 Web Search 도구 (`tools/web_search.rs`)

이슈 없음. 깔끔한 구현.

### 4.8 GitHub 도구 (`tools/github.rs`)

### 이슈 #17 — `gh` CLI 인증 상태가 매 호출마다 체크됨
**심각도:** Low  
**위치:** `tools/github.rs:32-45`

```rust
async fn check_gh_auth() -> Result<(), ToolError> {
    let output = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await...;
}
```

모든 GitHub 도구 호출이 `gh auth status` 서브프로세스를 먼저 실행합니다. 에이전트 턴당 여러 GitHub 호출이 있을 경우 상당한 오버헤드입니다.

**개선 제안:** `OnceLock`으로 인증 상태를 캐시하고, 실패 시에만 재확인.

### 4.9 Context7 도구 (`tools/context7.rs`)

### 이슈 #18 — API 키가 `OnceLock`에 캐시되어 런타임 갱신 불가
**심각도:** Low  
**위치:** `tools/context7.rs:44-67`

```rust
static API_KEY: OnceLock<Option<String>> = OnceLock::new();
```

프로세스 시작 시 한 번 로드되며, 이후 환경 변수나 파일 변경이 반영되지 않습니다.

**개선 제안:** TTL 기반 캐시 또는 설정 리로드 명령 지원.

### 4.10 Questionnaire 도구 (`tools/questionnaire.rs`)

### 이슈 #19 — `QuestionnaireBridge`가 단일 질문만 보류 가능
**심각도:** Low  
**위치:** `tools/questionnaire.rs:28-41`

```rust
pub fn set(&self, pending: PendingQuestionnaire) -> bool {
    let mut lock = self.inner.lock();
    if lock.is_some() { return false; }
    *lock = Some(pending);
    true
}
```

병렬 도구 실행 모드에서 두 도구가 동시에 questionnaire를 요청하면 두 번째가 조용히 실패합니다. 현재 순차 실행 모드에서는 문제가 없지만, 향후 병렬 모드에서 이슈가 될 수 있습니다.

**개선 제안:** 큐 기반으로 변경하여 다중 보류 질문을 지원.

---

## 5. 재시도 및 스트리밍 재시도 로직

### 파일: `stream_retry.rs`, `agent_loop/retry.rs`

### 이슈 #20 — `stream_with_retry_core`가 비-레이트리밋 에러를 첫 시도에만 즉시 실패
**심각도:** Medium  
**위치:** `stream_retry.rs:48-52`

```rust
Err(e) => {
    on_failure();
    let msg = e.to_string();
    let is_rate_limit = matches!(e, oxi_ai::ProviderError::HttpError(429, _));

    // Non-retryable on the first attempt → bail immediately.
    if !is_rate_limit && attempt == 0 {
        return Err(AgentError::Stream(msg));
    }
```

`attempt == 0`일 때 레이트리밋이 아닌 에러는 즉시 실패합니다. 하지만 `attempt == 1` 이후에는 모든 에러에 대해 재시도합니다. 이는 첫 번째 시도의 네트워크 오류는 재시도하지만, 두 번째 시도의 같은 오류는 재시도하는 모순된 동작을 만듭니다.

**개선 제안:** 에러 타입 기반으로 재시도 가능 여부를 결정 (예: `HttpError(5xx)` → 재시도, `HttpError(4xx)` → 재시도 안 함).

### 이슈 #21 — 재시도 지수 백오프가 `BACKOFF_BASE_SECS^n`으로 너무 급격히 증가
**심각도:** Low  
**위치:** `stream_retry.rs:56`

```rust
let mut delay = BACKOFF_BASE_SECS.pow(attempt as u32 + 1);
```

`BACKOFF_BASE_SECS = 2`이므로: 1차 재시도 4초, 2차 8초, 3차 16초. 3회 재시도에 총 28초 대기. `max_delay` 캡이 있지만 기본값이 `None`입니다.

**개선 제안:** `delay = min(BASE * 2^attempt, max_delay)`로 변경하고 `max_delay` 기본값을 30초로 설정.

### 이슈 #22 — 서킷 브레이커의 `half_open_successes`가 성공 시마다 초기화되지 않음
**심각도:** Low  
**위치:** `recovery.rs:109-118`

```rust
CircuitState::HalfOpen => {
    let prev = self.consecutive_successes.fetch_add(1, Ordering::SeqCst);
    if prev + 1 >= self.config.half_open_successes as u64 {
        self.state.store(CircuitState::Closed as u8, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
    }
}
```

HalfOpen → Closed 전환 시 `consecutive_successes`를 0으로 리셋하지 않습니다. 다음 HalfOpen 진입 시 카운터가 0으로 리셋되는 로직(`allow_request`의 `self.consecutive_successes.store(0, ...)`)이 있으므로 실제 버그는 아니지만, 명시적으로 리셋하는 것이 더 안전합니다.

---

## 6. 상태 관리 및 동시성

### 파일: `state.rs`

### 이슈 #23 — `SharedState::get_state()`가 전체 상태 클론 반환
**심각도:** Medium  
**위치:** `state.rs:111-114`

```rust
pub fn get_state(&self) -> AgentState {
    self.state.read().clone()
}
```

메시지 히스토리가 길어질 경우(수천 개 메시지, 수십 MB) 매번 전체 클론이 발생합니다. `AgentLoop`에서 컴팩션 체크, 루프 시작, 최종 동기화 등 여러 곳에서 호출됩니다.

**개선 제안:** 
- 읽기 전용 접근을 위한 `RwLockReadGuard` 반환 메서드 추가
- 또는 메시지 길이/토큰 수만 필요한 경우를 위한 경량 쿼리 메서드 추가

### 이슈 #24 — `Agent::run_with_channel_inner`의 상태 복사 레이스 컨디션
**심각도:** Medium  
**위치:** `agent.rs:265-272`, `agent.rs:306-310`

```rust
// 새 SharedState에 현재 상태를 복사
let fresh_state = crate::state::SharedState::new();
let current = self.state.get_state();
fresh_state.update(|s| { *s = current; });
// ... AgentLoop 실행 ...
// 다시 복사해옴
let loop_state = al.state().get_state();
self.state.update(|s| { *s = loop_state; });
```

AgentLoop 실행 중 `self.state`에 다른 스레드가 쓰기를 하면 손실될 수 있습니다. `is_running` AtomicBool로 동시 실행은 방지하지만, hooks의 `get_steering_messages` 등이 별도 스레드에서 상태를 읽을 수 있습니다.

**개선 제안:** AgentLoop에 `self.state`의 참조를 직접 전달하고, AgentLoop 내부에서 같은 `SharedState`를 사용.

### 이슈 #25 — `AgentHooks`의 `should_stop_after_turn` 훅이 소유권 이동 후 사용됨
**심각도:** High  
**위치:** `agent.rs:285-291`

```rust
let maybe_hook = {
    drop(hooks);
    let mut hooks_w = self.hooks.write();
    hooks_w.should_stop_after_turn.take()  // Option에서 take!
};
```

`take()`로 훅을 이동시키므로, 최초 실행 후 `should_stop_after_turn` 훅이 `None`이 됩니다. 두 번째 `run()` 호출에서는 훅이 동작하지 않습니다.

**개선 제안:** `take()` 대신 `clone()` (Box<dyn Fn>은 Clone이 아니므로 `Arc`로 래핑) 또는 참조로 접근.

---

## 7. MCP 통합 품질

### 파일: `mcp/mod.rs`, `mcp/client.rs`, `mcp/tool.rs`

### 이슈 #26 — `McpManager::lazy_connect`에서 연결 실패 시 60초 백오프가 너무 긺
**심각도:** Medium  
**위치:** `mcp/mod.rs:235`

```rust
const FAILURE_BACKOFF_SECS: u64 = 60;
```

MCP 서버가 일시적으로 응답하지 않는 경우 60초 동안 재시도가 불가합니다. 설정 가능해야 합니다.

**개선 제안:** `McpSettings`에 `failure_backoff_secs` 필드 추가.

### 이슈 #27 — `McpClient::read_message`에 타임아웃 없음
**심각도:** High  
**위치:** `mcp/client.rs:217-243`

```rust
async fn read_message(&mut self) -> Result<RawJsonRpcMessage> {
    loop {
        let mut line = String::new();
        let bytes_read = self.stdout.read_line(&mut line).await?;
        // ...
    }
}
```

`send_request`에 타임아웃이 있지만, `read_message` 자체에는 없습니다. 서버가 헤더만 보내고 본문을 보내지 않으면 무한 대기에 빠집니다.

**개선 제안:** `read_message`에도 `REQUEST_TIMEOUT_SECS` 타임아웃 적용.

### 이슈 #28 — MCP 클라이언트가 단일 스레드에서만 사용 가능
**심각도:** Low  
**위치:** `mcp/client.rs`

`McpClient`는 `&mut self`를 요구하므로 동시에 여러 요청을 보낼 수 없습니다. JSON-RPC의 ID 기반 멀티플렉싱을 활용할 수 없습니다.

현재 설계에서는 `McpManager`가 `tokio::sync::Mutex`로 보호되므로 실제 경합은 없지만, 병렬 MCP 도구 호출이 불가합니다.

**개선 제안:** 장기적으로는 요청 채널 기반 아키텍처로 리팩토링.

---

## 8. 파일 뮤테이션 큐 및 편집 충돌 해결

### 파일: `tools/file_mutation_queue.rs`

### 이슈 #29 — 전역 싱글톤 큐의 메모리 누수
**심각도:** Medium  
**위치:** `tools/file_mutation_queue.rs:20-23`

```rust
static QUEUE: std::sync::OnceLock<FileMutationQueue> = std::sync::OnceLock::new();
```

`queues: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>`에 등록된 파일 뮤텍스가 `cleanup()`이 명시적으로 호출되지 않으면 해제되지 않습니다. 장기 실행 세션에서 수천 개의 파일 경로가 누적됩니다.

**개선 제안:** `with_queue` 완료 후 자동으로 cleanup 호출, 또는 LRU 캐시로 변경.

### 이슈 #30 — `with_queue` 내에서 파일 미존재 시 경로 기반 락 충돌 가능
**심각도:** Low  
**위치:** `tools/file_mutation_queue.rs:42-45`

```rust
let canonical = fs::canonicalize(path)
    .await
    .unwrap_or_else(|_| path.to_path_buf());
```

파일이 존재하지 않는 경우 원시 경로를 키로 사용합니다. 그러나 두 도구가 다른 상대 경로(예: `./foo.txt`와 `foo.txt`)로 같은 파일을 생성하려는 경우, 각각 다른 뮤텍스를 획득하여 충돌이 발생합니다.

**개선 제안:** 경로를 절대 경로로 정규화한 후 사용 (`std::path::Path::canonicalize` 또는 `dunce::canonicalize`).

---

## 9. 경로 보안 및 샌드박싱

### 파일: `tools/path_security.rs`, `tools/path_utils.rs`

### 이슈 #31 — `PathGuard::validate`가 심볼릭 링크를 통해 샌드박스 탈출 허용
**심각도:** High  
**위치:** `tools/path_security.rs:39-54`

```rust
pub fn validate(&self, path: &Path) -> Result<PathBuf, PathSecurityError> {
    // 1. 순회 방지
    if path.components().any(|c| c.as_os_str() == "..") {
        return Err(PathSecurityError::Traversal(path.to_path_buf()));
    }
    // 2. 존재하는 경로면 canonicalize로 실제 경로 확인
    if path.exists() {
        let canonical = path.canonicalize()...;
        if !canonical.starts_with(&self.root) {
            return Err(PathSecurityError::OutsideWorkspace(canonical));
        }
        Ok(canonical)
    } else {
        // 존재하지 않는 경로는 순회만 확인
        Ok(path.to_path_buf())
    }
}
```

**문제 1:** `..`이 없더라도 심볼릭 링크를 통해 작업 공간 밖을 가리킬 수 있습니다. 예: `ln -s /etc link; read link/passwd` → `..`이 없으므로 통과하지만, `canonicalize` 후 루트 체크로 방어됩니다.

**문제 2 (Critical):** `PathGuard`는 대부분의 도구에서 **사용되지 않습니다!** `ReadTool`, `WriteTool`, `EditTool`, `BashTool` 모두 자체적인 `..` 검사만 수행하고 `PathGuard`를 사용하지 않습니다. 심볼릭 링크를 통한 샌드박스 탈출이 모든 파일 도구에서 가능합니다.

**개선 제안:** 모든 파일 도구에 `PathGuard`를 적용하고, `validate()` 후 canonicalize된 경로를 사용.

### 이슈 #32 — `..` 차단이 너무 엄격하여 정상적인 사용이 불가
**심각도:** Low  
**위치:** 여러 도구

현재 `..`이 포함된 모든 경로를 차단합니다. 그러나 작업 디렉토리 내에서 `src/../lib/foo.rs` 같은 정규화된 경로를 사용하는 경우는 합법적입니다.

**개선 제안:** 경로를 먼저 정규화(`canonicalize` 또는 `Path::canonicalize`)한 후 작업 공간 내부인지 확인.

### 이슈 #33 — Bash 도구의 `cwd` 검증이 불완전
**심각도:** Medium  
**위치:** `tools/bash.rs:120-130`

```rust
let work_dir = match cwd {
    Some(dir) if !dir.is_empty() => {
        let path = Path::new(dir);
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal (..) not allowed in working directory".to_string());
        }
        if !path.exists() {
            return Err(format!("Working directory does not exist: {}", dir));
        }
        Some(dir.to_string())
    }
    _ => None,
};
```

`..`만 검사하고 심볼릭 링크나 절대 경로(`/etc`)를 통한 탈출은 방지하지 않습니다. 또한 명령어 내에서 `cd ../../../etc`를 실행하는 것은 막을 수 없습니다.

**개선 제안:** Bash 자체의 샌드박싱은 제한적이므로, 위험한 명령어 패턴 감지 또는 컨테이너 기반 격리를 장기적으로 고려.

---

## 10. 컴팩션 전략 효과성

### 파일: `compaction.rs`, `agent_loop/mod.rs:386-430`

### 이슈 #34 — `maybe_compact`이 메시지를 두 번 직렬화
**심각도:** Medium  
**위치:** `agent_loop/mod.rs:390-392`

```rust
let context_text = serde_json::to_string(&*messages).unwrap_or_default();
let context_tokens = estimate_tokens(&context_text);
```

메시지 배열을 JSON 문자열로 직렬화한 후 토큰 수를 추정합니다. 대규모 컨텍스트(수만 토큰)에서 이 직렬화는 상당한 CPU/메모리 오버헤드를 발생시킵니다.

**개선 제안:** `oxi_ai::estimate_tokens`에 메시지 배열을 직접 전달하는 API 추가, 또는 메시지 수 × 평균 토큰 수로 근사.

### 이슈 #35 — 컴팩션 실패 시 에러가 무시됨
**심각도:** Low  
**위치:** `agent_loop/mod.rs:421-424`

```rust
Err(e) => {
    emit(AgentEvent::Compaction {
        event: CompactionEvent::Failed { error: e.to_string() },
    });
}
```

컴팩션 실패 시 경고만 표시하고 계속 진행합니다. 컨텍스트가 계속 증가하면 결국 컨텍스트 창 초과로 에이전트가 실패합니다.

**개선 제안:** 연속 컴팩션 실패 횟수를 추적하고, 임계치 초과 시 사용자에게 알림.

---

## 11. 프록시 구성 처리

### 파일: `proxy.rs` (1,208줄)

### 이슈 #36 — `ProxyStream::connect_and_stream`에서 `cancel_rx`가 이동 후 사용됨
**심각도:** High  
**위치:** `proxy.rs:262-290`

```rust
async fn connect_and_stream(
    ...
    cancel_rx: oneshot::Receiver<()>,
    ...
) -> Result<()> {
    ...
    loop {
        tokio::select! {
            _ = cancel_rx => { break; }  // cancel_rx가 이미 이동됨
            ...
        }
    }
}
```

`cancel_rx: oneshot::Receiver<()>`는 `tokio::select!`에서 첫 번째 폴 후 소비됩니다. 이후 루프 반복에서 `cancel_rx`가 이미 소진되어 컴파일 에러가 발생하거나, 실제로는 `tokio::select!` 매크로가 핀을 고정하므로 동작하지만 의미론적으로 올바르지 않습니다.

**개선 제안:** `cancel_rx`를 `Option<oneshot::Receiver<()>>`로 래핑하고, 소비 후 `None`으로 설정.

### 이슈 #37 — `ProxyEventStripper`의 `content_index` 추적이 불완전
**심각도:** Low  
**위치:** `proxy.rs:694-710`

`ProviderEvent::Start`는 첫 번째 콘텐츠 블록의 인덱스만 처리합니다. 여러 콘텐츠 블록(예: 텍스트 + 도구 호출)이 있는 응답에서 두 번째 이후 블록의 시작 이벤트가 손실될 수 있습니다.

**개선 제안:** `TextStart`, `ThinkingStart`, `ToolCallStart` 이벤트를 직접 처리.

### 이슈 #38 — 프록시 모듈이 `#[allow(dead_code)]`로 절반 이상이 미사용
**심각도:** Low  
**위치:** `proxy.rs` 전체

`ProxyStream`, `ProxyEventReconstructor`, `ProxyEventStripper`, `ProxyServerConfig` 등 상당수의 타입과 메서드가 `dead_code` 상태입니다. 이는 서버 측 프록시 구현이 아직 완료되지 않았음을 시사합니다.

**개선 제안:** 사용되지 않는 코드를 `#[cfg(feature = "proxy")]` 뒤로 이동하거나, TODO 주석과 함께 명시.

---

## 12. 종합 평가 및 권장사항 요약

### 전체 평가

| 영역 | 평점 | 비고 |
|------|------|------|
| 아키텍처 | ★★★★☆ | 깔끔한 2-레벨 루프, 좋은 관심사 분리 |
| 도구 시스템 | ★★★★☆ | 트레이트 설계 우수, 등록/발견 유연 |
| 에러 복구 | ★★★☆☆ | 서킷 브레이커/폴백 있지만 정규식 기반 판별은 취약 |
| 보안 | ★★★☆☆ | 기본 차단은 있으나 PathGuard 미사용, 환경변수 검증 누락 |
| 동시성 | ★★★★☆ | parking_lot + Atomic 활용 좋음, 상태 복사는 비효율 |
| MCP | ★★★★☆ | JSON-RPC 전송계층 견고, 보안 환경변수 차단 좋음 |
| 테스트 | ★★★☆☆ | 단위 테스트는 많으나, 통합 테스트(MockProvider)는 기본적 |
| 문서화 | ★★★☆☆ | 모듈/함수 문서는 있으나 아키텍처 결정 이유(Architecture Decision Record) 부족 |

### 심각도별 이슈 요약

| 심각도 | 개수 | 이슈 번호 |
|--------|------|-----------|
| **Critical** | 0 | — |
| **High** | 4 | #8(환경변수 주입), #14(심볼릭링크 순환), #25(훅 소유권 이동), #27(MCP 타임아웃) |
| **Medium** | 12 | #2, #3, #5, #7, #9, #10, #12, #15, #20, #23, #24, #26, #29, #33, #34, #31(path_security) |
| **Low** | 14 | #1, #4, #6, #11, #13, #16, #17, #18, #19, #21, #22, #30, #32, #35, #37, #38 |

### 최우선 해결 권장사항 (Top 5)

1. **PathGuard를 모든 파일 도구에 적용** (#31) — 현재 심볼릭 링크를 통한 작업 공간 탈출이 가능합니다.
2. **Bash 도구에 환경변수 블록리스트 추가** (#8) — LLM이 `PATH`, `LD_PRELOAD` 등을 변경하는 것을 방지.
3. **should_stop_after_turn 훅 소유권 문제 수정** (#25) — 두 번째 `run()` 호출에서 Ctrl+C 감지가 불가.
4. **McpClient::read_message에 타임아웃 추가** (#27) — 악의적/버그 있는 MCP 서버가 에이전트를 영구 블로킹.
5. **Grep/Find에 심볼릭 링크 순환 방지 추가** (#14) — 순환 심볼릭 링크가 있는 디렉토리에서 무한 루프.

---

*이 보고서는 정적 코드 분석 기반으로 작성되었으며, 런타임 동작은 다를 수 있습니다.*
