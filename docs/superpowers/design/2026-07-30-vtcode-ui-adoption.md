# 2026-07-30 — vtcode-ui 채택: oxicode TUI 재구축 설계

## 0. 결정

**Vendoring (Apache 2.0 → MIT 호환) vtcode-ui 소스를 `oxicode-vtui` 크레이트로 들인다.** 통째로 한 라이브러리 dep 이 아님 — vtcode-ui 는 `vtcode-config` + `vtcode-commons` 의존성이 oxicode 와 충돌하므로, 해당 import 지점만 스텁 크레이트로 대체하는 가벼운 vendoring. vtcode-ui 의 MIT 라이선스는 oxicode 의 MIT 와 충돌 없음 (LICENSE 확인 결과 MIT 맞음 — 원래 scout 보고가 Apache 2.0 이라더니 MIT 임이 실제 확인).

**핵심 가져오는 것:**
- `design/` — color 변환, layout, diff, panel primitives
- `theme/` — 40+ theme registry, runtime theme swap, syntax theme binding
- `tui/` — core_tui (InlineSession/InlineCommand/InlineEvent 프로토콜 + render loop + 위젯 트리)

**버리는 것 (oxicode 쪽):**
- `oxicode-tui` 크레이트 전체 (~10K LOC). `oxicode-hashline` 만 살아남음 (독립, 따로 존재).
- `oxicode-cli/src/tui/` (~13K LOC) → `oxicode-cli/src/tui_vt/` 로 대체.

---

## 1. 의존성 전략

### 1.1 vtcode-ui 의 vtcode-config 사용처 5군데

| 위치 | 임포트 항목 | 스텁 난이도 |
|---|---|---|
| `design/color.rs:102` | `vtcode_config::constants::ui::agent_mode_hue` | 단일 fn. oxicode 는 `|_| None` OK |
| `theme/color_math.rs:2` | `vtcode_config::constants::ui` | 상수 1개 |
| `theme/runtime.rs:5` | `vtcode_config::constants::ui` | 상수 set |
| `theme/types.rs:2` | `vtcode_config::constants::{defaults, ui}` | 2개 상수 모듈 |
| `tui/config/mod.rs:66` | `vtcode_config::core::tools::ToolPolicy` | 단일 enum |
| `tui/config/types.rs:10` | `vtcode_config::types::{SystemPromptMode, ToolDocumentationMode, VerbosityLevel}` | 3개 enum |
| `tui/config/constants/ui.rs` | 18개 상수 | 전부 스트링 리터럴로 교체 가능 |
| `tui/core_tui/style.rs:96` | 테스트에서만 사용 | 테스트 스킵 가능 |
| `tui/core_tui/session/styling.rs:3` | `vtcode_config::constants::tools` | 상수 set |
| `tui/core_tui/widgets/transcript.rs:12` | `vtcode_config::constants::tools` | 상수 set |

**전략**: `crates/oxicode-vtui-compat/` 크레이트 생성. 위 10군데에서 요구하는 모든 심볼의 스텁 제공. 실제 값은 oxicode 기본값으로.

### 1.2 vtcode-ui 의 vtcode-commons 사용처

vtcode-ui 는 `vtcode::commons::ui_protocol` 에서 10개 타입만 import:
`InlineMessageKind`, `SlashCommandItem`, `InlineListSearchConfig`, `SecurePromptConfig`, `SessionSurface`, `KeyboardProtocolSettings`, `UiMode`, `LayoutModeOverride`, `ReasoningDisplayMode`, `ThinkingBlockState`, `PlanStep`, `PlanPhase`, `PlanContent`

vtcode-commons 는 가벼워서 dep 로 남길 수도 있지만, 우리 workspace 에 없는 crate. vendoring or re-export 함.

**전략**: `oxicode-vtui-compat` 안에 `ui_protocol` 모듈로 위 10개 타입 복제 (단순 데이터 struct/enum, std only).

### 1.3 새 크레이트 트리

