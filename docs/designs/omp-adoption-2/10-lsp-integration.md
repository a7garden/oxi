# 세부 설계 ⑧ — LSP 통합 (oxicode-lsp 독립 크레이트)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §1·§10 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md), 1차 [`02-internal-url-router.md`](../omp-adoption/02-internal-url-router.md)
> omp 분석: `lsp/` (~7,600줄, 17개 파일), `lsp/index.ts` (2,481줄), `lsp/client.ts` (1,194줄)
> 후속: N4 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

omp의 LSP 서브시스템은 **두 가지 얼굴**을 가진다:

1. **LspTool** — 모델이 직접 호출하는 14개 오퍼레이션 (diagnostics, definition, references, rename, code_actions...).
2. **writethrough 훅** — `write`/`edit` 도구가 파일 저장 직후 자동으로 끌어쓰는 포맷 + 진단 주입.

oxicode는 LSP가 **완전히 부재**다. 본 설계는 **`oxicode-lsp` 독립 크레이트**를 신규 생성하고, feature 게이트로 `oxicode-cli`에 통합한다.

**설계 원칙**: omp의 `types.ts` (LSP 3.16 스펙)는 거의 그대로 Rust struct로 옮기고, `client.ts`의 JSON-RPC 메시지 리더(chunk-list 프레이밍)와 진단 버전 정책이 핵심 이식 대상이다.

### omp가 검증한 가치
- **IDE 지식 공유** — 에이전트가 LSP가 아는 심볼/참조/진단을 알게 됨.
- **rename 안전성** — `workspace/willRenameFiles`로 barrel 파일/재export 자동 갱신.
- **저장 후 진단** — write/edit 직후 LSP 진단을 결과에 주입 → 모델이 에러를 즉시 인지.
- **다중 서버** — 확장자 기반 라우팅 (rust-analyzer, tsserver, gopls 동시).

---

## 1. omp 메커니즘

### 1.1 14개 LSP 오퍼레이션 (`lsp/index.ts`)

| # | action | LSP method | 반환 |
|---|---|---|---|
| 1 | diagnostics | publishDiagnostics + build | `N error(s): grouped messages` |
| 2 | definition | textDocument/definition | `Found N definition(s): file:line:col + 3행 컨텍스트` |
| 3 | type_definition | textDocument/typeDefinition | 동일 |
| 4 | implementation | textDocument/implementation | 동일 |
| 5 | references | textDocument/references | `Found N reference(s):` (앞 50개 컨텍스트) |
| 6 | hover | textDocument/hover | 타입 시그니처 + 문서 (마크다운) |
| 7 | symbols | documentSymbol / workspace/symbol | 트리 구조 |
| 8 | rename | textDocument/rename → applyWorkspaceEdit | 미리보기 또는 `Applied rename:` |
| 9 | rename_file | workspace/willRenameFiles | `Renamed → ...` + 편집 |
| 10 | code_actions | textDocument/codeAction | `N code action(s):` 또는 `Applied "..."` |
| 11 | status | getActiveClients | `Language servers: name (status)` |
| 12 | reload | rust-analyzer/reloadWorkspace | `Reloaded name` |
| 13 | capabilities | serverCapabilities 덤프 | JSON |
| 14 | request | 임의 sendRequest | `name ← method: JSON` |

### 1.2 JSON-RPC 클라이언트 (`lsp/client.ts`, 1,194줄)

핵심 — **chunk-list 프레이밍** (O(n²) concat 회피):

```
서버 stdout → pendingChunks: Buffer[]
  → findHeaderEndInChunks (\r\n\r\n 탐색)
  → Content-Length 파싱 → copyChunkRange / dropChunkFront
  → JSON.parse → 라우팅:
      - id + pendingRequests hit → resolve/reject
      - id + method → handleServerRequest (workspace/configuration 등)
      - method only → notification (publishDiagnostics, $/progress)
```

