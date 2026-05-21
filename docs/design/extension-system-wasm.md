# oxi Extension System Design — Extism/WASM

> Date: 2026-05-09
> Status: Design
> Author: oxi team

## 1. 목표

oxi 사용자가 **자신만의 확장을 작성**하여 oxi에 **런타임에 로드**할 수 있게 한다.
oxi 버전이 업데이트되어도 **기존 확장이 깨지지 않아야** 한다.

## 2. 왜 Extism/WASM인가

| 대안 | 문제 |
|------|------|
| Rust cdylib | ABI 불안정, oxi 업데이트 시 전부 깨짐 |
| stabby | 성숙도 낮음, 여전히 Rust 버전 의존 |
| C ABI + JSON | 복잡도 대비 이점 적음, 샌드박스 없음 |
| **Extism/WASM** | **ABI 안정, 샌드박스, 다언어, 프로덕션 검증** |

## 3. 아키텍처

```
┌──────────────────────────────────────────────────────────┐
│                       oxi Core                           │
│                                                          │
│  ┌────────────────────────────────────────────────────┐  │
│  │              Extension Manager                     │  │
│  │                                                    │  │
│  │  discover() → ~/.oxi/extensions/*.wasm             │  │
│  │  load()     → Extism::Plugin::new()                │  │
│  │  invoke()   → plugin.call("method", json)          │  │
│  └────────────────────────────────────────────────────┘  │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐   │
│  │ extism       │  │ extism-pdk   │  │ wasmtime     │   │
│  │ (host SDK)   │  │ (확장 PDK)   │  │ (런타임)     │   │
│  │ v1.21        │  │ v1.4         │  │ (extism 내부) │   │
│  └──────────────┘  └──────────────┘  └──────────────┘   │
└──────────────────────────────────────────────────────────┘
```

## 4. 확장 인터페이스 (WASM → oxi)

확장은 WASM 모듈로 컴파일되며, 다음 **함수들을 export**합니다:

### 4.1 필수 함수

| Export 함수 | 목적 | 입력 | 출력 |
|-------------|------|------|------|
| `init` | 확장 초기화 | `{}` | `{ "name": "...", "version": "...", "description": "..." }` |
| `register_tools` | 도구 등록 | `{}` | `{ "tools": [{ "name": "...", "description": "...", "schema": {...} }] }` |
| `execute_tool` | 도구 실행 | `{ "tool": "...", "params": {...}, "context": {...} }` | `{ "success": true, "output": "...", "metadata": {...} }` |

### 4.2 선택 함수 (이벤트 훅)

| Export 함수 | 목적 | 입력 |
|-------------|------|------|
| `on_load` | 로드 완료 알림 | `{ "cwd": "...", "session_id": "..." }` |
| `on_unload` | 언로드 알림 | `{}` |
| `on_event` | 제네릭 이벤트 | `{ "event": "tool_call", "data": {...} }` |

모든 함수는 **JSON in → JSON out**입니다. 함수가 없으면 스킵 (기본값 적용).

## 5. Host Functions (oxi → 확장)

oxi가 확장에게 제공하는 호스트 함수들:

| Host 함수 | 목적 |
|-----------|------|
| `oxi_read_file(path) → content` | 파일 읽기 (권한 필요) |
| `oxi_write_file(path, content)` | 파일 쓰기 (권한 필요) |
| `oxi_exec(cmd, args) → output` | 셸 실행 (권한 필요) |
| `oxi_http_request(url, method, body) → response` | HTTP 요청 (권한 필요) |
| `oxi_log(level, message)` | 로그 출력 (항상 허용) |
| `oxi_get_config(key) → value` | 확장 설정 읽기 (항상 허용) |

## 6. 확장 매니페스트

```json
// ~/.oxi/extensions/my-ext/manifest.json
{
  "name": "my-ext",
  "version": "1.0.0",
  "description": "My custom extension",
  "wasm": "my_ext.wasm",
  "permissions": ["file_read", "network"],
  "config": {
    "api_key": { "type": "string", "required": false }
  }
}
```

