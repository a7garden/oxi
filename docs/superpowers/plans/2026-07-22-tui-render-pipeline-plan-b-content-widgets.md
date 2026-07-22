# oxi-tui v2 — Plan B: Streaming Markdown + Content State + Chat Widgets

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the content+text+widget layers on top of Plan A's pipeline foundation — streaming markdown with checkpoint rendering, ChatLog/ChatView state model, concrete chat widgets (ChatView, MessageItem, ToolCall, Footer, Sticky, Overlay), and the RetainedChild<T> wrapper that makes per-subtree memoization automatic.

**Architecture:** Three layers stacked on Plan A:
1. **Memoization helper** — `RetainedChild<T>` wraps any Renderable, tracks last_hash, auto-skips render when unchanged. Composite widgets compose children via this wrapper instead of bare `Box<dyn Renderable>`.
2. **Text layer** — streaming markdown checkpoint renderer (stable prefix frozen, tail re-parsed per token), CJK-aware word wrap, optional syntect syntax highlighting.
3. **Content + Widget layer** — ChatLog (append-only), ChatView (scroll/follow/selection + Renderable), MessageItem, ToolCall, Footer, Sticky, Overlay panels, and full primitive set (Border, List, Scrollbar).

**Tech Stack:** Rust 2024, ratatui 0.30, pulldown-cmark (existing dep), syntect (feature-gated for syntax), unicode-width (existing), linkify (existing).

**Spec:** `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md` (§5.4, §6, §8, §10)
**Plan A:** `docs/superpowers/plans/2026-07-21-tui-render-pipeline-plan-a-foundation.md` (completed)

## Global Constraints

- Workspace: oxi monorepo, branch `oxi-tui-v2-plan-a` (continued from Plan A).
- Rust 2024. MSRV per workspace.
- Every module ≤ 500 LOC.
- All gates per task: `cargo fmt --check`, `cargo clippy -p oxi-tui -- -D warnings`, `cargo nextest run -p oxi-tui`.
- `oxi-tui` has zero oxi-* dependencies.
- No `unwrap()`/`expect()` in shipped code (tests only).
- `#![forbid(unsafe_code)]` enforced.
- Clean-room — no grok code copying.
- Plan A caveat fix already applied: `RenderCtx` carries `&Theme + &TerminalCaps`.

---

## File Structure (Plan B additions)

```
oxi-tui/src/
├── widget/
│   ├── retained_child.rs     NEW — RetainedChild<T> wrapper (~80 LOC)
│   ├── chat/                 NEW directory
│   │   ├── mod.rs            ChatView Renderable (~150 LOC)
│   │   ├── message_item.rs   per-message Renderable (~150 LOC)
│   │   ├── tool_call.rs      tool call card (~150 LOC)
│   │   └── spinner.rs        streaming indicator (~50 LOC)
│   ├── panel/                NEW directory
│   │   ├── mod.rs
│   │   ├── footer.rs         status + token bar (~150 LOC)
│   │   ├── sticky.rs         sticky headers/panels (~120 LOC)
│   │   └── overlay.rs        modal container (~130 LOC)
│   └── primitive/            EXPAND from Task 10
│       ├── mod.rs            (existing — re-exports)
│       ├── text.rs           (existing from Task 10)
│       ├── border.rs         NEW — box borders (~80 LOC)
│       ├── list.rs           NEW — virtualized list (~120 LOC)
│       └── scrollbar.rs      NEW — scroll indicator (~70 LOC)
├── content/                  NEW directory
│   ├── mod.rs
│   ├── chat_log.rs           append-only Vec<ChatMessage> (~150 LOC)
│   ├── chat_view.rs          scroll/follow/selection (~250 LOC)
│   ├── message.rs            ChatMessage + ContentBlock (~150 LOC)
│   └── streaming.rs          StreamingState (~120 LOC)
└── text/                     NEW directory
    ├── mod.rs
    ├── streaming_md.rs       checkpoint renderer (~250 LOC)
    ├── wrap.rs               CJK-aware word wrap (~150 LOC)
    └── syntax.rs             syntect + tmTheme (feature = "syntax") (~100 LOC)
```