**클라이언트 관리 전략**:
- 캐시 키: `${command}:${cwd}` → 동일 서버 재사용.
- 락: `clientLocks` (생성 중복 방지) + `fileOperationLocks` (per-file 직렬화).
- 네거티브 캐시: `initFailures` (3분) — 결정론적 실패는 fast-fail.
- 아들 타임아웃: 60s 간격 체크, 초과 시 shutdown 핸드셰이크.
- 크래시 복구: 프로세스 종료 시 clients에서 제거, pending reject.

### 1.3 진단 버전 정책

```typescript
// waitForDiagnostics:
// - exact-version-match → 즉시 반환
// - unversioned → settleMs(250ms) 정찰 (tsserver가 버전 안 echo)
```

### 1.4 writethrough (write/edit 통합)

```
WriteTool/EditTool 저장 직후:
  → createLspWritethrough(enableFormat, enableDiagnostics)
  → runLspWritethrough:
      1. captureDiagnosticVersions (사전 베이스라인)
      2. syncFileContent (didOpen/didChange)
      3. formatContent (LSP formatting 또는 linter CLI)
      4. writeContent to disk
      5. notifyFileSaved (didSave)
      6. fetchDiagnosticsWithDeferral:
           - 500ms 내 fresh 결과 → inline
           - 초과 → 백그라운드 fetch(12s) → 지연 주입 채널
```

### 1.5 DiagnosticsLedger (`lsp/diagnostics-ledger.ts`, 53줄)

```typescript
class DiagnosticsLedger {
    // per-path Set<identity>로 진단 중복 제거
    private seen = new Map<string, Set<string>>();
    
    diagnosticIdentity(message: string): string {
        // `path:line:col ` 접두사 제거 (본문만 키로)
        return message.replace(/^[\w./-]+:\d+:\d+\s*/, "");
    }
    
    reduce(absPath: string, result: Diagnostic[]): Diagnostic[] {
        // 이전에 본 것 제외한 fresh만 반환
        // 전부 사라지면 맵에서 삭제
    }
}
```

### 1.6 edits.ts — 편집 적용

- `applyTextEditsToString`: 역순 정렬 후 적용 (인덱스 보존).
- `sortAndValidateTextEdits`: 겹침 감지 → `ToolError` (멀티서버 rename 불일치 방어).
- `applyWorkspaceEdit`: 사전 일괄 검증 → 충돌 시 절반 적용 방지.

### 1.7 config — 서버 설정

소스 우선순위 (높→낮):
1. 프로젝트 루트 `lsp.json` / `.lsp.json` / `.yaml`
2. `.oxicode/lsp.*`
3. `~/.oxicode/lsp.*`
4. 번들 `defaults.json` (40+ 서버)

자동감지: rootMarkers 존재 + `resolveCommand` (node_modules/.bin, .venv/bin, $PATH) 통과 시만 활성화.

---

## 2. oxicode화 설계: `oxicode-lsp` 크레이트

### 2.1 크레이트 구조

```
oxicode-lsp/  (feature-gated, oxicode-cli가 --features lsp로 활성화)
├── Cargo.toml          의존: lsp-server, lsp-types, serde, tokio, parking_lot
├── src/
│   ├── lib.rs          공개 API
│   ├── types.rs        LSP 3.16 타입 (omp types.ts 이식)
│   ├── client.rs       JSON-RPC 클라이언트 (omp client.ts 이식)
│   ├── manager.rs      다중 서버 관리 (lspmux 대응)
│   ├── operations.rs   14개 오퍼레이션
│   ├── diagnostics.rs  진단 누적/버전 추적 (DiagnosticsLedger)
│   ├── edits.rs        TextEdit/WorkspaceEdit 적용
│   ├── config.rs       서버 설정 로드/병합/자동감지
│   ├── writethrough.rs write/edit 통합 훅
│   └── render.rs       결과 → ToolResult 포맷
├── defaults.json       40+ 서버 기본 정의
└── tests/
```

### 2.2 Cargo.toml

