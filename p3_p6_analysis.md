# P3~P6 실제 사용 패턴 분석

> 분석 일시: 2026-05-17
> 대상 파일: `oxi-sdk/src/kernel_bridge.rs`, `oxios-kernel/src/engine.rs`, `oxios-kernel/src/agent_runtime.rs`

---

## P3: KernelToolContext 확장

### 3.1 SDK 측 정의 (`oxi-sdk/src/kernel_bridge.rs`)

```rust
pub struct KernelToolContext {
    pub workspace_dir: PathBuf,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub permissions: Vec<String>,   // CSpace 기반 권한 목록
}
```

- 빌더 패턴: `new()` → `.with_session()` → `.with_permissions()`
- **현재 필드**: workspace, agent_id, session_id, permissions (4개)
- **없는 필드**: `space_id`, `cspace`, `kernel_handle` 참조 — 이들은 context에 포함되지 않고 개별 전달됨

### 3.2 oxios에서 실제 생성 지점

oxios-kernel은 `KernelToolContext`를 **직접 생성하지 않음**. 대신 두 가지 경로로 툴을 등록:

| 경로 | 설명 |
|------|------|
| `OxiosKernelBridge` | SDK의 `KernelToolProvider` 트레잇 구현. `register_tools()`에서 `SdkKernelToolContext` 수신 |
| `register_tools_from_cspace()` | 직접 `ToolRegistry` + `KernelHandle` + `CSpace`를 인자로 받음. `KernelToolContext` 우회 |

**핵심 발견**: `agent_runtime.rs`는 `register_tools_from_cspace()`를 호출하며, 이 경로에서는 `KernelToolContext`를 생성하지 않음. CSpace, Space ID, KernelHandle이 **함수 인자로 직접 전달**됨.

```rust
// agent_runtime.rs (L340~342)
let registry = ToolRegistry::new();
register_tools_from_cspace(&registry, &kernel_handle, &cspace, search_cache, agent_id);
```

### 3.3 CSpace / Space ID를 툴에 전달하는 방식

- **CSpace → 툴 등록**: `register_tools_from_cspace()`가 CSpace를 iterate하며 권한 체크 후 툴 생성
- **KernelHandle**: 모든 커널 툴이 `from_kernel(&KernelHandle)` 패턴으로 커널 접근
- **Space ID**: `AgentRuntimeConfig.space_id`에 보관되나, **현재 툴에 직접 전달되지 않음**. Space 접근은 `SpaceTool` → `SpaceApi` → `SpaceManager` 경로로 런타임에 이루어짐
- **permissions**: `KernelToolContext.permissions`에 `Vec<String>`으로 존재하나, 실제 권한 제어는 CSpace의 `Rights` enum (`READ`, `WRITE`, `EXECUTE`) 기반

### P3 결론

`KernelToolContext`는 SDK 브릿지 패턴(`KernelToolProvider`)용으로 설계되었으나, oxios-kernel의 메인 실행 경로(`agent_runtime.rs`)는 이를 우회하고 직접 인자 전달 방식을 사용. **space_id, cspace, kernel_handle이 Context에 통합되지 않은 상태**.

---

## P4: ToolRegistry 발견/쿼리

### 4.1 쿼리 패턴

oxios-kernel에서 `registry.get(name)` / `registry.names()` 사용 패턴:

#### 패턴 A: 프로그램 의존성 검증 (agent_runtime.rs L396~410)

```rust
let missing_tools: Vec<&str> = program
    .meta
    .dependencies
    .iter()
    .filter(|tool_name| registry.get(tool_name).is_none())
    .map(|s| s.as_str())
    .collect();
if !missing_tools.is_empty() {
    tracing::warn!(missing_tools = ?missing_tools, "Skipping program");
    continue;
}
```

이 패턴은 **3번 반복**됨:
- `agent_runtime.rs` L396~410 (런타임 프로그램 등록)
- `agent_runtime.rs` L700 (테스트 `test_requires_tools_validation_passes`)
- `agent_runtime.rs` L731 (테스트 `test_requires_tools_validation_fails`)

