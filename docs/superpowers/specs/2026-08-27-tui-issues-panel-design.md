# TUI 이슈 패널 (Issues Panel) 설계

**날짜**: 2026-08-27
**상태**: 설계 승인 완료 (사용자 승인 2026-08-27)
**범위**: `oxicode-cli` 단일 크레이트. 신규 크레이트 없음. `oxicode-vtui`/`oxicode-textarea` 위젯 코드 변경 없음(재사용만).

---

## 0. 배경

`.oxicode/issues/` 백엔드(파일당 이슈 1개, YAML frontmatter + 마크다운 본문, CAS 기반 동시성 제어, 세션 liveness 소유권)는 이미 완성되어 있다:

- `oxicode-cli/src/store/issues/`: `types.rs`(`Issue`/`IssueMeta`/`IssuePatch`/`Status`/`Priority`/`Assignment`/`GithubRef`), `store.rs`(`FileIssueStore`), `liveness.rs`(`TUI_OWNERSHIP_ID`/`is_session_alive`/`reap_orphans`), `filter.rs`(`IssueFilter`), `serialize.rs`, `error.rs`(`IssueError`).
- `oxicode-cli/src/tools/issue_tool.rs`: 에이전트 툴 (`list/read/create/update/start/release/close/reopen/link_session`, `cas_retry` 4회 재시도).
- `oxicode-cli/src/cli/commands/issue.rs` + `cli.rs::IssueCommands`: `oxicode issue list/show/new/close/reopen/reap`.

**빠진 것은 TUI 패널 하나뿐.** `oxicode-tui`(구 위젯 라이브러리, `tui/overlay/issues_panel/`)가 `oxicode-vtui`/`tui_vt` 마이그레이션 과정에서 삭제되며 함께 유실됐다. `tui_vt/slash/registry.rs`에 `"/issue is not yet wired (no issue overlay in this harness)"`라고 명시되어 있다. `docs/designs/2026-07-20-grok-pager-redesign.md`(미승인·미구현, `oxicode-pager`/`oxicode-tui` 기반이라 현재 아키텍처와 무관)와 `docs/designs/2026-06-17-tui-issues-deferred.md`(필터/undo/마크다운 렌더링 개선안, 구현 이력 없음)는 참고만 하고 코드는 재사용하지 않는다(구 크레이트와 함께 삭제됨).

본 설계는 **스키마·백엔드 변경 없이** TUI 패널만 새로 만든다.

---

## 1. 범위 / 비범위

**범위**:
- `.oxicode/issues/` 백엔드를 그대로 소비하는 전체화면 오버레이 패널.
- 목록(필터 포함) · 상세 · 생성/수정 폼을 패널 안에서 완결(완전한 CRUD).
- 진입점: `/issue` 슬래시 명령 (+ `Ctrl+P` 커맨드 팔레트 자동 노출).
- 다른 세션의 claim(assignee) 표시(배지), 읽기 전용.

**비범위**:
- 전용 단축키 추가 없음.
- 스키마 변경 없음(`archived` 필드 등 추가 안 함) — close/reopen 기존 액션으로 충분.
- 사용자가 패널에서 `start`/`release`(claim) 직접 조작 — 표시만, 액션 없음.
- GitHub 동기화(Phase 6, 미착수) — `IssueMeta.github` 필드 손대지 않음.
- print/RPC 모드 — TUI 전용.

---

## 2. 데이터 모델 & 백엔드 재사용

스키마 변경 없음. 기존 타입 그대로:

```rust
// 재사용, 변경 없음
crate::store::issues::{
    FileIssueStore, Issue, IssueMeta, IssuePatch, IssueError,
    IssueFilter, Priority, Status, Assignment,
};
crate::store::issues::liveness::is_session_alive;
```

- **동기 읽기** (`FileIssueStore::list`, `read`, `summary`, `next_id`, `create`) — input 스레드에서 직접 호출. 로컬 디스크 I/O + 인메모리 캐시라 블로킹 허용 범위.
- **비동기 쓰기 + CAS** (`apply_patch`, `start`, `release`, `close`, `reopen`, `link_session`) — input 스레드는 `InlineEvent::IssueAction(IssueAction)`을 만들어 `evt_tx`로 async 메인 루프에 전달. 메인 루프가 `tokio::spawn`으로 실행하고, 완료 후 `state.lock()`으로 결과를 반영(기존 oauth/resume 스폰과 동일 패턴, `main_loop.rs:2453`/`2672`/`3166` 참고).
- **CAS 재시도**: `issue_tool.rs`의 `cas_retry` 헬퍼를 `pub(crate)`로 노출해 패널에서도 그대로 호출 — 중복 구현 금지.

---

## 3. 진입점