**Plan B total target**: ~2,500 LOC across ~20 new files.

---

## Task 1: RetainedChild<T> wrapper

**Files:**
- Create: `oxi-tui/src/widget/retained_child.rs`
- Modify: `oxi-tui/src/widget/mod.rs`

**Interfaces:**
- Produces: `pub struct RetainedChild<T: Renderable> { inner: T, last_hash: u64, last_height: u16 }` with `new(child)`, `inner(&self) -> &T`, `inner_mut(&mut self) -> &mut T`, `render_if_changed(area, ctx) -> bool`.

- [ ] **Step 1: Write the failing test**

Create `oxi-tui/src/widget/retained_child.rs` with this skeleton + tests:
```rust
//! Per-child memoization wrapper. Composite widgets (ChatView, Footer, etc.)
//! wrap children in `RetainedChild<T>` to get automatic per-subtree skip
//! instead of each reinventing the pattern.
//!
//! ## The problem this solves
//!
//! `RetainedTree` only checks the root hash. During streaming, a token change
//! in ChatView trips the root hash → full tree re-render every frame. Without
//! `RetainedChild`, unchanged siblings (Footer, Input) re-render needlessly.
//!
//! ## The fix
//!
//! Composite widgets store children as `RetainedChild<T>` and call
//! `render_if_changed(area, ctx)` instead of `child.render(area, ctx)`. The
//! wrapper tracks `last_hash` and short-circuits when unchanged.

use ratatui::layout::Rect;

use crate::widget::{RenderCtx, Renderable};

pub struct RetainedChild<T: Renderable> {
    inner: T,
    last_hash: u64,
    last_height: u16,
}

impl<T: Renderable> RetainedChild<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, last_hash: 0, last_height: 0 }
    }

    pub fn inner(&self) -> &T { &self.inner }
    pub fn inner_mut(&mut self) -> &mut T { &mut self.inner }

    /// Compute height, caching if hash unchanged.
    pub fn height_for(&mut self, width: u16, ctx: &RenderCtx) -> u16 {
        let h = self.inner.content_hash();
        if h == self.last_hash && self.last_height > 0 {
            return self.last_height;
        }
        let height = self.inner.height_for(width, ctx);
        self.last_hash = h;
        self.last_height = height;
        height
    }

    /// Render only if hash changed since last render. Returns true if rendered.
    pub fn render_if_changed(&mut self, area: Rect, ctx: &mut RenderCtx) -> bool {
        let h = self.inner.content_hash();
        if h == self.last_hash {
            return false;
        }
        self.last_hash = h;
        self.inner.render(area, ctx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Text;
    use ratatui::backend::TestBackend;
    use ratatui::terminal::Terminal;
    use crate::theme::Theme;
    use crate::theme::TerminalCaps;

    #[test]
    fn first_render_always_renders() {
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let mut child = RetainedChild::new(Text::new("hello"));
            let rendered = child.render_if_changed(frame.area(), &mut ctx);
            assert!(rendered, "first render must always run");
        }).unwrap();
    }

    #[test]
    fn unchanged_hash_skips_render() {
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let mut child = RetainedChild::new(Text::new("hello"));
            let _ = child.render_if_changed(frame.area(), &mut ctx);
            let rendered2 = child.render_if_changed(frame.area(), &mut ctx);
            assert!(!rendered2, "second render with same hash must skip");
        }).unwrap();
    }

    #[test]
    fn content_change_triggers_rerender() {
        let backend = TestBackend::new(20, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            let mut child = RetainedChild::new(Text::new("hello"));
            let _ = child.render_if_changed(frame.area(), &mut ctx);
            child.inner_mut().set_content("world");
            let rendered2 = child.render_if_changed(frame.area(), &mut ctx);
            assert!(rendered2, "content change must trigger re-render");
        }).unwrap();
    }
}
```