```
crates/
├── oxicode-vtui/                   ← vendored vtcode-ui (design/ + theme/ + tui/)
│   ├── Cargo.toml              ← 의존성: ratatui 0.30, crossterm, syntect, pulldown-cmark, ...
│   ├── src/
│   │   ├── lib.rs
│   │   ├── design/
│   │   ├── theme/
│   │   └── tui/
│   └── LICENSE                 ← MIT, vtcode-ui 원본
├── oxicode-vtui-compat/            ← 스텁 crate
│   ├── Cargo.toml              ← 의존성: anstyle, hashbrown (vtcode-config가 요구하는 것만)
│   └── src/
│       ├── lib.rs
│       ├── constants/
│       │   ├── ui.rs           ← vtcode_config::constants::ui
│       │   ├── defaults.rs     ← vtcode_config::constants::defaults
│       │   └── tools.rs        ← vtcode_config::constants::tools
│       ├── types.rs            ← ToolPolicy, SystemPromptMode, ...
│       └── ui_protocol/        ← vtcode_commons::ui_protocol (10 types)
├── oxicode-hashline/               ← 유지, 무변경
```

oxicode-cli 의 의존성:
```toml
oxicode-vtui = { path = "../oxicode-vtui" }
oxicode-vtui-compat = { path = "../oxicode-vtui-compat" }
```

oxicode-vtui 의 의존성:
```toml
oxicode-vtui-compat = { path = "../oxicode-vtui-compat" }
ratatui = "0.30"
crossterm = "..."
syntect = "..."
pulldown-cmark = "0.13"
# ... (vtcode-ui Cargo.toml 그대로, vtcode-config, vtcode-commons만 교체)
```

---

## 2. 통합 아키텍처

### 2.1 HostAdapter 구현

```rust
// oxicode-cli/src/tui_vt/host.rs

use oxicode_vtui::tui::host::{HostAdapter, WorkspaceInfoProvider, NotificationProvider, ThemeProvider};
use oxicode_vtui::tui::host::HostSessionDefaults;

pub struct OxicodeHostAdapter {
    app: Arc<crate::App>,
    settings: Arc<Settings>,
}

impl WorkspaceInfoProvider for OxicodeHostAdapter {
    fn workspace_name(&self) -> String {
        // CWD의 basename
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "oxicode".into())
    }

    fn workspace_root(&self) -> Option<std::path::PathBuf> {
        std::env::current_dir().ok()
    }
}

impl NotificationProvider for OxicodeHostAdapter {
    fn set_terminal_focused(&self, focused: bool) {
        // Optionally: signal the agent that terminal focus changed
        // (oxicode does not currently use this, noop OK)
    }
}

impl ThemeProvider for OxicodeHostAdapter {
    fn available_themes(&self) -> Vec<String> {
        oxicode_vtui::theme::registry::all_theme_ids()
    }

    fn active_theme_name(&self) -> Option<String> {
        self.settings.theme.as_ref().map(|t| t.name.clone())
    }
}

impl HostAdapter for OxicodeHostAdapter {
    fn app_name(&self) -> String {
        "oxicode".into()
    }

    fn session_defaults(&self) -> HostSessionDefaults {
        HostSessionDefaults::default()
    }
}
```

### 2.2 메인 이벤트 루프

기존 `oxicode-cli/src/tui/app.rs:931` `run_tui_interactive_impl()` 대체:

```rust
// oxicode-cli/src/tui_vt/main_loop.rs

pub async fn run_tui(app: crate::App) -> Result<()> {
    let host = Arc::new(OxicodeHostAdapter::new(&app));

    // 1. 테마 결정
    let theme = resolve_theme(app.settings());

    // 2. 세션 스폰
    let options = CoreSessionOptions {
        app_name: "oxicode".into(),
        workspace_root: std::env::current_dir().ok(),
        ..host.session_defaults().into()
    };
    let mut session = spawn_core_session(theme, options)?;
    let handle = session.handle;

    // 3. AgentSession 스폰 (기존 AgentSession wrapping 유지)
    let agent_session = create_agent_session(&app);
    let mut session_rx = agent_session.subscribe();

    // 4. 초기 상태 push
    handle.set_prompt("> ".into(), prompt_style());
    handle.set_header_context(build_header(&app));

    // 5. Main loop
    let result = run_event_loop(&mut session, &mut session_rx, &handle, &agent_session).await;

    // 6. Cleanup
    handle.shutdown();
    result
}
```

