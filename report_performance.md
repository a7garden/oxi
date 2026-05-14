# 🔍 Oxi 프로젝트 성능 및 효율성 분석 보고서

**분석 대상:** `/Volumes/MERCURY/PROJECTS/oxi` (Rust AI 코딩 어시스턴트)  
**분석 일자:** 2026-05-14  
**분석자:** Performance Analysis Agent  

---

## 📋 요약

| 카테고리 | 심각도 | 발견 수 |
|----------|--------|---------|
| 🔴 크리티컬 (Critical) | 높음 | 5 |
| 🟠 주요 (Major) | 중간 | 8 |
| 🟡 경고 (Warning) | 낮음 | 7 |
| 🔵 정보 (Info) | 참고 | 4 |

---

## 1. 메모리 할당 패턴 (Memory Allocation Patterns)

### 🔴 CRITICAL-01: 스트리밍 핫 패스에서 `partial.clone()` 과다 호출

**파일:** `oxi-agent/src/proxy.rs` (라인 513, 563, 584, 608, 636, 670)

`ProxyEventReconstructor::process_assistant_event()`에서 모든 이벤트 분기에서 `self.partial.clone()`을 호출합니다. `AssistantMessage`는 `Vec<ContentBlock>`을 포함하며, 텍스트/생각 델타 이벤트마다 전체 메시지가 복제됩니다.

```rust
// 현재: 모든 TextDelta 이벤트마다 전체 AssistantMessage 복제
return vec![ProviderEvent::TextDelta {
    content_index,
    delta,
    partial: self.partial.clone(),  // ⚠️ O(n) 복제, n = 콘텐츠 블록 수
}];
```

**영향:** LLM 응답당 수백~수천 개의 델타 이벤트가 발생합니다. 각 이벤트에서 전체 메시지(모든 누적 텍스트 + 툴콜)를 복제하므로, 토큰 출력량에 따라 O(n²) 메모리 할당이 발생합니다.

**개선 제안:**
- `Arc<AssistantMessage>`로 래핑하여 `Arc::clone`으로 변경 (O(1) 복제)
- 또는 Cow(Copy-on-Write) 패턴 도입
- 델타 이벤트에 `partial` 필드 대신 변경된 부분만 참조로 전달

### 🔴 CRITICAL-02: 스트리밍 핫 패스에서 `messages.last().expect().clone()` 반복

**파일:** `oxi-agent/src/agent_loop/streaming.rs` (라인 70, 84, 108, 142, 194, 197)

`stream_assistant_response()`에서 거의 모든 이벤트 분기에서 `messages.last().expect("non-empty").clone()`이 호출됩니다. 이 `messages` 벡터의 마지막 요소는 `Message::Assistant(AssistantMessage)`이며, 전체 AssistantMessage를 복제합니다.

```rust
emit(AgentEvent::MessageUpdate {
    message: messages.last().expect("non-empty").clone(),  // ⚠️ 전체 메시지 복제
    delta: Some(delta),
});
```

**영향:** 스트리밍 중 초당 수십~수백 번 호출됩니다. 긴 응답에서는 메모리 압력이 심각합니다.

**개선 제안:**
- `Message`를 `Arc<Message>`로 래핑
- 이벤트 emit에 참조 전달 또는 Arc 사용

### 🟠 MAJOR-01: `ToolCall` 필드의 과도한 `.clone()` 

**파일:** `oxi-agent/src/agent_loop/tool_exec.rs` (라인 68-70, 102-106, 137-139, 152-156 등)

툴 실행 파이프라인에서 `tool_call.id.clone()`, `tool_call.name.clone()`, `tool_call.arguments.clone()`이 동일한 툴콜에 대해 최대 5~6회 반복 복제됩니다.

```rust
// 동일한 tool_call에 대해 반복 복제
emit(AgentEvent::ToolExecutionStart {
    tool_call_id: tool_call.id.clone(),     // 1차 복제
    tool_name: tool_call.name.clone(),      // 1차 복제  
    args: tool_call.arguments.clone(),      // 1차 복제 (JSON Value)
});
// ...
tool_call_id: finalized.tool_call.id.clone(),  // 2차 복제
tool_name: finalized.tool_call.name.clone(),   // 2차 복제
```

**개선 제안:**
- `ToolCall` 전체를 한 번만 clone
- `Arc<ToolCall>` 사용 또는 이벤트 구조체에서 `&str` 참조 사용