- [ ] **Step 2: Wire into widget/mod.rs**

Add `pub mod retained_child; pub use retained_child::RetainedChild;`.

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p oxi-tui`
Expected: 100 tests pass (97 prior + 3 new).

- [ ] **Step 4: Commit**

```bash
git add oxi-tui/src/widget/
git commit -m "feat(oxi-tui/widget): add RetainedChild<T> per-subtree memoization wrapper

Composite widgets wrap children in RetainedChild<T> and call
render_if_changed() instead of bare render(). The wrapper tracks
last_hash and short-circuits when unchanged.

This is THE fix that makes streaming actually benefit from memoization:
without it, any token change trips the root hash and re-renders every
subtree every frame.

Plan B Task 1"
```

---

## Task 2: Content — ChatMessage + ContentBlock + StreamingState

**Files:**
- Create: `oxi-tui/src/content/mod.rs`
- Create: `oxi-tui/src/content/message.rs`
- Create: `oxi-tui/src/content/streaming.rs`
- Modify: `oxi-tui/src/lib.rs`

**Interfaces:**
- Produces: `pub struct ChatMessage { id, role, blocks, ... }`, `pub enum ContentBlock { Text(...), ToolCall(...), ToolResult(...), Thinking(...) }`, `pub enum MessageRole { User, Assistant, System, Tool }`, `pub struct StreamingState { active_stream_id, partial_text }`.

- [ ] **Step 1: Define domain types**

Read `oxi-tui-legacy/src/widgets/chat/types.rs` (67 LOC) for the legacy versions. Migrate to `oxi-tui/src/content/message.rs` with these public types:

```rust
pub type MessageId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageRole { User, Assistant, System, Tool }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    Text(String),
    Thinking(String),
    ToolCall { id: String, name: String, args: String, status: ToolCallStatus },
    ToolResult { call_id: String, content: String, is_error: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus { Pending, Running, Completed, Failed }

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: MessageId,
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub created_at: std::time::Instant,
}

impl ChatMessage {
    pub fn new(id: MessageId, role: MessageRole) -> Self { ... }
    pub fn text_content(&self) -> Option<&str> { ... }  // first Text block
    pub fn append_text(&mut self, text: &str) { ... }   // append to last Text block or create new
}
```

Add `content_hash` to ChatMessage (combines role + block hashes). Used by ChatLog's aggregate hash.

- [ ] **Step 2: StreamingState**

Create `oxi-tui/src/content/streaming.rs`:
```rust
pub type StreamId = u64;

#[derive(Debug, Clone, Default)]
pub struct StreamingState {
    pub active_stream: Option<StreamId>,
    pub partial_text: String,
}

impl StreamingState {
    pub fn is_streaming(&self) -> bool { self.active_stream.is_some() }
    pub fn start(&mut self, id: StreamId) { self.active_stream = Some(id); self.partial_text.clear(); }
    pub fn push_token(&mut self, token: &str) { self.partial_text.push_str(token); }
    pub fn finalize(&mut self) -> String { self.active_stream = None; std::mem::take(&mut self.partial_text) }
}
```

- [ ] **Step 3: mod.rs + lib.rs wire-up**

Create `oxi-tui/src/content/mod.rs`:
```rust
pub mod message;
pub mod streaming;
pub use message::{ChatMessage, ContentBlock, MessageId, MessageRole, ToolCallStatus};
pub use streaming::{StreamId, StreamingState};
```

Add `pub mod content;` to lib.rs.

- [ ] **Step 4: Tests**

Add tests for ChatMessage::append_text (creates new block if none, appends to last if Text, creates new if last is non-Text) and StreamingState lifecycle.

- [ ] **Step 5: Verify + Commit**

Run gates. Expected: 103+ tests pass (100 + 3+ new).

Commit: `feat(oxi-tui/content): ChatMessage + ContentBlock + StreamingState domain types`

---

## Task 3: Content — ChatLog (append-only)

**Files:**
- Create: `oxi-tui/src/content/chat_log.rs`
- Modify: `oxi-tui/src/content/mod.rs`

**Interfaces:**
- Produces: `pub struct ChatLog { messages, next_id, active_stream }` with `append_message`, `append_token`, `finalize_stream`, `messages()`, `active_stream()`, `content_hash()`.

- [ ] **Step 1: ChatLog implementation**

```rust
pub struct ChatLog {
    messages: Vec<ChatMessage>,
    next_id: MessageId,
    active_stream: StreamingState,
    cached_hash: u64,  // updated incrementally
}