- `/issue` 슬래시 명령 신규 등록. 파일: `oxicode-cli/src/tui_vt/slash/commands.rs`(`register_extra()`). 인자 없이 호출 시 패널을 List 모드로 오픈.
- 전용 단축키 없음. `SlashRegistry::builtin_commands()`에 등록되면 `Ctrl+P` 커맨드 팔레트(`build_command_palette`, `main_loop.rs:4220`)에 자동 노출되어 별도 배선 불필요.

---

## 4. 상태 구조

`RenderState`(`tui_vt/main_loop.rs`)에 필드 추가:

```rust
pub issues_panel: Option<IssuesPanelState>,
```

신규 타입 (`tui_vt/issues_panel/state.rs`):

```rust
pub struct IssuesPanelState {
    pub mode: IssuesPanelMode,
    pub status_filter: Status,       // 기본 Open, 'f'로 토글
    pub extra_filter: IssueFilter,   // priority/label/text, '/' 모달에서 설정
    pub rows: Vec<IssueRow>,         // refresh() 시점에만 재계산
    pub selected: usize,
    pub pending: bool,               // 비동기 뮤테이션 진행중 — 입력 잠금 + 스피너
    pub error: Option<String>,       // 마지막 실패, 다음 성공 액션에 자동 클리어
}

pub enum IssuesPanelMode {
    List,
    Detail { id: u32, scroll: usize },
    Form(IssueFormState),
    FilterInput(String),             // 자유 텍스트 버퍼, Enter 시 파싱
}

pub struct IssueRow {
    pub id: u32,
    pub title: String,
    pub status: Status,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub assignee_badge: Option<AssigneeBadge>,  // refresh 시 is_session_alive 1회 호출, 렌더마다 재확인 안 함
}

pub enum AssigneeBadge { Live(String), Stale(String) }

pub struct IssueFormState {
    pub editing_id: Option<u32>,     // None=생성, Some=수정
    pub content_hash: Option<String>, // 수정 시 CAS용
    pub title: String,
    pub priority: Priority,
    pub labels_input: String,        // 콤마 구분 원문
    pub body: oxicode_textarea::TextArea,
    pub focus: FormField,            // Title | Priority | Labels | Body
}
```

**입력 게이팅**: 기존 `handle_overlay_key`/`handle_confirmation_key`/`handle_file_search_key`(모두 `spawn_input_thread` 초입, `main_loop.rs:3346-3390` 근처)와 같은 위치에 `handle_issues_panel_key(state, evt_tx, code)` 분기를 추가. `s.issues_panel.is_some()`이면 다른 키 핸들러보다 먼저 소비.

**close 확인**: 신규 확인 모달을 만들지 않고 기존 `ModalConfirmation` + `ConfirmationAction`(AGENTS.md에 문서화된 `/clear --yes` 재디스패치 패턴) 재사용 — `ConfirmationAction`에 `CloseIssue(u32)` variant 추가.

---

## 5. UI / 키맵

전체화면 오버레이(Model/Theme 피커와 동일한 급의 화면 점유). `Esc`는 한 단계씩 뒤로: Form/Detail/FilterInput → List → 패널 닫기(`issues_panel = None`).

### List
한 줄 포맷: `#{id} [{priority}] {title}  {status}  {labels}  {assignee_badge}`

| 키 | 동작 |
|---|---|
| `j`/`k`, `↑`/`↓` | 선택 이동 |
| `Enter` | 상세로 진입 |
| `n` | 새 이슈 폼 (`IssuesPanelMode::Form(IssueFormState::default())`) |
| `e` | 선택 이슈 수정 폼 (읽어서 폼 필드 채움, `content_hash` 보관) |
| `c` | close — `ModalConfirmation` 경유, 확인 시 async `close` 디스패치 |
| `r` | reopen — 확인 없이 즉시 async `reopen` 디스패치 (파괴적이지 않음) |
| `f` | `status_filter` Open↔All 토글, `refresh()` |
| `/` | `IssuesPanelMode::FilterInput` 진입 |
| `Esc` | 패널 닫기 |

### FilterInput (`/` 진입)
자유 텍스트 한 줄. 힌트: `priority=critical label=auth text` (스페이스 구분 `key=value` 토큰, `text`는 나머지 전부). 신규 파서 `parse_issue_filter(input: &str) -> IssueFilter` 작성(구 `parse_new_opts`는 삭제된 `tui/slash.rs`와 함께 사라졌으므로 **로직만 참고해 새로 구현**, 코드 재사용 없음). `Enter` 적용 후 List로 복귀 + `refresh()`. `Esc` 취소(기존 `extra_filter` 유지). `Ctrl+U` 버퍼 클리어.

