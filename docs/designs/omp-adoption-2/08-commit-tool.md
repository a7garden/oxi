# 세부 설계 ⑪ — Commit 도구 (atomic split + conventional 커밋)

> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §3·§5 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `commit/` (~3,000줄), `commands/commit.ts`, `utils/git.ts` (1,838줄)
> 후속: N4 구현 → CHANGELOG.md
> 의존: ⑧ LSP (선택 — rename_file 연동)
>
> **⚠️ v2**: `oxi_ai::high_level::complete` 시그니처 수정 (provider 인자 제거), `Tool::new` 사용, `find_git_root` 재사용. hunk 스테이징을 MVP로 격상.

---

## 0. 핵심 (TL;DR)

omp의 **commit 도구**는 무관련 변경사항을 atomic 커밋으로 분할하고, 의존성 순서로 정렬하며, conventional 커밋 메시지를 생성한다. 두 파이프라인이 있다:

1. **agentic (기본)** — LLM 에이전트가 도구를 통해 diff를 탐색하고 `propose_commit`/`split_commit`으로 결과 제출.
2. **legacy (결정론적)** — analysis → scope → summary → message 직렬 LLM 호출.

**진입 분기**: 강제 fallback → trivial 변경 감지(공백/임포트만) → agentic 세션 → 실패 시 결정론적 fallback.

**oxi 설계 전략**: omp의 **결정론적 휴리스틱**(scope 추출, 검증, 위상정렬, 메시지 포맷)은 LLM 비의존적이므로 Rust로 직접 이식. agentic 파이프라인은 oxi의 기존 에이전트 루프 위에서 별도 에이전트 정의로 구현. **초기 구현은 legacy 결정론적 경로 우선** — agentic은 후순위.

### omp가 검증한 가치
- **atomic 커밋** — 무관련 변경이 섞인 working tree를 의미 단위로 분할.
- **의존성 정렬** — 소스 > 테스트 > 문서 > 설정 순서로, 의존성 위상정렬.
- **conventional 메시지** — `type(scope): summary` 형식 자동 생성.
- **changelog 동기화** — Keep-a-Changelog `## [Unreleased]` 섹션 자동 갱신.

---

## 1. omp 메커니즘

### 1.1 중심 타입 (`commit/types.ts`)

```typescript
type CommitType = "feat" | "fix" | "docs" | "style" | "refactor" |
                  "perf" | "test" | "build" | "ci" | "chore" | "revert";

interface ConventionalAnalysis {
    type: CommitType;
    scope: string;                    // 최대 2세그먼트, 소문자
    details: ConventionalDetail[];
    issueRefs: string[];
}

interface ConventionalDetail {
    text: string;                     // ≤120자, 마침표로 끝남
    changelogCategory?: ChangelogCategory;
    userVisible: boolean;
}

type ChangelogCategory = "added" | "changed" | "deprecated" |
                          "removed" | "fixed" | "security" | "internal";
```

### 1.2 결정론적 파이프라인 (`commit/pipeline.ts`)

```
runCommitCommand(args)
  → stagedFiles (또는 전체 working tree)
  → isExcludedFile (lock 파일 제외 — 26개 패턴)
  → changelogFlow (CHANGELOG.md 감지)
  → diff / stat / numstat 수집
  → extractScopeCandidates (결정론적 휴리스틱)
  → generateConventionalAnalysis (LLM — create_conventional_analysis 툴)
  → generateSummary (LLM — create_commit_summary 툴, 3회 재시도)
  → validateSummary / validateScope / validateAnalysis
  → formatCommitMessage → type(scope): summary\n\n- detail...
  → git commit (또는 dry-run 미리보기)
```

### 1.3 scope 추출 휴리스틱 (`commit/analysis/scope.ts`, 210줄)

omp에서 가장 정교한 결정론ic 부분. **LLM 없이** 동작:

```
numstat 기반 컴포넌트별 라인 비중
  → buildScopeCandidates (2-세그먼트 가중치 ×1.2/×0.8)
  → isWideChange (top<60% 또는 distinctRoots≥3)
  → wide 패턴 분류:
      - deps (의존성 파일 다수)
      - docs (문서 다수)
      - tests (테스트 다수)
      - error-handling
      - type-refactor
      - config
  → PLACEHOLDER_DIRS (src/lib/bin 등)로 필터링
  → SKIP_DIRS (test/benches 등)로 필터링
```