### 2.3 이벤트 루프 (biased tokio::select!)

```rust
async fn run_event_loop(
    session: &mut InlineSession,
    session_rx: &mut UnboundedReceiver<SessionEvent>,
    handle: &InlineHandle,
    agent: &AgentSession,
) -> Result<()> {
    loop {
        tokio::select! {
            // TUI → Host 이벤트 (사용자 입력)
            Some(event) = session.next_event() => {
                match event {
                    InlineEvent::Submit(input) => {
                        agent.submit(input.text);
                        handle.set_input_enabled(false);
                        handle.set_input_status(Some("Thinking...".into()), None);
                    }
                    InlineEvent::Interrupt => {
                        agent.interrupt();
                    }
                    InlineEvent::Cancel => {
                        agent.cancel();
                    }
                    InlineEvent::CyclePrimaryAgent => {
                        agent.cycle_model_forward();
                        handle.set_header_context(build_header_from_agent(agent));
                    }
                    InlineEvent::OpenFileInEditor(path) => {
                        // spawn EDITOR
                    }
                    InlineEvent::ToggleToolDisplayMode => {
                        // toggle tool output collapse
                    }
                    InlineEvent::ScrollPageUp | InlineEvent::ScrollPageDown => {
                        // handled by TUI internally
                    }
                    InlineEvent::Exit => break,
                    // ... 나머지
                }
            }

            // Agent → TUI 이벤트 (AgentEvent streaming)
            Some(session_event) = session_rx.recv() => {
                match session_event {
                    SessionEvent::Agent(agent_event) => {
                        match *agent_event {
                            AgentEvent::TokenDelta { delta, .. } => {
                                handle.inline(InlineMessageKind::Agent, segment(&delta));
                            }
                            AgentEvent::ToolCall { name, id, .. } => {
                                handle.set_header_context(header_with_stage(&format!("Running {}", name)));
                                handle.append_line(
                                    InlineMessageKind::Tool,
                                    vec![segment(&format!("⚙ {}", name))]
                                );
                            }
                            AgentEvent::ToolResult { content, .. } => {
                                handle.append_line(
                                    InlineMessageKind::Tool,
                                    vec![segment(&content)]
                                );
                            }
                            AgentEvent::ResponseEnd => {
                                handle.set_input_enabled(true);
                                handle.set_input_status(None, None);
                            }
                            AgentEvent::Error { message, .. } => {
                                handle.append_line(InlineMessageKind::Error, vec![segment(&message)]);
                                handle.set_input_enabled(true);
                            }
                        }
                    }
                    SessionEvent::CompactionStart { reason } => {
                        handle.set_header_context(header_with_stage(&format!("Compacting ({:?})", reason)));
                    }
                    SessionEvent::CompactionEnd { .. } => {
                        handle.set_header_context(build_header_from_agent(agent));
                    }
                    SessionEvent::QueueUpdate { .. } => {
                        // update queued input display
                    }
                    SessionEvent::Advisor { body, .. } => {
                        handle.append_line(InlineMessageKind::Info, vec![segment(&body)]);
                    }
                    _ => {}
                }
            }

            // SIGINT (Ctrl+C twice)
            _ = signal(SignalKind::interrupt()) => {
                agent.interrupt();
            }
        }
    }
    Ok(())
}
```

### 2.4 InlineCommand 매핑 전체

