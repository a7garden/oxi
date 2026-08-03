# RFC-004: Extension & Skills 시스템 고도화 — 플러그인 아키텍처, SKILL.md, 패키지 매니저

**상태**: 초안  
**우선순위**: P2 — 확장성 생태계의 기반  
**현재 완성도**: ~80%  
**목표**: 기능 동등성 100% + pi 대비 우위 강화  

---

## 1. 문제 정의

### 1.1 현재 구현 현황 (정확한 팩트 체크)

oxicode의 Extension & Skills 시스템은 이미 상당히 성숙합니다. 아래는 코드베이스 기반 정확한 현황입니다.

#### Extension 트레이트 (`oxicode-cli/src/extensions/mod.rs`)

현재 Extension 트레이트는 **34개 메서드**를 포함합니다:

| 카테고리 | 메서드 | 상태 |
|----------|--------|------|
| 메타데이터 | `name()`, `description()`, `manifest()` | ✅ 구현 완료 |
| 등록 | `register_tools()`, `register_commands()` | ✅ 구현 완료 |
| 라이프사이클 | `on_load()`, `on_unload()` | ✅ 구현 완료 |
| 메시지 | `on_message_sent()`, `on_message_received()` | ✅ 구현 완료 |
| 툴 | `on_tool_call()`, `on_tool_result()`, `on_before_tool_call()`, `on_after_tool_call()` | ✅ 구현 완료 |
| 세션 | `on_session_start()`, `on_session_end()`, `session_before_switch()`, `session_before_fork()`, `session_before_compact()`, `session_compact()`, `session_shutdown()`, `session_before_tree()`, `session_tree()` | ✅ 구현 완료 |
| 컴팩션 | `on_before_compaction()`, `on_after_compaction()` | ✅ 구현 완료 |
| 프로바이더 | `before_provider_request()`, `after_provider_response()` | ✅ 구현 완료 |
| 모델 | `model_select()`, `thinking_level_select()` | ✅ 구현 완료 |
| 컨텍스트 | `context()` (메시지 수정) | ✅ 구현 완료 |
| 설정 | `on_settings_changed()` | ✅ 구현 완료 |
| 범용 | `on_event()` | ✅ 구현 완료 |
| 에러 | `on_error()` | ✅ 구현 완료 |
| 실행 | `bash()`, `input()` | ✅ 구현 완료 |

#### 이벤트 타입 (`oxicode-cli/src/extensions/types.rs`)

**14개 이벤트 구조체**가 이미 정의되어 있습니다:

| 이벤트 | 목적 |
|--------|------|
| `SessionBeforeSwitchEvent` | 세션 전환 전 (취소 가능) |
| `SessionBeforeForkEvent` | 세션 포크 전 (취소 가능) |
| `SessionBeforeCompactEvent` | 컴팩션 전 (취소 가능) |
| `SessionCompactEvent` | 컴팩션 완료 후 |
| `SessionShutdownEvent` | 세션 종료 |
| `SessionBeforeTreeEvent` | 트리 내비게이션 전 (취소 가능) |
| `SessionTreeEvent` | 트리 내비게이션 후 |
| `ContextEvent` | 컨텍스트 메시지 수정 |
| `BeforeProviderRequestEvent` | 프로바이더 요청 전 (페이로드 수정 가능) |
| `AfterProviderResponseEvent` | 프로바이더 응답 후 |
| `ModelSelectEvent` | 모델 선택/변경 |
| `ThinkingLevelSelectEvent` | 사고 수준 선택 |
| `BashEvent` | Bash 명령 실행 |
| `InputEvent` | 사용자 입력 (변환/핸들 가능) |

#### ExtensionRegistry (`oxicode-cli/src/extensions/registry.rs`)

이미 **panic-safe 이벤트 디스패치**가 구현되어 있습니다:
- `ExtensionRegistry`: 등록/해제/활성화/비활성화 관리
- `ExtensionRunner`: 라이프사이클 오케스트레이션, 실행 순서 관리
- `call_hook_safe()`: `catch_unwind` 기반 panic-safe 훅 호출
- 이벤트 Emit 결과 타입: `ToolCallEmitResult`, `ToolResultEmitResult`, `ContextEmitResult`, `ProviderRequestEmitResult`, `SessionBeforeEmitResult`
- 에러 리스너 시스템 (`ExtensionErrorListener`)

