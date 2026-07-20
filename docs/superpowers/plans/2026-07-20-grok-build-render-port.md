# grok-build TUI 렌더링 이식 구현 계획 (oxi 백엔드 유지)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** grok-build의 TUI 렌더링 파이프라인을 oxi에 통합. oxi-agent, oxi-sdk, oxi-ai는 유지. oxi-cli의 TUI 레이어만 grok 렌더링으로 교체.

**Architecture:** ~13개 vendored render crate를 수정/통합. oxi-pager를 grok의 렌더 파이프라인을 제대로 구동하는 상태머신으로 재구현. oxi-agent의 이벤트를 grok-style 렌더로 변환. 기존 oxi-cli/src/tui/ 제거.

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm 0.29, grok-build vendored rendering primitives

## Global Constraints

- oxi workspace: `/Volumes/MERCURY/PROJECTS/oxi`
- grok-build source: `/tmp/ref-porter/xai-org-grok-build` (commit `ba76b0a683fa52e4e60685017b85905451be17bc`)
- oxi-agent, oxi-sdk, oxi-ai **유지** — agent 런타임은 변경하지 않음
- oxi-cli/src/tui/, oxi-pager **제거** — grok 렌더링으로 대체
- ratatui 0.30 사용 (oxi workspace 기준)
- vendored crate는 `#![allow(clippy::all, ...)]` 프리앰블 적용

---

### Task 1: Vendored Render Crate 정비

**Files:**
- Modify: `oxi-vendor-grok-pager-render/Cargo.toml`
- Modify: `oxi-vendor-grok-mermaid/Cargo.toml`
- Modify: `oxi-vendor-grok-markdown/Cargo.toml`
- Modify: `oxi-vendor-grok-markdown-core/Cargo.toml`
- Modify: `oxi-vendor-ratatui-textarea/Cargo.toml`
- Modify: `oxi-vendor-ratatui-inline/Cargo.toml`
- Modify: `oxi-vendor-grok-paths/Cargo.toml`
- Modify: `oxi-vendor-tty-utils/Cargo.toml`
- Modify: `oxi-vendor-dagre_rust/Cargo.toml`
- Modify: `oxi-vendor-graphlib_rust/Cargo.toml`
- Modify: `oxi-vendor-mermaid-to-svg/Cargo.toml`
- Modify: `oxi-vendor-ordered_hashmap/Cargo.toml`
- Modify: `Cargo.toml` (workspace)

**Interfaces:**
- Consumes: 현재 disk에 있는 vendored crate들 (Cargo.toml 참조 깨짐)
- Produces: 모든 vendored crate가 컴파일되는 상태

- [ ] **Step 1: Fix vendored Cargo.toml — internal references**

각 vendored crate의 Cargo.toml에서 grok 내부 crate 참조를 oxi-vendor path로 변환한다.

`scripts/fix-vendored-toml.py` 생성 및 실행:

```python
#!/usr/bin/env python3
"""Fix vendored Cargo.toml: internal refs → oxi-vendor paths, inline workspace deps."""
import re, os
from pathlib import Path

OXI = Path("/Volumes/MERCURY/PROJECTS/oxi")
GROK = Path("/tmp/ref-porter/xai-org-grok-build")

# Parse grok workspace deps for version inlining
grok_ws = (GROK / "Cargo.toml").read_text()
grok_deps = {}
for m in re.finditer(r'^(\S+)\s*=\s*(\{.+\}|\".+\")', grok_ws, re.MULTILINE):
    grok_deps[m.group(1)] = m.group(2)

# Build oxi-vendor name map
name_map = {}
for d in os.listdir(OXI):
    if d.startswith('oxi-vendor-') and os.path.isdir(OXI / d) and d != 'oxi-vendor-grok-shim' and d != 'oxi-vendor-grok-pager':
        suffix = d.replace('oxi-vendor-', '')
        name_map[suffix] = d
        name_map['xai-' + suffix] = d

FIXED = 0
for d in sorted(os.listdir(OXI)):
    if not d.startswith('oxi-vendor-') or not os.path.isdir(OXI / d):
        continue
    if d in ('oxi-vendor-grok-shim', 'oxi-vendor-grok-pager'):
        continue

    toml = OXI / d / 'Cargo.toml'
    if not toml.exists():
        continue
    content = toml.read_text()
    original = content

    # Fix package name
    for grok_name, oxi_name in name_map.items():
        if f'name = "{grok_name}"' in content:
            content = content.replace(f'name = "{grok_name}"', f'name = "{oxi_name}"')
            break

    # Fix edition
    content = content.replace('edition.workspace = true', 'edition = "2024"')

    # Fix internal path refs: xai-grok-foo = { workspace = true } → oxi-vendor-grok-foo = { path = ... }
    for grok_name, oxi_name in sorted(name_map.items(), key=lambda x: -len(x[0])):
        pattern = f'{grok_name} = {{ workspace = true'
        if pattern in content:
            content = content.replace(pattern, f'{oxi_name} = {{ path = "../{oxi_name}"')

    # Inline remaining workspace = true deps
    def inline_ws(m):
        dep = m.group(1)
        if dep in grok_deps:
            return f'{dep} = {grok_deps[dep]}'
        return m.group(0)
    content = re.sub(
        r'^(\S+)\s*=\s*\{\s*workspace\s*=\s*true[^}]*\}',
        inline_ws,
        content,
        flags=re.MULTILINE
    )

    if content != original:
        toml.write_text(content)
        FIXED += 1
        print(f"FIXED: {d}")
    else:
        print(f"OK:    {d}")

print(f"\nFixed {FIXED} crates.")
```

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
python3 scripts/fix-vendored-toml.py
```

Expected: 8-12 crate의 Cargo.toml이 수정됨.

- [ ] **Step 2: Add lint preamble to all vendored lib.rs**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
PREAMBLE='#![allow(deprecated, dead_code, unused_imports, unused_variables, unused_mut, clippy::all, clippy::pedantic, rustdoc::broken_intra_doc_links)]'

for lib in oxi-vendor-*/src/lib.rs; do
    if [ -f "$lib" ] && ! grep -q 'allow(deprecated' "$lib"; then
        printf '%s\n\n' "$PREAMBLE" | cat - "$lib" > "$lib.tmp"
        mv "$lib.tmp" "$lib"
        echo "PREAMBLE: $lib"
    fi
done
```

- [ ] **Step 3: Add vendored crates to workspace + register deps**

`Cargo.toml`의 `members`에 모든 vendored crate 추가:

```toml
members = [
    "oxi-ai", "oxi-agent", "oxi-cli", "oxi-sdk",
    "oxi-vendor-grok-markdown-core", "oxi-vendor-grok-markdown",
    "oxi-vendor-ratatui-textarea", "oxi-vendor-ratatui-inline",
    "oxi-vendor-grok-pager-render", "oxi-vendor-grok-mermaid",
    "oxi-vendor-grok-paths", "oxi-vendor-tty-utils",
    "oxi-vendor-dagre_rust", "oxi-vendor-graphlib_rust",
    "oxi-vendor-mermaid-to-svg", "oxi-vendor-ordered_hashmap",
]
```

`[workspace.dependencies]`에 필요한 external deps 추가:

```toml
base64 = "0.22"
camino = "1.1.10"
dirs = "5.0"
dunce = "1"
fontdb = "0.23"
image = { version = "0.25.9", default-features = false }
libc = "0.2"
nix = { version = "0.30", features = ["signal"] }
notify = "8"
resvg = { version = "0.47", default-features = false, features = ["text"] }
serde_json = "1"
tempfile = "3"
tiny-skia = "0.12"
toml = "0.9"
toml_edit = "0.22"
uuid = { version = "1", features = ["serde", "v4", "v5"] }
wait-timeout = "0.2"
which = "8"
```

- [ ] **Step 4: Compile vendored crates**

```bash
cargo check -p oxi-vendor-grok-pager-render 2>&1 | tail -20
```

Expected: ratatui-inline 오류 21개 외 0 errors. ratatui-inline은 oxi가 이미 패치 완료되었으므로 재확인.

- [ ] **Step 5: Commit**

```bash
git add -A oxi-vendor-*/ scripts/fix-vendored-toml.py Cargo.toml
git commit -m "chore: fix vendored grok render crates — Cargo.toml refs + compile"
```

---

### Task 2: oxi-pager 재구현 — grok 렌더 파이프라인 통합

