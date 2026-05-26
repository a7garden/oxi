# 02. 보안/샌드박스 (Capability-Based Security)

모듈 경로: `oxi-sdk/src/security/`

---

## 2.1 설계 원칙

| 원칙 | 의미 |
|------|------|
| **Deny-by-default** | 명시적으로 허용된 것만 실행. 빈 CapabilitySet = 모든 툴 차단 |
| **Fine-grained** | 툴 + 파라미터 수준 제어. `FileRead { "/workspace/**" }` |
| **Auditable** | 모든 권한 체크는 AuditLog에 기록 |
| **Composable** | CapabilitySet preset으로 일반적 패턴 제공 |
| **Hierarchical** | 역할(role) 기반으로 그룹에 권한 부여, 에이전트는 역할 상속 |

---

## 2.2 Capability 타입 시스템

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Capability {
    // ── File system ──
    FileRead   { path_pattern: String },
    FileWrite  { path_pattern: String },
    FileEdit   { path_pattern: String },
    FileList   { path_pattern: String },
    FileFind   { path_pattern: String },

    // ── Execution ──
    Bash {
        allowed_commands: Vec<StringPattern>,
        timeout_secs: Option<u64>,
    },

    // ── Network ──
    Network   { allowed_domains: Vec<String> },
    WebBrowse { allowed_domains: Vec<String> },

    // ── Agent ──
    Subagent { max_children: Option<usize> },
    BusRead  { channel: Option<String> },
    BusWrite { channel: Option<String> },

    // ── Environment ──
    EnvRead { allowed_vars: Vec<String> },

    // ── Meta ──
    ToolUse   { tool_name: String },
    McpAccess { resource_patterns: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StringPattern {
    Literal(String),
    Wildcard,
}
```

---

## 2.3 CapabilitySet Presets

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySet {
    capabilities: Vec<Capability>,
    expires_at_ms: Option<u64>,
}

impl CapabilitySet {
    // ── Presets ──

    /// 모든 권한 (시스템 에이전트용)
    pub fn all() -> Self { ... }

    /// 읽기 전용 (리서치 에이전트)
    pub fn read_only(workspace: &str) -> Self { ... }

    /// 코딩 에이전트 표준 권한
    pub fn coding(workspace: &str) -> Self {
        Self {
            capabilities: vec![
                FileRead/Write/Edit/List/Find for workspace,
                Bash { [git, cargo, npm, node, python3, ls, cat, grep, rg], timeout: 30s },
                Subagent { max_children: 2 },
                BusRead { all },
            ],
            ...
        }
    }

    /// 리서치 에이전트 (읽기 + 웹 브라우징, 쓰기 금지)
    pub fn research(workspace: &str) -> Self { ... }

    /// 브라우징 전용 (스크래핑 에이전트)
    pub fn browser(workspace: &str) -> Self {
        Self {
            capabilities: vec![
                FileRead { workspace },
                FileWrite { workspace + "/output/**" },  // 출력만
                WebBrowse { ["*"] },
                Network { ["*"] },
            ],
            ...
        }
    }

    // ── Builder ──
    pub fn add(&mut self, cap: Capability) -> &mut Self;
    pub fn with_ttl(mut self, duration: Duration) -> Self;
    pub fn is_expired(&self) -> bool;
    pub fn capabilities(&self) -> &[Capability];
}
```

---

## 2.4 Authorizer

```rust
pub struct Authorizer {
    /// Subject → granted capabilities
    grants: Arc<RwLock<HashMap<CapabilitySubject, CapabilitySet>>>,
    /// Role → capabilities (그룹 권한)
    roles: Arc<RwLock<HashMap<String, CapabilitySet>>>,
    /// Agent → roles 매핑
    role_bindings: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// 기본 정책
    default_policy: DefaultPolicy,
    /// 감사 로그
    audit: Arc<AuditLog>,
}

pub enum DefaultPolicy {
    DenyAll,    // 알 수 없는 subject는 모두 거부
    AllowAll,   // 하위 호환 (기존 코드)
}

pub enum CapabilitySubject {
    Agent(String),
    Tool(String),
    Group(String),
}
```

### 핵심 메서드

```rust
impl Authorizer {
    pub fn new(audit: Arc<AuditLog>) -> Self;
    pub fn new_permissive(audit: Arc<AuditLog>) -> Self;

    // ── 직접 권한 부여 ──
    pub fn grant(&self, subject: CapabilitySubject, caps: CapabilitySet);
    pub fn grant_one(&self, subject: CapabilitySubject, cap: Capability);
    pub fn revoke(&self, subject: &CapabilitySubject);

    // ── 역할 기반 권한 ──
    pub fn define_role(&self, role_name: &str, caps: CapabilitySet);
    pub fn bind_role(&self, agent_id: &str, role_name: &str);
    pub fn unbind_role(&self, agent_id: &str, role_name: &str);

    // ── 체크 ──
    pub fn check(&self, subject: &CapabilitySubject, required: &Capability) -> bool;
    pub fn require(&self, subject: &CapabilitySubject, required: &Capability) -> Result<(), SdkError>;
}
```

### 권한 체크 흐름

```rust
fn evaluate(&self, subject: &CapabilitySubject, required: &Capability) -> bool {
    let grants = self.grants.read();

    // 1. 직접 부여된 권한
    if let Some(set) = grants.get(subject) {
        if !set.is_expired() && self.satisfies_any(&set.capabilities, required) {
            return true;
        }
    }

    // 2. 역할을 통해 상속된 권한
    if let CapabilitySubject::Agent(id) = subject {
        let bindings = self.role_bindings.read();
        if let Some(roles) = bindings.get(id) {
            let role_defs = self.roles.read();
            for role_name in roles {
                if let Some(role_caps) = role_defs.get(role_name) {
                    if self.satisfies_any(&role_caps.capabilities, required) {
                        return true;
                    }
                }
            }
        }
    }

    // 3. 기본 정책
    matches!(self.default_policy, DefaultPolicy::AllowAll)
}
```

### 역할 기반 예시

```rust
let authorizer = Authorizer::new(audit.clone());

// 역할 정의
authorizer.define_role("coder", CapabilitySet::coding("/workspace"));
authorizer.define_role("reviewer", CapabilitySet::read_only("/workspace"));
authorizer.define_role("browser", CapabilitySet::browser("/workspace"));

// 에이전트에 역할 바인딩
authorizer.bind_role("agent-001", "coder");
authorizer.bind_role("agent-002", "reviewer");
authorizer.bind_role("agent-003", "browser");

// 다중 역할도 가능
authorizer.bind_role("agent-001", "reviewer");  // coder + reviewer
```

---

## 2.5 SecurityMiddleware

툴 실행 전 authorization check를 수행. **MiddlewarePipeline → AgentHooks bridge**의 일부로 작동 (§5.5 참조).

```rust
pub struct SecurityMiddleware {
    authorizer: Arc<Authorizer>,
}

impl SecurityMiddleware {
    pub fn new(authorizer: Arc<Authorizer>) -> Self;

    /// 툴 이름 + 파라미터에서 필요한 Capability 추론
    pub fn required_capability(tool_name: &str, params: &serde_json::Value) -> Option<Capability>;

    /// Middleware trait 구현 (§5.5 참조)
    /// BeforeTool 단계에서 권한 체크
}
```

Capability 추론 매핑:

| 툴 | 추론된 Capability |
|------|-------------|
| `read` | `FileRead { params["path"] }` |
| `write` | `FileWrite { params["path"] }` |
| `edit` | `FileEdit { params["path"] }` |
| `ls` | `FileList { params["path"] }` |
| `bash` | `Bash { params["command"]의 첫 단어 }` |
| `browse` | `WebBrowse { params["url"]의 도메인 }` |
| `subagent` | `Subagent { }` |

---

## 2.6 아키텍처

```
┌─────────────────────────────────────────────┐
│  Authorizer                                 │
│                                             │
│  grants:  Agent → CapabilitySet (직접)      │
│  roles:   RoleName → CapabilitySet (역할)    │
│  bindings: Agent → [RoleName]               │
│                                             │
│  check(subject, required) ──┐               │
│    1. 직접 권한 검색         │               │
│    2. 역할 상속 검색         │               │
│    3. 기본 정책              │               │
│                              ▼               │
│  AuditLog ◀── SecurityDecision             │
└─────────────────────────────────────────────┘
         │
         │ MiddlewarePipeline의 BeforeTool 단계
         ▼
┌─────────────────────────────────────────────┐
│  SecurityMiddleware                         │
│                                             │
│  Tool Call ──▶ required_capability()        │
│                  │                          │
│         granted? ──YES──▶ execute tool      │
│                  ──NO───▶ block + error     │
└─────────────────────────────────────────────┘
```

---

## 2.7 사용 예시

```rust
use oxi_sdk::security::*;

let audit = Arc::new(AuditLog::new(1024));
let authorizer = Arc::new(Authorizer::new(audit.clone()));

// 역할 정의
authorizer.define_role("coder", CapabilitySet::coding("/workspace"));
authorizer.define_role("reviewer", CapabilitySet::read_only("/workspace"));

// 에이전트에 역할 바인딩
authorizer.bind_role("coder-001", "coder");
authorizer.bind_role("reader-001", "reviewer");

// AgentBuilder에 통합 (§5.5 bridge가 자동 처리)
let agent = oxi.agent(config)
    .workspace("/workspace")
    .coding_tools()
    .authorizer(authorizer.clone())
    .capabilities(CapabilitySet::coding("/workspace"))
    .build()?;

// coder-001은 파일 수정 가능, reader-001은 읽기만
```