### 🟠 MAJOR-02: `ToolResultMessage` 이중 복제

**파일:** `oxi-agent/src/agent_loop/tool_exec.rs` (라인 113-114, 218-219)

```rust
emit(AgentEvent::MessageStart { message: Message::ToolResult(tool_result_message.clone()) });
emit(AgentEvent::MessageEnd { message: Message::ToolResult(tool_result_message.clone()) });
```

동일한 `tool_result_message`가 두 번 복제됩니다.

**개선 제안:** `Arc<ToolResultMessage>` 사용

### 🟡 WARNING-01: `AgentLoopConfig` 전체 Clone

**파일:** `oxi-agent/src/agent_loop/mod.rs` (라인 61)

```rust
config: config.clone(),
```

`AgentLoopConfig`에는 `system_prompt`, `session_id`, `compaction_instruction`, `api_key` 등 여러 `String` 및 `Option<String>` 필드가 포함되어 있으며, 모든 필드가 복제됩니다.

**개선 제안:** `Arc<AgentLoopConfig>` 사용

---

## 2. 비동기 런타임 효율성 (Async Runtime Efficiency)

### 🔴 CRITICAL-03: `tokio::sync::Mutex`를 사용한 MCP 매니저 잠금

**파일:** `oxi-agent/src/mcp/mod.rs` (라인 72)

```rust
inner: tokio::sync::Mutex<McpManagerInner>,
```

`McpManagerInner`은 `HashMap<String, McpClient>` + `HashMap<String, Vec<ToolMetadata>>` + `HashMap<String, Instant>`를 포함합니다. 모든 MCP 작업이 동일한 `tokio::sync::Mutex`를 거치므로 병목이 발생합니다.

**영향:** MCP 툴 검색, 툴 호출, 서버 연결 등 모든 작업이 직렬화됩니다.

**개선 제안:**
- `parking_lot::RwLock`으로 전환 (비동기 컨텍스트가 아닌 곳에서만 사용)
- 또는 세분화된 잠금: clients별 HashMap, metadata별 HashMap 분리
- 읽기 작업(search, list)은 RwLock 읽기 잠금으로 동시성 확보

### 🟠 MAJOR-03: `std::sync::Mutex`를 `BashTool`/`ReadTool`의 진행 콜백에 사용

**파일:** `oxi-agent/src/tools/bash.rs` (라인 27, 361, 367)  
**파일:** `oxi-agent/src/tools/read.rs` (라인 14, 39, 333, 345)

```rust
progress_callback: Arc<std::sync::Mutex<Option<ProgressCallback>>>,
```

비동기 컨텍스트에서 `std::sync::Mutex`를 잠근 후 `.clone()`을 수행합니다.

```rust
let progress_cb = self.progress_callback.lock().expect("...").clone();
```

**영향:** 짧은 잠금이므로 크레이트 overhead는 적지만, `parking_lot::Mutex`가 더 효율적입니다.

**개선 제안:** `parking_lot::Mutex`로 통일 (이미 다른 도구에서 사용 중)

### 🟡 WARNING-02: `SubagentTool`에서 `tokio::spawn`으로 자식 프로세스 읽기

**파일:** `oxi-agent/src/tools/subagent.rs` (라인 442, 453)

```rust
let _reader_handle = tokio::spawn(async move {
    let reader = BufReader::new(stdout);
    // ...
});
```

stdout/stderr 읽기에 각각 별도의 `tokio::spawn` 태스크를 생성합니다. 수백 개의 서브에이전트가 동시에 실행되면 태스크 오버헤드가 발생할 수 있습니다.

**개선 제안:** `tokio::select!`로 단일 태스크에서 stdout/stderr을 동시에 처리

---

## 3. 스트리밍 성능 (Streaming Performance)

### 🔴 CRITICAL-04: SSE 파싱 시 `buffer.drain()` + 재수집(re-collection)

**파일:** `oxi-agent/src/proxy.rs` (라인 284-298)

```rust
let mut buffer = Vec::new();
// ...
buffer.extend_from_slice(&chunk);
while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
    let line = buffer.drain(..=pos).collect::<Vec<_>>();  // ⚠️ O(n) 드레인
    let line_str = String::from_utf8_lossy(&line);         // ⚠️ 또다른 할당
    // ...
}
```