**Files:**
- Rewrite: `oxi-pager/src/render/mod.rs`
- Rewrite: `oxi-pager/src/main_loop.rs`
- Rewrite: `oxi-pager/src/state.rs`
- Modify: `oxi-pager/Cargo.toml`
- Keep: `oxi-pager/src/reducer.rs`, `oxi-pager/src/scrollback.rs`, `oxi-pager/src/prompt.rs`, `oxi-pager/src/keymap.rs`

**Interfaces:**
- Consumes: `oxi-vendor-grok-pager-render` (theme, terminal, wrapping, glyphs, scrollbar)
- Consumes: `oxi-vendor-grok-markdown` (streaming markdown render)
- Consumes: `oxi-vendor-ratatui-textarea` (prompt input)
- Produces: `oxi_pager::run(app)` — grok-quality TUI 렌더링 루프

- [ ] **Step 1: Update oxi-pager dependencies**

`oxi-pager/Cargo.toml`:

```toml
[package]
name = "oxi-pager"
version = "0.1.0"
edition = "2024"
license = "MIT"

[dependencies]
# Vendored grok render primitives
oxi-vendor-grok-pager-render = { path = "../oxi-vendor-grok-pager-render" }
oxi-vendor-grok-markdown = { path = "../oxi-vendor-grok-markdown" }
oxi-vendor-ratatui-textarea = { path = "../oxi-vendor-ratatui-textarea" }
oxi-vendor-ratatui-inline = { path = "../oxi-vendor-ratatui-inline" }
oxi-vendor-tty-utils = { path = "../oxi-vendor-tty-utils" }

# Runtime
ratatui = { version = "0.30", features = ["crossterm", "unstable-widget-ref", "unstable-rendered-line-info", "unstable-backend-writer"] }
crossterm = { version = "0.29", features = ["event-stream", "bracketed-paste"] }
tokio = { version = "1", features = ["sync", "rt", "macros", "rt-multi-thread", "signal", "io-util", "io-std"] }
parking_lot = "0.12"
anyhow = "1"
tracing = "0.1"
```

- [ ] **Step 2: Rewrite state.rs — grok-style PagerState**

`oxi-pager/src/state.rs`:

```rust
//! Pager state — the single source of truth for the TUI.
//!
//! Designed to match grok-build's render contract: the render function
//! reads PagerState and produces a ratatui Frame. Agent events mutate
//! state through the reducer.

use crate::prompt::PromptState;
use crate::scrollback::ScrollbackState;
use crate::status::StatusState;
use parking_lot::RwLock;
use std::sync::Arc;

/// Shared mutable state passed between the agent worker and the TUI.
pub type SharedState = Arc<RwLock<PagerState>>;

/// Which overlay/modal is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    None,
    /// /-command input
    Slash,
    /// Settings overlay
    Settings,
    /// File picker
    FilePicker,
    /// Issue panel
    Issues,
}

/// Sticky panel pinned to a position in scrollback.
#[derive(Debug, Clone, Default)]
pub struct StickyPanelState {
    pub active: bool,
    pub position: usize,
    pub content: String,
}

/// The full pager state — single source of truth.
#[derive(Debug, Clone)]
pub struct PagerState {
    /// Chat scrollback — user + assistant + tool blocks
    pub scrollback: ScrollbackState,
    /// Current prompt input
    pub prompt: PromptState,
    /// Status bar / token counter
    pub status: StatusState,
    /// Which modal/overlay is active
    pub modal: ModalKind,
    /// Whether the agent is currently streaming
    pub is_streaming: bool,
    /// Spinner tick (incremented each frame while streaming)
    pub spinner_tick: u64,
    /// Sticky panel (e.g. plan mode)
    pub sticky: StickyPanelState,
    /// Agent is waiting for user input (ask tool)
    pub waiting_for_input: bool,
}

impl Default for PagerState {
    fn default() -> Self {
        Self {
            scrollback: ScrollbackState::default(),
            prompt: PromptState::default(),
            status: StatusState::default(),
            modal: ModalKind::None,
            is_streaming: false,
            spinner_tick: 0,
            sticky: StickyPanelState::default(),
            waiting_for_input: false,
        }
    }
}
```

- [ ] **Step 3: Rewrite main_loop.rs — real event loop**