impl ChatLog {
    pub fn new() -> Self { ... }
    pub fn append_message(&mut self, role: MessageRole) -> MessageId { ... }
    pub fn append_token(&mut self, token: &str) { ... }  // routes to active stream's last Assistant message
    pub fn finalize_stream(&mut self) { ... }
    pub fn messages(&self) -> &[ChatMessage] { &self.messages }
    pub fn active_stream(&self) -> Option<StreamId> { self.active_stream.active_stream }
    
    /// O(1) hash of the log state. Used by ChatView's content_hash.
    pub fn content_hash(&self) -> u64 {
        // Combine message count + last message hash + streaming state
        // (avoids iterating all messages every frame)
        ...
    }
}
```

**Critical**: `content_hash` must be O(1) — combine `messages.len()` + last message hash + streaming.partial_text hash. ChatView uses this to know when to re-render.

- [ ] **Step 2: Tests**

- `append_message_assigns_incrementing_ids`
- `append_token_creates_assistant_message_if_none_active`
- `append_token_appends_to_last_assistant_text_block`
- `finalize_stream_clears_active_stream`
- `content_hash_changes_on_append_token`

- [ ] **Step 3: Verify + Commit**

Commit: `feat(oxi-tui/content): ChatLog append-only message store with O(1) hash`

---

## Task 4: Content — ChatView (scroll/follow/selection)

**Files:**
- Create: `oxi-tui/src/content/chat_view.rs`
- Modify: `oxi-tui/src/content/mod.rs`

**Interfaces:**
- Produces: `pub struct ChatView { scroll_offset, follow_mode, selection, viewport_cache }` with scroll methods, `visible_msg_range(log, viewport_h) -> (usize, usize)`, `viewport_hash(log, width, theme) -> u64`.

- [ ] **Step 1: ChatView implementation**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowMode {
    #[default]
    Bottom,       // stick to bottom, follow new messages
    Pinned,       // user scrolled up, don't follow
}

pub struct ChatView {
    scroll_offset: u32,  // virtual coordinate (W1-style)
    follow_mode: FollowMode,
    selection: Option<Selection>,
    viewport_height_cache: u16,
}

impl ChatView {
    pub fn scroll_to_bottom(&mut self) { self.follow_mode = FollowMode::Bottom; }
    pub fn scroll_up(&mut self, lines: u32) { self.follow_mode = FollowMode::Pinned; self.scroll_offset = self.scroll_offset.saturating_add(lines); }
    pub fn scroll_down(&mut self, lines: u32) { ... }
    pub fn follow_mode(&self) -> FollowMode { self.follow_mode }
    
    /// Which messages are visible given the current scroll + viewport.
    /// Returns (start_idx, end_idx) into ChatLog::messages().
    pub fn visible_msg_range(&self, log: &ChatLog, viewport_h: u16) -> (usize, usize) { ... }
    
    /// Hash of viewport-visible content. Used by ChatView Renderable.
    pub fn viewport_hash(&self, log: &ChatLog, width: u16) -> u64 {
        let (start, end) = self.visible_msg_range(log, self.viewport_height_cache);
        let mut h = crate::widget::hash_str(&format!("{}:{}:{}", self.scroll_offset, start, end));
        for msg in &log.messages()[start..end] {
            h = crate::widget::hash_combine(h, msg.content_hash());
        }
        h
    }
}
```