### 1.4 atomic split — Kahn 위상정렬 (`commit/agentic/topo-sort.ts`)

```typescript
function computeDependencyOrder(groups: CommitGroup[]): CommitGroup[] {
    // 그래프 구축: A가 B에 의존 (B가 먼저 커밋되어야 함)
    const graph = buildDependencyGraph(groups);
    
    // Kahn 알고리즘
    const inDegree = new Map<string, number>();
    const queue: string[] = [];
    
    for (const [node, deps] of graph) {
        inDegree.set(node, deps.size);
        if (deps.size === 0) queue.push(node);
    }
    
    const sorted: CommitGroup[] = [];
    while (queue.length > 0) {
        const node = queue.shift()!;
        sorted.push(groups.find(g => g.id === node)!);
        for (const [dependent, deps] of graph) {
            if (deps.has(node)) {
                deps.delete(node);
                if (deps.size === 0 && !sorted.find(g => g.id === dependent)) {
                    queue.push(dependent);
                }
            }
        }
    }
    
    // 사이클 감지
    if (sorted.length !== groups.length) {
        throw new Error("Dependency cycle detected — cannot order commits");
    }
    
    return sorted;
}
```

**의존성 규칙**:
- 소스 파일 > 테스트 파일 (테스트가 소스에 의존)
- 소스 파일 > 문서 (문서가 소스를 참조)
- 인터페이스 > 구현체
- 설정 > 소스 (설정이 먼저)

### 1.5 메시지 포맷 (`commit/message.ts`, 11줄)

```typescript
function formatCommitMessage(analysis: ConventionalAnalysis, summary: string): string {
    const header = analysis.scope
        ? `${analysis.type}(${analysis.scope}): ${summary}`
        : `${analysis.type}: ${summary}`;
    
    const details = analysis.details
        .map(d => `- ${d.text}`)
        .join("\n");
    
    const issues = analysis.issueRefs.length > 0
        ? `\n\n${analysis.issueRefs.map(r => `Refs ${r}`).join("\n")}`
        : "";
    
    return `${header}\n\n${details}${issues}`;
}
```

### 1.6 changelog (`commit/changelog/`)

Keep-a-Changelog 형식:
- `## [Unreleased]` 섹션 감지/파싱
- 파일 위치 기반 루트 + 패키지별 CHANGELOG.md 자동 발견
- `ConventionalDetail.changelogCategory` → 해당 섹션에 항목 추가

### 1.7 검증 (`commit/analysis/validation.ts`)

```typescript
function validateSummary(summary: string): string[] {
    const errors = [];
    if (summary.length > 72) errors.push("Summary exceeds 72 characters");
    if (summary.endsWith(".")) errors.push("Summary must not end with period");
    if (summary.includes("\n")) errors.push("Summary must be single line");
    return errors;
}

function validateScope(scope: string): string[] {
    const errors = [];
    if (scope.split("/").length > 2) errors.push("Scope has more than 2 segments");
    if (scope !== scope.toLowerCase()) errors.push("Scope must be lowercase");
    if (!/^[a-z0-9][a-z0-9-_]*$/.test(scope)) errors.push("Scope has invalid characters");
    return errors;
}
```

---

## 2. oxi화 설계

### 2.1 모듈 구조

```
oxi-cli/src/commit/
├── mod.rs              파이프라인 오케스트레이터
├── types.rs            CommitType, ConventionalAnalysis, CommitGroup
├── scope.rs            결정론적 scope 추출 (omp analysis/scope.ts 이식)
├── analysis.rs         LLM 기반 분석 (ConventionalAnalysis 생성)
├── summary.rs          LLM 기반 요약 (summary 생성)
├── validation.rs       검증 규칙 (omp analysis/validation.ts 이식)
├── message.rs          format_commit_message
├── topo_sort.rs        Kahn 위상정렬 (omp agentic/topo-sort.ts 이식)
├── changelog.rs        Keep-a-Changelog 파싱/갱신
├── git.rs              git 조작 (diff, stage, commit)
└── exclusions.rs       lock 파일 제외
```