| oxicode AgentEvent | InlineCommand 호출 |
|---|---|
| `TokenDelta` | `handle.inline(Agent, segment)` |
| `ToolCall(start)` | `handle.append_line(Tool, name)` + `handle.set_header_context(stage)` |
| `ToolResult(streaming)` | `handle.inline(Tool, segment)` → `handle.replace_last(N, Tool, lines)` |
| `ToolResult(end)` | `handle.append_line(Tool, result)` |
| `ResponseEnd` | `handle.set_input_enabled(true)` + `handle.set_input_status(None, None)` |
| `Error` | `handle.append_line(Error, msg)` |
| `CompactionStart` | `handle.set_header_context(stage)` |
| `CompactionEnd` | `handle.set_header_context(restore)` |
| `ThinkingLevelChanged` | `handle.set_header_context(update_badge)` |
| `QueueUpdate` | `handle.set_queued_inputs(entries)` |
| `Advisor` | `handle.append_line(Info, body)` |
| 모델 순환 | `handle.set_header_context(new_model)` |

| InlineEvent (사용자 입력) | oxicode AgentSession 동작 |
|---|---|
| `Submit` | `agent.submit(text)` → streaming 시작 |
| `Interrupt` | `agent.interrupt()` |
| `Cancel` | `agent.cancel()` |
| `Exit` | session 종료 |
| `CyclePrimaryAgent` | `agent.cycle_model_forward()` |
| `CyclePrimaryAgentPrevious` | `agent.cycle_model_backward()` |
| `OpenFileInEditor` | `std::process::Command::new(EDITOR)` |
| `LaunchEditor { draft }` | 임시 파일에 write + EDITOR |
| `Scroll*` | TUI 내부 처리 (no host action) |
| `Overlay(_)` | TUI 내부 처리 |
| `ProcessLatestQueued` | `agent.process_queue()` |
| `EditQueue` | `agent.edit_queue()` |
| `RequestInlinePromptSuggestion` | `agent.suggest_prompt()` |

### 2.5 원하지 않는 vtcode-ui 기능 컷

| 모듈 | 유지/컷 | 이유 |
|---|---|---|
| `tui/vim/` | ❌ 컷 | oxicode 는 vim 모드 없음 |
| `tui/core_tui/app/` 일부 | ❌ 컷 | diffs, agent palette, transcript review 등 oxicode 와 필요 없는 것 |
| `tui/core_tui/session/modal/` | 일부 컷 | Wizard/secure prompt 등 oxicode 에서 사용 안 하는 modal variant |
| `tui/core_tui/session/file_palette/` | ❌ 컷 | oxicode 가 자체 file picker overlay 갖고 있음 |
| `tui/ui/interactive_list/` | ✅ 유지 | 검색+선택 UI 기본기 |
| `tui/ui/syntax_highlight.rs` | ✅ 유지 | syntect wrapping |
| `tui/ui/search.rs` | ✅ 유지 | search overlay |
| `theme/` 전체 | ✅ 유지 | 40+ themes registry |
| `design/` 전체 | ✅ 유지 | color/layout/panel/diff |
| `tui/core_tui/runner/` | ✅ 유지 | TUI event loop engine |
| `tui/core_tui/widgets/` | 일부 컷 | transcript 만 유지, tool-specific widget 은 oxicode 쪽 |

---

## 3. 테마 마이그레이션

### 3.1 기존 oxicode-tui: 28 color slots × 6 schemes + GlyphSet (Unicode/Ascii/Nerd)

→ **전부 폐기**. vtcode-ui 의 40+ theme registry (`ThemeDefinition` + `ThemePalette` 기반) 로 대체.

### 3.2 손실 및 회복

| oxicode-tui 특징 | vtcode-ui status | 회복 방안 |
|---|---|---|
| GlyphSet (Unicode/Ascii/Nerd) | ❌ 없음. vtcode-ui 는 유니코드 기호 하드코딩 | vtcode-ui 의 symbol 상수들 — 현재 대부분 Unicode. Nerd 폰트/Ascii 모드는 추후 필요 시 Settings toggle + vtcode-ui 내 상수 치환 |
| 28 color slots (response_bg, thinking_bg, surface_bg, …) | ❌ 없음. vtcode-ui ThemePalette 는 8개 정도 + primary_accent/secondary_accent 체계 | ThemePalette 확장 — oxicode fork 에서 ThemePalette 필드 추가, theme_from_styles 가 읽도록 확장 |
| Theme hot-reload (FileWatcher) | ❌ 없음 (runtime swap 은 있음) | vtcode-ui runtime.rs 의 `set_active_theme()` 를 FileWatcher 에서 호출. oxicode 쪽에서 추가 |
| 5 built-in + user TOML/JSON theme | ✅ 40+ built-in themes | 추가 theme 도 ThemeDefinition 으로 등록 가능 |
| Dark/light auto-follow | ❌ 없음 (vtcode-ui 단일 테마) | oxicode 의 Lazy init 에서 `OSC 10/11` 읽어 auto-determine (Codex 의 terminal_palette.rs 차용) |