```toml
[package]
name = "oxicode-lsp"
version = "0.1.0"
edition = "2024"

[dependencies]
lsp-server = "0.7"
lsp-types = "0.95"      # LSP 3.17 타입 (omp는 3.16, 최신 사용)
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
parking_lot = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[features]
default = []
# tree-sitter 구문 강조 (선택)
syntax-highlight = ["tree-sitter", "tree-sitter-rust"]
```

### 2.3 JSON-RPC 클라이언트 (`client.rs`)

omp의 chunk-list 프레이밍을 Rust로 이식:

```rust
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command, ChildStdin, ChildStdout};

pub struct LspClient {
    process: Child,
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending_requests: parking_lot::RwLock<HashMap<i64, oneshot::Sender<jsonrpc::Response>>>,
    diagnostics: parking_lot::RwLock<HashMap<String, FileDiagnostics>>,
    diagnostics_version: Arc<AtomicU64>,
    server_capabilities: parking_lot::RwLock<Option<lsp_types::ServerCapabilities>>,
    open_files: parking_lot::RwLock<HashSet<String>>,
}

impl LspClient {
    pub async fn spawn(command: &str, args: &[&str], cwd: &Path) -> anyhow::Result<Arc<Self>> {
        let mut process = Command::new(command)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        let stdin = process.stdin.take().unwrap();
        let stdout = process.stdout.take().unwrap();
        
        let client = Arc::new(Self {
            process,
            stdin: tokio::sync::Mutex::new(stdin),
            pending_requests: Default::default(),
            diagnostics: Default::default(),
            diagnostics_version: Arc::new(AtomicU64::new(0)),
            server_capabilities: Default::default(),
            open_files: Default::default(),
        });
        
        // 메시지 리더 태스크 시작
        let client_clone = Arc::clone(&client);
        tokio::spawn(async move {
            client_clone.message_reader(stdout).await;
        });
        
        // initialize 핸드셰이크
        client.initialize().await?;
        
        Ok(client)
    }
    
    /// 메시지 리더 — chunk-list 프레이밍 (omp startMessageReader 이식).
    async fn message_reader(self: Arc<Self>, stdout: ChildStdout) {
        let mut reader = BufReader::new(stdout);
        let mut buffer = BytesMut::with_capacity(8192);
        
        loop {
            // Content-Length 헤더까지 읽기
            let mut header_buf = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                if reader.read(&mut byte).await.unwrap_or(0) == 0 { break; }
                header_buf.push(byte[0]);
                if header_buf.ends_with(b"\r\n\r\n") { break; }
            }
            
            // Content-Length 파싱
            let header_str = String::from_utf8_lossy(&header_buf);
            let content_length: usize = header_str
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: ")
                    .and_then(|v| v.trim().parse().ok()))
                .unwrap_or(0);
            
            if content_length == 0 { continue; }
            
            // 본문 읽기
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).await.unwrap_or(0);
            
            // JSON 파싱 + 라우팅
            if let Ok(msg) = serde_json::from_slice::<jsonrpc::Message>(&body) {
                self.route_message(msg);
            }
        }
    }
    
    fn route_message(&self, msg: jsonrpc::Message) {
        match msg {
            jsonrpc::Message::Response(resp) => {
                if let Some(sender) = self.pending_requests.write().remove(&resp.id) {
                    let _ = sender.send(resp);
                }
            }
            jsonrpc::Message::Request(req) => {
                // workspace/configuration, workspace/workspaceFolders, workspace/applyEdit
                tokio::spawn(self.clone().handle_server_request(req));
            }
            jsonrpc::Message::Notification(notif) => {
                if notif.method == "textDocument/publishDiagnostics" {
                    self.handle_diagnostics(notif.params);
                }
            }
        }
    }
    
    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        let id = next_request_id();
        let (tx, rx) = oneshot::channel();
        self.pending_requests.write().insert(id, tx);
        
        let request = jsonrpc::Request {
            id: jsonrpc::Id::Number(id),
            method: method.into(),
            params: Some(params),
        };
        
        let json = serde_json::to_string(&request)?;
        let framed = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        
        self.stdin.lock().await.write_all(framed.as_bytes()).await?;
        
        // 타임아웃
        let resp = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| anyhow::anyhow!("LSP request timeout: {}", method))??;
        
        match resp.result {
            Some(result) => Ok(result),
            None => Err(anyhow::anyhow!("LSP error: {:?}", resp.error)),
        }
    }
}
```