### 2.2 핵심 타입 (`types.rs`)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitType {
    Feat, Fix, Docs, Style, Refactor,
    Perf, Test, Build, Ci, Chore, Revert,
}

impl CommitType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Feat => "feat", Self::Fix => "fix", Self::Docs => "docs",
            Self::Style => "style", Self::Refactor => "refactor",
            Self::Perf => "perf", Self::Test => "test", Self::Build => "build",
            Self::Ci => "ci", Self::Chore => "chore", Self::Revert => "revert",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConventionalAnalysis {
    #[serde(rename = "type")]
    pub commit_type: CommitType,
    pub scope: String,
    pub details: Vec<ConventionalDetail>,
    pub issue_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConventionalDetail {
    pub text: String,
    pub changelog_category: Option<ChangelogCategory>,
    pub user_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangelogCategory {
    Added, Changed, Deprecated,
    Removed, Fixed, Security, Internal,
}

/// atomic split 결과 — 하나의 커밋 그룹.
#[derive(Debug, Clone)]
pub struct CommitGroup {
    pub id: String,
    pub files: Vec<String>,
    pub analysis: ConventionalAnalysis,
    pub summary: String,
    pub dependencies: Vec<String>,  // 선행 커밋 그룹 ID
}
```

### 2.3 결정론적 scope 추출 (`scope.rs`)

omp의 `analysis/scope.ts`를 Rust로 직접 이식 (LLM 없음):

```rust
/// numstat 기반 scope 후보 추출.
pub fn extract_scope_candidates(numstat: &[NumstatEntry]) -> Vec<ScopeCandidate> {
    let mut components: HashMap<String, usize> = HashMap::new();
    
    for entry in numstat {
        if is_excluded_file(&entry.path) { continue; }
        
        // 경로에서 컴포넌트 추출 (첫 1-2 세그먼트)
        let component = extract_path_component(&entry.path);
        *components.entry(component).or_default() +=
            entry.additions + entry.deletions;
    }
    
    // 비중 기반 정렬 + 2-세그먼트 가중치
    let mut candidates: Vec<_> = components.into_iter()
        .map(|(name, lines)| ScopeCandidate {
            name,
            weight: lines as f64,
            segments: name.split('/').count(),
        })
        .collect();
    
    // 가중치 조정: 2세그먼트는 ×1.2, 1세그먼트는 ×0.8
    for c in &mut candidates {
        c.weight *= if c.segments == 2 { 1.2 } else { 0.8 };
    }
    
    candidates.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap());
    candidates
}

const PLACEHOLDER_DIRS: &[&str] = &["src", "lib", "bin", "app", "cmd"];
const SKIP_DIRS: &[&str] = &["test", "tests", "benches", "examples", "vendor", "node_modules"};

pub fn is_wide_change(numstat: &[NumstatEntry]) -> bool {
    let candidates = extract_scope_candidates(numstat);
    if candidates.is_empty() { return false; }
    
    let total: f64 = candidates.iter().map(|c| c.weight).sum();
    let top_share = candidates[0].weight / total;
    let distinct_roots = candidates.iter()
        .filter(|c| !PLACEHOLDER_DIRS.contains(&c.name.as_str()))
        .count();
    
    top_share < 0.6 || distinct_roots >= 3
}
```

### 2.4 LLM 분석 (`analysis.rs`)

```rust
use oxi_ai::{self, high_level::complete, Context, Message, UserMessage, Tool};

/// LLM에게 ConventionalAnalysis 생성 요청.
/// v2: `complete(model, context, options)` — provider 인자 없음 (글로벌 레지스트리).
pub async fn generate_conventional_analysis(
    model: &oxi_ai::Model,
    diff: &str,
    scope_candidates: &[ScopeCandidate],
) -> anyhow::Result<ConventionalAnalysis> {
    let system = include_str!("../prompts/commit-analysis-system.md");
    let user = format_analysis_prompt(diff, scope_candidates);

    let mut ctx = Context::new()
        .with_system_prompt(system);
    ctx.add_message(Message::User(UserMessage::new(user)));
    ctx.tools = vec![conventional_analysis_tool()];

    let response = complete(model, &ctx, Some(oxi_ai::StreamOptions {
        max_tokens: Some(2400),
        ..Default::default()
    })).await?;

    // 툴콜에서 ConventionalAnalysis 추출, 실패 시 텍스트에서 JSON 폴백
    parse_conventional_analysis(&response)
        .or_else(|| parse_json_from_text(&response))
        .ok_or_else(|| anyhow!("Failed to parse analysis from LLM response"))
}

/// v2: `Tool::new(name, description, parameters)` — Default 파생 없음.
fn conventional_analysis_tool() -> Tool {
    Tool::new(
        "create_conventional_analysis",
        "Analyze a git diff and produce a conventional commit analysis",
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": ["feat","fix","docs","style","refactor","perf","test","build","ci","chore","revert"]},
                "scope": {"type": "string", "pattern": "^[a-z0-9][a-z0-9-_]*(/[a-z0-9][a-z0-9-_]*)?$"},
                "details": {"type": "array", "items": {"type": "object", "properties": {
                    "text": {"type": "string", "maxLength": 120},
                    "changelogCategory": {"type": "string"},
                    "userVisible": {"type": "boolean"}
                }}},
                "issueRefs": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["type", "scope", "details"]
        }),
    )
}
```

### 2.5 위상정렬 (`topo_sort.rs`)

```rust
/// Kahn 알고리즘으로 커밋 그룹을 의존성 순서로 정렬.
/// 사이클 감지 시 에러 반환.
pub fn compute_dependency_order(groups: &mut Vec<CommitGroup>) -> anyhow::Result<()> {
    use std::collections::{HashMap, HashSet, VecDeque};
    
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    
    for g in groups.iter() {
        in_degree.entry(&g.id).or_insert(0);
        for dep in &g.dependencies {
            *in_degree.entry(&g.id).or_insert(0) += 1;
            dependents.entry(dep).or_default().push(&g.id);
        }
    }
    
    let mut queue: VecDeque<&str> = in_degree.iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();
    
    let mut sorted_ids = Vec::with_capacity(groups.len());
    while let Some(id) = queue.pop_front() {
        sorted_ids.push(id);
        if let Some(deps) = dependents.get(id) {
            for dep in deps {
                let d = in_degree.get_mut(*dep).unwrap();
                *d -= 1;
                if *d == 0 { queue.push_back(dep); }
            }
        }
    }
    
    if sorted_ids.len() != groups.len() {
        let cycle: Vec<_> = groups.iter()
            .filter(|g| !sorted_ids.contains(&g.id.as_str()))
            .map(|g| g.id.clone())
            .collect();
        anyhow::bail!("Dependency cycle detected among: {:?}", cycle);
    }
    
    // groups를 sorted_ids 순서로 재배열
    groups.sort_by_key(|g| sorted_ids.iter().position(|id| *id == g.id));
    Ok(())
}

