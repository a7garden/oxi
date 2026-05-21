# oxi SDK 설계 리뷰 — 정직한 진단

**날짜:** 2026-05-16  
**관점:** TUI 코딩 에이전트 + AI OS SDK 양쪽 모두

---

## 크레이트 구조

```
의존성 그래프 (실제):

oxi-ai ──────────── (최하위, 의존 없음)
  ├── oxi-agent ─── oxi-ai만 의존
  ├── oxi-store ─── oxi-ai만 의존
  └── oxi-sdk ───── oxi-ai + oxi-agent

oxi-tui ─────────── (독립, oxi 크레이트 의존 없음)
  └── oxi-cli ───── oxi-ai + oxi-agent + oxi-store + oxi-tui + oxi-sdk
```

### ✅ 잘된 점

**1. 계층 분리가 명확함**

oxi-ai → oxi-agent → oxi-sdk의 계층이 깔끔합니다.
oxios는 oxi-ai + oxi-agent만 가져가고, oxi-cli는 전부 가져갑니다.

**2. oxi-tui가 완전히 분리됨**

TUI 렌더링(ratatui 등)이 다른 크레이트에 영향을 주지 않습니다.
oxios 같은 SDK 사용자는 TUI 코드를 한 줄도 가져오지 않습니다.

**3. oxi-store가 oxi-ai만 의존**

파일 기반 세션/설정/auth 관리가 agent나 tui에 결합되지 않았습니다.

### 🔴 문제

**4. ModelRegistry가 두 곳에 존재함**

```
oxi-ai/src/model_registry.rs   (908줄) — 정적 모델 DB + 인스턴스 API
oxi-store/src/model_registry.rs (1785줄) — AuthStorage 통합, 모델 해석, 동적 로딩
```

**두 레지스트리의 책임이 다릅니다:**
- oxi-ai: `Model` 데이터 조회 (provider, base_url, api 타입)
- oxi-store: `Model` + API 키 해석 (auth_storage 통합, models.json 파일 파싱)

**oxios는 oxi-store를 사용하지 않으므로** oxi-store의 ModelRegistry에 있는
auth 통합, 동적 모델 로딩, models.json 파싱 기능을 사용할 수 없습니다.
→ oxi-sdk에서 비슷한 걸 다시 만들어야 함 (중복).

**5. oxi-cli가 oxi-sdk를 의존하지만 사용하지 않음**

```toml
# oxi-cli/Cargo.toml
oxi-sdk = { version = "0.12.0", path = "../oxi-sdk" }
```

```bash
$ grep -rn "oxi_sdk\|use oxi_sdk" oxi-cli/src/
# 결과 없음 — 아무 데서도 import 안 함
```

**불필요한 컴파일 의존성만 추가됨.** oxi-sdk는 oxi-cli 전용이 아니라
oxios 같은 외부 사용자를 위한 것입니다. oxi-cli는 계속 oxi-ai, oxi-agent를
직접 사용해야 합니다 (그리고 그렇게 하고 있습니다).

---

## workspace_dir 파이프라인 — 끊어진 연결

### 설계상 흐름
```
AgentConfig.workspace_dir
  → Agent::new()에 저장
    → Agent::run()에서 AgentLoopConfig.workspace_dir에 복사
      → AgentLoop가 도구에 전달???
```

### 🔴 실제 상태: 3번째 단계가 없음

```
AgentLoopConfig.workspace_dir  ← 값이 들어있음 ✅
         ↓
   AgentLoop.config.workspace_dir  ← 읽을 수 있음 ✅
         ↓
   execute_tool_calls(loop_ref, ...)  ← loop_ref로 접근 가능 ✅
         ↓
   tool.execute(tool_call_id, params, signal)  ← workspace_dir 전달 안 됨 ❌
```

**AgentTool::execute()의 시그니처:**
```rust
async fn execute(
    &self,
    tool_call_id: &str,
    params: Value,
    signal: Option<oneshot::Receiver<()>>,
) -> Result<AgentToolResult, ToolError>;
```

workspace_dir을 받을 파라미터가 없습니다.

**그런데... 작동은 합니다. 왜?**