**문제점:**
1. `buffer.iter().position()` → O(n) 선형 검색
2. `buffer.drain(..=pos)` → 나머지 요소를 앞으로 이동시키는 O(n) 작업
3. `.collect::<Vec<_>>()` → 새로운 Vec 할당
4. `String::from_utf8_lossy()` → 또다른 문자열 할당

**영향:** 대역폭이 높은 스트리밍에서 매 청크마다 버퍼 전체를 재배열합니다.

**개선 제안:**
- `bytes::BytesMut` 또는 인덱스 기반 슬라이스로 드레인 대체
- 읽기 위치 커서를 유지하고 `buffer.split_to(pos)` 사용
- 또는 `BufReader` + `read_line()` 활용

### 🟠 MAJOR-04: 프록시 ToolCallDelta에서 매번 JSON 파싱

**파일:** `oxi-agent/src/proxy.rs` (라인 620-626)

```rust
ProxyAssistantMessageEvent::ToolCallDelta { content_index, delta } => {
    // ...
    let arguments: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(partial_json).unwrap_or_default();  // ⚠️ 매 델타마다 전체 JSON 파싱
}
```

툴콜의 `partial_json`에 매 델타마다 전체 문자열을 JSON으로 파싱합니다. 툴콜 하나에 수십~수백 개의 델타가 발생할 수 있습니다.

**개영향:** 누적 JSON이 커질수록 파싱 비용이 선형적으로 증가합니다.

**개선 제안:**
- 스트리밍 JSON 파싱 라이브러리 사용 (`simd-json`, `streaming-iterator`)
- 또는 부분 파싱만 수행하고 Done 이벤트에서 최종 파싱

### 🟠 MAJOR-05: `stream_assistant_response`에서 `serde_json::to_value` 반복 호출

**파일:** `oxi-agent/src/agent_loop/streaming.rs` (라인 38)

```rust
let schema = serde_json::to_value(&def.input_schema).unwrap_or_else(|_| {
    serde_json::json!({"type": "object", "properties": {}})
});
```

매 스트리밍 호출마다 모든 툴 정의의 스키마를 다시 직렬화합니다.

**개선 제안:** 툴 스키마를 캐시하거나 `ToolDefinition`에 미리 직렬화된 JSON 포함

---

## 4. 파일 I/O 패턴 (File I/O Patterns)

### 🟠 MAJOR-06: `ReadTool`에서 파일을 8KB 버퍼로 반복 읽기 + UTF-8 손실 변환

**파일:** `oxi-agent/src/tools/read.rs` (라인 127-153)

```rust
let mut detect_buf = vec![0u8; BINARY_DETECT_BYTES.min(file_size as usize)];
// ...
let mut content = String::from_utf8_lossy(&detect_buf[..n]).into_owned();  // ⚠️ 할당
let mut buffer = vec![0u8; 8192];
loop {
    let n = file.read(&mut buffer).await?;
    content.push_str(&String::from_utf8_lossy(&buffer[..n]));  // ⚠️ 매 반복마다 할당
}
```

**문제점:**
1. 이진 감지용 버퍼와 콘텐츠 읽기가 분리되어 있어 이중 읽기
2. 매 8KB 반복마다 `String::from_utf8_lossy()` → 새 할당 → `push_str` 복사
3. `into_owned()`로 또다른 할당

**개선 제안:**
- 파일 전체를 `Vec<u8>`에 한 번에 읽고 마지막에 한 번만 UTF-8 변환
- 또는 `BufReader` 사용하여 한 번에 읽기
- `with_capacity(file_size)`로 미리 용량 할당

### 🟡 WARNING-03: `EditTool`에서 전체 파일 내용을 복제

**파일:** `oxi-agent/src/tools/edit.rs` (라인 151)

```rust
let final_content_clone = final_content.clone();
```

편집 작업을 위해 전체 파일 내용을 복제합니다. 대용량 파일에서는 메모리 낭비입니다.

**개선 제안:** 필요한 경우에만 복제, 또는 참조로 편집 작업 수행

### 🟡 WARNING-04: `tokio::fs::read_to_string` 대 `std::fs::read_to_string`

프로젝트 전반에 걸쳐 파일 읽기에 `tokio::fs`를 사용합니다. 작은 파일의 경우 `tokio::fs`의 오버헤드(`spawn_blocking` 래핑)가 이득보다 큽니다.

**개선 제안:** 작은 설정 파일의 경우 `std::fs::read_to_string` + `spawn_blocking` 직접 제어