#### ExtensionContext (`oxicode-cli/src/extensions/context.rs`)

이미 확장에 다양한 제어 표면을 제공합니다:
- `register_tool()` — 툴 등록
- `send_message()` — 메시지 전송
- `set_model()` — 모델 변경
- `set_thinking_level()` — 사고 수준 변경
- `append_system_prompt()` — 시스템 프롬프트 추가
- `set_session_name()` — 세션 이름 설정
- `fork_session()` — 세션 포크
- `get_session_entries()` — 세션 엔트리 조회
- `get_tools()` / `set_tools()` — 툴 조회/교체
- `read_file()` — 파일 읽기
- `config_get()` — 설정 조회

#### WASM 샌드박스 (`oxicode-cli/src/extensions/wasm.rs`, `wasm_hooks.rs`, `wasm_tool.rs`)

이미 **완전한 WASM 샌드박스**가 구현되어 있습니다:
- Extism 기반 `.wasm` 확장 로딩
- `init()`, `register_tools()`, `register_commands()`, `execute_tool()` 프로토콜
- 호스트 함수: `oxicode_http_request` (네트워크 접근)
- `WasmExtensionManager`로 전체 라이프사이클 관리
- JSON-in/JSON-out 프로토콜

#### Skills (`oxicode-cli/src/skills/mod.rs`)

`SkillManager`가 이미 구현되어 있습니다:
- `load_from_dir()` — 디렉토리 스캔 (`<name>/SKILL.md` 구조)
- `get()` — 이름으로 조회 (대소문자 무시)
- `all()` — 전체 목록 (정렬)
- `search()` — 이름/설명/내용 검색
- `skills_dir()` — 기본 경로 (`~/.oxicode/skills/`)

#### 패키지 매니저 (`oxicode-cli/src/storage/packages.rs`)

**매우 정교한** 패키지 시스템이 이미 구현되어 있습니다:
- **5가지 소스 타입**: npm, git, GitHub shorthand, URL archive, local
- **Lockfile** (`oxicode-lock.json`): 정확한 버전/ref 기록
- **매니페스트** (`oxicode-package.toml`): 확장, 스킬, 프롬프트, 테마 리소스 선언
- **자동 리소스 발견**: 매니페스트 없이도 `.so`/`.dylib`/`.dll`, `SKILL.md`, `.md`, `.json` 스캔
- **의존성 해결**: `resolve_dependencies()`
- **업데이트 확인**: `check_for_updates()` — npm + git 모두 지원
  - npm: `NpmPackageInfo::fetch()`로 최신 버전 조회
  - git: `git_has_update()`로 원격 변경 감지
- **설치/업데이트/제거**: `install()`, `update()`, `update_all()`, `remove()`
- **의존성 그래프**: `resolve_dependencies()`
- **해시 검증**: SHA256 체크섬

### 1.2 Extension 시스템 비교 (pi vs oxicode — 수정됨)