`oxi-pager/src/main_loop.rs`:

```rust
//! Main event loop — crossterm input + agent events → dispatch → render.
//!
//! Uses grok-vendored terminal backend (ratatui-inline) and
//! render primitives (grok-pager-render) for grok-quality output.

use crate::emitter::{BackgroundEvent, PagerEvent, ResolvedKey};
use crate::keymap::KeyRouter;
use crate::reducer::{reduce, PagerAction};
use crate::render;
use crate::state::{PagerState, SharedState};
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use oxi_vendor_grok_pager_render::terminal;
use parking_lot::RwLock;
use ratatui::Terminal;
use std::sync::Arc;
use tokio::sync::mpsc;

pub async fn run<A: Send + 'static>(
    app: A,
    background_rx: mpsc::UnboundedReceiver<BackgroundEvent>,
) -> anyhow::Result<()> {
    let state: SharedState = Arc::new(RwLock::new(PagerState::default()));

    // Terminal setup using grok's vendored backend
    let mut terminal = terminal::init()?;
    let result = event_loop(&mut terminal, state, app, background_rx).await;
    terminal::restore()?;
    result
}

async fn event_loop<A>(
    terminal: &mut Terminal<oxi_vendor_ratatui_inline::InlineBackend>,
    state: SharedState,
    _app: A,
    mut background_rx: mpsc::UnboundedReceiver<BackgroundEvent>,
) -> anyhow::Result<()> {
    let mut reader = EventStream::new();
    let key_router = KeyRouter::new();

    loop {
        // Render current state using grok's render pipeline
        terminal.draw(|frame| {
            let state = state.read();
            let theme = oxi_vendor_grok_pager_render::theme::Theme::default();
            render::render(frame, &state, &theme);
        })?;

        // Wait for next event (input or background)
        tokio::select! {
            // Crossterm input
            Some(Ok(event)) = reader.next() => {
                let action = match event {
                    CrosstermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        let resolved = ResolvedKey::from(key);
                        key_router.route(resolved, &state.read())
                    }
                    CrosstermEvent::Resize(w, h) => {
                        PagerAction::Resize { cols: w, height: h }
                    }
                    _ => PagerAction::NoOp,
                };
                if let PagerAction::Quit = action {
                    return Ok(());
                }
                if action != PagerAction::NoOp {
                    reduce(&state, action);
                }
            }
            // Agent background events (streaming tokens, tool calls, ...)
            Some(event) = background_rx.recv() => {
                apply_background_event(&state, event);
            }
            else => break,
- [ ] **Step 2: Rewrite bootstrap.rs — pager + agent wiring**

`oxi-cli/src/bootstrap.rs`:

```rust
//! Composition root — wires oxi-agent AgentSession to oxi-pager (grok render).

use crate::app::agent_session::AgentSession;
use oxi_agent::AgentEvent;
use oxi_pager::{BackgroundEvent, run as run_pager};
use tokio::sync::mpsc;

/// Bridge: subscribe to AgentSession events, forward to pager as BackgroundEvents.
pub async fn run_with_session(
    mut session: AgentSession,
    initial_message: Option<String>,
) -> anyhow::Result<()> {
    let (bg_tx, bg_rx) = mpsc::unbounded_channel();

    // Subscribe to agent events
    let mut events = session.subscribe();

    // Send initial user message if provided
    if let Some(msg) = initial_message {
        session.submit(&msg).await?;
    }

    // Spawn agent worker: forward AgentEvent → BackgroundEvent
    let worker = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let bg = match event {
                AgentEvent::AssistantDelta { delta } => {
                    BackgroundEvent::AssistantDelta(delta)
                }
                AgentEvent::AssistantMessageComplete { .. } => {
                    BackgroundEvent::AssistantDone
                }
                AgentEvent::ToolCall { tool_call_id, name, params } => {
                    BackgroundEvent::ToolCall {
                        id: tool_call_id,
                        name,
                        params: serde_json::to_string_pretty(&params).unwrap_or_default(),
                    }
                }
                AgentEvent::ToolResult { tool_call_id, content } => {
                    BackgroundEvent::ToolResult { id: tool_call_id, content }
                }
                AgentEvent::AgentStateChange { state } => {
                    // Update status bar with token counts etc.
                    BackgroundEvent::StatusUpdate(StatusState {
                        tokens_used: state.usage.total_tokens,
                        model: state.model.clone().unwrap_or_default(),
                    })
                }
                AgentEvent::Advisory { .. } => continue, // handled separately
                _ => continue,
            };
            if bg_tx.send(bg).is_err() {
                break; // pager closed
            }
        }
    });

    // Run pager (blocks until user quits)
    let result = run_pager(session, bg_rx).await;
    worker.abort();
    result
}
```

참고: `AgentSession::subscribe()`가 존재하지 않으면 `tokio::sync::broadcast`로 구독 패턴 추가.
`AgentEvent` enum이 정확히 일치하지 않으면 실제 variant에 맞춰 조정.
```