---

## 5. 문자열 처리 (String Processing)

### 🔴 CRITICAL-05: Regex가 매 호출마다 재컴파일되는 위치

**파일:** `oxi-cli/src/ui/changelog.rs` (라인 65-66)

```rust
let version_regex = Regex::new(r"##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").ok();
```

함수 내에서 정규식이 매 호출마다 컴파일됩니다.

**파일:** `oxi-cli/src/prompt/templates.rs` (라인 189, 202)

```rust
let positional_re = regex::Regex::new(r"\$(\d+)").unwrap();
// ...
let slice_re = regex::Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").unwrap();
```

템플릿 처리 함수 내에서 매번 정규식이 컴파일됩니다.

**파일:** `oxi-cli/src/storage/packages.rs` (라인 365)

```rust
let re = regex::Regex::new(r"^(@?[^@]+(?:/[^@]+)?)(?:@(.+))?$").expect("valid static regex");
```

**개선 제안:** `LazyLock` 또는 `OnceLock`로 전환 (이미 `oxi-agent/src/agent_loop/retry.rs`와 `oxi-cli/src/infra/output_guard.rs`에서 올바르게 사용 중)

### 🟠 MAJOR-07: `format!()` 남용 — 1,088건 포맷 호출

프로젝트 전체에 `format!()` 호출이 1,088건, `.to_string()` 호출이 수백 건 존재합니다. 특히 핫 패스에서:

```rust
// oxi-agent/src/agent_loop/tool_exec.rs
status: if finalized.is_error { "error".to_string() } else { "success".to_string() },
```

**개선 제안:** 정적 문자열 상수 사용:

```rust
status: if finalized.is_error { "error".into() } else { "success".into() },
// 또는
status: if finalized.is_error { Cow::Borrowed("error") } else { Cow::Borrowed("success") },
```

### 🟡 WARNING-05: `GrepTool`에서 파일 전체를 `Vec<String>`으로 변환

**파일:** `oxi-agent/src/tools/grep.rs` (`read_file_lines` 함수)

```rust
let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
Ok(normalized.lines().map(|s| s.to_string()).collect())
```

파일의 모든 라인을 개별 `String`으로 할당합니다. 대용량 파일에서는 메모리 오버헤드가 큽니다.

**개선 제안:**
- 라인을 `&str` 슬라이스로 처리 (문자열 전체를 하나의 String으로 유지)
- 또는 라인 인덱스 기반으로 검색

---

## 6. TUI 렌더링 성능 (TUI Rendering Performance)

### 🟡 WARNING-06: `definitions()` 호출 시 모든 툴 정의 재생성

**파일:** `oxi-agent/src/tools.rs` (라인 276-280)

```rust
pub fn definitions(&self) -> Vec<ToolDefinition> {
    self.tools
        .read()
        .values()
        .map(|t| t.to_definition())  // ⚠️ 매 호출마다 모든 툴 정의 재생성
        .collect()
}
```

`to_definition()`은 매번 `serde_json::from_value(self.parameters_schema())`를 호출합니다.

**영향:** 스트리밍 시작 시마다 호출되며, 툴 수가 많을수록 비용 증가

**개선 제안:** ToolDefinition을 `OnceCell`에 캐시

### 🟡 WARNING-07: `should_stop_after_turn`에서 매 턴마다 전체 메시지 스캔

**파일:** `oxi-agent/src/agent_loop/helpers.rs` (라인 53)

```rust
let current_iteration = _messages.iter()
    .filter(|m| matches!(m, oxi_ai::Message::Assistant(_)))
    .count();
```

매 턴마다 전체 메시지 히스토리를 순회하여 Assistant 메시지를 카운트합니다.

**개선 제안:** 턴 카운터를 `AgentLoop`에 직접 유지 (`AtomicUsize`)

---

## 7. 직렬화 오버헤드 (Serialization Overhead)

### 🟠 MAJOR-08: `estimate_tokens()`에서 전체 메시지 JSON 직렬화

**파일:** `oxi-agent/src/state.rs` (라인 112)

```rust
pub fn estimate_tokens(&self) -> usize {
    let json = serde_json::to_string(&self.messages).unwrap_or_default();
    json.len() / 4
}
```

토큰 추정을 위해 전체 메시지 히스토리를 JSON으로 직렬화합니다. 긴 대화에서는 매우 비쌉니다.