| 기능 | pi | oxicode | 격차 |
|------|----|-----|------|
| 로딩 방식 | jiti (TS 직접 로드) | 네이티브 .dylib/.so/.dll + WASM (Extism) | 다름 (oxicode가 더 다양) |
| 샌드박스 | ❌ (Node.js 전체 권한) | ✅ WASM (Extism) | **oxicode 우위** |
| 이벤트 훅 | 30+ | 34개 트레이트 메서드 + 14개 이벤트 타입 | ⚠️ 약간 부족 (pi의 세분화된 메시지/에이전트 훅 누락) |
| 커스텀 Provider | ✅ registerProvider() | ✅ register_provider() | 동등 |
| UI 컨텍스트 | select, confirm, input, editor, notify, widget, status | ⚠️ RPC 브릿지는 있으나 TUI 직접 접근 부족 | ⚠️ 부족 |
| 툴 등록 | ToolDefinition with render functions | Arc\<dyn AgentTool\> | ⚠️ render 누락 |
| stale 감지 | ✅ 세션 전환 시 자동 무효화 | ❌ ExtensionContext에 수명 관리 없음 | ❌ 누락 |
| 단축키 등록 | ✅ extension shortcuts | ❌ | ❌ 누락 |
| 명령어 등록 | ✅ slash commands | ✅ `register_commands()` + `Command` 타입 | 동등 |
| panic-safe 디스패치 | ❌ (Node.js 예외 전파) | ✅ `catch_unwind` 기반 | **oxicode 우위** |
| 에러 리스너 | ❌ | ✅ `ExtensionErrorListener` + `ExtensionErrorRecord` | **oxicode 우위** |
| 컨텍스트 수정 | ✅ | ✅ `context()` + `ContextEmitResult` | 동등 |
| 프로바이더 훅 | ✅ | ✅ `before_provider_request()` + `after_provider_response()` | 동등 |

### 1.3 Skills 비교 (수정됨)

| 기능 | pi | oxicode | 격차 |
|------|----|-----|------|
| SKILL.md 포맷 | YAML frontmatter + body | 단순 마크다운 (frontmatter 건너뛰기만 구현) | ⚠️ frontmatter 파싱 필요 |
| disable-model-invocation | ✅ | ❌ | ⚠️ |
| 조상 디렉토리 검색 | ✅ (.agents/skills/) | ❌ (단일 디렉토리만) | ⚠️ |
| 충돌 감지 | ✅ canonicalize + 중복 보고 | ❌ | ⚠️ |
| 이름 검증 | a-z, 0-9, -, max 64 | 기본 (디렉토리명 그대로 사용) | ⚠️ |
| 검색 기능 | 기본 | ✅ 이름/설명/내용 검색 | **oxicode 우위** |
| 발견 규칙 | 글로벌 + 프로젝트 + 조상 | 글로벌 또는 프로젝트 (단일 dir) | ⚠️ |

### 1.4 패키지 매니저 비교 (수정됨)

| 기능 | pi | oxicode | 격차 |
|------|----|-----|------|
| 소스 타입 | npm, git, local | npm, git, GitHub, URL archive, local | **oxicode 우위** (5 vs 3) |
| 매니페스트 | package.json (pi.extensions) | oxicode-package.toml | 다름 (동등) |
| 업데이트 확인 | 4병렬 npm + 4병렬 git | npm + git 직렬 | ⚠️ 병렬화 필요 |
| 리소스 필터링 | !제외, +강제포함 | ❌ | ⚠️ |
| 우선순위 랭킹 | 7단계 (project > user > package) | 2단계 (user, project) | ⚠️ |
| Lockfile | ❌ | ✅ oxicode-lock.json (SHA256 검증) | **oxicode 우위** |
| 자동 리소스 발견 | 기본 | ✅ `.so`/`.dylib`/`.dll`, `SKILL.md`, `.md`, `.json` | **oxicode 우위** |
| 의존성 해결 | 기본 | ✅ `resolve_dependencies()` | 동등 |

### 1.5 실제 갭 요약

기존 구현 대비 **실제로 누락된 기능**만 정리:

1. **Extension UI 컨텍스트**: `select()`, `confirm()`, `input()`, `editor()`, `notify()`, `setWidget()`, `setStatus()` — TUI 직접 접근 인터페이스
2. **단축키 등록**: `register_shortcuts()` — Extension 트레이트에 `ExtensionShortcut` 반환 메서드 필요
3. **Stale 감지**: 세션 전환 시 확장 컨텍스트 자동 무효화 (`ExtensionContextGuard`)
4. **Skills YAML frontmatter**: `disable-model-invocation`, `name`, `description` 필드 파싱
5. **Skills 조상 검색**: `.agents/skills/` 조상 디렉토리 + 설정 경로 통합 발견
6. **Skills 이름 검증**: `a-z`, `0-9`, `-`, max 64 + 중복 감지 (canonicalize)
7. **패키지 업데이트 병렬화**: npm/git 업데이트 확인을 tokio join_all로 병렬화
8. **패키지 리소스 필터링**: `!exclude`, `+include` 패턴 시스템
9. **패키지 우선순위 랭킹**: 7단계 리소스 우선순위 (project-local > project-auto > user-local > ...)

