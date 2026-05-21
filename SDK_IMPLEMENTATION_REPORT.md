# oxi SDK — 구현 완료 보고서

**날짜:** 2026-05-16  
**상태:** ✅ Phase 0, 1, 2, 3 모두 완료

---

## 구현된 크레이트 구조

```
oxi/
├── oxi-ai/        ← ProviderRegistry, ModelRegistry (인스턴스화 가능)
├── oxi-agent/     ← AgentConfig.workspace_dir, AgentLoopConfig.workspace_dir
├── oxi-store/     ← CLI 전용 (변경 없음)
├── oxi-sdk/       ← [NEW] OxiBuilder, AgentBuilder, tool factories
├── oxi-tui/       ← TUI 전용 (변경 없음)
└── oxi-cli/       ← oxi-sdk 의존, 기존 API 유지
```

---

## 변경 내역

### Phase 0: 인프라

| 파일 | 변경 |
|------|------|
| `oxi-agent/src/agent_loop/config.rs` | `workspace_dir: Option<PathBuf>` 필드 추가 |
| `oxi-agent/src/config.rs` | `AgentConfig`에 `workspace_dir` 필드 추가 |
| `oxi-agent/src/agent.rs` | `AgentConfig.workspace_dir` → `AgentLoopConfig.workspace_dir` 전달 |
| `oxi-agent/src/tools/path_security.rs` | `PathGuard::with_root()` 추가 |
| `oxi-agent/src/tools/{edit,find,grep,ls,read,write}.rs` | `workspace_dir` 우선 사용 |
| `oxi-ai/src/providers/mod.rs` | `ProviderRegistry` struct 추가 (인스턴스화 가능) |
| `oxi-ai/src/model_registry.rs` | `ModelRegistry` struct 추가 (인스턴스화 가능) |

### Phase 1: SDK 크레이트

| 파일 | 설명 |
|------|------|
| `oxi-sdk/Cargo.toml` | 새 크레이트 |
| `oxi-sdk/src/lib.rs` | re-exports (oxi-ai + oxi-agent) |
| `oxi-sdk/src/builder.rs` | `OxiBuilder`, `Oxi` |
| `oxi-sdk/src/agent_builder.rs` | `AgentBuilder` (fluent API) |
| `oxi-sdk/src/tool_factory.rs` | `coding_tools()`, `readonly_tools()` |
| `oxi-sdk/src/prelude.rs` | commonly used types |

### Phase 2: CLI 통합

| 파일 | 변경 |
|------|------|
| `oxi-cli/Cargo.toml` | `oxi-sdk` 의존성 추가 |
| `oxi-cli/src/lib.rs` | `AgentConfig.workspace_dir` 추가 |

### Phase 3: oxios 연동

| 파일 | 변경 |
|------|------|
| `oxios/.../agent_runtime.rs` | `WORKSPACE_MUTEX` 제거, `workspace_dir` 전달 |

---

## SDK API

### 기본 사용법

```rust
use oxi_sdk::{OxiBuilder, AgentConfig};
use std::path::PathBuf;

let oxi = OxiBuilder::new().with_builtins().build();

let agent = oxi.agent(AgentConfig {
    model_id: "zai/glm-5.1".into(),
    max_iterations: 20,
    workspace_dir: Some(PathBuf::from("/workspace/agent-1")),
    ..Default::default()
})
.system_prompt("You are a coding assistant.")
.build()?;

agent.run("Build a REST API".into(), |event| {
    if let AgentEvent::ToolExecutionEnd { tool_name, .. } = event {
        println!("Tool: {}", tool_name);
    }
}).await?;
```

### 다중 에이전트 (병렬)

```rust
let oxi = OxiBuilder::new().with_builtins().build();

// 에이전트 A — 워크스페이스 분리됨
let agent_a = oxi.agent(config_a)
    .workspace("/workspace/frontend")
    .build()?;

// 에이전트 B — 동시 실행 가능!
let agent_b = oxi.agent(config_b)
    .workspace("/workspace/backend")
    .build()?;

// 병렬 실행 — CWD 충돌 없음
let (r_a, r_b) = tokio::join!(
    agent_a.run("Build React app".into(), |_| {}),
    agent_b.run("Build Rust API".into(), |_| {}),
);
```

### 커스텀 도구

```rust
let agent = oxi.agent(config)
    .workspace("/workspace")
    .tool(my_memory_tool)
    .tool(my_exec_tool)
    .build()?;
```

### 테스트 (격리)

```rust
#[test]
fn test_with_mock() {
    let oxi = OxiBuilder::new()
        .provider("mock", MockProvider::new())
        .build();
    // 완전 격리 — 전역 상태 오염 없음
}
```

---

## 성능 지표

| 지표 | 값 |
|------|-----|
| 컴파일 에러 | 0 |
| 컴파일러 경고 | 0 |
| 테스트 통과 | 1,209 |
| 테스트 실패 | 0 |
| 릴리즈 빌드 시간 | ~21초 |
| oxios-kernel 에러 | 0 |

---

## oxios에 미치는 영향

### Before (WORKSPACE_MUTEX)
```rust
// 에이전트를 한 번에 1개만 실행 가능
static WORKSPACE_MUTEX: Mutex<()> = ...;
let _guard = WORKSPACE_MUTEX.lock();  // 전체 직렬화
std::env::set_current_dir(&workspace);  // 프로세스 전역 CWD 변경
```

### After (workspace_dir)
```rust
// AgentLoopConfig에 workspace_dir 전달
let loop_config = AgentLoopConfig {
    workspace_dir: config.project_paths.first().cloned(),
    ...
};
// 병렬 에이전트 실행 가능!
```

---

*이 보고서는 oxi v0.12.0 SDK 구현 완료를 기록합니다.*