### 3.3 oxicode 전용 theme 등록 예

```rust
// oxicode-vtui/src/theme/oxicode_themes.rs

pub fn register_oxicode_themes(registry: &mut HashMap<&str, ThemeDefinition>) {
    registry.insert("oxicode-dark", ThemeDefinition {
        id: "oxicode-dark",
        label: "oxicode Dark",
        palette: ThemePalette {
            primary_accent:  RgbColor(0x88, 0xC0, 0xD0),  // nord blue
            secondary_accent: RgbColor(0xBF, 0x61, 0x6A), // nord red
            background: RgbColor(0x2E, 0x34, 0x40),
            foreground: RgbColor(0xD8, 0xDE, 0xE9),
            // ... 나머지
        },
    });
}
```

---

## 4. vendoring 구조

### 4.1 vtcode-ui → oxicode-vtui 매핑

```
vtcode-ui/src/                             oxicode-vtui/src/
├── design/          ──복사──▶             ├── design/
│   ├── color.rs                            │   └── (vtcode_config::agent_mode_hue → oxicode-vtui-compat)
│   ├── layout.rs
│   ├── panel.rs
│   ├── diff.rs
│   └── style.rs
├── theme/           ──복사──▶             ├── theme/
│   ├── registry.rs                         │   ├── registry.rs         (40+ themes)
│   ├── runtime.rs                          │   ├── runtime.rs          (vtcode_config::ui 상수 → oxicode-vtui-compat)
│   ├── types.rs                            │   ├── types.rs            "
│   ├── color_math.rs                       │   ├── color_math.rs       "
│   ├── syntax.rs                           │   ├── syntax.rs
│   └── scheme.rs                           │   └── scheme.rs
├── tui/              ──부분복사──▶        ├── tui/
│   ├── mod.rs        (re-exports)           │   ├── mod.rs
│   ├── host.rs        ✅                    │   ├── host.rs
│   ├── core.rs        ✅                    │   ├── core.rs
│   ├── tui/config/    ✅                    │   ├── config/
│   ├── tui/core_tui/
│   │   ├── types.rs        ✅               │   ├── types.rs
│   │   ├── types/protocol.rs ✅             │   ├── protocol.rs
│   │   ├── types/style.rs  ✅               │   ├── style_types.rs
│   │   ├── session.rs      ✅               │   ├── session.rs
│   │   ├── runner/         ✅               │   ├── runner/
│   │   ├── widgets/        일부             │   ├── widgets/
│   │   ├── style.rs        ✅               │   ├── style.rs
│   │   └── theme_parser.rs ✅               │   └── theme_parser.rs
│   ├── tui/ui/
│   │   ├── markdown/       ✅               │   ├── markdown/
│   │   ├── syntax_highlight.rs ✅          │   ├── syntax_highlight.rs
│   │   └── interactive_list/ ✅             │   └── interactive_list/
│   ├── tui/utils/          ✅               │   └── utils/
│   ├── tui/app/            ❌ (대부분 컷)   │
│   ├── tui/vim/            ❌                │
│   └── tui/cache.rs        ✅               │   └── cache.rs
└── vim/              ❌ 컷                │
```

### 4.2 oxicode-vtui-compat 스텁