/// 파일 기반 의존성 추론.
/// 소스 > 테스트, 인터페이스 > 구현체 등.
pub fn infer_dependencies(groups: &mut Vec<CommitGroup>) {
    for i in 0..groups.len() {
        for j in 0..groups.len() {
            if i == j { continue; }
            // groups[j]의 파일이 groups[i]의 파일에 의존하면
            // groups[j].dependencies.push(groups[i].id)
            if depends_on(&groups[j], &groups[i]) {
                groups[j].dependencies.push(groups[i].id.clone());
            }
        }
    }
}

fn depends_on(a: &CommitGroup, b: &CommitGroup) -> bool {
    // 휴리스틱:
    // - a가 테스트 파일을 포함하고 b가 해당 소스 파일을 포함
    // - a가 b의 모듈을 import
    // - a가 b가 정의한 타입을 사용
    a.files.iter().any(|f| is_test_file(f)) &&
    b.files.iter().any(|f| !is_test_file(f) && shares_module(f, &a.files))
}
```

### 2.6 메시지 포맷 (`message.rs`)

```rust
pub fn format_commit_message(analysis: &ConventionalAnalysis, summary: &str) -> String {
    let header = if analysis.scope.is_empty() {
        format!("{}: {}", analysis.commit_type.as_str(), summary)
    } else {
        format!("{}({}): {}", analysis.commit_type.as_str(), analysis.scope, summary)
    };
    
    let details: Vec<String> = analysis.details.iter()
        .map(|d| format!("- {}", d.text))
        .collect();
    
    let mut message = header;
    if !details.is_empty() {
        message.push_str("\n\n");
        message.push_str(&details.join("\n"));
    }
    
    if !analysis.issue_refs.is_empty() {
        message.push_str("\n\n");
        message.push_str(&analysis.issue_refs.iter()
            .map(|r| format!("Refs {}", r))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    
    message
}
```

### 2.7 git 조작 (`git.rs`)

```rust
pub struct GitOps {
    cwd: PathBuf,
}

impl GitOps {
    pub fn diff_stat(&self) -> anyhow::Result<Vec<NumstatEntry>> {
        let output = std::process::Command::new("git")
            .args(["diff", "--numstat", "HEAD"])
            .current_dir(&self.cwd)
            .output()?;
        // 출력 파싱 → NumstatEntry
        Ok(parse_numstat(&String::from_utf8_lossy(&output.stdout)))
    }
    
    pub fn stage_hunks(&self, file: &str, hunk_indices: &[usize]) -> anyhow::Result<()> {
        // git apply --cached로 특정 hunk만 스테이징
        // omp의 git.stage.hunks 패턴
        todo!()
    }
    
    pub fn commit(&self, message: &str) -> anyhow::Result<String> {
        let output = std::process::Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.cwd)
            .output()?;
        let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(hash)
    }
    
    pub fn stage_all(&self) -> anyhow::Result<()> {
        std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.cwd)
            .output()?;
        Ok(())
    }
}
```

---

## 3. Commit 도구 (oxi-agent)

### 3.1 `commit` 도구

`oxi-agent/src/tools/commit.rs`:

```rust
pub struct CommitTool {
    cwd: PathBuf,
    /// v2: LLM 분석용 모델. bootstrap에서 주입.
    model: oxi_ai::Model,
}

