# Subagent Tool 개선 설계서 v3

> v2 리뷰에서 발견한 두 Agent 인스턴스 이원화 문제를 근본적으로 해결하는 설계.

---

## 근본 문제: Agent 소유권 이원화

### 현황

```
main.rs
  ├─ print 모드 → App.agent (agent_1, ToolRegistry: 빈 + extension만)
  └─ TUI 모드   → create_agent_session_from_services() (agent_2, ToolRegistry: 빈)
```

- `App::new()`에서 `Agent::new()` → `ToolRegistry::new()` (빈)
- `main.rs`에서 extension tools만 `app.agent_tools().register_arc()`
- TUI 모드는 `app`에서 settings/model_id만 가져오고 **agent_1을 버림**
- `create_agent_session_from_services()`에서 **새 agent_2** 생성 (역시 빈 ToolRegistry)

**→ 현재 oxicode는 builtin 도구가 0개인 상태로 실행됩니다.**
**→ main.rs에서 등록한 extension 도구도 TUI 모드에서 유실됩니다.**

> **v3 리뷰 피드백 반영:** cwd 획득, libc 의존성, --append-system-prompt 구체화,
> io::Error 변환, UsageStats 필드 유지

### 해결: ToolRegistry를 App 수준에서 관리

```
main.rs
  │
  ├─ App::new() → ToolRegistry 생성 (아직 도구 없음)
  │
  ├─ builtin 도구 등록: registry.register_arc(...)
  ├─ extension 도구 등록: registry.register_arc(...)
  │
  ├─ print 모드 → App.agent (ToolRegistry: builtin + extension)
  └─ TUI 모드   → create_agent_session_from_services(registry) → agent_2도 같은 도구
```

**핵심 원칙:** ToolRegistry는 App에서 **한 번만** 구성하고, 모든 경로에 전달.

---

## 개선 항목

### 항목 0: ToolRegistry 단일 소유권

**변경 파일:** `oxicode-cli/src/main.rs`, `oxicode-cli/src/lib.rs`, `oxicode-cli/src/agent_session_runtime.rs`

#### main.rs

```rust
// cwd 획득 (v3 리뷰: 명시적 획득 필요)
let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

// App 생성 (agent_1 포함, ToolRegistry는 아직 빈 상태)
let app = oxicode::App::new(settings).await?;

// --append-system-prompt 처리 (v3 리뷰: set_system_prompt 활용)
if let Some(ref prompt_path) = args.append_system_prompt {
    let content = std::fs::read_to_string(prompt_path)
        .map_err(|e| anyhow::anyhow!("Failed to read system prompt: {}", e))?;
    let current = app.agent().config().system_prompt.clone().unwrap_or_default();
    app.agent().set_system_prompt(format!("{}\n\n{}", current, content));
}

// ToolRegistry에 builtin + extension 등록
let tools = app.agent_tools();
let builtins = oxicode_agent::ToolRegistry::with_builtins_cwd(cwd.clone());
for name in builtins.names() {
    if let Some(tool) = builtins.get(&name) {
        tools.register_arc(tool);
    }
}
for tool in ext_registry.all_tools() {
    tools.register_arc(tool);
}

if prompt.is_empty() || args.interactive {
    oxicode::tui_interactive::run_tui_interactive(app).await?;
} else if args.mode.as_deref() == Some("json") || args.print {
    // print_mode는 app.agent() 사용 → 이미 도구 등록됨
    // ...
} else {
    run_single_prompt(app, &prompt).await?;
}
```

#### agent_session_runtime.rs

`CreateAgentSessionFromServicesOptions`에 `tool_registry` 필드 추가:

```rust
pub struct CreateAgentSessionFromServicesOptions {
    pub services: Arc<AgentSessionServices>,
    pub session_manager: SessionManager,
    pub model_id: Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    pub scoped_models: Vec<ScopedModel>,
    pub tool_registry: Option<Arc<oxicode_agent::ToolRegistry>>,  // ← 추가
}
```