### Detail
메타 헤더(`id`/`status`/`priority`/`labels`/`assignee_badge`/`created_at`/`updated_at`/`closed_at`) + `oxicode_vtui::tui::ui::markdown::render_markdown`으로 렌더링한 본문, 스크롤 가능(`PageUp`/`PageDown`, `j`/`k`). `e`/`c`/`r` List와 동일 동작(대상은 현재 열린 이슈). `Esc` → List.

### Form (생성/수정 공용, 단일 화면)
- 제목: 한 줄 입력(개행 불가 텍스트 필드).
- 우선순위: `←`/`→`로 `Low ↔ Medium ↔ High ↔ Critical` 순환(양끝 wrap).
- 라벨: 콤마 구분 한 줄 텍스트(`labels_input`), 제출 시 `split(',').map(trim).filter(non-empty)`.
- 본문: `oxicode_textarea::TextArea` 재사용(컴포저와 동일 위젯 — CJK/이모지 캐럿, undo/redo 그대로 상속).
- `Tab`/`Shift+Tab`: 필드 순환 이동.
- `Ctrl+Enter`: 제출. 생성이면 동기 `store.create(...)` 즉시 반영 + List 복귀. 수정이면 `IssuePatch` 구성 후 async `apply_patch`(`pending=true`, 완료 시 반영).
- `Esc`: 취소, List로 복귀(변경 버림).

---

## 6. 에러 처리

`IssueError`의 `Display` 문자열을 그대로 `IssuesPanelState.error`에 저장, 패널 하단 한 줄에 표시. 다음 성공 액션에서 자동 클리어.

- `Conflict`: `apply_patch`는 이미 `cas_retry`(4회)로 자동 재조회 재시도하므로, 이 에러가 패널까지 올라오는 경우는 4회 모두 실패한 드문 경합 — 에러 표시 후 사용자가 수동 재시도(폼 재오픈).
- `Assigned`/`NotAssigned`: 다른 살아있는 세션이 소유 중. `e`(edit) 진입 자체를 막고 배지 강조("이 이슈는 세션 X가 작업 중")로 안내 — 폼을 열어놓고 제출 시점에야 실패시키지 않는다.
- `NotFound`: 목록이 stale한 상태에서 다른 프로세스가 파일을 지운 극단적 경우 — 에러 표시 후 `refresh()`.
- `Io`/`Other`: 그대로 표시.

---

## 7. 테스트

- `parse_issue_filter`: 단위 테스트 — 빈 입력, `priority=` 오탈자, `label=`만, `text` only, 복합.
- Form → `IssuePatch`/`create` 인자 매핑: 단위 테스트 — 라벨 콤마 분리/trim/빈 라벨 필터링, 우선순위 순환 양끝 wrap.
- 렌더: 기존 `render_frame` 테스트 패턴(`RenderState { issues_panel: Some(..), ..Default::default() }` + 테스트 백엔드, `main_loop.rs` 하단 기존 스냅샷 테스트들과 동일 스타일) — List/Detail/Form 세 모드가 패닉 없이 렌더되는지.
- `cas_retry` 통합: tempdir 기반 `FileIssueStore`로 두 태스크가 동시에 `apply_patch` 호출 → 한쪽만 성공하고 다른 쪽은 재시도 후 성공(이미 `issue_tool.rs`에 유사 테스트 있음 — 패널 쪽은 신규 호출 경로만 스모크 테스트).
- 회귀: `Assigned` 상태에서 `e` 키가 폼 진입을 막는지, `close` 확인 모달이 실제로 뜨는지, `reopen`이 확인 없이 즉시 실행되는지.

---

## 8. 영향받는 파일 (구현 단계 참고용)

| 파일 | 변경 |
|---|---|
| `oxicode-cli/src/tui_vt/issues_panel/mod.rs` (신규) | `IssuesPanelState`/`IssuesPanelMode`/`IssueRow`/`IssueFormState` 등 타입 |
| `oxicode-cli/src/tui_vt/issues_panel/render.rs` (신규) | List/Detail/Form 렌더 함수 |
| `oxicode-cli/src/tui_vt/issues_panel/input.rs` (신규) | `handle_issues_panel_key` |
| `oxicode-cli/src/tui_vt/issues_panel/filter_parse.rs` (신규) | `parse_issue_filter` |
| `oxicode-cli/src/tui_vt/main_loop.rs` | `RenderState.issues_panel` 필드, `render_frame` 분기, 키 게이팅 훅, `ConfirmationAction::CloseIssue`, async 디스패치 지점 |
| `oxicode-cli/src/tui_vt/slash/commands.rs` | `/issue` 슬래시 명령 등록 |
| `oxicode-cli/src/tools/issue_tool.rs` | `cas_retry`를 `pub(crate)`로 가시성 확대(로직 변경 없음) |

---

## 9. 검증

```bash
cargo fmt --all -- --check
cargo clippy -p oxicode-cli --all-targets -- -D warnings
cargo nextest run -p oxicode-cli
```