---

## 2. 설계 원칙

1. **기존 구현 위에 증강**: ExtensionRegistry, ExtensionRunner, ExtensionContext는 이미 잘 설계되어 있음. 새로운 "이벤트 버스"를 만들지 않고 기존 시스템을 확장.
2. **Rust의 타입 시스템 활용**: pi는 TypeScript duck typing + TypeBox로 런타임 검증하지만, oxicode는 컴파일 타임에 안전한 트레이트 기반 설계.
3. **WASM 샌드박스 강화**: pi보다 안전한 확장 실행 환경 유지.
4. **점진적 개선**: panic-safe 디스패치, 에러 리스너 등 oxicode 우위 기능은 유지하면서, 누락 기능만 채움.

---

## 3. 아키텍처

### 3.1 Extension 트레이트 확장

현재 Extension 트레이트(34개 메서드)에 **1개 메서드**만 추가:

```rust
/// oxicode-cli/src/extensions/mod.rs 확장 (기존 34개 메서드에 추가)

pub trait Extension: Send + Sync {
    // ── 기존 34개 메서드 (변경 없음) ──
    // name, description, manifest, register_tools, register_commands,
    // on_load, on_unload, on_message_sent, on_message_received,
    // on_tool_call, on_tool_result, on_session_start, on_session_end,
    // on_settings_changed, on_event, on_before_tool_call, on_after_tool_call,
    // on_before_compaction, on_after_compaction, on_error,
    // session_before_switch, session_before_fork, session_before_compact,
    // session_compact, session_shutdown, session_before_tree, session_tree,
    // context, before_provider_request, after_provider_response,
    // model_select, thinking_level_select, bash, input

    // ── 새로 추가 ──

    /// 단축키 등록 (pi의 extension shortcuts 이식)
    fn register_shortcuts(&self) -> Vec<ExtensionShortcut> {
        vec![]
    }
}

/// 단축키 정의
#[derive(Debug, Clone)]
pub struct ExtensionShortcut {
    /// 단축키 (예: "ctrl+shift+x")
    pub key: String,
    /// 단축키 설명
    pub description: String,
    /// 단축키 액션 식별자
    pub action: String,
}
```

> **참고**: 기존 Extension 트레이트는 이미 세션 라이프사이클(`session_before_switch`, `session_before_fork`, `session_before_compact`, `session_compact`, `session_shutdown`, `session_before_tree`, `session_tree`), 프로바이더 훅(`before_provider_request`, `after_provider_response`), 컨텍스트 수정(`context`), 모델/사고수준 선택(`model_select`, `thinking_level_select`), 실행(`bash`, `input`)을 모두 포함합니다. RFC 초안에서 제안했던 대부분의 "새" 훅은 이미 존재합니다.

### 3.2 ExtensionContext UI 확장

기존 `ExtensionContext`(`context.rs`)에 UI 접근 인터페이스를 추가합니다. 기존의 `set_model()`, `fork_session()`, `register_tool()` 등은 그대로 유지.

