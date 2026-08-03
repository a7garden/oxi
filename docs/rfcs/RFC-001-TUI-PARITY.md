# RFC-001: TUI Pi Parity — Differential Rendering, Editor, Keybindings, Completion, Overlays

**Status**: Draft
**Priority**: P0 — User experience frontline
**Current Completion**: ~28%
**Target**: 95%+ feature parity

---

## 1. Problem Definition

The oxicode TUI layer totals **15,602 LOC** across two crates:

| Crate | Path | LOC |
|-------|------|-----|
| `oxicode-tui` | `oxicode-tui/src/` | 6,601 |
| `oxicode-cli` | `oxicode-cli/src/tui/` | 9,001 |
| **Total** | | **15,602** |

pi-tui uses a custom rendering engine at ~11,300 LOC. The core gaps:

| Area | pi | oxicode (current) | Gap |
|------|----|---------------|-----|
| Differential rendering | Line-level diff (60fps cap) | ratatui full-frame redraw | Performance |
| Editor | 76KB full-featured (undo, kill-ring, history, jump) | `ratatui-textarea` wrapping (oxicode-tui `widgets/input.rs`, 330 LOC) | ~50% feature coverage |
| Keybindings | 31 bindings + dynamic rebinding + conflict detection | Hardcoded in `handlers.rs` (1,492 LOC) | Critical |
| Slash completion | 23KB (slash commands, file paths, fuzzy) | **Existing system** in `slash.rs` (1,030 LOC) + `app.rs` (completion state, ~50 LOC) — slash commands only, no file path or fuzzy | Moderate |
| Overlay | 9-direction anchor + z-index + focus | **Existing system**: `OverlayComponent` trait + 7 components (3,585 LOC), but center-only placement, no anchor positioning | Moderate (extend, don't replace) |
| Terminal image | Kitty + iTerm2 protocol | None | Critical |
| Render pipeline | Custom `doRender()` | `render::draw()` in `render.rs` (1,207 LOC) — popup, slash overlay, status, input area — full-frame via `tui.draw()` | Needs differential backend |

**Existing implementations the previous draft missed:**

1. **Overlay system** (`oxicode-cli/src/tui/overlay/`, 3,585 LOC): `OverlayComponent` trait with `handle_key()`, `render()`, `hint()` methods. 10 implementations across 7 files: `provider_select` (765 LOC), `model_select` (214 LOC), `questionnaire` (650 LOC), `resume_select` (196 LOC), `logout_select` (150 LOC), `router_setup` (701 LOC), plus `factories.rs` (684 LOC) with 4 implementations: `ModelSelectOverlay`, `LogoutSelectOverlay`, `ResumeSelectOverlay`, `RoutingOverlay`. `router_integration.rs` (146 LOC) provides helper functions, not a component.

2. **Slash completion system** (`oxicode-cli/src/tui/slash.rs`, 1,030 LOC + `app.rs` state): `SlashCompletion` type, `update_slash_completions()`, `selected_slash_command()`, `next_slash_completion()`, `prev_slash_completion()`. Popup rendering in `render.rs:render_slash_popup_overlay()` (~60 LOC). Covers slash commands only — no file path completion or fuzzy file search.

3. **Render pipeline** (`oxicode-cli/src/tui/render.rs`, 1,207 LOC): `draw()` function (starts at line 432) called via `tui.draw(|f| render::draw(f, &mut state, &theme))` at `app.rs:1238`. Manages layout chunks, overlay dispatch (`render_overlay`), slash popup overlay (line 475, defined at line 765), status line, queue panel, input area.

4. **App overlay state** (`app.rs:284-340`): Dual overlay system — legacy `AppOverlay` enum (ModelSelect, LogoutSelect, ResumeSelect, RoutingStatus) and migrated `overlay_state: Option<Box<dyn OverlayComponent>>`. Integration in `handlers.rs` lines 767-785: `handle_key` dispatches to `OverlayComponent::handle_key()`, processes `OverlayAction` variants (Close, SwitchSession, NewSession, OpenRouterSetup).

**Root cause**: ratatui is a general-purpose TUI framework, not optimized for AI chat interface specifics (streaming, differential updates, images, complex editor). However, the existing codebase already has significant overlay and completion infrastructure that should be extended rather than replaced.

---

## 2. Design Principles

1. **Extend, don't replace**: The existing overlay system (`OverlayComponent` trait) and slash completion system are functional. Extend them with missing capabilities (anchor positioning, file completion) rather than building parallel systems.
2. **Respect crate boundaries**: Per AGENTS.md dependency flow: `oxicode-tui (independent) ← oxicode-cli`. Overlays and completion depend on `AgentSession`, `Settings`, `auth_storage` — they stay in `oxicode-cli/src/tui/`. Only shared interfaces (e.g., `OverlayAnchor` enum) belong in `oxicode-tui`.
3. **Layered rendering**: Rendering backend (differential) / widget layer (interfaces) / application layer (logic). The `DifferentialRenderer` coexists with ratatui by replacing the `crossterm` Backend — ratatui's `Terminal::draw()` continues to work; only the write-to-terminal step is optimized.
4. **Zero-copy rendering**: Send only changed lines to the terminal. Port pi's differential algorithm directly to Rust.
5. **Unicode correctness**: CJK, emoji (ZWJ sequences), combining characters handled via grapheme clusters. Rust's `unicode-segmentation` + `unicode-width` crates.
6. **Incremental migration**: Each phase must leave the TUI functional. No big-bang rewrites.

---

## 3. Architecture

### 3.1 Crate Responsibility (Corrected)

```
oxicode-tui/src/                    # Shared types and widgets — NO dependency on oxicode-cli
├── render/
│   ├── mod.rs                  # DifferentialRenderer
│   ├── diff.rs                 # Line diff algorithm
│   ├── ansi.rs                 # ANSI code tracking/parsing
│   ├── image.rs                # Kitty/iTerm2 image protocols
│   └── terminal.rs             # Terminal capability detection
├── keybindings/                # NEW: keybinding system (no CLI deps)
│   ├── mod.rs                  # KeybindingsManager
│   ├── keys.rs                 # Key parsing + Kitty protocol
│   ├── registry.rs             # Action → key mapping
│   └── conflict.rs             # Conflict detection
├── overlay_anchor.rs           # NEW: OverlayAnchor enum (shared type only)
├── theme.rs                    # Existing (keep)
├── markdown_styles.rs          # Existing (keep)
├── table_renderer.rs           # Existing (keep)
├── fuzzy.rs                    # Existing (keep)
└── widgets/
    ├── chat.rs                 # Existing (extend)
    ├── input.rs                # Existing — enhanced with differential rendering support
    ├── footer.rs               # Existing (keep)
    ├── routing.rs              # Existing (keep)
    └── tool_renderer.rs        # Existing (keep)

oxicode-cli/src/tui/                # Application layer — depends on oxicode-tui
├── app.rs                      # Existing AppState + main loop (1,530 LOC)
├── handlers.rs                 # Existing key dispatch (1,492 LOC) — refactor to use keybindings/
├── render.rs                   # Existing draw() pipeline (1,207 LOC) — integrate DifferentialRenderer
├── slash.rs                    # Existing slash command system (1,030 LOC) — extend for file completion
├── welcome.rs                  # Existing (keep)
├── overlay/                    # Existing system — EXTEND with anchor positioning
│   ├── mod.rs                  # Existing OverlayComponent trait + OverlayAction (79 LOC)
│   ├── anchor.rs               # NEW: 9-direction anchor layout (from pi's resolveOverlayLayout)
│   ├── factories.rs            # Existing (684 LOC) — refactor to use anchor positioning
│   ├── provider_select.rs      # Existing (765 LOC)
│   ├── model_select.rs         # Existing (214 LOC)
│   ├── questionnaire.rs        # Existing (650 LOC)
│   ├── resume_select.rs        # Existing (196 LOC)
│   ├── logout_select.rs        # Existing (150 LOC)
│   ├── router_setup.rs         # Existing (701 LOC)
│   └── router_integration.rs   # Existing (146 LOC)
└── completion/                 # NEW: general completion (file paths, fuzzy)
    ├── mod.rs                  # CompletionManager — builds on slash.rs foundation
    ├── path.rs                 # File path completion
    └── fuzzy_file.rs           # Fuzzy file search (fd integration)
```

**Key difference from previous draft**: `overlay/` and `completion/` live in `oxicode-cli/src/tui/`, not `oxicode-tui/src/`. They depend on CLI types (`AgentSession`, `Settings`, `auth_storage`). Only the `OverlayAnchor` enum lives in `oxicode-tui` as a shared type.

### 3.2 Core Type Design

#### Differential Rendering

**Integration point**: `app.rs:1238` calls `tui.draw(|f| render::draw(f, &mut state, &theme))`. The `DifferentialRenderer` replaces crossterm as ratatui's Backend — it intercepts the buffer diff after `Terminal::draw()` computes the frame but before bytes go to the terminal.

```rust
/// Wraps crossterm Backend, adding line-level diffing.
/// ratatui's Terminal<DiffBackend<Stdout>> works unchanged.
pub struct DiffBackend<W: Write> {
    inner: CrosstermBackend<W>,
    prev_buffer: Option<Buffer>,
    prev_size: Rect,
}

impl<W: Write> ratatui::Backend for DiffBackend<W> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // Build new buffer from content iterator
        // Compare with prev_buffer line-by-line
        // Write only changed lines via crossterm cursor moves
        // Update prev_buffer
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
```

This means `render::draw()` and all existing rendering code is unchanged. The optimization is transparent at the Backend level — no changes to widget code.

```rust
/// ANSI code state tracker — port of pi's AnsiCodeTracker
pub struct AnsiTracker {
    bold: bool, dim: bool, italic: bool, underline: bool,
    blink: bool, inverse: bool, hidden: bool, strikethrough: bool,
    fg: Option<Color>, bg: Option<Color>,
    hyperlink: Option<String>,
}

impl AnsiTracker {
    pub fn process(&mut self, code: &str);
    pub fn active_codes(&self) -> String;
    pub fn line_end_reset(&self) -> String;
}

/// Terminal capability detection (port of pi's detectCapabilities)
pub struct TerminalCapabilities {
    pub image_protocol: Option<ImageProtocol>,
    pub true_color: bool,
    pub hyperlinks: bool,
    pub kitty_protocol: bool,
    pub cell_size: Option<(u16, u16)>,
}

pub enum ImageProtocol { Kitty, ITerm2 }
```

**Core algorithm (pi's doRender, adapted for Backend trait):**

```
1. Terminal::draw() computes Frame → Buffer (unchanged)
2. DiffBackend::draw() receives Buffer cells
3. Compare new buffer with prev_buffer:
   a. Scan for first_changed / last_changed rows
   b. Skip identical rows entirely
   c. For changed rows: move cursor, write cells, reset styles
4. On resize: force full redraw
5. Kitty image lifecycle: collect IDs, delete orphaned
6. Wrap output in CSI ?2026h/?2026l synchronized output
```

#### Keybindings

```rust
/// Declarative keybinding definitions — port of pi's TUI_KEYBINDINGS
/// Lives in oxicode-tui (no CLI dependencies)
pub struct KeybindingsManager {
    defaults: HashMap<Action, Vec<KeyId>>,
    user_overrides: HashMap<Action, Vec<KeyId>>,
    resolved: HashMap<Action, Vec<KeyId>>,
    conflicts: Vec<KeybindingConflict>,
    kitty_protocol_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum Action {
    // Editor navigation (12)
    CursorUp, CursorDown, CursorLeft, CursorRight,
    CursorWordLeft, CursorWordRight,
    CursorLineStart, CursorLineEnd,
    JumpForward, JumpBackward,
    PageUp, PageDown,
    // Editor editing (9)
    DeleteCharBackward, DeleteCharForward,
    DeleteWordBackward, DeleteWordForward,
    DeleteToLineStart, DeleteToLineEnd,
    Yank, YankPop, Undo,
    // Input (4)
    NewLine, Submit, Tab, Copy,
    // Selection (6)
    SelectUp, SelectDown,
    SelectPageUp, SelectPageDown,
    SelectConfirm, SelectCancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyId {
    key: BaseKey,
    ctrl: bool, shift: bool, alt: bool, super_: bool,
}

impl KeybindingsManager {
    pub fn set_user_bindings(&mut self, config: &HashMap<Action, Vec<KeyId>>);
    pub fn match_action(&self, data: &[u8]) -> Option<Action>;
    pub fn decode_printable(&self, data: &[u8]) -> Option<String>;
}
```

**Integration with `handlers.rs`**: The current 1,492 LOC `handlers.rs` uses a massive `match` on `KeyEvent`. Refactor to:
1. Convert `KeyEvent` → `KeyId`
2. Look up `KeyId` → `Action` via `KeybindingsManager`
3. Dispatch `Action` → handler function

This replaces hardcoded key matching with declarative bindings while preserving all existing functionality.

#### Overlay System (Extended)

The existing `OverlayComponent` trait in `oxicode-cli/src/tui/overlay/mod.rs` already provides:

```rust
// EXISTING — keep unchanged
pub trait OverlayComponent: std::fmt::Debug {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme);
    fn hint(&self) -> &str;
}

pub enum OverlayAction {
    None, Close,
    SwitchSession(String), NewSession,
    ExecuteSlashCommand(String), SendPrompt(String),
    OpenRouterSetup { initial: RouterSetupData, models: Vec<String> },
}
```

**What's missing** (and what to add):

```rust
// NEW in oxicode-tui/src/overlay_anchor.rs — shared type
#[derive(Debug, Clone, Copy)]
pub enum OverlayAnchor {
    Center, TopLeft, TopRight, BottomLeft, BottomRight,
    TopCenter, BottomCenter, LeftCenter, RightCenter,
}

pub struct OverlayLayout {
    pub anchor: OverlayAnchor,
    pub width: Option<SizeValue>,
    pub min_width: Option<u16>,
    pub max_height: Option<u16>,
    pub offset_x: i16,
    pub offset_y: i16,
}

// NEW in oxicode-cli/src/tui/overlay/anchor.rs
/// Resolve overlay Rect from anchor + constraints (port of pi's resolveOverlayLayout)
pub fn resolve_overlay_layout(layout: &OverlayLayout, term_w: u16, term_h: u16) -> Rect {
    // 1. Compute available area from margins
    // 2. Resolve width: parse SizeValue → clamp to min_width/available
    // 3. Resolve height: max_height → clamp
    // 4. Position by anchor + offset
    // 5. Clamp to terminal bounds
}

/// Line compositing (port of pi's compositeLineAt)
fn composite_line_at(base: &str, overlay: &str, col: usize, width: u16) -> String;
```

**Migration plan for existing overlays**: All 7 existing overlay components (`provider_select`, `model_select`, `questionnaire`, `resume_select`, `logout_select`, `router_setup`, `router_integration`) currently use `centered_popup()` for center-only placement. Upgrade path:
1. Add `OverlayLayout` field to each component
2. Replace `centered_popup()` calls with `resolve_overlay_layout()` in `render()`
3. No trait changes needed — `render()` already receives the full `Rect`

This is a ~50 LOC change per component (layout field + render call), not a migration to a new system.

#### Completion System (Extended)

The existing slash completion in `app.rs` + `slash.rs` provides:

```rust
// EXISTING in app.rs (lines 335-488)
pub slash_completions: Vec<slash::SlashCompletion>,
pub slash_completion_index: usize,
pub slash_completion_active: bool,

pub fn update_slash_completions(&mut self);
pub fn selected_slash_command(&self) -> Option<&slash::SlashCompletion>;
pub fn next_slash_completion(&mut self);
pub fn prev_slash_completion(&mut self);
pub fn clear_slash_completions(&mut self);

// EXISTING in render.rs (lines 765-803)
fn render_slash_popup_overlay(f, input_area, state, theme);
```

**What's missing**: File path completion and fuzzy file search. Add these as new modules that plug into the existing completion state:

```rust
// NEW in oxicode-cli/src/tui/completion/mod.rs
pub struct CompletionManager {
    // Wraps existing slash completion + adds new providers
    slash: SlashProvider,          // delegates to existing slash.rs
    path: PathProvider,            // NEW: file path completion
    fuzzy_file: FuzzyFileProvider, // NEW: fuzzy file search via fd
}

pub enum CompletionKind {
    SlashCommand,                  // existing
    SlashArgument { command: String }, // NEW
    FilePath,                      // NEW
    FuzzyFile { query: String },   // NEW
}

// NEW in oxicode-cli/src/tui/completion/path.rs
fn complete_path(prefix: &str, cwd: &Path) -> Vec<CompletionItem> {
    // 1. ~/ expansion
    // 2. directory/prefix split
    // 3. readdir + case-insensitive filter
    // 4. symlink resolution
    // 5. directory-first sort
}

// NEW in oxicode-cli/src/tui/completion/fuzzy_file.rs
async fn fuzzy_file_search(query: &str, base_dir: &Path) -> Vec<CompletionItem> {
    // 1. resolve_scoped_query
    // 2. Command::new("fd") --base-directory --max-results 100 --type f --type d --follow --hidden --exclude .git
    // 3. Scoring: exact(100) > starts-with(80) > substring(50) > path(30)
    // 4. Top 20 results
}
```

**Integration with `app.rs`**: Extend the existing `update_slash_completions()` pattern:
- Add `completion_kind: Option<CompletionKind>` to `AppState`
- `update_completions()` dispatches to the appropriate provider
- `render_slash_popup_overlay()` generalizes to `render_completion_popup()`

#### Terminal Image

```rust
/// Kitty image encoding (4096-byte chunks)
pub fn encode_kitty(base64: &str, opts: &ImageOptions) -> String;

/// iTerm2 image encoding
pub fn encode_iterm2(base64: &str, opts: &ImageOptions) -> String;

/// Image dimension detection (PNG, JPEG, GIF, WebP)
pub fn detect_dimensions(data: &[u8]) -> Option<ImageDimensions>;
```

#### Editor (Deferred — See Section 6)

`ratatui-textarea` (used by `oxicode-tui/src/widgets/input.rs`, 330 LOC) already provides:
- Undo/redo
- Word movement (forward/backward)
- CJK input support
- Bracketed paste
- Line history
- Selection (via shift+movement)

The RFC's original proposal for a 76KB custom editor at 3 weeks cost is **deferred** until specific limitations are documented. See Section 6 (Risks) for details.

---

## 4. Implementation Plan

### Phase 1: Keybindings Infrastructure (2 weeks)

| Task | Deliverable | Location | Dependencies |
|------|-------------|----------|-------------|
| Key parsing + Kitty protocol | `KeyId`, `parse_kitty_sequence()` | `oxicode-tui/src/keybindings/keys.rs` | None |
| Action registry | `KeybindingsManager`, `Action` enum | `oxicode-tui/src/keybindings/registry.rs` | None |
| Conflict detection | `detect_conflicts()` | `oxicode-tui/src/keybindings/conflict.rs` | None |
| Settings integration | Load user bindings from `settings.toml` | `oxicode-cli/src/tui/` (app.rs) | Phase 1 |
| Handlers refactor | Replace hardcoded match with Action dispatch | `oxicode-cli/src/tui/handlers.rs` | Phase 1 |

**Crate boundary**: `keybindings/` goes in `oxicode-tui` — it has no CLI dependencies (just key parsing and action mapping). `handlers.rs` in `oxicode-cli` consumes the `KeybindingsManager`.

### Phase 2: Differential Rendering (2 weeks)

| Task | Deliverable | Location | Dependencies |
|------|-------------|----------|-------------|
| `DiffBackend` | `impl ratatui::Backend for DiffBackend<W>` | `oxicode-tui/src/render/mod.rs` | None |
| Buffer diff algorithm | Line comparison, skip identical | `oxicode-tui/src/render/diff.rs` | None |
| ANSI tracker | `AnsiTracker` | `oxicode-tui/src/render/ansi.rs` | None |
| Terminal capabilities | `TerminalCapabilities::detect()` | `oxicode-tui/src/render/terminal.rs` | None |
| Integration | Swap `CrosstermBackend` → `DiffBackend` in `app.rs` | `oxicode-cli/src/tui/app.rs` | Phase 2 |

**Integration point**: In `app.rs`, change:
```rust
// Before:
let backend = CrosstermBackend::new(stdout);
let mut terminal = Terminal::new(backend)?;

// After:
let backend = DiffBackend::new(CrosstermBackend::new(stdout));
let mut terminal = Terminal::new(backend)?;
```

All `tui.draw(|f| render::draw(f, &mut state, &theme))` calls continue to work unchanged.

### Phase 3: Overlay Anchor Positioning (1 week)

| Task | Deliverable | Location | Dependencies |
|------|-------------|----------|-------------|
| `OverlayAnchor` enum + layout resolver | Shared anchor type | `oxicode-tui/src/overlay_anchor.rs` | None |
| `resolve_overlay_layout()` | 9-direction anchor positioning | `oxicode-cli/src/tui/overlay/anchor.rs` | Phase 3 |
| Line compositing | `composite_line_at()` | `oxicode-cli/src/tui/overlay/anchor.rs` | Phase 3 |
| Upgrade existing overlays | Add `OverlayLayout` to 7 components | `oxicode-cli/src/tui/overlay/*.rs` | Phase 3 |

**No trait changes**. Each overlay component gets an `OverlayLayout` field and uses `resolve_overlay_layout()` in its `render()` method instead of `centered_popup()`.

### Phase 4: Extended Completion (2 weeks)

| Task | Deliverable | Location | Dependencies |
|------|-------------|----------|-------------|
| `CompletionManager` | Unified completion dispatch | `oxicode-cli/src/tui/completion/mod.rs` | None |
| File path completion | `complete_path()` | `oxicode-cli/src/tui/completion/path.rs` | None |
| Fuzzy file search | `fuzzy_file_search()` via fd | `oxicode-cli/src/tui/completion/fuzzy_file.rs` | None |
| AppState integration | Extend `update_slash_completions()` pattern | `oxicode-cli/src/tui/app.rs` | Phase 4 |
| Render integration | Generalize `render_slash_popup_overlay()` | `oxicode-cli/src/tui/render.rs` | Phase 4 |

**Builds on existing**: `slash.rs` (1,030 LOC) and `app.rs` completion state are the foundation. New providers plug into the same pattern.

### Phase 5: Terminal Image (1 week)

| Task | Deliverable | Location | Dependencies |
|------|-------------|----------|-------------|
| Kitty image protocol | `encode_kitty()` + chunk splitting | `oxicode-tui/src/render/image.rs` | Phase 2 (TerminalCapabilities) |
| iTerm2 image protocol | `encode_iterm2()` | `oxicode-tui/src/render/image.rs` | Phase 2 |
| Dimension detection | `detect_dimensions()` (PNG, JPEG, GIF, WebP) | `oxicode-tui/src/render/image.rs` | None |
| Chat widget integration | Image rendering in message bubbles | `oxicode-tui/src/widgets/chat.rs` | Phase 5 |

### Phase 6: Editor Evaluation (Research, 1 week)

| Task | Deliverable | Location | Dependencies |
|------|-------------|----------|-------------|
| Audit `ratatui-textarea` | Document specific limitations vs pi editor | Analysis doc | None |
| Gap analysis | Identify missing features with user impact | Analysis doc | Phase 6 |
| Decision | Proceed with custom editor OR enhance textarea | RFC amendment | Phase 6 |

**Rationale for deferral**: `ratatui-textarea` already provides undo/redo, word movement, CJK, and bracketed paste. The original RFC proposed a 76KB custom editor at 3 weeks cost without documenting what's actually broken. A 1-week evaluation is warranted before committing to a full rewrite.

---

## 5. New Dependencies

```toml
# oxicode-tui/Cargo.toml
[dependencies]
unicode-segmentation = "1"      # Grapheme cluster segmentation (editor/input)
unicode-width = "0.2"           # Already used (extend)
base64 = "0.22"                 # Image encoding
memchr = "2"                    # Fast byte search (ANSI parsing)
```

Existing dependencies preserved: `ratatui`, `crossterm`, `pulldown-cmark`.

---

## 6. Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| **ratatui + DifferentialRenderer coexistence** | ratatui's `Terminal::draw()` computes a full `Buffer` every frame — the `DiffBackend` must intercept at the Backend level, not replace `Terminal::draw()` | `DiffBackend` implements `ratatui::Backend` trait, wrapping `CrosstermBackend`. ratatui's `Terminal` and all widget code unchanged. Integration: swap `CrosstermBackend::new(stdout)` → `DiffBackend::new(CrosstermBackend::new(stdout))` in `app.rs`. Verified: `render::draw()` at `render.rs:432` calls only `f.render_widget()` — no direct terminal writes. |
| **Kitty protocol environment dependency** | Hard to test outside Kitty/Ghostty/WezTerm | Virtual terminal mock + CI skip. `TerminalCapabilities::detect()` gracefully falls back to no-image mode. |
| **Crate boundary violations** | Previous draft put `overlay/` and `completion/` in `oxicode-tui`, but these depend on `AgentSession`, `Settings`, `auth_storage` (CLI types) | All overlay and completion code stays in `oxicode-cli/src/tui/`. Only `OverlayAnchor` enum and `DiffBackend` go in `oxicode-tui`. Verified against AGENTS.md: `oxicode-tui (independent) ← oxicode-cli`. |
| **Overlay migration disruption** | 7 existing overlay components (3,585 LOC) work today | Extend, don't replace. Add `OverlayLayout` field to each component. `OverlayComponent` trait unchanged. No parallel system. |
| **Editor replacement ROI** | 3-week custom editor with unclear benefit over `ratatui-textarea` | Deferred to Phase 6. Audit existing textarea first. If gaps are minor, enhance textarea instead of replacing. |
| **CJK/emoji edge cases** | Cursor positioning errors | `unicode-segmentation` implements full UAX #29. Same algorithm as pi. |
| **Dual overlay state in AppState** | Both `overlay: Option<AppOverlay>` and `overlay_state: Option<Box<dyn OverlayComponent>>` exist — potential confusion | Migrate remaining `AppOverlay` variants (Setup, ProviderConfig) to `OverlayComponent` implementations. Remove `AppOverlay` enum entirely. This simplifies `handlers.rs` dispatch. |

---

## 7. Success Criteria

- [ ] **Keybindings**: 31 default bindings + user rebinding from `settings.toml` + conflict detection. `handlers.rs` refactored from hardcoded match to `Action` dispatch.
- [ ] **Differential rendering**: `DiffBackend` implements `ratatui::Backend`. Only changed lines sent to terminal. Verified via `render::draw()` unchanged.
- [ ] **Overlay anchors**: All 7 existing overlay components support 9-direction anchor positioning. `OverlayComponent` trait unchanged.
- [ ] **Completion**: Slash commands (existing) + file path completion + fuzzy file search (fd). Unified `CompletionManager`.
- [ ] **Terminal image**: Kitty + iTerm2 protocol with auto-detection. Image display in chat widget.
- [ ] **Editor evaluation**: Documented gap analysis of `ratatui-textarea` vs pi editor. Decision on custom editor vs enhancement.
- [ ] **No regressions**: All existing overlay components (provider_select, model_select, questionnaire, resume_select, logout_select, router_setup, router_integration) continue working.
- [ ] **Crate boundaries clean**: `oxicode-tui` has zero imports from `oxicode-cli`. Verified by `cargo clippy --workspace`.