### 2.4 클라이언트 매니저 (`manager.rs`)

```rust
pub struct LspManager {
    clients: parking_lot::RwLock<HashMap<ClientKey, Arc<LspClient>>>,
    config: LspConfig,
    cwd: PathBuf,
    init_failures: parking_lot::RwLock<HashMap<ClientKey, Instant>>,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
struct ClientKey {
    command: String,
    cwd: String,
}

impl LspManager {
    pub async fn get_or_create(&self, server_config: &ServerConfig) -> anyhow::Result<Arc<LspClient>> {
        let key = ClientKey {
            command: server_config.command.clone(),
            cwd: self.cwd.to_string_lossy().to_string(),
        };
        
        // 캐시 확인
        if let Some(client) = self.clients.read().get(&key) {
            return Ok(Arc::clone(client));
        }
        
        // 네거티브 캐시 확인 (3분)
        if let Some(failed_at) = self.init_failures.read().get(&key) {
            if failed_at.elapsed() < Duration::from_secs(180) {
                anyhow::bail!("LSP server recently failed to initialize");
            }
        }
        
        // 생성
        match LspClient::spawn(&server_config.command, &server_config.args, &self.cwd).await {
            Ok(client) => {
                self.clients.write().insert(key.clone(), Arc::clone(&client));
                Ok(client)
            }
            Err(e) => {
                self.init_failures.write().insert(key, Instant::now());
                Err(e)
            }
        }
    }
    
    /// 파일 확장자로 적절한 서버 찾기.
    pub fn servers_for_file(&self, path: &str) -> Vec<ServerConfig> {
        let ext = Path::new(path).extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        
        self.config.servers.iter()
            .filter(|s| s.file_types.iter().any(|ft| ft == ext || path.ends_with(ft)))
            .cloned()
            .collect()
    }
}
```

### 2.5 14개 오퍼레이션 (`operations.rs`)

```rust
pub enum LspAction {
    Diagnostics { file: String },
    Definition { file: String, line: u32, symbol: Option<String> },
    TypeDefinition { file: String, line: u32, symbol: Option<String> },
    Implementation { file: String, line: u32, symbol: Option<String> },
    References { file: String, line: u32, symbol: Option<String> },
    Hover { file: String, line: u32, symbol: Option<String> },
    Symbols { file: String, query: Option<String> },
    Rename { file: String, line: u32, symbol: String, new_name: String, apply: bool },
    RenameFile { file: String, new_name: String, apply: bool },
    CodeActions { file: String, line: u32, symbol: Option<String>, apply: bool, query: Option<String> },
    Status,
    Reload { file: Option<String> },
    Capabilities { file: Option<String> },
    Request { file: Option<String>, query: String, payload: Option<serde_json::Value> },
}

impl LspAction {
    pub async fn execute(
        &self,
        manager: &LspManager,
    ) -> anyhow::Result<String> {
        match self {
            Self::Diagnostics { file } => {
                let diags = manager.get_diagnostics(file).await?;
                Ok(format_diagnostics(&diags))
            }
            Self::Definition { file, line, symbol } => {
                let client = manager.client_for_file(file).await?;
                let col = resolve_symbol_column(&client, file, *line, symbol).await?;
                let locations = client.definition(file, *line, col).await?;
                Ok(format_locations("definition", &locations))
            }
            // ... 나머지 12개
        }
    }
}
```

### 2.6 workspace/willRenameFiles (`rename_file`)