- [ ] **Step 2: Tests**

- `follow_mode_starts_at_bottom`
- `scroll_up_switches_to_pinned`
- `scroll_to_bottom_switches_to_follow`
- `visible_msg_range_returns_last_n_at_bottom`
- `viewport_hash_changes_on_scroll`

- [ ] **Step 3: Verify + Commit**

Commit: `feat(oxi-tui/content): ChatView scroll/follow/selection state with viewport hash`

---

## Task 5: Text — Word wrap (CJK-aware)

**Files:**
- Create: `oxi-tui/src/text/mod.rs`
- Create: `oxi-tui/src/text/wrap.rs`
- Modify: `oxi-tui/src/lib.rs`

**Interfaces:**
- Produces: `pub fn wrap_lines(text: &str, width: usize) -> Vec<Line<'static>>` with CJK width handling, soft/hard break tracking.

- [ ] **Step 1: wrap.rs implementation**

Read `oxi-tui-legacy/src/widgets/chat/markdown.rs` for the legacy wrap_lines_styled. Migrate the core algorithm (without styling — just plain text wrapping). Use `unicode-width` for CJK.

```rust
//! Word-aware line wrapping with CJK support.
//!
//! Tracks soft vs hard line breaks so renderers know whether a visual
//! break is a word wrap (skip trailing whitespace) or a paragraph break
//! (preserve).

use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

pub fn wrap_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    // For each hard-separated paragraph (split on \n):
    //   Greedy fill words until adding the next would exceed `width`.
    //   CJK chars are width 2 and can break anywhere.
    //   ASCII words break on whitespace.
    ...
}

/// Wrap with style preservation. Each output Line carries the input style.
pub fn wrap_lines_styled(text: &str, width: usize, style: ratatui::style::Style) -> Vec<Line<'static>> {
    ...
}
```

- [ ] **Step 2: Tests**

- `wraps_long_line_at_word_boundary`
- `preserves_hard_breaks`
- `handles_cjk_double_width`
- `empty_string_returns_one_empty_line`

- [ ] **Step 3: Verify + Commit**

Commit: `feat(oxi-tui/text): CJK-aware word wrap`

---

## Task 6: Text — Streaming markdown checkpoint renderer

**Files:**
- Create: `oxi-tui/src/text/streaming_md.rs`
- Modify: `oxi-tui/src/text/mod.rs`

**Interfaces:**
- Produces: `pub struct StreamingMarkdown { frozen_lines, checkpoint, pending_tail }` with `push_token`, `lines(width, theme) -> Vec<Line>`.

- [ ] **Step 1: StreamingMarkdown implementation**