또는 단일 파일:
```
~/.oxi/extensions/my_ext.wasm    ← manifest 없이도 로드 가능
```

## 7. 디렉토리 구조

```
~/.oxi/
├── extensions/
│   ├── my_ext.wasm              # 직접 배치
│   ├── other-ext/
│   │   ├── manifest.json
│   │   └── other_ext.wasm
│   └── installed/
│       └── via-oxi-ext-install/
└── settings.toml                # extensions = ["my_ext", "other-ext"]
```

## 8. 확장 작성 (사용자 관점)

### Rust PDK 사용

```rust
// my-ext/Cargo.toml
[dependencies]
extism-pdk = "1.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[lib]
crate-type = ["cdylib"]
```

```rust
// my-ext/src/lib.rs
use extism_pdk::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct ToolDef {
    name: String,
    description: String,
    schema: serde_json::Value,
}

#[plugin_fn]
pub fn init() -> FnResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "name": "my_ext",
        "version": "1.0.0",
        "description": "My custom extension"
    })))
}

#[plugin_fn]
pub fn register_tools() -> FnResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "tools": [{
            "name": "my_tool",
            "description": "Does something useful",
            "schema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }
        }]
    })))
}

#[plugin_fn]
pub fn execute_tool(Json(params): Json<serde_json::Value>) -> FnResult<Json<serde_json::Value>> {
    let tool = params["tool"].as_str().unwrap_or("");
    match tool {
        "my_tool" => {
            let input = params["params"]["input"].as_str().unwrap_or("");
            Ok(Json(serde_json::json!({
                "success": true,
                "output": format!("Processed: {}", input)
            })))
        }
        _ => Ok(Json(serde_json::json!({
            "success": false,
            "output": format!("Unknown tool: {}", tool)
        })))
    }
}
```

### 빌드

```bash
cd my-ext
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/my_ext.wasm ~/.oxi/extensions/
```

## 9. oxi 쪽 구현

### Cargo.toml

```toml
[dependencies]
extism = "1.21"
```

### ExtensionManager

```rust
pub struct ExtensionManager {
    plugins: HashMap<String, extism::Plugin>,
    tool_to_plugin: HashMap<String, String>,  // tool_name → plugin_name
}

impl ExtensionManager {
    /// ~/.oxi/extensions/ 에서 .wasm 파일 발견
    pub fn discover() -> Vec<PathBuf>;

    /// .wasm → extism::Plugin 로드
    pub fn load(path: &Path) -> Result<LoadedExtension>;

    /// init() 호출 → 이름, 버전, 설명 획득
    pub fn initialize(&mut self, plugin: &mut Plugin) -> Result<ExtensionInfo>;

    /// register_tools() 호출 → 도구 스키마 획득
    pub fn get_tools(&self, plugin: &Plugin) -> Result<Vec<ToolDef>>;

    /// execute_tool() 호출 → 도구 실행
    pub fn execute_tool(&self, tool: &str, params: Value) -> Result<AgentToolResult>;
}
```

### AgentTool 래퍼

```rust
/// WASM 확장의 도구를 AgentTool로 래핑
struct WasmTool {
    manager: Arc<ExtensionManager>,
    plugin_name: String,
    tool_name: String,
    schema: Value,
}

#[async_trait]
impl AgentTool for WasmTool {
    fn name(&self) -> &str { &self.tool_name }
    fn description(&self) -> &str { /* schema에서 추출 */ }
    fn parameters_schema(&self) -> Value { self.schema.clone() }

    async fn execute(&self, _: &str, params: Value, _: Option<...>) -> Result<AgentToolResult, String> {
        // Extism은 동기 호출이므로 spawn_blocking으로 감싸기
        let mgr = self.manager.clone();
        let plugin = self.plugin_name.clone();
        let tool = self.tool_name.clone();
        tokio::task::spawn_blocking(move || {
            mgr.execute_tool(&plugin, &tool, params)
        }).await.map_err(|e| e.to_string())?
         .map_err(|e| e.to_string())
    }
}
```