**개선 제안:**
- 누적 토큰 카운트 유지 (이미 `total_tokens` 필드가 있음)
- 또는 문자열 길이 기반 경량 추정 사용

### 🟡 INFO-01: `maybe_compact`에서도 메시지 JSON 직렬화

**파일:** `oxi-agent/src/agent_loop/mod.rs` (라인 508)

```rust
let context_text = serde_json::to_string(&*messages).unwrap_or_default();
let context_tokens = estimate_tokens(&context_text);
```

컴팩션 확인을 위해 매 턴마다 전체 메시지를 JSON으로 직렬화합니다.

**개선 제안:** 증분 토큰 카운트 추적, 직렬화 없이 추정

---

## 8. 잠금 경합 (Lock Contention)

### 🔵 INFO-01: `ToolRegistry`에 `parking_lot::RwLock` 사용 — 양호

**파일:** `oxi-agent/src/tools.rs` (라인 220)

```rust
tools: Arc<parking_lot::RwLock<std::collections::HashMap<String, Arc<dyn AgentTool>>>>,
```

읽기 작업이 많은 툴 레지스트리에 `parking_lot::RwLock`을 사용하는 것은 적절합니다.

### 🔵 INFO-02: `CircuitBreaker`에 락프리 원자 연산 사용 — 양호

**파일:** `oxi-agent/src/recovery.rs`

`AtomicU8`, `AtomicU64`, `Ordering::SeqCst`를 사용한 락프리 회로 차단기 구현은 효율적입니다.

### 🟡 WARNING-08: `SearchCache`에 `parking_lot::Mutex<HashMap>` 사용

**파일:** `oxi-agent/src/tools/search_cache.rs` (라인 39)

```rust
entries: Mutex<HashMap<String, CachedSearch>>,
```

캐시 읽기(get)와 쓰기(insert)가 동일한 뮤텍스를 공유합니다.

**개선 제안:** `DashMap` 또는 `RwLock`으로 읽기 동시성 향상

---

## 9. 네트워크 요청 효율성 (Network Request Efficiency)

### 🟠 MAJOR-09: `reqwest::Client`가 여러 위치에서 재생성됨

**파일 목록:**
- `oxi-agent/src/tools/github_search.rs:137` — `reqwest::Client::new()`
- `oxi-agent/src/proxy.rs:261` — `reqwest::Client::builder().build()`
- `oxi-cli/src/infra/tools_manager.rs:230, 252` — `reqwest::Client::builder()`
- `oxi-cli/src/storage/packages.rs:481, 1388` — `reqwest::Client::builder()`
- `oxi-cli/src/extensions/ext_cli.rs:95, 178` — `reqwest::Client::new()`

**영향:** `reqwest::Client`는 내부적으로 연결 풀과 TLS 세션을 관리합니다. 재생성하면:
- 연결 풀이 초기화되어 TCP/TLS 핸드셰이크 재수행
- DNS 캐시 손실
- HTTP/2 연결 재설정

**긍정적 발견:**
- `oxi-ai/src/providers/mod.rs:63-65` — `shared_client()` 함수로 글로벌 싱글톤 사용 중 ✅
- `oxi-agent/src/tools/context7.rs:44` — `OnceLock<reqwest::Client>` 사용 중 ✅

**개선 제안:** `shared_client()` 패턴을 모든 모듈에 확장 적용

### 🔵 INFO-03: 커스텀 PRNG 구현

**파일:** `oxi-agent/src/tools/search_cache.rs` (mod rand)

xorshift 기반 커스텀 PRNG를 구현했습니다. `rand` 크레이트 의존성을 피하기 위한 것으로 보이며, 이 목적에는 충분합니다.

---

## 10. 데이터 구조 선택 (Data Structure Choices)

### 🟡 WARNING-09: `SearchCache`에 `HashMap` + 수동 제거 정책

**파일:** `oxi-agent/src/tools/search_cache.rs` (라인 80-85)

```rust
while entries.len() >= self.max_entries {
    if let Some(key) = entries.keys().next().cloned() {  // ⚠️ 임의 제거, LRU 아님
        entries.remove(&key);
    }
}
```

`HashMap`의 반복 순서는 비결정적이므로 LRU가 아닌 임의 항목이 제거됩니다.

**개선 제안:**
- `lru` 크레이트 사용
- 또는 `IndexMap` + 접근 시간 추적으로 근사 LRU 구현