```rust
//! Streaming markdown renderer with checkpoint optimization.
//!
//! Stable content (behind a checkpoint boundary) is parsed once and frozen.
//! Only the unstable tail is re-parsed per token. Reduces CPU from O(N) per
//! token to O(tail) per token — critical for long streaming responses.
//!
//! Checkpoint boundaries:
//! - `\n\n` (paragraph break)
//! - Closed code block (```...```)
//! - Closed list item

use ratatui::text::Line;
use crate::theme::Theme;
use crate::text::wrap::wrap_lines;

pub struct StreamingMarkdown {
    /// Frozen, fully-rendered lines from content behind the last checkpoint.
    frozen_lines: Vec<Line<'static>>,
    /// Byte offset in the input where frozen content ends.
    checkpoint: usize,
    /// Accumulated text since last checkpoint (unstable tail).
    pending_tail: String,
    /// Full accumulated text (frozen + tail), for re-render on width change.
    full_text: String,
}

impl StreamingMarkdown {
    pub fn new() -> Self { ... }
    
    pub fn push_token(&mut self, token: &str) {
        self.pending_tail.push_str(token);
        self.full_text.push_str(token);
        self.advance_checkpoint();
    }
    
    /// Scan pending_tail for new stable boundaries; freeze everything up to them.
    fn advance_checkpoint(&mut self) {
        // Look for \n\n or closed ``` in pending_tail
        // Move frozen content from pending to frozen_lines (parsed + wrapped)
        ...
    }
    
    /// Current rendered lines: frozen + tail rendered fresh.
    pub fn lines(&self, width: u16, theme: &Theme) -> Vec<Line> {
        let mut out = self.frozen_lines.clone();
        // Parse + wrap the tail
        let tail_lines = parse_markdown_to_lines(&self.pending_tail, width, theme);
        out.extend(tail_lines);
        out
    }
    
    /// Force re-render of everything (e.g. on width change).
    pub fn invalidate(&mut self) {
        let full = std::mem::take(&mut self.full_text);
        self.frozen_lines.clear();
        self.pending_tail = full;
        self.checkpoint = 0;
    }
}

fn parse_markdown_to_lines(md: &str, width: u16, theme: &Theme) -> Vec<Line> {
    // Use pulldown-cmark to parse, then convert to ratatui Lines with theme styles.
    // Code blocks get monospace style, headings get bold, etc.
    ...
}
```

- [ ] **Step 2: Tests**

- `push_token_accumulates`
- `paragraph_break_advances_checkpoint`
- `closed_code_block_advances_checkpoint`
- `lines_returns_frozen_plus_tail`
- `invalidate_clears_frozen`
- `long_response_cpu_profile` — push 1000 tokens, verify only tail re-parses (count parse calls)

- [ ] **Step 3: Verify + Commit**

Commit: `feat(oxi-tui/text): streaming markdown checkpoint renderer`

---

## Task 7: Text — Syntax highlighting (feature-gated)

**Files:**
- Create: `oxi-tui/src/text/syntax.rs`
- Modify: `oxi-tui/Cargo.toml` (add syntect as optional dep)
- Modify: `oxi-tui/src/text/mod.rs`

**Interfaces:**
- Produces (under `feature = "syntax"`): `pub struct SyntaxHighlighter { syntax_set, theme_set }` with `highlight(code, lang) -> Vec<Line>`.

- [ ] **Step 1: Add syntect optional dep**

In `oxi-tui/Cargo.toml`:
```toml
[dependencies]
# ... existing ...
syntect = { version = "5", optional = true }

[features]
syntax = ["dep:syntect"]
```

- [ ] **Step 2: SyntaxHighlighter impl**

```rust
//! Optional syntect-based code highlighting. Enable with `feature = "syntax"`.
//! Without the feature, StreamingMarkdown falls back to plain monospace.

#[cfg(feature = "syntax")]
pub struct SyntaxHighlighter {
    syntax_set: syntect::parsing::SyntaxSet,
    theme_set: syntect::highlighting::ThemeSet,
}

#[cfg(feature = "syntax")]
impl SyntaxHighlighter {
    pub fn new() -> Self { ... }  // loads default syntaxes + themes
    
    pub fn highlight(&self, code: &str, lang: &str) -> Vec<ratatui::text::Line<'static>> {
        // Use syntect to highlight, convert anstyle → ratatui Style
        ...
    }
}

#[cfg(not(feature = "syntax"))]
pub struct SyntaxHighlighter;