`create_agent_session_from_services()`에서:

```rust
let agent = Arc::new(oxicode_agent::Agent::new(Arc::from(provider), config));

// 도구 등록: 전달받은 registry를 복사하여 새 agent에 설정
let registry = options.tool_registry.unwrap_or_else(|| {
    // fallback: builtin만 (extension 없이 호출된 경우)
    Arc::new(oxicode_agent::ToolRegistry::with_builtins_cwd(PathBuf::from(&cwd)))
});
let agent_tools = agent.tools();
for name in registry.names() {
    if let Some(tool) = registry.get(&name) {
        agent_tools.register_arc(tool);
    }
}
```

#### tui_interactive.rs

```rust
pub async fn run_tui_interactive(app: crate::App) -> Result<()> {
    // ...
    let create_result = create_agent_session_from_services(
        CreateAgentSessionFromServicesOptions {
            services: services.clone(),
            session_manager,
            model_id: Some(app.model_id()),
            thinking_level: Some(settings.thinking_level),
            scoped_models: Vec::new(),
            tool_registry: Some(app.agent().tools()),  // ← App의 ToolRegistry 전달
        },
    )?;
    // ...
}
```

---

### 항목 1: CLI 인자 확장

**변경 파일:** `oxicode-cli/src/cli.rs`, `oxicode-cli/src/main.rs`

cli.rs에 인자 5개 추가:

```rust
pub struct CliArgs {
    // 기존...

    /// Output mode: text or json (newline-delimited JSON events)
    #[arg(long)]
    pub mode: Option<String>,

    /// Comma-separated list of tools to enable. Default: all builtins.
    #[arg(long)]
    pub tools: Option<String>,

    /// Append system prompt from a file
    #[arg(long)]
    pub append_system_prompt: Option<PathBuf>,

    /// Single-shot print mode (shorthand for non-interactive)
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Disable session persistence
    #[arg(long)]
    pub no_session: bool,
}
```

main.rs에서 `--mode json` 분기 (항목 0의 도구 등록 이후):

```rust
if args.mode.as_deref() == Some("json") || args.print {
    let mode = if args.mode.as_deref() == Some("json") {
        oxicode::print_mode::PrintMode::Json
    } else {
        oxicode::print_mode::PrintMode::Text
    };
    let options = oxicode::print_mode::PrintModeOptions {
        mode,
        initial_message: if prompt.is_empty() { None } else { Some(prompt) },
        messages: vec![],
    };
    let exit_code = oxicode::print_mode::run_print_mode(&app, options).await?;
    std::process::exit(exit_code);
}
```

`--append-system-prompt`는 main.rs에서 settings에 반영 후 App 생성 시 반영.
`--no-session`은 session 생성 시 플래그로 전달.

---

### 항목 2: --tools로 도구 필터링

**변경 파일:** `oxicode-agent/src/tools.rs`, `oxicode-cli/src/main.rs`, `oxicode-agent/src/tools/subagent.rs`

#### tools.rs

```rust
impl ToolRegistry {
    /// Create registry with selected builtins only.
    pub fn with_selected_tools(cwd: PathBuf, names: &[&str]) -> Self {
        let full = Self::with_builtins_cwd(cwd);
        let registry = Self::new();
        let set: std::collections::HashSet<&str> = names.iter().copied().collect();
        for name in full.names() {
            if set.contains(name.as_str()) {
                if let Some(tool) = full.get(&name) {
                    registry.register_arc(tool);
                }
            }
        }
        registry
    }
}
```

#### main.rs

```rust
// 도구 등록 시 --tools 인자 고려
let builtin_registry = if let Some(ref tools_str) = args.tools {
    let names: Vec<&str> = tools_str.split(',').map(|s| s.trim()).collect();
    oxicode_agent::ToolRegistry::with_selected_tools(cwd.clone(), &names)
} else {
    oxicode_agent::ToolRegistry::with_builtins_cwd(cwd.clone())
};

let tools = app.agent_tools();
for name in builtin_registry.names() {
    if let Some(tool) = builtin_registry.get(&name) {
        tools.register_arc(tool);
    }
}
```