```rust
/// oxicode-cli/src/extensions/context.rs 확장

/// 확장의 UI 접근 인터페이스 (pi의 ExtensionUIContext 이식)
pub struct ExtensionUI {
    inner: ExtensionUIInner,
}

enum ExtensionUIInner {
    Tui(Arc<RefCell<TuiContext>>),
    Rpc(Arc<Mutex<RpcUiBridge>>),
}

impl ExtensionUI {
    /// 선택 다이얼로그 (pi의 select())
    pub async fn select(&self, prompt: &str, options: &[SelectOption]) -> Result<Option<usize>>;
    /// 확인 다이얼로그 (pi의 confirm())
    pub async fn confirm(&self, prompt: &str) -> Result<bool>;
    /// 텍스트 입력 (pi의 input())
    pub async fn input(&self, prompt: &str, default: &str) -> Result<Option<String>>;
    /// 멀티라인 에디터 (pi의 editor())
    pub async fn editor(&self, prompt: &str, content: &str) -> Result<Option<String>>;
    /// 알림 (pi의 notify())
    pub fn notify(&self, message: &str, level: NotifyLevel);
    /// 상태 표시줄 설정 (pi의 setStatus())
    pub fn set_status(&self, message: &str);
    /// 커스텀 위젯 (pi의 setWidget())
    pub fn set_widget(&self, name: &str, widget: Box<dyn ExtensionWidget>);
    /// 테마 접근
    pub fn get_theme(&self) -> Theme;
    pub fn set_theme(&self, name: &str);
}

/// ExtensionContext 확장 (기존 필드에 ui 추가)
pub struct ExtensionContext {
    // ── 기존 (유지) ──
    pub cwd: PathBuf,
    // settings, config, session_id, idle,
    // tool_registrar, message_sender, errors, etc.
    
    // ── 새로 추가 ──
    pub ui: Option<ExtensionUI>,
    pub has_ui: bool,
}
```

### 3.3 Stale 감지 시스템

```rust
/// oxicode-cli/src/extensions/stale.rs — 신규

/// 확장 컨텍스트 수명 관리
pub struct ExtensionContextGuard {
    generation: u64,
    invalidator: Arc<AtomicU64>,
}

impl ExtensionContextGuard {
    /// 현재 컨텍스트가 유효한지 검사
    pub fn is_valid(&self) -> bool {
        self.generation == self.invalidator.load(Ordering::Acquire)
    }
    
    /// 호출 전 반드시 검사
    fn check_valid(&self) -> Result<()> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(anyhow!("Extension context is stale — session has been switched or reloaded"))
        }
    }
}

/// 세션 전환 시 호출 — 모든 확장 컨텍스트 무효화
pub fn invalidate_all_contexts(invalidator: &Arc<AtomicU64>) {
    invalidator.fetch_add(1, Ordering::Release);
}
```

### 3.4 Skills 시스템 강화

기존 `SkillManager`(`oxicode-cli/src/skills/mod.rs`)를 확장합니다. `load_from_dir()`, `get()`, `all()`, `search()`는 그대로 유지.