#[cfg(not(feature = "syntax"))]
impl SyntaxHighlighter {
    pub fn new() -> Self { Self }
    pub fn highlight(&self, code: &str, _lang: &str) -> Vec<ratatui::text::Line<'static>> {
        // Plain monospace fallback
        code.lines().map(|l| ratatui::text::Line::raw(l.to_string())).collect()
    }
}
```

- [ ] **Step 3: Test**

- `highlights_rust_code` (under `#[cfg(feature = "syntax")]`)
- `fallback_returns_plain_lines` (under `#[cfg(not(feature = "syntax"))]`)

- [ ] **Step 4: Verify + Commit**

Commit: `feat(oxi-tui/text): optional syntect syntax highlighting (feature = "syntax")`

---

## Task 8: Widget — Border primitive

**Files:**
- Create: `oxi-tui/src/widget/primitive/border.rs`

**Interfaces:**
- Produces: `pub struct Border { title, style, bordered }` impl Renderable.

- [ ] **Step 1: Border implementation**

Simple Renderable that draws a ratatui Block::bordered() with optional title.

- [ ] **Step 2: Test**

- `border_renders_box_with_title`

- [ ] **Step 3: Commit**

Commit: `feat(oxi-tui/widget/primitive): Border`

---

## Task 9: Widget — List primitive (virtualized)

**Files:**
- Create: `oxi-tui/src/widget/primitive/list.rs`

**Interfaces:**
- Produces: `pub struct List<T: Renderable> { items: Vec<RetainedChild<T>>, scroll: usize, visible_count: u16 }` impl Renderable.

- [ ] **Step 1: List implementation**

Virtualized list — only renders items in the visible window. Uses RetainedChild<T> per item so unchanged items skip. content_hash combines scroll position + visible item hashes.

- [ ] **Step 2: Tests**

- `renders_only_visible_items`
- `scroll_changes_hash`
- `unchanged_items_skip_render`

- [ ] **Step 3: Commit**

Commit: `feat(oxi-tui/widget/primitive): virtualized List with RetainedChild per item`

---

## Task 10: Widget — Scrollbar primitive

**Files:**
- Create: `oxi-tui/src/widget/primitive/scrollbar.rs`

- [ ] Implement Scrollbar Renderable (position indicator, optional follow-mode awareness).

Commit: `feat(oxi-tui/widget/primitive): Scrollbar`

---

## Task 11: Widget/chat — MessageItem

**Files:**
- Create: `oxi-tui/src/widget/chat/mod.rs`
- Create: `oxi-tui/src/widget/chat/message_item.rs`

**Interfaces:**
- Produces: `pub struct MessageItem { message: ChatMessage, md_renderer: StreamingMarkdown, cached_hash: u64 }` impl Renderable.

- [ ] **Step 1: MessageItem implementation**

Each message wraps a StreamingMarkdown renderer for its text content. content_hash combines message.id + role + text hash + tool call status. render() lays out role label + content (text via md_renderer.lines(), tool calls via ToolCall widget).

- [ ] **Step 2: Tests**

- `renders_user_message_with_label`
- `renders_assistant_message_with_markdown`
- `streaming_token_updates_hash`

- [ ] **Step 3: Commit**

Commit: `feat(oxi-tui/widget/chat): MessageItem Renderable`

---

## Task 12: Widget/chat — ToolCall card

**Files:**
- Create: `oxi-tui/src/widget/chat/tool_call.rs`
- Create: `oxi-tui/src/widget/chat/spinner.rs`

- [ ] Implement ToolCall Renderable (bordered card with name + args + status + result). Spinner is a simple frame-based animation Renderable.

Tests: `tool_call_pending_renders_dots`, `tool_call_completed_renders_check`.

Commit: `feat(oxi-tui/widget/chat): ToolCall + Spinner`

---

## Task 13: Widget/chat — ChatView (the centerpiece)

**Files:**
- Modify: `oxi-tui/src/widget/chat/mod.rs`

**Interfaces:**
- Produces: `pub struct ChatView { log: ChatLog, view: ChatView, messages: Vec<RetainedChild<MessageItem>> }` impl Renderable.

- [ ] **Step 1: ChatView implementation**