```rust
pub async fn rename_file(
    manager: &LspManager,
    source: &Path,
    dest: &Path,
    apply: bool,
) -> anyhow::Result<String> {
    // 1. 사전 검증
    if !source.exists() { anyhow::bail!("Source does not exist"); }
    if dest.exists() { anyhow::bail!("Destination already exists"); }
    
    // 2. 파일 쌍 열거 (디렉토리면 재귀, 최대 1000쌍)
    let pairs = enumerate_rename_pairs(source, dest)?;
    if pairs.len() > 1000 {
        anyhow::bail!("Too many files to rename ({} > 1000)", pairs.len());
    }
    
    // 3. 관련 서버 필터링
    let servers = pairs.iter()
        .flat_map(|(old, new)| manager.servers_for_file(old))
        .dedup();
    
    // 4. 각 서버에 willRenameFiles 요청
    let mut all_edits = Vec::new();
    for server in servers {
        let client = manager.get_or_create(&server).await?;
        let edit = client.will_rename_files(&pairs).await?;
        if let Some(edit) = edit {
            all_edits.push((server.name.clone(), edit));
        }
    }
    
    if !apply {
        return Ok(format_workspace_edit_preview(&all_edits));
    }
    
    // 5. 편집 병합 + 적용 (사전 검증)
    let merged = merge_workspace_edits(all_edits)?;
    validate_no_conflicts(&merged)?;
    apply_workspace_edit(&merged)?;
    
    // 6. 파일시스템 rename
    std::fs::rename(source, dest)?;
    
    // 7. didRenameFiles 통지
    for server in servers {
        let client = manager.get_or_create(&server).await?;
        client.did_rename_files(&pairs).await?;
    }
    
    Ok(format!("Renamed {} → {}", source.display(), dest.display()))
}
```

### 2.7 writethrough (`writethrough.rs`)

```rust
pub struct LspWritethrough {
    manager: Arc<LspManager>,
    enable_format: bool,
    enable_diagnostics: bool,
    diagnostics_ledger: Arc<DiagnosticsLedger>,
}

impl LspWritethrough {
    /// write/edit 도구가 파일 저장 직후 호출.
    pub async fn run(&self, path: &Path, content: &str) -> anyhow::Result<FileDiagnosticsResult> {
        // 1. 진단 베이스라인
        let baseline = self.manager.capture_diagnostic_versions(path).await;
        
        // 2. 파일 내용 동기화 (didOpen/didChange)
        let servers = self.manager.servers_for_file(&path.to_string_lossy());
        for server in &servers {
            let client = self.manager.get_or_create(server).await?;
            client.sync_content(path, content).await?;
        }
        
        // 3. 포맷 (선택)
        let formatted = if self.enable_format {
            self.format_content(path, content).await?
        } else {
            content.to_string()
        };
        
        // 4. 진단 수집 (500ms 인라인 예산)
        let diagnostics = if self.enable_diagnostics {
            self.fetch_diagnostics_with_deferral(path, &baseline, Duration::from_millis(500)).await
        } else {
            Vec::new()
        };
        
        // 5. 중복 제거 (DiagnosticsLedger)
        let fresh = self.diagnostics_ledger.reduce(&path.to_string_lossy(), diagnostics);
        
        Ok(FileDiagnosticsResult {
            formatted_content: formatted,
            diagnostics: fresh,
        })
    }
}
```

---

## 3. oxicode-agent 통합: `lsp` 도구

### 3.1 도구 정의

`oxicode-agent/src/tools/lsp.rs` (oxicode-lsp 브릿지):

```rust
pub struct LspTool {
    manager: Option<Arc<LspManager>>,  // None = LSP 비활성화
}

impl AgentTool for LspTool {
    fn name(&self) -> &str { "lsp" }
    fn essential(&self) -> bool { false }
    fn description(&self) -> &str {
        "Language Server Protocol operations: diagnostics, definition, references, \
         rename, code_actions, hover, symbols. Requires LSP enabled."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["diagnostics", "definition", "type_definition", "implementation",
                             "references", "hover", "symbols", "rename", "rename_file",
                             "code_actions", "status", "reload", "capabilities", "request"]
                },
                "file": {"type": "string"},
                "line": {"type": "integer"},
                "symbol": {"type": "string"},
                "new_name": {"type": "string"},
                "apply": {"type": "boolean"},
                "query": {"type": "string"}
            },
            "required": ["action"]
        })
    }
    
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        let manager = self.manager.as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("LSP not enabled".into()))?;
        
        let action: LspAction = serde_json::from_value(params)?;
        let result = action.execute(manager).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        
        Ok(AgentToolResult::success(result))
    }
}
```