impl AgentTool for CommitTool {
    fn name(&self) -> &str { "commit" }
    fn essential(&self) -> bool { false }
    fn description(&self) -> &str {
        "Analyze working tree changes, split into atomic commits ordered by \
         dependencies, generate conventional commit messages. Use --dry-run \
         for preview without committing."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "dry_run": {"type": "boolean", "description": "Preview without committing"},
                "push": {"type": "boolean", "description": "Push after commit"},
                "no_changelog": {"type": "boolean", "description": "Skip changelog update"},
                "context": {"type": "string", "description": "Additional context for analysis"}
            }
        })
    }
    
    async fn execute(&self, ..., ctx: &ToolContext) -> Result<AgentToolResult, ToolError> {
        let args: CommitArgs = serde_json::from_value(params)?;
        let git = GitOps::new(self.cwd.clone());
        
        // 1. 변경사항 수집
        let numstat = git.diff_stat()?;
        let filtered: Vec<_> = numstat.into_iter()
            .filter(|e| !is_excluded_file(&e.path))
            .collect();
        
        if filtered.is_empty() {
            return Ok(AgentToolResult::success("No changes to commit."));
        }
        
        // 2. trivial 변경 감지
        if is_trivial_change(&filtered) {
            let msg = generate_trivial_message(&filtered);
            if !args.dry_run {
                git.stage_all()?;
                git.commit(&msg)?;
            }
            return Ok(AgentToolResult::success(format!("Trivial change:\n{}", msg)));
        }
        
        // 3. scope 추출 (결정론적)
        let candidates = extract_scope_candidates(&filtered);
        
        // 4. diff 수집
        let diff = git.diff_full()?;
        
        // 5. LLM 분석 — v2: provider 인자 없음, model은 도구가 보유
        let analysis = generate_conventional_analysis(
            &self.model, &diff, &candidates
        ).await.map_err(|e| e.to_string())?;

        // 6. summary 생성
        let summary = generate_summary(&self.model, &analysis, &diff)
            .await.map_err(|e| e.to_string())?;
        
        // 7. 검증
        let errors = validate_all(&analysis, &summary);
        if !errors.is_empty() {
            return Ok(AgentToolResult::error(format!(
                "Validation failed:\n{}", errors.join("\n")
            )));
        }
        
        // 8. 메시지 포맷
        let message = format_commit_message(&analysis, &summary);
        
        if args.dry_run {
            return Ok(AgentToolResult::success(format!(
                "Dry run — would commit:\n\n{}", message
            )));
        }
        
        // 9. 커밋
        git.stage_all()?;
        let hash = git.commit(&message)?;
        
        // 10. changelog (선택)
        if !args.no_changelog {
            if let Err(e) = update_changelog(&self.cwd, &analysis).await {
                // changelog 실패는 커밋을 롤백하지 않음 — 경고만
                eprintln!("Warning: changelog update failed: {}", e);
            }
        }
        
        // 11. push (선택)
        if args.push {
            git.push()?;
        }
        
        Ok(AgentToolResult::success(format!(
            "Committed {}:\n\n{}", &hash[..7], message
        )))
    }
}
```

### 3.2 `/commit` 슬래시 명령

```rust
// oxi-cli/src/tui/slash/builtin/commit.rs
pub struct CommitCommand;