```rust
pub struct ChatView {
    log: ChatLog,
    view: crate::content::ChatView,
    /// One RetainedChild per message. Indexed parallel to log.messages().
    /// During streaming, only the active message's hash changes → only it re-renders.
    items: Vec<RetainedChild<MessageItem>>,
}

impl Renderable for ChatView {
    fn content_hash(&self) -> u64 {
        // Combine log.content_hash() (which is O(1)) + view.viewport_hash()
        self.log.content_hash() ^ self.view.viewport_hash(&self.log, /* width */)
    }
    
    fn height_for(&self, width: u16, ctx: &RenderCtx) -> u16 {
        // Sum visible message heights
        ...
    }
    
    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        let (start, end) = self.view.visible_msg_range(&self.log, area.height);
        let mut y = area.y;
        for i in start..end {
            // Ensure items[i] exists (append if new message)
            if i >= self.items.len() {
                self.items.push(RetainedChild::new(MessageItem::new(self.log.messages()[i].clone())));
            }
            // Sync item with log message (in case of streaming token)
            self.items[i].inner_mut().sync_from(&self.log.messages()[i]);
            
            let h = self.items[i].height_for(width, ctx);
            let item_area = Rect { x: area.x, y, width: area.width, height: h };
            self.items[i].render_if_changed(item_area, ctx);
            y += h;
        }
    }
}
```

- [ ] **Step 2: Tests**

- `renders_visible_messages_only`
- `streaming_token_re_renders_only_active_message` (★ THE benefit test — verify only 1 RetainedChild re-renders)
- `scroll_skips_offscreen_messages`

- [ ] **Step 3: Commit**

Commit: `feat(oxi-tui/widget/chat): ChatView Renderable with per-message memoization`

---

## Task 14: Widget/panel — Footer

**Files:**
- Create: `oxi-tui/src/widget/panel/mod.rs`
- Create: `oxi-tui/src/widget/panel/footer.rs`

- [ ] Implement Footer Renderable (model name + token count + cost + spinner). Uses theme for styling.

Tests: `footer_renders_model_and_tokens`, `footer_spinner_animates_on_tick`.

Commit: `feat(oxi-tui/widget/panel): Footer`

---

## Task 15: Widget/panel — Sticky + Overlay

**Files:**
- Create: `oxi-tui/src/widget/panel/sticky.rs`
- Create: `oxi-tui/src/widget/panel/overlay.rs`

- [ ] Sticky: panel that sticks to top/bottom of viewport (for todo/issues panels).
- [ ] Overlay: modal container with border + optional title.

Tests: `sticky_renders_at_top`, `overlay_renders_centered_popup`.

Commit: `feat(oxi-tui/widget/panel): Sticky + Overlay`

---

## Task 16: Integration — ChatView in RetainedTree

**Files:**
- Create: `oxi-tui/tests/chat_integration.rs`

- [ ] Write integration test: build a composite tree with ChatView + Footer + Sticky. Stream tokens, verify:
  - Only ChatView's hash changes during streaming
  - Footer + Sticky skip render via RetainedChild
  - draw_frame returns Rendered (correct) but bytes flushed are minimal (DiffBackend cell diff)

Tests: `streaming_updates_only_chat_subtree`, `composite_tree_idle_when_chat_stable`.

Commit: `test(oxi-tui): ChatView integration in composite RetainedTree`

---

## Self-Review

After all tasks:
- [ ] Every module ≤500 LOC
- [ ] All gates pass: fmt, clippy, nextest, native-browser
- [ ] Composite widgets use RetainedChild for per-subtree skip
- [ ] Streaming markdown checkpoint actually reduces CPU (verify with bench test)
- [ ] ChatView streaming updates only re-render the active message
- [ ] No `unwrap()`/`expect()` in shipped code
- [ ] No `unsafe`
- [ ] No oxi-* deps