### 3.2 write/edit 통합

`oxicode-agent/src/tools/write.rs` / `edit.rs`에 writethrough 훅 추가:

```rust
// write.rs — 파일 저장 후
if let Some(writethrough) = ctx.lsp_writethrough.as_ref() {
    match writethrough.run(&path, &content).await {
        Ok(result) => {
            // 포맷된 내용으로 디스크 갱신
            if result.formatted_content != content {
                std::fs::write(&path, &result.formatted_content)?;
            }
            // 진단을 결과에 추가
            if !result.diagnostics.is_empty() {
                output.push_str(&format_diagnostics_note(&result.diagnostics));
            }
        }
        Err(e) => {
            // writethrough 실패는 쓰기를 롤백하지 않음 — 경고만
            tracing::warn!("LSP writethrough failed: {}", e);
        }
    }
}
```

---

## 4. 설정

```rust
pub struct Settings {
    pub lsp_enabled: bool,                  // 기본 false (무거운 의존)
    pub lsp_format_on_write: bool,          // 기본 true (lsp_enabled 시)
    pub lsp_diagnostics_on_write: bool,     // 기본 true
    pub lsp_config_path: Option<PathBuf>,   // 기본 .oxicode/lsp.json
    pub lsp_idle_timeout_secs: u64,         // 기본 300 (5분)
}
```

`oxicode-cli/Cargo.toml`:
```toml
[features]
default = []
lsp = ["oxicode-lsp"]
```

---

## 5. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N4.16 | `oxicode-lsp` 크레이트 스캐폴드 + Cargo.toml | — |
| N4.17 | `types.rs` (LSP 3.17 타입 — lsp-types 크레이트 활용) | N4.16 |
| N4.18 | `client.rs` (JSON-RPC 클라이언트 + 메시지 리더) | N4.17 |
| N4.19 | `manager.rs` (다중 서버 관리 + 캐시 + 네거티브 캐시) | N4.18 |
| N4.20 | `config.rs` (설정 로드 + defaults.json) | N4.16 |
| N4.21 | `operations.rs` — diagnostics, definition, hover (기본 3개) | N4.19 |
| N4.22 | `operations.rs` — references, symbols, implementation | N4.21 |
| N4.23 | `operations.rs` — rename, code_actions | N4.22 |
| N4.24 | `rename_file` (workspace/willRenameFiles) | N4.23 |
| N4.25 | `edits.rs` (TextEdit 역순 적용 + 겹침 검증) | N4.17 |
| N4.26 | `diagnostics.rs` (DiagnosticsLedger + 버전 추적) | N4.18 |
| N4.27 | `writethrough.rs` (write/edit 훅) | N4.26 |
| N4.28 | `render.rs` (결과 포맷 — omp render.ts 이식) | N4.21 |
| N4.29 | `lsp` 도구 (oxicode-agent 브릿지) | N4.28 |
| N4.30 | write/edit writethrough 통합 | N4.27, N4.29 |
| N4.31 | ⑪ Commit rename_file 연동 | N4.24, ⑪ |
| N4.32 | 단위 테스트 (omp 계약 이식) | N4.29 |

> **독립성**: ⑧은 ⑪ Commit 이외와 독립. 1차 ② URL Router와 `lsp://` 스킴 연동 (후순위).

---