각 도구가 `self.root_dir: PathBuf`를 생성자에서 받기 때문입니다.
`ToolRegistry::with_builtins_cwd(cwd)`가 cwd를 각 도구에 전달합니다.

**문제는 Agent::run()이 이 cwd를 도구에 전달하지 않는다는 것:**

```rust
// Agent::new() — 빈 ToolRegistry 생성
let tools = Arc::new(ToolRegistry::new());  // ← cwd 없음

// Agent::run() — self.tools를 AgentLoop에 넘김
let agent_loop = AgentLoop::new(
    provider,
    loop_config,       // ← workspace_dir 있음
    Arc::clone(&self.tools),  // ← 하지만 도구는 self.root_dir = current_dir()
    fresh_state,
);
```

**결과:** `workspace_dir`이 `AgentLoopConfig`에 저장되지만, 도구는 여전히
`std::env::current_dir()` (또는 `Tool::new()` 호출 시점의 cwd)를 사용합니다.

oxios에서 `WORKSPACE_MUTEX`를 제거했지만, 실제로는 도구들이 여전히
current_dir을 사용하고 있어서 병렬 실행 시 경로 충돌이 발생할 수 있습니다.

### 해결 방법

**옵션 A:** Agent::new()가 workspace_dir을 읽어서 도구에 전달
```rust
// Agent::new() 안에서
let cwd = config.workspace_dir.clone()
    .unwrap_or_else(|| std::env::current_dir().unwrap());
// tools에 cwd 전달
```
문제: Agent::new() 시점에 config.workspace_dir이 설정되지 않았을 수 있음.
AgentBuilder는 new() 후에 .workspace()를 설정.

**옵션 B:** Agent::run()에서 workspace_dir을 읽어서 도구 재구성
```rust
// Agent::run() 안에서
let tools = if let Some(ref dir) = workspace_dir {
    let registry = ToolRegistry::with_builtins_cwd(dir.clone(), &[]);
    // 커스텀 도구도 다시 등록... 복잡함
    Arc::new(registry)
} else {
    Arc::clone(&self.tools)
};
```

**옵션 C (pi 방식):** ToolContext를 execute에 전달
```rust
async fn execute(
    &self,
    tool_call_id: &str,
    params: Value,
    signal: Option<oneshot::Receiver<()>>,
    ctx: &ToolContext,  // ← 추가
) -> Result<AgentToolResult, ToolError>;

pub struct ToolContext {
    pub workspace: PathBuf,
    pub session_id: Option<String>,
}
```
→ 가장 깔끔하지만 AgentTool 트레이트 breaking change.

**옵션 D (최소 변경):** Agent::run()에서 workspace_dir과 함께
ToolRegistry를 새로 만들어서 AgentLoop에 전달
```rust
// Agent::run()에서 self.tools 대신 새 registry 사용
let effective_tools = match &workspace_dir {
    Some(dir) => {
        let reg = ToolRegistry::with_builtins_cwd(dir.clone(), &[]);
        // self.tools의 커스텀 도구도 복사
        for name in self.tools.names() {
            if let Some(tool) = self.tools.get(&name) {
                reg.register_arc(tool);
            }
        }
        Arc::new(reg)
    }
    None => Arc::clone(&self.tools),
};
// AgentLoop::new(... effective_tools ...)
```

→ 옵션 D가 가장 현실적. 도구 자체의 root_dir은 커스텀 도구에만 해당되고,
빌트인 도구는 새 ToolRegistry에서 workspace_dir을 받음.

---

## TUI 코딩 에이전트로서의 평가

### ✅ 잘됨
- oxi-tui가 완전히 분리되어 외부 사용자에게 영향 없음
- CLI는 print mode, interactive mode 모두 작동
- 세션 관리, auth, 설정이 oxi-store에 잘 격리됨
- 도구 확장(AgentTool 트레이트)이 유연함

### 🟡 개선 필요
- oxi-cli가 oxi-sdk에 의존하지만 사용 안 함 → 의존성 제거
- main.rs가 길고(app + subcommand + pkg + config + models)
  → app 초기화를 별도 모듈로 분리 가능

---