impl BuiltinSlashCommand for CommitCommand {
    fn name(&self) -> &str { "/commit" }
    fn description(&self) -> &str { "Atomic split + conventional commit" }
    
    async fn execute(&self, args: &str, state: &mut AppState) -> NotificationKind {
        let commit_args = parse_commit_args(args);
        // commit 도구를 직접 호출하거나 에이전트에게 위임
        match run_commit(&state.cwd, commit_args).await {
            Ok(result) => {
                state.add_notification(result, NotificationKind::Success);
                NotificationKind::Success
            }
            Err(e) => {
                state.add_notification(format!("Commit failed: {}", e), NotificationKind::Error);
                NotificationKind::Error
            }
        }
    }
}
```

---

## 4. 설정

```rust
pub struct Settings {
    pub commit_tool_enabled: bool,         // 기본 false (LLM 비용)
    pub commit_default_dry_run: bool,       // 기본 true (안전)
    pub commit_model_role: String,          // 기본 "smol" (비용 절감)
    pub commit_auto_changelog: bool,        // 기본 true
    pub commit_auto_push: bool,             // 기본 false
}
```

---

## 5. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N4.1 | `types.rs` (CommitType, ConventionalAnalysis, CommitGroup) | — |
| N4.2 | `exclusions.rs` (lock 파일 제외 — 26 패턴) | N4.1 |
| N4.3 | `scope.rs` (결정론적 scope 추출) | N4.1 |
| N4.4 | `validation.rs` (검증 규칙) | N4.1 |
| N4.5 | `message.rs` (format_commit_message) | N4.1 |
| N4.6 | `topo_sort.rs` (Kahn 위상정렬) | N4.1 |
| N4.7 | `git.rs` (diff, stage, commit) | — |
| N4.8 | `analysis.rs` (LLM 분석 + 툴) | N4.3, N4.7 |
| N4.9 | `summary.rs` (LLM 요약) | N4.8 |
| N4.10 | `changelog.rs` (Keep-a-Changelog) | N4.5 |
| N4.11 | `CommitTool` (AgentTool 구현) | N4.8, N4.9 |
| N4.12 | `/commit` 슬래시 명령 | N4.11 |
| N4.13 | atomic split (다중 커밋 그룹) | N4.6, N4.11 |
| N4.14 | dry-run 미리보기 | N4.11 |
| N4.15 | (후순위) agentic 파이프라인 | N4.13 |

> **⑧ LSP 의존 (선택)**: rename_file 커밋 시 LSP willRenameFiles 연동. N4 범위 외, 후순위.
> **초기 범위**: 단일 커밋(trivial + 결정론적 + LLM 분석). atomic split(N4.13)은 안정화 후.

---

## 6. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| LLM 비용 (분석 + 요약 2회 호출) | 🟠 위험 | `commit_model_role: "smol"` 기본. 비용 토글 |
| agentic vs 결정론ic 우선순위 | 🟢 결정됨 | 결정론ic 우선. agentic은 N4.15 후순위 |
| atomic split 정확도 | 🟡 검증 필요 | 의존성 추론 휴리스틱. 사용자 dry-run 검토 권장 |
| hunk 단위 스테이징 | 🔴 후순위 | `git apply --cached` 복잡도. 초기는 파일 단위 |
| changelog 파일 발견 | 🟢 결정됨 | 루트 + 패키지별. 파일 위치 기반 |
| git 의존 (시스템 git) | 🟢 필요 | `git` CLI 필수. libgit2 대안 검토 가능 |
| 커밋 실패 시 롤백 | 🟡 미결정 | stage는 유지, commit만 실패. changelog는 별도 |
| conventional 커밋 강제 | 🟢 결정됨 | validation으로 형식 검증. 위반 시 에러 |

---

## 7. 테스트 계획

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn scope_extraction_single_component() {
        let numstat = vec![
            NumstatEntry { path: "src/auth/login.rs".into(), additions: 50, deletions: 10 },
            NumstatEntry { path: "src/auth/logout.rs".into(), additions: 20, deletions: 5 },
        ];
        let candidates = extract_scope_candidates(&numstat);
        assert_eq!(candidates[0].name, "src/auth");
    }

    #[test]
    fn wide_change_detection() {
        let numstat = vec![
            NumstatEntry { path: "src/a.rs".into(), additions: 30, deletions: 0 },
            NumstatEntry { path: "src/b.rs".into(), additions: 30, deletions: 0 },
            NumstatEntry { path: "src/c.rs".into(), additions: 30, deletions: 0 },
        ];
        assert!(is_wide_change(&numstat));  // distinct_roots >= 3
    }

    #[test]
    fn topo_sort_no_cycle() {
        let mut groups = vec![
            CommitGroup { id: "a".into(), dependencies: vec![], .. },
            CommitGroup { id: "b".into(), dependencies: vec!["a".into()], .. },
            CommitGroup { id: "c".into(), dependencies: vec!["b".into()], .. },
        ];
        compute_dependency_order(&mut groups).unwrap();
        assert_eq!(groups[0].id, "a");
        assert_eq!(groups[1].id, "b");
        assert_eq!(groups[2].id, "c");
    }

    #[test]
    fn topo_sort_cycle_detected() {
        let mut groups = vec![
            CommitGroup { id: "a".into(), dependencies: vec!["b".into()], .. },
            CommitGroup { id: "b".into(), dependencies: vec!["a".into()], .. },
        ];
        assert!(compute_dependency_order(&mut groups).is_err());
    }

    #[test]
    fn message_format_with_scope() {
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Feat,
            scope: "auth".into(),
            details: vec![ConventionalDetail {
                text: "Add OAuth2 login flow.".into(),
                changelog_category: Some(ChangelogCategory::Added),
                user_visible: true,
            }],
            issue_refs: vec!["#42".into()],
        };
        let msg = format_commit_message(&analysis, "Add OAuth2 login");
        assert!(msg.starts_with("feat(auth): Add OAuth2 login"));
        assert!(msg.contains("- Add OAuth2 login flow."));
        assert!(msg.contains("Refs #42"));
    }

    #[test]
    fn validation_rejects_long_summary() {
        let long = "x".repeat(73);
        let errors = validate_summary(&long);
        assert!(errors.iter().any(|e| e.contains("72 characters")));
    }
}
```