### 🔵 INFO-04: `parking_lot::RwLock<Vec<Message>>`로 스티어링/팔로우업 큐 관리 — 적절

**파일:** `oxi-agent/src/agent_loop/mod.rs`

`Vec` + `drain()`으로 큐를 관리합니다. 메시지가 소규모이므로 `VecDeque`보다 간단하고 충분합니다.

참고: `oxi-cli/src/app/agent_session.rs`에서는 `Arc<RwLock<VecDeque<String>>>`을 사용합니다 — 세션 간 메시지 큐에는 `VecDeque`가 더 적절합니다 ✅

---

## 🏗️ 아키텍처 수준 개선 제안

### 1. `Arc<Message>` / `Arc<AssistantMessage>` 도입

가장 큰 성능 이득을 가져올 변경입니다. 스트리밍 이벤트, 툴 실행 결과, 에이전트 이벤트 등 모든 곳에서 메시지가 복제됩니다. `Arc`로 래핑하면:

```rust
// Before
emit(AgentEvent::MessageUpdate {
    message: messages.last().expect("...").clone(),  // O(n) 복제
});

// After
emit(AgentEvent::MessageUpdate {
    message: Arc::clone(&messages.last().expect("...")),  // O(1)
});
```

**예상 효과:** 스트리밍 중 메모리 할당 50-80% 감소

### 2. 스트리밍 버퍼 최적화

`proxy.rs`의 SSE 파서를 `bytes::BytesMut` 기반으로 전환:

```rust
// Before
let line = buffer.drain(..=pos).collect::<Vec<_>>();

// After  
let line = buffer.split_to(pos + 1);
```

**예상 효과:** SSE 파싱 시 메모리 복사 90% 감소

### 3. 글로벌 HTTP 클라이언트 풀

`shared_client()` 패턴을 프로젝트 전체에 확장:

```rust
// 모든 모듈에서 사용
pub fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .expect("HTTP client init failed"))
}
```

### 4. 증분 토큰 추적

`estimate_tokens()`의 전체 JSON 직렬화를 피하고, 증분 토큰 카운트를 유지:

```rust
pub struct AgentState {
    // ...
    pub estimated_context_tokens: usize,  // 증분 업데이트
}
```

### 5. 정규식 캐싱 일관성

`LazyLock` / `OnceLock` 패턴을 모든 정규식에 일관되게 적용:

```rust
// 권장 패턴 (이미 일부 사용 중)
static MY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"pattern").unwrap()
});
```

---

## 📊 총 `.clone()` 호출 분석

| 모듈 | `.clone()` 수 (비테스트) | 주요 대상 |
|------|--------------------------|-----------|
| `agent_loop/tool_exec.rs` | 30+ | ToolCall, Message, Arc |
| `proxy.rs` | 25+ | AssistantMessage, String |
| `agent_loop/streaming.rs` | 15+ | Message, AssistantMessage |
| `agent_loop/mod.rs` | 15+ | Message, String, Config |
| `tools/subagent.rs` | 20+ | String, PathBuf |
| `mcp/mod.rs` | 15+ | String, Value, ToolMetadata |

**총 `.clone()` 호출 (비테스트 코드):** 약 1,222건

---

## ✅ 잘 구현된 부분

1. **`shared_client()` 싱글톤** (`oxi-ai/src/providers/mod.rs`) — 프로바이더 레이어에서 연결 풀 재사용 ✅
2. **`OnceLock<Regex>` 캐싱** (`agent_loop/retry.rs`, `infra/output_guard.rs`) — 정규식 캐싱 올바르게 구현 ✅  
3. **락프리 CircuitBreaker** (`recovery.rs`) — 원자 연산만 사용 ✅
4. **`parking_lot` 우선 사용** — `std::sync::Mutex` 대비 미세한 성능 이점 ✅
5. **`parking_lot::RwLock`으로 ToolRegistry** — 읽기 다중화 적절 ✅
6. **`BufReader` 사용** — MCP 클라이언트, 서브에이전트에서 버퍼링된 읽기 적용 ✅
7. **`Vec::with_capacity` 사용** — `tool_exec.rs:187`에서 적절한 사전 할당 ✅
8. **`String::with_capacity` 사용** — `render_utils.rs:38`, `path_utils.rs:86`에서 적절 ✅

---

*본 보고서는 정적 코드 분석을 기반으로 작성되었으며, 실제 런타임 프로파일링을 통해 확인이 필요합니다.*