#### 패턴 B: 등록 완료 후 이름 목록 조회 (registration.rs L179)

```rust
let tool_names = registry.names();
// 로깅/디버깅용
```

#### 패턴 C: HostToolValidator (host_tools.rs)

```rust
pub struct HostToolStatus {
    pub all_required_present: bool,
    pub missing_required: Vec<String>,
    pub optional_available: HashMap<String, bool>,
}
```

호스트 도구(git, gh, osascript 등)의 가용성 체크. `ToolRegistry`가 아닌 OS 명령 체크.

### 4.2 반복 패턴 분석

프로그램 의존성 검증 로직이 `registry.get()` + `.filter(is_none())` + `.collect()` 형태로 **동일하게 반복**됨. `ToolRegistry`에 다음과 같은 헬퍼가 있으면 유용:

```rust
// 제안: ToolRegistry 확장 메서드
fn missing_tools(&self, required: &[&str]) -> Vec<&str>
fn has_all(&self, required: &[&str]) -> bool
```

### P4 결론

- 주요 쿼리 패턴: `registry.get(name).is_none()` → 의존성 검증
- 반복 패턴 존재: 3곳에서 동일한 missing-tools 검증 로직
- `registry.names()`는 등록 후 로깅/디버깅에만 사용
- **추천**: `ToolRegistry`에 `missing()`/`has_all()` 헬퍼 추가로 중복 제거

---

## P5: AgentEvent Progress

### 5.1 AgentEvent 처리 (agent_runtime.rs L467~506)

`AgentLoop::run()`의 콜백에서 4개의 이벤트 타입을 처리:

```rust
match event {
    AgentEvent::ToolExecutionEnd { is_error: false, .. } => {
        s.steps_completed += 1;
    }
    AgentEvent::AgentEnd { messages, stop_reason, .. } => {
        // 최종 텍스트 추출, 성공 여부 판단
        s.final_content = a.text_content();
        s.success = stop_reason.as_deref() == Some("Stop");
    }
    AgentEvent::Error { message, .. } => {
        s.final_content = message.clone();
        s.success = false;
    }
    AgentEvent::Compaction { event } => {
        // 컴팩션 결과 → MemoryEntry로 저장
        if let CompactionEvent::Completed { result, .. } = event {
            mm.remember(entry).await;
        }
    }
    _ => {}  // 나머지 이벤트 무시
}
```

### 5.2 채널 전달 방식

**현재 구조**:

```
AgentRuntime.execute()
  → run_agent_loop() [spawn_blocking]
    → AgentLoop::run(callback)
      → callback: ExecuteState 업데이트 (Arc<Mutex<ExecuteState>>)
    → Result<(final_content, steps_completed, success)>
  → ExecutionResult { output, steps_completed, success }
  
Gateway.route()
  → Orchestrator.handle_message()
    → ExecutionResult를 OutgoingMessage로 변환
    → Channel.send(OutgoingMessage)
```

**핵심 발견**: 
- AgentEvent 콜백은 **내부 상태 업데이트 용도로만 사용** (steps 카운트, final_content 수집)
- **실시간 스트리밍/SSE 없음**: 채널(web, cli, telegram)로 진행 상황이 실시간 전달되지 않음
- Gateway는 요청-응답 모델: `IncomingMessage` → `OutgoingMessage` (최종 결과만)
- `oxios-gateway/`에 SSE, progress, streaming 관련 코드 **전무**
- `Channel` 트레잇: `receive()` → `send()` 만 있고, progress/streaming 메서드 없음

### 5.3 처리되지 않는 AgentEvent 변종

SDK의 `AgentEvent`에는 추가 변종이 있을 수 있으나, 현재 `_ => {}`로 무시됨:
- `ToolExecutionStart`, `TokenStream`, `ToolExecutionEnd { is_error: true }` 등
- 이들은 로깅/스트리밍에 활용 가능하나 현재 미사용

### P5 결론