---

## 8. 부록: omp → oxi 매핑

| omp 위치 | oxi 위치 |
|---|---|
| `commit/types.ts` (110) | `oxi-cli/src/commit/types.rs` |
| `commit/pipeline.ts` (220) | `oxi-cli/src/commit/mod.rs` |
| `commit/analysis/scope.ts` (210) | `oxi-cli/src/commit/scope.rs` |
| `commit/analysis/conventional.ts` (62) | `oxi-cli/src/commit/analysis.rs` |
| `commit/analysis/summary.ts` (95) | `oxi-cli/src/commit/summary.rs` |
| `commit/analysis/validation.ts` (60) | `oxi-cli/src/commit/validation.rs` |
| `commit/message.ts` (11) | `oxi-cli/src/commit/message.rs` |
| `commit/agentic/topo-sort.ts` (44) | `oxi-cli/src/commit/topo_sort.rs` |
| `commit/changelog/` (234) | `oxi-cli/src/commit/changelog.rs` |
| `commit/utils/exclusions.ts` (35) | `oxi-cli/src/commit/exclusions.rs` |
| `commit/git/diff.ts` (148) | `oxi-cli/src/commit/git.rs` |
| `utils/git.ts` (1,838) | `oxi-cli/src/commit/git.rs` (필요 부분만) |
| `commands/commit.ts` (63) | `oxi-cli/src/tui/slash/builtin/commit.rs` |
| `commit/agentic/` (전체) | 후순위 (N4.15) |