- [ ] **Step 4: Update emitter.rs — background event types**

`oxi-pager/src/emitter.rs`:

```rust
use crate::keymap::{FocusTarget, ModalInput};
use crate::status::StatusState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Events sent from the background agent worker to the TUI.
#[derive(Debug, Clone)]
pub enum BackgroundEvent {
    /// Streaming text delta from assistant
    AssistantDelta(String),
    /// Assistant finished current message
    AssistantDone,
    /// Tool call initiated
    ToolCall { id: String, name: String, params: String },
    /// Tool call result
    ToolResult { id: String, content: String },
    /// User message from agent loop
    UserMessage(String),
    /// Status bar update
    StatusUpdate(StatusState),
    /// Agent finished streaming
    StreamDone,
    /// Agent is asking user for input (ask tool)
    AskPrompt(String),
}

/// Resolved keyboard input.
#[derive(Debug, Clone)]
pub struct ResolvedKey {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl From<KeyEvent> for ResolvedKey {
    fn from(k: KeyEvent) -> Self {
        Self { code: k.code, modifiers: k.modifiers }
    }
}

/// Events emitted by the pager (for external consumers).
#[derive(Debug, Clone)]
pub enum PagerEvent {
    /// User submitted prompt text
    Submit(String),
    /// User pressed Ctrl-C (cancel)
    Cancel,
    /// User requested quit
    Quit,
}
```

- [ ] **Step 5: Rewrite render/mod.rs — grok-quality rendering**

`oxi-pager/src/render/mod.rs`의 `render()` 함수를 grok의 테마/글리프/랩핑을 완전히 활용하도록 재작성.

grok의 `oxi-vendor-grok-pager-render`는 다음을 제공한다:
- `theme::Theme` — 전체 컬러 스킴 (dark, light, nord, catppuccin, ...)
- `glyphs::GlyphSet` — Nerd Font 유니코드 글리프
- `render::wrapping::word_wrap_lines_with_joiners` — markdown 래핑
- `render::scrollbar` — 커스텀 스크롤바
- `syntax` — syntect 기반 신택스 하이라이팅

`render()` 진입점:

```rust
use oxi_vendor_grok_pager_render::theme::Theme as GrokTheme;
use oxi_vendor_grok_pager_render::glyphs;
use oxi_vendor_grok_pager_render::render::wrapping;
use oxi_vendor_grok_markdown::render_markdown_ratatui_full;
use crate::state::PagerState;
use ratatui::prelude::*;
use ratatui::widgets::*;

pub fn render(frame: &mut Frame, state: &PagerState, theme: &GrokTheme) {
    let area = frame.area();
    
    // Layout: scrollback | ---- | status | prompt
    let layout = Layout::vertical([
        Constraint::Min(1),      // scrollback
        Constraint::Length(1),   // status bar
        Constraint::Length(3),   // prompt
    ]).split(area);
    
    let scrollback_area = layout[0];
    let status_area = layout[1];
    let prompt_area = layout[2];
    
    // Clear background
    frame.render_widget(Clear, area);
    
    // Render scrollback with grok theme
    render_scrollback(frame, scrollback_area, state, theme);
    
    // Render status bar
    render_status(frame, status_area, state, theme);
    
    // Render prompt
    render_prompt(frame, prompt_area, state, theme);
    
    // Render modal overlays
    render_modal(frame, area, state, theme);
}
```

- [ ] **Step 6: Verify oxi-pager compiles**

```bash
cargo check -p oxi-pager
```