```rust
/// oxicode-cli/src/skills/mod.rs 확장

use serde::{Deserialize, Serialize};

/// YAML frontmatter 파싱 (pi와 동일 포맷)
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: String,
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

/// 기존 Skill 구조체 확장
pub struct Skill {
    pub name: String,
    pub description: String,
    pub location: PathBuf,
    pub content: String,
    /// 새 필드: 모델 호출 비활성화 (pi의 disable-model-invocation)
    pub disable_model_invocation: bool,
}

impl SkillManager {
    /// 전체 검색 (pi의 발견 규칙 이식)
    /// 기존 load_from_dir() 단일 디렉토리 → 다중 소스 발견으로 확장
    pub fn discover_all(cwd: &Path, settings: &Settings) -> Result<Self> {
        let mut skills = HashMap::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        
        // 1. 글로벌: ~/.oxicode/skills/
        Self::discover_from_dir(
            Self::skills_dir()?,
            &mut skills,
            &mut seen_paths,
        )?;
        
        // 2. 프로젝트: .oxicode/skills/
        Self::discover_from_dir(
            cwd.join(".oxicode/skills"),
            &mut skills,
            &mut seen_paths,
        )?;
        
        // 3. 조상 디렉토리 (pi의 .agents/skills/ 이식)
        for ancestor in cwd.ancestors() {
            let agents_skills = ancestor.join(".agents/skills");
            if agents_skills.is_dir() {
                Self::discover_from_dir(
                    agents_skills,
                    &mut skills,
                    &mut seen_paths,
                )?;
            }
            if ancestor.join(".git").is_dir() {
                break;  // git 루트까지만
            }
        }
        
        // 4. 설정에 명시된 경로
        for path in &settings.skills {
            Self::discover_from_dir(
                PathBuf::from(path),
                &mut skills,
                &mut seen_paths,
            )?;
        }
        
        Ok(Self { skills })
    }
    
    /// 디렉토리 스캔 (기존 load_from_dir() 로직 재사용 + 중복 감지)
    fn discover_from_dir(
        dir: PathBuf,
        skills: &mut HashMap<String, Skill>,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        if !dir.is_dir() { return Ok(()); }
        
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            
            // 심볼릭 링크 정규화 (pi의 canonicalizePath)
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            
            // 중복 감지
            if seen.contains(&canonical) {
                tracing::warn!(
                    "Duplicate skill path detected (skipping): {}",
                    canonical.display()
                );
                continue;
            }
            
            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.is_file() {
                    // 이름 검증
                    let dir_name = path.file_name().unwrap_or_default().to_string_lossy();
                    if let Ok(valid_name) = Self::validate_name(&dir_name) {
                        seen.insert(canonical);
                        match Self::load_skill_with_frontmatter(&valid_name, &skill_file) {
                            Ok(skill) => { skills.insert(valid_name, skill); }
                            Err(e) => {
                                tracing::warn!("Failed to load skill from {}: {}", skill_file.display(), e);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    
    /// 이름 검증 (pi와 동일)
    fn validate_name(name: &str) -> Result<String> {
        let name = name.to_lowercase();
        if name.len() > 64 {
            return Err(anyhow!("Skill name too long (max 64 chars)"));
        }
        if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(anyhow!("Skill name must contain only a-z, 0-9, and hyphens"));
        }
        if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
            return Err(anyhow!("Skill name must not have leading/trailing/consecutive hyphens"));
        }
        Ok(name)
    }
    
    /// SKILL.md 로딩 + YAML frontmatter 파싱
    fn load_skill_with_frontmatter(name: &str, path: &Path) -> Result<Skill> {
        let content = fs::read_to_string(path)?;
        let location = path.parent().unwrap_or(path).to_path_buf();
        
        // YAML frontmatter 파싱 (--- 사이의 내용)
        let (frontmatter, body) = if content.starts_with("---") {
            let rest = &content[3..];
            if let Some(end) = rest.find("\n---") {
                let yaml_str = &rest[..end];
                let body_content = rest[end + 4..].trim_start();
                let fm: SkillFrontmatter = serde_yaml::from_str(yaml_str)
                    .unwrap_or(SkillFrontmatter {
                        name: None,
                        description: String::new(),
                        disable_model_invocation: false,
                    });
                (fm, body_content.to_string())
            } else {
                (SkillFrontmatter::default(), content.clone())
            }
        } else {
            (SkillFrontmatter::default(), content.clone())
        };
        
        let description = frontmatter.description.clone()
            .or_else(|| Self::extract_description(&body))
            .unwrap_or_else(|| "No description".to_string());
        
        Ok(Skill {
            name: frontmatter.name.clone().unwrap_or_else(|| name.to_string()),
            description,
            location,
            content: body,
            disable_model_invocation: frontmatter.disable_model_invocation,
        })
    }
    
    /// 시스템 프롬프트용 XML 포맷 (pi와 동일)
    pub fn format_for_prompt(&self) -> String {
        let visible: Vec<_> = self.skills.values()
            .filter(|s| !s.disable_model_invocation)
            .collect();
        
        if visible.is_empty() { return String::new(); }
        
        let mut xml = String::from("<available_skills>\n");
        for skill in visible {
            xml.push_str(&format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n",
                skill.name, skill.description, skill.location.display()
            ));
        }
        xml.push_str("</available_skills>");
        xml
    }
}
```

### 3.5 패키지 매니저 강화

기존 `PackageManager`(`oxicode-cli/src/storage/packages.rs`)는 이미 npm + git 업데이트 확인, lockfile, 자동 발견 등을 지원합니다. 아래 기능만 추가:

```rust
/// oxicode-cli/src/storage/packages.rs 확장

impl PackageManager {
    /// 업데이트 확인 병렬화 (기존 check_for_updates 직렬 → 병렬)
    pub async fn check_for_updates_parallel(&self) -> Vec<PackageUpdateInfo> {
        let mut tasks = Vec::new();
        
        for lock_entry in self.lockfile.packages.values() {
            let parsed = ParsedSource::parse(&lock_entry.source);
            match parsed {
                ParsedSource::Npm { name: pkg_name, .. } => {
                    let name = pkg_name.clone();
                    let current_version = lock_entry.version.clone();
                    let scope = lock_entry.scope;
                    let source = lock_entry.source.clone();
                    tasks.push(tokio::spawn(async move {
                        // npm view로 최신 버전 확인
                        match NpmPackageInfo::fetch(&name).await {
                            Ok(info) => {
                                if let Some(latest) = info.latest_version() {
                                    if latest != current_version {
                                        return Some(PackageUpdateInfo {
                                            source,
                                            display_name: name,
                                            source_type: "npm".to_string(),
                                            scope,
                                        });
                                    }
                                }
                            }
                            Err(_) => {}
                        }
                        None
                    }));
                }
                ParsedSource::Git { host, path, ref_, .. } => {
                    let install_path = self.git_install_path(&host, &path, lock_entry.scope);
                    let source = lock_entry.source.clone();
                    let display = format!("{}/{}", host, path);
                    let scope = lock_entry.scope;
                    tasks.push(tokio::spawn(async move {
                        if install_path.exists() {
                            // git ls-remote는 I/O가 많아 비동기로 처리
                            match tokio::task::spawn_blocking(move || {
                                git_has_update(&install_path)
                            }).await {
                                Ok(Ok(true)) => {
                                    return Some(PackageUpdateInfo {
                                        source,
                                        display_name: display,
                                        source_type: "git".to_string(),
                                        scope,
                                    });
                                }
                                _ => {}
                            }
                        }
                        None
                    }));
                }
                _ => {}
            }
        }
        
        // 모든 태스크 병렬 대기
        futures::future::join_all(tasks)
            .await
            .into_iter()
            .filter_map(|r| r.ok().flatten())
            .collect()
    }
    
    /// 리소스 필터링 (pi의 패턴 시스템 이식)
    pub fn filter_resources(
        resources: &[ResolvedResource],
        filters: &[ResourceFilter],
    ) -> Vec<ResolvedResource> {
        let mut result = resources.to_vec();
        
        for filter in filters {
            match filter {
                ResourceFilter::Include(pattern) => {
                    // +path: 강제 포함 (이미 목록에 없으면 추가)
                }
                ResourceFilter::Exclude(pattern) => {
                    // !pattern: 제외
                    result.retain(|r| !glob_match(pattern, &r.path.to_string_lossy()));
                }
            }
        }
        
        result
    }
    
    /// 우선순위 랭킹 (기존 2단계 → 5단계 확장)
    pub fn rank_resources(resources: &[ResolvedResource]) -> Vec<ResolvedResource> {
        let mut ranked = resources.to_vec();
        ranked.sort_by(|a, b| {
            let rank = |r: &ResolvedResource| -> u8 {
                match (r.metadata.scope, r.metadata.origin) {
                    (SourceScope::Project, ResourceOrigin::TopLevel) => 0,  // 프로젝트 로컬
                    (SourceScope::Project, ResourceOrigin::Package) => 1,   // 프로젝트 패키지
                    (SourceScope::User, ResourceOrigin::TopLevel) => 2,     // 사용자 로컬
                    (SourceScope::User, ResourceOrigin::Package) => 3,      // 사용자 패키지
                }
            };
            rank(a).cmp(&rank(b))
        });
        ranked
    }
}

/// 리소스 필터 타입
#[derive(Debug, Clone)]
pub enum ResourceFilter {
    /// +path: 강제 포함
    Include(String),
    /// !path: 제외
    Exclude(String),
}
```

---

## 4. 구현 계획

### Phase 1: Extension UI 컨텍스트 (1.5주)

| 작업 | 대상 파일 | 변경 유형 |
|------|-----------|-----------|
| ExtensionUI 트레이트 | `extensions/context.rs` | 확장 |
| TUI 다이얼로그 구현 | `extensions/context.rs` | 신규 |
| RPC 다이얼로그 브릿지 | `extensions/context.rs` | 확장 |
| select, confirm, input, editor | `extensions/context.rs` | 신규 |
| notify, setStatus, setWidget | `extensions/context.rs` | 신규 |
| 단축키 등록 | `extensions/mod.rs`, `types.rs` | 확장 |
| ExtensionRunner에 단축키 수집 | `extensions/registry.rs` | 확장 |