#### subagent.rs

에이전트 정의의 `tools` 필드를 `--tools`로 전달:

```rust
if let Some(ref agent_tools) = agent.tools {
    if !agent_tools.is_empty() {
        args.push("--tools".to_string());
        args.push(agent_tools.join(","));
    }
}
```

---

### 항목 3: 프로세스 스폰

**변경 파일:** `oxicode-agent/src/tools/subagent.rs`

`run_single_agent`에 `binary_path` 파라미터 추가:

```rust
async fn run_single_agent(
    cwd: &Path,
    agents: &[AgentConfig],
    agent_name: &str,
    task: &str,
    agent_cwd: Option<&str>,
    step: Option<usize>,
    signal: Option<oneshot::Receiver<()>>,
    on_progress: Option<ProgressFn>,
    binary_path: &Path,   // ← 추가
) -> SingleResult {
    // ...
    let mut cmd = Command::new(binary_path);
    // ...
}
```

`SubagentTool`에서 호출:

```rust
impl SubagentTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            binary_path: None,
        }
    }

    fn get_binary(&self) -> PathBuf {
        self.binary_path.clone()
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("oxicode"))
    }
}

// execute() 내부
let binary = self.get_binary();
let result = run_single_agent(
    &self.cwd, &agents, agent_name, task,
    agent_cwd, None, None, None,
    &binary,   // ← 전달
).await;
```

`run_parallel`에도 `binary_path` 전파:

```rust
async fn run_parallel(
    cwd: &Path,
    agents: &[AgentConfig],
    tasks: Vec<ParallelTask>,
    binary_path: PathBuf,   // ← 추가
    on_progress: Option<ProgressFn>,
) -> Vec<SingleResult> {
    // 각 태스크에서:
    run_single_agent(&cwd, &agents, ..., &binary_path).await
}
```

---

### 항목 4: Abort 처리 — 채널 기반 select! + 조건부 대기

**변경 파일:** `oxicode-agent/src/tools/subagent.rs`

```rust
async fn run_single_agent(/* ... */) -> SingleResult {
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // stdout 읽기 태스크 분리
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    let _reader_handle = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line_tx.send(line).is_err() { break; }
        }
    });

    // stderr 읽기 태스크 분리
    let stderr_handle = tokio::spawn(async move {
        let mut err = String::new();
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            err.push_str(&line);
            err.push('\n');
        }
        err
    });

    // 메인 루프
    let mut final_text = String::new();
    let mut signal_rx = signal;

    loop {
        let aborted = tokio::select! {
            line = line_rx.recv() => {
                match line {
                    Some(line) => {
                        process_json_line(&line, &mut result, &mut final_text, &on_progress);
                        continue;
                    }
                    None => break false,  // stdout EOF
                }
            }
            _ = async {
                match &mut signal_rx {
                    Some(rx) => { let _ = rx.await; }
                    None => std::future::pending::<()>().await,
                }
            } => true,
        };

        if aborted {
            result.stop_reason = Some("aborted".into());
            // SIGTERM → 자식 종료 대기 (최대 5초) → SIGKILL
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
            }

            // 자식이 정상 종료되면 바로 진행, 아니면 5초 후 SIGKILL
            #[cfg(unix)]
            {
                let deadline = tokio::time::sleep(Duration::from_secs(5));
                tokio::pin!(deadline);
                tokio::select! {
                    _ = &mut deadline => { let _ = child.start_kill(); }
                    _ = child.wait() => {}
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                let _ = child.start_kill();
            }
            break;
        }
    }

    // 정상 종료 시
    if result.stop_reason.is_none() {
        if let Ok(err_output) = stderr_handle.await {
            result.stderr = err_output;
        }
        match child.wait().await {
            Ok(status) => result.exit_code = status.code().unwrap_or(1),
            Err(_) => result.exit_code = 1,
        }
    }

    result.output = final_text;
    result
}
```