```rust
// oxicode-vtui-compat/src/constants/ui.rs
pub const TOOL_OUTPUT_MODE_COMPACT: &str = "compact";
pub const TOOL_OUTPUT_MODE_FULL: &str = "full";
pub const DEFAULT_REASONING_VISIBLE: bool = false;
pub const INLINE_PTY_PLACEHOLDER: &str = "...";
pub const HEADER_UNKNOWN_PLACEHOLDER: &str = "—";
pub const CHAT_INPUT_PLACEHOLDER_BOOTSTRAP: &str = "Describe what you want to build...";
pub const CHAT_INPUT_PLACEHOLDER_FOLLOW_UP: &str = "Follow-up...";
pub const WELCOME_TEXT_WIDTH: usize = 72;
pub const WELCOME_SHORTCUT_SECTION_TITLE: &str = "Shortcuts";
// ... remaining 10 constants

pub fn agent_mode_hue(_token: &str) -> Option<f32> { None }

// oxicode-vtui-compat/src/types.rs
#[derive(Debug, Clone)]
pub enum ToolPolicy { Allow, Ask, Deny }

#[derive(Debug, Clone)]
pub enum SystemPromptMode { Default, Compact }

#[derive(Debug, Clone)]
pub enum ToolDocumentationMode { Full, Compact, None }

#[derive(Debug, Clone)]
pub enum VerbosityLevel { Quiet, Normal, Verbose }
```

---

## 5. 모듈 레이아웃 최종

```
crates/
├── oxicode-vtui/                              (vendored vtcode-ui)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                         (pub mod design; pub mod theme; pub mod tui;)
│       ├── design/                        (from vtcode-ui)
│       ├── theme/                         (from vtcode-ui + oxicode_themes.rs 추가)
│       ├── tui/
│       │   ├── host.rs                    (HostAdapter trait — from vtcode-ui)
│       │   ├── core.rs                    (CoreSessionOptions, spawn_core_session)
│       │   ├── config/                    (keyboard, surface pref, constants)
│       │   ├── types/                     (InlineCommand, InlineEvent, InlineHandle, InlineSession, …)
│       │   ├── session.rs                 (Session driver — from vtcode-ui core_tui/session)
│       │   ├── runner/                    (run_tui, alternate_screen, panic_hook)
│       │   ├── widgets/                   (transcript, spinner, modal list)
│       │   ├── markdown/                  (pulldown-cmark + syntect rendering)
│       │   ├── syntax_highlight.rs        (syntect theme loader)
│       │   └── style.rs                   (theme_from_styles)
│       └── vim/                           ❌ 컷
│
├── oxicode-vtui-compat/                       (스텁)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── constants/
│       │   ├── ui.rs
│       │   ├── defaults.rs
│       │   └── tools.rs
│       ├── types.rs
│       └── ui_protocol/
│           ├── style.rs                   (InlineTextStyle, InlineSegment, InlineTheme, InlineHeaderContext)
│           ├── types.rs                   (InlineMessageKind, SlashCommandItem, SessionSurface, …)
│           ├── selection.rs               (InlineListItem, InlineListSelection, …)
│           ├── markdown.rs                (markdown-related types)
│           └── mod.rs
│
├── oxicode-hashline/                          (무변경)
│
├── oxicode-cli/
│   └── src/
│       ├── tui_vt/
│       │   ├── mod.rs                     (pub use main_loop::run_tui)
│       │   ├── main_loop.rs               (이벤트 루프 — 위 2.2-2.3)
│       │   ├── host.rs                    (OxicodeHostAdapter — 위 2.1)
│       │   ├── mapping.rs                 (AgentEvent → InlineCommand, InlineEvent → AgentSession)
│       │   ├── theme.rs                   (oxicode theme resolve, auto dark/light)
│       │   ├── slash.rs                   (slash command registration)
│       │   └── ext.rs                     (oxicode-specific widget extensions)
│       ├── tui/                           ❌ 전체 삭제
│       └── bootstrap.rs                   (dispatch_run_mode: tui_vt::run_tui 호출로 변경)
```

---

## 6. 마이그레이션 계획

### Phase 0: vendoring + 빌드 확립 (예상 2-3일)