## 10. 보안 모델

### 기본: 최소 권한

확장은 기본적으로:
- ❌ 파일 시스템 접근 불가
- ❌ 네트워크 접근 불가
- ❌ 셸 실행 불가
- ✅ JSON 입출력만 가능
- ✅ oxi_log()로 로그 출력

### 권한 부여

```toml
# ~/.oxi/settings.toml
[[extension_permissions]]
name = "my_ext"
permissions = ["file_read", "network"]
```

또는 manifest.json에서:
```json
{ "permissions": ["file_read", "network"] }
```

Host function이 권한 체크:
```rust
fn host_read_file(plugin: &mut Plugin, path: &str) -> Result<String> {
    check_permission(plugin, "file_read")?;
    // ... 읽기
}
```

## 11. 성능

| 항목 | 측정치 |
|------|--------|
| WASM 콜드 스타트 | ~1-5ms |
| 도구 실행 (JSON 직렬화 포함) | ~0.1-1ms |
| 메모리 오버헤드 (확장당) | ~100KB-1MB |
| 네이티브 대비 성능 | 80-95% |

에이전트 도구 호출은 초당 몇 건 안 되므로 오버헤드는 무시 가능합니다.

## 12. 마이그레이션 경로

### Phase 1: 기반 (이번 구현)
- `extism` 의존성 추가
- `ExtensionManager` 구현 (discover, load, init, register_tools, execute_tool)
- `WasmTool` AgentTool 래퍼
- `~/.oxi/extensions/*.wasm` 자동 발견
- `/extensions` 슬래시 명령 (목록, 로드, 언로드)

### Phase 2: Host Functions
- `oxi_read_file`, `oxi_write_file` 등 호스트 함수 제공
- 권한 시스템 구현

### Phase 3: 생태계
- `oxi ext init` — 확장 프로젝트 스캐폴딩
- `oxi ext build` — WASM 빌드
- `oxi ext install <url>` — 원격 확장 설치
- 확장 레지스트리 (crates.io 유사)

### Phase 4: 다언어 지원
- TypeScript PDK (extism-pdk-js)
- Python PDK
- Go PDK

## 13. 기존 코드와의 관계

### 유지
- `Extension` 트레잇 → **in-process Rust 확장용** (oxi 자체 기능)
- `ExtensionRegistry` → in-process 확장 관리용
- `ExtensionRunner` → 이벤트 디스패치용

### 추가
- `WasmExtensionManager` → WASM 확장 관리 (새 모듈)
- `WasmTool` → AgentTool 트레잇 구현체
- 두 시스템은 **공존**: Rust 내장 확장 + WASM 사용자 확장

### 변경
- `loading.rs` → Extism 기반으로 교체
- `main.rs` → WASM 확장 발견 + 로드 추가
- `agent_session.rs` → WASM 도구 등록

## 14. 의존성

```toml
# oxi-cli/Cargo.toml
[dependencies]
extism = "1.21"          # Host SDK
# extism-pdk은 확장 작성자만 필요 — oxi 자체는 불필요
# wasmtime은 extism이 내부적으로 관리
```

추가 크기: ~2-3MB (wasmtime 런타임 포함)

## 15. 리스크

| 리스크 | 완화 |
|--------|------|
| wasmtime 바이너리 크기 | release 빌드에서 ~2MB, 허용 가능 |
| Extism 프로젝트 중단 | wasmtime 직접 사용으로 전환 가능 (Extism은 얇은 래퍼) |
| WASM 디버깅 어려움 | extism-pdk의 log 함수 + oxi 로그 통합 |
| 비동기 지원 | spawn_blocking으로 감싸기, 확장 내부는 동기 |
