# oxi TUI 아키텍처 리팩토링 설계

## 문제 진단

현재 구조의 핵심 문제: **오버레이가 늘어날 때마다 handlers.rs와 render.rs 양쪽에 match 분기가 산재**

```
handlers.rs: "어떤 오버레이?" → match → 키 처리 로직
render.rs:   "어떤 오버레이?" → match → 렌더 로직
app.rs:      AppOverlay enum + AppState에 모든 상태가 몰려있음
```

결과:
- 오버레이 추가 시 3개 파일 동시 수정
- handlers.rs 970줄, render.rs 582줄 — God object
- 각 오버레이의 상태/이벤트/렌더가 분산되어 파악 어려움
- ratatui의 StatefulWidget 철학(상태와 위젯의 쌍)과 맞지 않음

## 목표 아키텍처: Component 패턴

ratatui는 StatefulWidget에서 **상태(State)와 위젯(Widget)의 쌍**을 기본 단위로 삼는다.
이를 오버레이 레벨로 확장하면 — 각 오버레이가 **자신의 상태, 이벤트 처리, 렌더링을 하나의 단위로 캡슐화**하는 구조가 된다.

```
┌─────────────────────────────────────────────────┐
│  App                                             │
│  ├── ChatView (StatefulWidget)                   │
│  ├── Input (StatefulWidget)                      │
│  ├── Footer (StatefulWidget)                     │
│  └── Overlay: dyn OverlayComponent               │
│       ├── ModelSelect { state, render, handle }  │
│       ├── ResumeTable { state, render, handle }  │
│       ├── SettingsPanel { state, render, handle } │
│       └── SetupWizard { state, render, handle }  │
└─────────────────────────────────────────────────┘
```

## Component trait

```rust
/// 오버레이의 공통 인터페이스.
/// 각 오버레이는 이 trait을 구현해서 자신의 이벤트+렌더를 캡슐화.
trait OverlayComponent {
    /// 키 입력 처리. 액션이 필요하면 반환.
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut AppContext) -> Option<OverlayAction>;
    
    /// 렌더링.
    fn render(&self, f: &mut Frame, area: Rect, theme: &Theme);
    
    /// 하단 힌트 텍스트.
    fn hint(&self) -> &str;
}
```

### AppContext
```rust
/// 오버레이가 App 상태에 접근할 때 사용 (필요한 것만 노출)
struct AppContext<'a> {
    pub session: &'a AgentSession,
    pub add_system_message: &'a mut dyn FnMut(String),
    pub set_model: &'a mut dyn FnMut(String) -> Result<()>,
    pub switch_session: &'a mut dyn FnMut(String),
    // ...
}
```

### OverlayAction
```rust
enum OverlayAction {
    None,
    Close,
    SendPrompt(String),
    SwitchSession(String),
    NewSession,
    ExecuteSlashCommand(String),
}
```

## 각 컴포넌트 설계

### 1. ChatView — 스크롤바 추가

`oxi-tui/src/widgets/chat.rs` 내부 수정만으로 충분.
별도 오버레이가 아니므로 Component trait과 무관.

**변경**: ScrollView 우측에 `ratatui::widgets::Scrollbar` 렌더링 추가.
현재 1칸을 이미 예약해두었으니 그 자리에 스크롤바 그으면 됨.

```
난이도: ★☆☆
독립성: 완전 독립 (다른 작업과 무관)
```

### 2. ResumeTable — 세션 목록 Table

**위치**: `oxi-tui/src/widgets/session_table.rs` (새 파일)

```rust
pub struct SessionTableState {
    pub sessions: Vec<SessionInfo>,
    pub selected: usize,
    table_state: TableState,
}

pub struct SessionTable<'a> {
    theme: &'a Theme,
}

impl StatefulWidget for SessionTable<'_> {
    type State = SessionTableState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State);
}
```

**열 구성** (Table 위젯):
| Name | Msgs | Preview | Time |
| `name` 또는 `id[:8]` | `message_count` | `first_message` 잘림 | 상대시간 |

**핸들러**: Up/Down → `table_state.select_prev/next()`, Enter → 선택.

### 3. SettingsPanel — 탭 패널

**위치**: `oxi-tui/src/widgets/settings_panel.rs` (새 파일)