## AI OS SDK로서의 평가

### ✅ 잘됨
- oxi-sdk가 oxi-ai + oxi-agent만 의존 (가벼움)
- OxiBuilder로 격리된 인스턴스 생성 가능
- AgentBuilder의 fluent API가 직관적
- coding_tools(cwd)로 워크스페이스별 도구 세트 생성 가능
- oxios는 oxi-store, oxi-tui, oxi-cli를 가져오지 않음

### 🔴 문제

**1. workspace_dir이 실제로 도구에 도달하지 않음** (위에서 설명)

**2. oxi-store의 ModelRegistry + AuthStorage 통합을 SDK에서 사용 불가**
- oxi-store는 CLI 전용(파일 시스템 결합)이지만,
- 모델 해석 + API 키 해석은 SDK 사용자에게도 필요
- oxios는 이걸 engine.rs에 직접 구현해놓음 (중복)

**3. Provider 생성이 여전히 글로벌 get_provider()에 의존**
```rust
// oxi-sdk/src/builder.rs
pub fn create_provider(&self, name: &str) -> Result<Box<dyn Provider>> {
    // 1. Instance registry (custom providers) — 없음
    // 2. Built-in providers from oxi-ai
    oxi_ai::get_provider(name)  // ← 글로벌 함수
}
```
Oxi struct가 ProviderRegistry를 들고 있지 않습니다.
(Provider가 stateless이므로 인스턴스화 의미가 적긴 하지만,
테스트 격리를 위해서는 필요합니다.)

**4. oxios가 oxi-sdk를 사용하지 않음**
```
oxios-kernel ← oxi-ai + oxi-agent (직접)
             ← oxi-sdk (사용 안 함)
```
oxios는 engine.rs에서 자체적으로 Provider/Model 추상화를 만들었습니다.
oxi-sdk가 있어도 oxios가 이점을 못 느끼는 구조.

---

## 권장 사항

### P0: workspace_dir을 Agent::run()에서 도구에 실제로 전달

옵션 D 적용 — Agent::run()에서 workspace_dir과 함께
ToolRegistry를 새로 만들어 AgentLoop에 전달.

```rust
// Agent::run() 수정
let effective_tools = match &workspace_dir {
    Some(dir) => {
        let reg = ToolRegistry::with_builtins_cwd(dir.clone(), &[]);
        for name in self.tools.names() {
            if let Some(tool) = self.tools.get(&name) {
                reg.register_arc(tool);
            }
        }
        Arc::new(reg)
    }
    None => Arc::clone(&self.tools),
};
```

### P1: oxi-cli에서 oxi-sdk 의존성 제거

oxi-cli는 직접 oxi-ai, oxi-agent를 사용. oxi-sdk는 외부 사용자용.

### P1: Oxi에 ProviderRegistry 추가

```rust
pub struct Oxi {
    providers: Arc<ProviderRegistry>,  // ← 추가
    models: Arc<ModelRegistry>,
}
```

테스트에서 MockProvider를 주입하려면 인스턴스 레지스트리가 필요.

### P2: oxios가 oxi-sdk를 사용하도록 마이그레이션

engine.rs를 OxiBuilder로 대체. AgentRuntimeConfig를 AgentConfig로 통합.

---

## 최종 평가

| 관점 | 점수 | 비고 |
|------|------|------|
| **크레이트 구조** | A | 계층 분리 우수, 의존성 방향 올바름 |
| **TUI 에이전트** | A | CLI로서 완전하게 작동 |
| **SDK API 설계** | B | OxiBuilder/AgentBuilder 설계 좋음 |
| **SDK 실제 동작** | C+ | workspace_dir이 도구에 도달하지 않음 |
| **oxios 연동** | C | WORKSPACE_MUTEX는 제거했지만 실제 효과 없음 |
| **테스트 격리** | B- | ModelRegistry는 격리, Provider는 아직 글로벌 |

**핵심 한마디:**
뼈대와 API는 잘 설계되었지만, workspace_dir → 도구 전달이
끊어져 있어서 "병렬 에이전트"라는 SDK의 핵심 가치가 아직 실현되지 않았습니다.