---

### 항목 5: Usage 추적

**변경 파일:** `oxicode-cli/src/print_mode.rs`, `oxicode-agent/src/tools/subagent.rs`, `oxicode-agent/src/events.rs`

#### 5a. UsageStats (v3 리뷰: 필드 유지 + turns 추가)

```rust
// subagent.rs — 기존 필드 유지, turns만 추가
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,   // 항상 0 (향후 확장)
    pub cache_write: u64,  // 항상 0 (향후 확장)
    pub cost: f64,          // 항상 0.0 (향후 확장)
    pub turns: u32,         // ← 새로 추가
}
```

cache_read, cache_write, cost 유지. 항상 0이지만 API 안정성을 위해 제거하지 않음.
AgentEvent::Usage가 확장되면 그때 실제 값으로 채움.

#### 5b. print_mode.rs에 Usage 이벤트 JSON 출력 추가

```rust
// event_to_json()에 추가:
AgentEvent::Usage { input_tokens, output_tokens } => serde_json::json!({
    "type": "usage",
    "input_tokens": input_tokens,
    "output_tokens": output_tokens,
}),
```

이 매치를 `_ => unknown` 앞에 배치.

#### 5c. subagent.rs에서 usage 이벤트 수집

```rust
fn process_json_line(
    line: &str,
    result: &mut SingleResult,
    text: &mut String,
    on_progress: &Option<ProgressFn>,
) {
    let event: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => return };
    match event["type"].as_str().unwrap_or("") {
        "text_delta" => {
            if let Some(t) = event["text"].as_str() { text.push_str(t); }
        }
        "usage" => {
            result.usage.input_tokens += event["input_tokens"].as_u64().unwrap_or(0);
            result.usage.output_tokens += event["output_tokens"].as_u64().unwrap_or(0);
            result.usage.turns += 1;
        }
        "complete" => { result.stop_reason = Some("complete".into()); }
        "error" => {
            result.error_message = Some(event["message"].as_str().unwrap_or("Unknown error").into());
            result.stop_reason = Some("error".into());
        }
        _ => {}
    }
}
```

---

### 항목 6: 임시 파일 정리 — RAII

**변경 파일:** `oxicode-agent/src/tools/subagent.rs`

```rust
/// RAII guard: Drop 시 임시 디렉토리 삭제.
struct TempDirGuard(PathBuf);

impl TempDirGuard {
    fn new(prefix: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn path(&self) -> &Path { &self.0 }
    fn prompt_path(&self) -> PathBuf { self.0.join("system_prompt.md") }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        // 프로세스가 종료된 후에만 삭제 (함수 스코프이므로 보장됨)
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
```

`run_single_agent` 함수 스코프에 배치:

```rust
let tmp_dir = TempDirGuard::new("oxicode-subagent")
    .map_err(|e| format!("Failed to create temp dir: {}", e))?;
if !agent.system_prompt.is_empty() {
    std::fs::write(tmp_dir.prompt_path(), &agent.system_prompt)
        .map_err(|e| format!("Failed to write prompt: {}", e))?;
    args.push("--append-system-prompt".to_string());
    args.push(tmp_dir.prompt_path().to_str().unwrap_or_default().to_string());
}
// tmp_dir은 함수 끝까지 살아있음 → Drop에서 정리
```

---

### 항목 7: 프로젝트 에이전트 순회 — .git 경계

**변경 파일:** `oxicode-agent/src/tools/subagent.rs`

```rust
/// Walk up from `cwd` to find `.oxicode/agents/`.
/// Stops at `.git` boundary (project root). Returns None if not found.
fn find_project_agents_dir(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let candidate = current.join(".oxicode").join("agents");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if current.join(".git").exists() {
            return None;  // 프로젝트 루트에 도달했는데 .oxicode/agents가 없음
        }
        current = current.parent()?;
    }
}
```

`discover_agents()`에서 사용:

```rust
pub fn discover_agents(cwd: &Path, scope: AgentScope) -> Vec<AgentConfig> {
    // ...
    if scope == AgentScope::Project || scope == AgentScope::Both {
        if let Some(project_dir) = find_project_agents_dir(cwd) {
            load_agents_from_dir(&project_dir, "project", &mut agents, &mut seen_names);
        }
    }
    // ...
}
```

---

### 항목 8: 스트리밍 진행 상황

**변경 파일:** `oxicode-agent/src/tools/subagent.rs`

```rust
type ProgressFn = Arc<dyn Fn(String) + Send + Sync>;

// run_single_agent에 on_progress 전달 (항목 4에서 이미 포함)

// 단일 모드:
if let Some(ref cb) = on_progress {
    cb(format!("[{}] running...", agent_name));
}

// 체인 모드 (execute 내부):
for (i, step) in steps.into_iter().enumerate() {
    if let Some(ref cb) = on_progress {
        cb(format!("Chain {}/{}: {} running...", i + 1, total, step.agent));
    }
    let result = run_single_agent(/* ..., on_progress.clone(), &binary */).await;
    // ...
}

// 병렬 모드:
// 각 태스크에 Arc::clone(&on_progress) 전달
// 태스크 내에서 cb(format!("Parallel [{}] running...", agent_name))
```

SubagentTool이 on_progress 콜백을 저장:

```rust
pub struct SubagentTool {
    cwd: PathBuf,
    binary_path: Option<PathBuf>,
    progress_callback: parking_lot::Mutex<Option<ProgressFn>>,
}

// AgentTool::on_progress 구현
fn on_progress(&self, callback: super::ProgressCallback) {
    *self.progress_callback.lock() = Some(callback);
}

// execute()에서:
let progress = self.progress_callback.lock().clone();
let result = run_single_agent(/* ..., progress, &binary */).await;
```

---

## 구현 순서

| 단계 | 항목 | 선행 | 난이도 | 변경 파일 |
|------|------|------|--------|-----------|
| **A** | 항목 0: ToolRegistry 단일 소유권 | 없음 | ⭐⭐ | main, lib, tui_interactive, agent_session_runtime |
| **B** | 항목 1: CLI 인자 확장 | A | ⭐⭐ | cli, main |
| **C** | 항목 3: 프로세스 스폰 (binary_path) | B | ⭐ | subagent |
| **D** | 항목 2: --tools 필터링 | A, B | ⭐ | tools, main, subagent |
| **E** | 항목 4: Abort + libc dep | 없음 | ⭐⭐⭐ | Cargo.toml, subagent |
| **F** | 항목 5: Usage (필드 유지 + 이벤트) | B | ⭐⭐ | print_mode, subagent |
| **G** | 항목 6: 임시 파일 RAII | 없음 | ⭐ | subagent |
| **H** | 항목 7: 프로젝트 순회 | 없음 | ⭐ | subagent |
| **I** | 항목 8: 스트리밍 (ProgressCallback 재사용) | 없음 | ⭐⭐ | subagent |

A → B → C → D 순차. E~I는 독립 (병렬 가능).
E, G, H는 subagent.rs 내부 변경이므로 **한 번에 통합 구현** 권장.

## 파일 변경 요약

```
oxicode-cli/src/cli.rs                  — 인자 5개 추가
oxicode-cli/src/main.rs                 — --mode 분기, ToolRegistry 구성, 도구 등록
oxicode-cli/src/lib.rs                  — App (변경 없음, 도구 등록은 main에서)
oxicode-cli/src/tui_interactive.rs      — tool_registry 전달
oxicode-cli/src/agent_session_runtime.rs — tool_registry 옵션 필드
oxicode-cli/src/print_mode.rs           — Usage 이벤트 JSON 출력
oxicode-agent/Cargo.toml              — libc (unix) 추가
oxicode-agent/src/tools.rs              — with_selected_tools() 추가
oxicode-agent/src/tools/subagent.rs     — 항목 3,4,5,6,7,8 통합 개선
```