```rust
pub enum SettingsTab { Model, Tools, Extensions, Auth }

pub struct SettingsPanelState {
    pub tab: SettingsTab,
    pub model: ModelTabState,
    pub tools: ToolsTabState,
    pub extensions: ExtensionsTabState,
    pub auth: AuthTabState,
}

pub struct SettingsPanel<'a> {
    theme: &'a Theme,
}

impl StatefulWidget for SettingsPanel<'_> {
    type State = SettingsPanelState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State);
}
```

각 탭 상태:
```rust
struct ModelTabState {
    filter: String,
    models: Vec<String>,
    selected: usize,
}

struct ToolsTabState {
    tools: Vec<ToolInfo>,  // name, enabled, essential, description
    selected: usize,
}

struct ExtensionsTabState {
    entries: Vec<ExtInfo>,
    selected: usize,
}

struct AuthTabState {
    providers: Vec<(String, bool)>,  // (name, has_key)
    selected: usize,
}
```

**핸들러**:
- `←/→`: 탭 전환
- `↑/↓`: 탭 내 선택
- `Enter`: 탭별 액션 (모델 전환, 툴 토글, auth 설정)
- `Esc`: 닫기
- 탭별로 자체 handle_key 구현

## 파일 구조 변경

```
oxi-tui/src/widgets/
├── mod.rs              # pub mod 추가
├── input.rs            # ✅ 완료 (ratatui-textarea)
├── chat.rs             # ★ 스크롤바 추가
├── footer.rs           # 그대로
├── session_table.rs    # ★ 새 파일
└── settings_panel.rs   # ★ 새 파일

oxi-cli/src/tui/
├── mod.rs
├── app.rs              # AppState 단순화, OverlayComponent enum
├── handlers.rs          #大幅 축소 (공통 키 처리만)
├── render.rs            #大幅 축소 (오버레이 디스패치만)
├── slash.rs             # 라우팅만 변경
└── overlay/             # ★ 새 디렉토리
    ├── mod.rs           # OverlayComponent trait + AppContext
    ├── model_select.rs  # 모델 선택 오버레이
    ├── resume.rs        # 세션 Resume 오버레이
    ├── settings.rs      # 설정 패널 오버레이
    └── setup.rs         # 초기 설정 위자드
```

## Before → After 비교

### Before (현재)
```rust
// handlers.rs — 오버레이별 키 처리가 한 파일에 몰려있음
async fn handle_overlay_key(key, state, session) {
    match &overlay {
        Some(AppOverlay::Setup(step)) => { /* 50줄 */ }
        Some(AppOverlay::ModelSelect { .. }) => { /* 30줄 */ }
        Some(AppOverlay::LogoutSelect { .. }) => { /* 20줄 */ }
        Some(AppOverlay::ResumeSelect { .. }) => { /* 20줄 */ }
        Some(AppOverlay::SettingsPanel { .. }) => { /* 40줄 */ }  // ← 추가될 때마다 늘어남
    }
}

// render.rs — 오버레이별 렌더링이 한 파일에 몰려있음
fn render_overlay(f, area, state, theme) {
    match &state.overlay {
        Some(AppOverlay::Setup(..)) => { /* 30줄 */ }
        Some(AppOverlay::ModelSelect { .. }) => { /* 25줄 */ }
        // ... 계속 늘어남
    }
}
```

### After
```rust
// handlers.rs — 간단한 디스패치만
async fn handle_overlay_key(key, state, session) -> Option<Action> {
    let action = state.overlay.handle_key(key, &mut ctx);
    match action {
        OverlayAction::Close => state.overlay = None,
        OverlayAction::SendPrompt(msg) => return Some(Action::SendPrompt(msg)),
        // ...
    }
    None
}

// render.rs — 간단한 디스패치만
fn render_overlay(f, area, state, theme) {
    state.overlay.render(f, area, theme);
}
```

각 오버레이의 복잡도는 자신의 파일 안에 캡슐화됨.

## 구현 순서

1. **스크롤바** (chat.rs만 수정, 즉시 효과)
2. **OverlayComponent trait + overlay/ 디렉토리** (인프라)
3. **기존 오버레이 마이그레이션** (ModelSelect → ResumeSelect → Setup)
4. **SessionTable 위젯** (oxi-tui에 추가)
5. **Resume 오버레이를 SessionTable로 교체**
6. **SettingsPanel 위젯** (oxi-tui에 추가)
7. **Settings 오버레이 구현 + slash.rs 라우팅 변경**