- AgentEvent 처리는 **최종 결과 수집**에만 사용 (steps, content, success)
- **실시간 progress 전달 메커니즘 부재**: 채널이 요청-응답 모델
- Compaction 이벤트만 부가 동작 수행 (메모리 저장)
- **추천**: Channel 트레잇에 `on_progress()` 추가 또는 EventBus를 통한 실시간 이벤트 브로드캐스트

---

## P6: Provider 팩토리

### 6.1 Provider 생성 구조 (engine.rs)

```
OxiEngineProvider (trait 구현체)
  └── OxiosEngine
       └── Oxi (oxi-sdk 인스턴스)
            └── OxiBuilder::new().with_builtins() → build()
```

### 6.2 하드코딩된 Provider 분기문

**유일한 분기**: `zai` provider 특수 처리 (engine.rs L35~54)

```rust
let provider_name = model_id
    .split_once('/')
    .map(|(p, _)| p)
    .unwrap_or("anthropic");

if provider_name == "zai" {
    // 1. CredentialStore에서 API 키 조회
    let api_key = crate::credential::CredentialStore::resolve("zai", None)
        .map(|(key, _)| key);
    
    // 2. 환경변수에서 base URL 조회
    let zai_base_url = std::env::var("ZAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.z.ai/api/coding/paas/v4".to_string());
    
    // 3. OpenAI 호환 프로바이더로 래핑
    let zai_provider = oxi_ai::OpenAiProvider::with_base_url_and_key(
        &zai_base_url, api_key,
    );
    builder = builder.provider("zai", zai_provider);
}
```

### 6.3 CredentialStore 다층 해석

```
CredentialStore::resolve(provider, config_key):
  1. config.toml [engine].api_key (명시적 오버라이드)
  2. ~/.oxi/auth.json (oxi CLI 공유 인증 저장소)
  3. 환경변수 (oxi_sdk::get_env_api_key)
```

### 6.4 EngineProvider 트레잇

```rust
pub trait EngineProvider: Send + Sync {
    fn create_provider(&self, provider_name: &str) -> Result<Arc<dyn Provider>>;
    fn resolve_model(&self, model_id: &str) -> Result<Model>;
    fn default_model_id(&self) -> &str;
}
```

- `OxiEngineProvider`가 기본 구현체
- 테스트용 Mock 교체 가능 (trait 기반)
- `OxiosEngine::new()`에서 `OxiBuilder::with_builtins()`로 **50+ 내장 모델/프로바이더** 자동 로드

### 6.5 문제점

1. **단일 하드코딩 분기**: `zai`만 특수 처리. 새 OpenAI-compatible provider 추가 시 코드 수정 필요
2. **OxiosEngine.new()의 부작용**: 생성자에서 credential 조회, 환경변수 읽기, 네트워크 설정 수행
3. **확장성**: provider_name 기반 if-chain은 새 provider마다 증가

### P6 결론

- **하드코딩 분기**: `zai` 1개뿐이나, 패턴이 if-chain이므로 확장 시 기술부채
- **추천 패턴**: ProviderPlugin trait 또는 설정 기반 provider 매핑:
  ```rust
  // 제안: 설정 기반 프로바이더 레지스트리
  trait ProviderFactory: Send + Sync {
      fn create(&self, config: &ProviderConfig) -> Result<Arc<dyn Provider>>;
  }
  ```
- CredentialStore의 다층 해석은 잘 설계됨 (config → auth.json → env)
- `EngineProvider` trait 분리는 테스트 용이성에 기여

---

## 요약: 개선 권장사항

| 항목 | 현황 | 권장사항 |
|------|------|---------|
| P3 | KernelToolContext 미사용, 인자 직접 전달 | space_id, cspace를 Context에 통합하거나 별도 AgentContext 도입 |
| P4 | missing-tools 검증 로직 3곳 중복 | `ToolRegistry::missing()` 헬퍼 추가 |
| P5 | 실시간 progress 전달 없음 (요청-응답만) | Channel trait에 `on_progress()` 추가 또는 EventBus 브로드캐스트 |
| P6 | zai 단일 하드코딩 분기 | ProviderFactory trait 또는 설정 기반 provider 매핑으로 일반화 |