### Phase 2: Stale 감지 (3일)

| 작업 | 대상 파일 | 변경 유형 |
|------|-----------|-----------|
| ExtensionContextGuard | `extensions/stale.rs` | 신규 |
| 세션 전환 시 무효화 | `extensions/registry.rs` | 확장 |
| ExtensionContext에 guard 통합 | `extensions/context.rs` | 확장 |

### Phase 3: Skills 강화 (1주)

| 작업 | 대상 파일 | 변경 유형 |
|------|-----------|-----------|
| YAML frontmatter 파싱 | `skills/mod.rs` | 확장 |
| SkillFrontmatter 타입 | `skills/mod.rs` | 신규 |
| discover_all() 다중 소스 | `skills/mod.rs` | 신규 |
| 이름 검증 | `skills/mod.rs` | 신규 |
| 중복 감지 (canonicalize) | `skills/mod.rs` | 신규 |
| disable_model_invocation | `skills/mod.rs` | 확장 |
| format_for_prompt() | `skills/mod.rs` | 신규 |

### Phase 4: 패키지 매니저 (1주)

| 작업 | 대상 파일 | 변경 유형 |
|------|-----------|-----------|
| 업데이트 확인 병렬화 | `storage/packages.rs` | 확장 |
| ResourceFilter 타입 | `storage/packages.rs` | 신규 |
| filter_resources() | `storage/packages.rs` | 신규 |
| rank_resources() 확장 | `storage/packages.rs` | 확장 |
| SourceScope 확장 (필요시) | `storage/packages.rs` | 확장 |

---

## 5. 새 의존성

```toml
[dependencies]
serde_yaml = "0.9"    # SKILL.md frontmatter 파싱
glob-match = "0.2"    # 리소스 필터링 패턴 (또는 glob crate)
```

---

## 6. 성공 기준

- [ ] Extension UI: select, confirm, input, editor, notify, setWidget TUI에서 동작
- [ ] Extension UI: RPC 모드에서도 동일한 인터페이스 동작
- [ ] 단축키: `register_shortcuts()`로 확장이 단축키 등록 가능
- [ ] Stale 감지: 세션 전환 시 모든 확장 컨텍스트 자동 무효화
- [ ] Skills: YAML frontmatter 파싱 (`disable-model-invocation` 포함)
- [ ] Skills: 조상 디렉토리 + 설정 경로 통합 발견
- [ ] Skills: 이름 검증 + canonicalize 중복 감지
- [ ] 패키지: 업데이트 확인 병렬화 (기존 직렬 대비 속도 개선)
- [ ] 패키지: 리소스 필터링 (!exclude, +include)
- [ ] 패키지: 5단계 우선순위 랭킹
- [ ] 기존 확장(WASM, 네이티브) 호환성 유지
- [ ] `cargo test --workspace` 통과
- [ ] `cargo clippy --workspace -- -D warnings` 통과

---

## 7. 기존 구현과의 호환성

| 변경 | 기존 API 영향 | 마이그레이션 |
|------|---------------|-------------|
| `register_shortcuts()` 추가 | 기본 구현 제공 (빈 Vec) | 없음 |
| `ExtensionUI` 추가 | `ExtensionContext`에 `Option<ExtensionUI>` 필드 추가 | 기존 `ExtensionContextBuilder`는 `None`으로 초기화 |
| `StaleGuard` | `ExtensionContext`에 선택적 필드 | 없음 |
| `SkillFrontmatter` | 기존 `Skill`에 `disable_model_invocation` 필드 추가 | 기본값 `false` |
| `discover_all()` | 기존 `load_from_dir()`은 유지 | 신규 메서드, 기존 호출부 변경 없음 |
| `ResourceFilter` | 신규 타입 | 기존 코드 영향 없음 |