1. vtcode-ui `design/` + `theme/` + 필요한 `tui/` 모듈 복사
2. `oxicode-vtui-compat` 스텁 crate 생성
3. `oxicode-vtui` Cargo.toml 수정: `vtcode-config` → `oxicode-vtui-compat`, `vtcode-commons` → `oxicode-vtui-compat`
4. Import path 수동 수정 (~20군데, `vtcode_config::` → `oxicode_vtui_compat::`)
5. `cargo build -p oxicode-vtui` 통과 확인 → 컷한 모듈 (`vim/`, `app/`) 미포함으로 인한 컴파일 에러 수정

### Phase 1: oxicode-cli glue (예상 2-3일)

1. `oxicode-vtui` dep 을 `oxicode-cli` 에 추가
2. `oxicode-cli/src/tui_vt/` 신설: `host.rs`, `main_loop.rs`, `mapping.rs`, `theme.rs`
3. `OxicodeHostAdapter` 구현
4. 최소 이벤트 루프: `spawn_core_session()` → submit 하나 → agent_stream → exit

### Phase 2: full event mapping (예상 2-3일)

1. 모든 `SessionEvent` → `InlineCommand` 매핑 구현
2. 모든 `InlineEvent` → `AgentSession` 동작 매핑 구현
3. Slash command 등록 (vtcode-ui `InlineHandle` + oxicode custom overlay)
4. Ctrl+C / interrupt / steering 처리

### Phase 3: 테마 마이그레이션 (예상 1-2일)

1. 기존 oxicode 6 theme → vtcode-ui `ThemeDefinition` 포맷 변환
2. GlyphSet (Ascii/Nerd) 지원을 vtcode-ui 상수로 주입 (Settings flag 로 분기)
3. Theme hot-reload: FileWatcher → `runtime::set_active_theme()`

### Phase 4: cleanup (예상 1일)

1. `oxicode-tui` crate 삭제
2. `oxicode-cli/src/tui/` 삭제
3. `cargo check --workspace`, `cargo clippy --workspace`, `cargo nextest run --workspace`
4. CHANGELOG, THIRD-PARTY-NOTICES (vtcode-ui MIT)

---

## 7. 리스크

| 리스크 | 확률 | 완화 |
|---|---|---|
| vtcode-ui 내부 API 변경 (`pub(crate)` → `pub`) 필요 | 중 | vendoring 이므로 우리가 직접 변경. upstream sync 는 git subtree merge |
| vtcode-ui 가 `vtcode-config` 의 타입을 실제로 *사용* 하는 부분이 더 있을 수 있음 | 저 | grep 으로 다 찾았고, 추가 발견 시 oxicode-vtui-compat 에 추가 |
| ratatui 0.30.0 호환성 (oxicode `0.30` vs vtcode `=0.30.2`) | 저 | minor patch. 우리 `Cargo.lock` 에서 조정 |
| Rust edition 2024 (oxicode 는 2024, vtcode 는 2024) — 큰 문제 없을 것 | 저 | 둘 다 2024 |
| vtcode-ui 에서 가져온 Session 타입 계층이 복잡해서 staging 에서 에러 | 중 | 최소 viable session (single-turn submit → response) 먼저 구현 후 점진 확장 |
| ThemePalette 필드 부족 (oxicode 28-slot → vtcode 8개) | 중 | ThemePalette 확장 + oxicode 확장 테마로. 초기에는 기본 vtcode-ui theme 만 사용 |

---

## 8. 검증 기준

- [ ] `cargo build -p oxicode-vtui` 통과 (vendoring 완료)
- [ ] `cargo build -p oxicode-vtui-compat` 통과 (스텁 완료)
- [ ] `cargo build -p oxicode-cli` 통과 (통합 완료)
- [ ] `oxicode` 실행 시 vtcode-ui 기반 TUI 가 화면에 렌더링됨
- [ ] prompt 입력 → agent 응답 스트리밍이 transcript 에 표시됨
- [ ] Ctrl+C 가 인터럽트로 전달됨
- [ ] theme 변경 (`/theme`) 이 작동함
- [ ] `cargo clippy --workspace -- -D warnings` 통과
- [ ] `cargo nextest run --workspace` 통과
- [ ] 기존 oxicode-tui 크레이트 workspace 에서 제거됨