Expected: 0 errors.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/
git commit -m "refactor(oxi-pager): reimplement with grok render pipeline"
```

---

### Task 3: oxi-cli TUI 레이어 교체

**Files:**
- Modify: `oxi-cli/Cargo.toml`
- Modify: `oxi-cli/src/main.rs`
- Modify: `oxi-cli/src/lib.rs`
- Delete: `oxi-cli/src/tui/` (전체)
- Modify: `oxi-cli/src/bootstrap.rs`

**Interfaces:**
- Consumes: `oxi-pager::run`, `oxi-agent::Agent`
- Produces: grok TUI로 oxi agent를 구동하는 binary

- [ ] **Step 1: Update oxi-cli dependencies**

`oxi-cli/Cargo.toml`에서 oxi-tui 의존성 제거, oxi-pager 의존성 업데이트:

```toml
oxi-pager = { path = "../oxi-pager" }
# oxi-tui 제거
```

- [ ] **Step 2: Rewrite bootstrap.rs — pager + agent wiring**

`oxi-cli/src/bootstrap.rs`:

```rust
//! Composition root — wires oxi-agent to oxi-pager (grok render).

use oxi_agent::Agent;
use oxi_pager::{BackgroundEvent, run as run_pager};
use tokio::sync::mpsc;

pub async fn run_with_agent(
    agent: Agent,
    initial_message: Option<String>,
) -> anyhow::Result<()> {
    let (bg_tx, bg_rx) = mpsc::unbounded_channel();

    // Spawn agent worker — streams events to pager via channel
    let agent_handle = tokio::spawn(async move {
        if let Some(msg) = initial_message {
            let _ = bg_tx.send(BackgroundEvent::UserMessage(msg));
        }
        // Agent loop: stream events, handle tool calls
        // Each event → BackgroundEvent sent to pager
    });

    // Run pager (blocks until user quits)
    run_pager((), bg_rx).await?;

    agent_handle.abort();
    Ok(())
}
```

- [ ] **Step 3: Update main.rs entry point**

`oxi-cli/src/main.rs`:

```rust
use oxi_cli::bootstrap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI args (simplified — kept from existing bootstrap)
    let app = bootstrap::build_app().await?;
    let agent = app.create_agent().await?;
    bootstrap::run_with_agent(agent, app.initial_message).await
}
```

- [ ] **Step 4: Clean up lib.rs exports**

`oxi-cli/src/lib.rs`에서 TUI 관련 export 제거. `mod tui` 제거. `bootstrap` 모듈만 유지.

- [ ] **Step 5: Verify oxi-cli compiles**

```bash
cargo check -p oxi-cli
```

Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add oxi-cli/
git rm -r oxi-cli/src/tui/ 2>/dev/null || true
git commit -m "refactor(oxi-cli): replace TUI with oxi-pager + grok render"
```

---

### Task 4: oxi-tui, oxi-vendor-grok-shim 정리

**Files:**
- Delete: `oxi-tui/`
- Delete: `oxi-vendor-grok-shim/`
- Modify: `Cargo.toml`

- [ ] **Step 1: Remove crates**

```bash
rm -rf oxi-tui oxi-vendor-grok-shim
```

- [ ] **Step 2: Update workspace members**

`Cargo.toml`에서 `"oxi-tui"`, `"oxi-vendor-grok-shim"` 제거.

- [ ] **Step 3: Verify workspace compiles**

```bash
cargo check --workspace
```

Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor: remove oxi-tui and oxi-vendor-grok-shim"
```

---

### Task 5: Verification

- [ ] **Step 1: Full workspace check**

```bash
cargo check --workspace 2>&1 | grep "^error" | wc -l
```

Expected: 0.

- [ ] **Step 2: Build release binary**

```bash
cargo build --release -p oxi-cli
```

Expected: binary at `target/release/oxi`.

- [ ] **Step 3: Smoke test — launch TUI**

```bash
./target/release/oxi
```

grok-quality TUI가 정상 실행되는지 확인. 다음을 체크:
- Markdown 렌더링 (bold, italic, code blocks, links)
- Stream 토큰이 자연스럽게 타이핑되는 애니메이션
- Scrollback 스크롤
- Prompt 입력

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore: final verification — grok TUI port complete"
```