## 6. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| `lsp-server` + `lsp-types` 의존 크기 | 🟠 확인 필요 | 바이너리 크기. feature 게이트로 완화 |
| 메시지 리더 성능 (BytesMut vs VecDeque) | 🟡 최적화 | omp는 chunk-list O(n²) 회피. Rust는 BytesMut + cursor |
| 진단 버전 정책 (tsserver 버전 미echo) | 🟢 해결 | unversioned는 250ms 정찰 (omp 계약) |
| rust-analyzer 워크스페이스 준비 폴링 | 🟢 이식 | analyzerStatus 폴링 (timeout 5s, poll 100ms) |
| lspmux 통합 (rust-analyzer 다중화) | 🔴 후순위 | omp는 rust-analyzer 한정. 초기는 직접 spawn |
| Biome/SwiftLint linter 클라이언트 | 🔴 후순위 | omp는 CLI 기반 linter. 초기는 LSP formatting만 |
| `lsp://` URL 스킴 (1차 ② 연동) | 🔴 후순위 | URL Router 완료 후 |
| tree-sitter 구문 강조 | 🔴 후순위 | `syntax-highlight` feature. 별도 설계 |

---

## 7. 테스트 계획

```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn client_initialize_handshake() {
        // rust-analyzer (또는 mock 서버)로 initialize/initialized 확인
    }

    #[test]
    fn servers_for_file_by_extension() {
        let config = load_test_config();
        let rust_servers = config.servers_for_file("src/main.rs");
        assert!(rust_servers.iter().any(|s| s.command.contains("rust-analyzer")));
        
        let ts_servers = config.servers_for_file("src/index.ts");
        assert!(ts_servers.iter().any(|s| s.command.contains("typescript-language-server")));
    }

    #[test]
    fn diagnostics_ledger_dedup() {
        let mut ledger = DiagnosticsLedger::new();
        let diags = vec![
            diagnostic("src/main.rs:10:5 unused variable"),
            diagnostic("src/main.rs:10:5 unused variable"),  // 중복
            diagnostic("src/main.rs:20:3 type mismatch"),
        ];
        let fresh = ledger.reduce("src/main.rs", diags);
        // 중복 제거 → 2개 (첫 "unused" + "type mismatch")
        // (실제로는 identity 기반으로 첫 등장만 fresh)
    }

    #[test]
    fn text_edit_reverse_order_preserves_indices() {
        let text = "line1\nline2\nline3";
        let edits = vec![
            text_edit(0, 0, 0, 4, "LINE"),    // line1 → LINE1
            text_edit(1, 0, 1, 4, "LINE"),    // line2 → LINE2
        ];
        let result = apply_text_edits_to_string(text, edits);
        assert!(result.contains("LINE1"));
        assert!(result.contains("LINE2"));
    }
}
```

---

## 8. 부록: omp → oxicode 매핑

| omp 위치 | oxicode 위치 |
|---|---|
| `lsp/index.ts` (2,481) | `oxicode-lsp/src/operations.rs` + `oxicode-agent/src/tools/lsp.rs` |
| `lsp/client.ts` (1,194) | `oxicode-lsp/src/client.rs` |
| `lsp/types.ts` (445) | `oxicode-lsp/src/types.rs` (또는 `lsp-types` 크레이트 직접 사용) |
| `lsp/utils.ts` (719) | `oxicode-lsp/src/utils.rs` |
| `lsp/render.ts` (669) | `oxicode-lsp/src/render.rs` |
| `lsp/config.ts` (503) | `oxicode-lsp/src/config.rs` |
| `lsp/edits.ts` (279) | `oxicode-lsp/src/edits.rs` |
| `lsp/lspmux.ts` (204) | 후순위 (oxicode-lsp/src/manager.rs에 통합) |
| `lsp/defaults.json` (500) | `oxicode-lsp/defaults.json` (직접 복사) |
| `lsp/format-options.ts` (122) | `oxicode-lsp/src/format_options.rs` |
| `lsp/diagnostics-ledger.ts` (53) | `oxicode-lsp/src/diagnostics.rs` |
| `lsp/startup-events.ts` (16) | `oxicode-lsp/src/events.rs` |
| `lsp/clients/` (biome, swiftlint) | 후순위 |
| `tools/write.ts` (writethrough) | `oxicode-lsp/src/writethrough.rs` + `oxicode-agent/src/tools/write.rs` |
| `edit/index.ts` (writethrough) | `oxicode-lsp/src/writethrough.rs` + `oxicode-agent/src/tools/edit.rs` |
