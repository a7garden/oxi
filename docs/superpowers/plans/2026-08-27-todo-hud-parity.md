# Todo HUD/Session Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution setup:** This plan is executed in an isolated git worktree, created via `superpowers:using-git-worktrees` at execution start — not in the workspace this plan was written in. Do that first, then work through tasks in order (later groups depend on earlier ones).

**Goal:** Bring oxicode's `todo` tool TUI/session behavior to parity with
upstream `oh-my-pi`'s HUD, `/todo` command, subagent linkage, and session
policy (eager prelude, reminders, auto-clear), per
`docs/superpowers/specs/2026-08-27-todo-hud-parity-design.md`.

**Architecture:** Five component groups, landed in dependency order:
**A** (HUD renderer rewrite) → **C** (subagent linkage) → **E** (settings) →
**B** (`/todo` slash command) → **F** (message-visibility primitive) →
**D** (session policy: reminders, nudges, `ToolChoice`, eager prelude,
auto-clear). Each task is independently buildable/testable; groups are
ordered so nothing depends on a task that comes later in this document.

**Tech Stack:** Rust 2024, `ratatui` (TUI), `tokio`, `serde`. No new
dependencies except a possible platform clipboard shell-out (no crate).

## Global Constraints

- `cargo fmt` before every commit; `cargo clippy --workspace --all-targets -- -D warnings` must stay clean.
- Follow existing module conventions: `oxicode-agent` uses `anyhow::Result`/`ToolError` for tools, `thiserror` typed errors for library-facing APIs; `oxicode-cli` uses `anyhow::Result`.
- `parking_lot::RwLock`/`Mutex` guards must be dropped before any `.await`.
- New public API in `oxicode-ai`/`oxicode-agent` gets `#[oxicode_stable(since = "…")]` matching the crate's current unreleased version in `Cargo.toml` — check the version before adding the attribute.
- Every new pure function (collapse policy, fuzzy match, reconcile) is unit-tested in the same module (`#[cfg(test)] mod tests`), independent of any TUI rendering.
- `todo_panel_enabled` (existing setting) continues to gate the entire HUD; all new settings default to values that preserve today's behavior (`todo_eager_mode` defaults to `Off`).
- No commit touches more than one task group's files, so a regression bisects to one task.

---

## Group A — HUD renderer rewrite

### Task A1: Port `select_collapsed_todos` / `select_within_cap`

**Files:**
- Modify: `oxicode-agent/src/tools/todo.rs` (add near `normalize_in_progress`, after line ~469)

**Interfaces:**
- Produces: `pub fn select_collapsed_todos<'a>(tasks: &'a [TodoItem], is_matched: impl Fn(&TodoItem) -> bool, cap: usize) -> CollapsedSelection<'a>` and `pub struct CollapsedSelection<'a> { pub items: Vec<&'a TodoItem>, pub summary: Option<String> }`. Consumed by Task A3.

- [ ] **Step 1: Write the failing tests**

```rust
// in oxicode-agent/src/tools/todo.rs, inside `mod tests`
fn task(content: &str, status: TodoStatus) -> TodoItem {
    TodoItem { content: content.into(), status, notes: None, block_reason: None }
}

#[test]
fn select_collapsed_todos_leads_with_in_progress_then_pending() {
    let tasks = vec![
        task("a", TodoStatus::Completed),
        task("b", TodoStatus::InProgress),
        task("c", TodoStatus::Pending),
        task("d", TodoStatus::Pending),
        task("e", TodoStatus::Pending),
        task("f", TodoStatus::Pending),
    ];
    let sel = select_collapsed_todos(&tasks, |_| false, 3);
    // 1 closed-context row ("a") + in_progress ("b") + 2 pending fill = cap 3 open rows total
    let contents: Vec<&str> = sel.items.iter().map(|t| t.content.as_str()).collect();
    assert_eq!(contents, vec!["a", "b", "c", "d"]);
    assert_eq!(sel.summary.as_deref(), Some("… 2 more todos"));
}

#[test]
fn select_collapsed_todos_all_closed_falls_back_to_closed_tasks() {
    let tasks = vec![task("a", TodoStatus::Completed), task("b", TodoStatus::Abandoned)];
    let sel = select_collapsed_todos(&tasks, |_| false, 5);
    assert_eq!(sel.items.len(), 2);
    assert!(sel.summary.is_none());
}

#[test]
fn select_collapsed_todos_matched_pending_counts_as_active() {
    let tasks = vec![
        task("a", TodoStatus::Pending),
        task("b", TodoStatus::Pending),
        task("c", TodoStatus::Pending),
    ];
    // "b" is matched to a live subagent -> treated as active, leads the fill.
    let sel = select_collapsed_todos(&tasks, |t| t.content == "b", 2);
    let contents: Vec<&str> = sel.items.iter().map(|t| t.content.as_str()).collect();
    assert_eq!(contents, vec!["b", "c"]);
    assert_eq!(sel.summary.as_deref(), Some("… 1 more todo"));
}

#[test]
fn select_collapsed_todos_no_cap_overflow_returns_everything() {
    let tasks = vec![task("a", TodoStatus::Pending), task("b", TodoStatus::Pending)];
    let sel = select_collapsed_todos(&tasks, |_| false, 5);
    assert_eq!(sel.items.len(), 2);
    assert!(sel.summary.is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-agent select_collapsed_todos`
Expected: FAIL — `select_collapsed_todos` not found.

- [ ] **Step 3: Implement**

```rust
/// One prior closed task stays visible above the open window so a
/// completion is seen as it happens, not silently dropped. Ports omp's
/// `COLLAPSED_CLOSED_CONTEXT` (`todo.ts:275`).
const COLLAPSED_CLOSED_CONTEXT: usize = 1;

/// Result of [`select_collapsed_todos`]: the rows to render plus an
/// optional "… N more" summary line.
pub struct CollapsedSelection<'a> {
    pub items: Vec<&'a TodoItem>,
    pub summary: Option<String>,
}

fn is_closed(t: &TodoItem) -> bool {
    matches!(t.status, TodoStatus::Completed | TodoStatus::Abandoned)
}

fn is_active(t: &TodoItem, is_matched: &impl Fn(&TodoItem) -> bool) -> bool {
    t.status == TodoStatus::InProgress || (t.status == TodoStatus::Pending && is_matched(t))
}

fn select_within_cap<'a>(
    base: &[&'a TodoItem],
    is_matched: &impl Fn(&TodoItem) -> bool,
    cap: usize,
) -> CollapsedSelection<'a> {
    if base.len() <= cap {
        return CollapsedSelection { items: base.to_vec(), summary: None };
    }
    let active: Vec<&'a TodoItem> = base.iter().copied().filter(|t| is_active(t, is_matched)).collect();
    if active.len() > cap {
        let hidden = active.len() - cap;
        return CollapsedSelection {
            items: active.into_iter().take(cap).collect(),
            summary: Some(format!("… {hidden} more active todo{}", if hidden == 1 { "" } else { "s" })),
        };
    }
    let first_active_idx = active.first().and_then(|f| base.iter().position(|t| std::ptr::eq(*t, *f))).unwrap_or(0);
    let mut items = active.clone();
    for &t in base.iter().skip(first_active_idx) {
        if items.len() >= cap {
            break;
        }
        if !is_active(t, is_matched) && !items.iter().any(|x| std::ptr::eq(*x, t)) {
            items.push(t);
        }
    }
    let hidden = base.len() - items.len();
    let summary = (hidden > 0).then(|| format!("… {hidden} more todo{}", if hidden == 1 { "" } else { "s" }));
    CollapsedSelection { items, summary }
}

/// Walking-viewport selection for a phase's collapsed todo preview.
/// Ports omp's `selectCollapsedTodos` (`todo.ts:332-350`).
pub fn select_collapsed_todos<'a>(
    tasks: &'a [TodoItem],
    is_matched: impl Fn(&TodoItem) -> bool,
    cap: usize,
) -> CollapsedSelection<'a> {
    let open: Vec<&'a TodoItem> = tasks.iter().filter(|t| !is_closed(t)).collect();
    if open.is_empty() {
        let all: Vec<&'a TodoItem> = tasks.iter().collect();
        return select_within_cap(&all, &is_matched, cap);
    }
    let lead: Vec<&'a TodoItem> = tasks.iter().filter(|t| is_closed(t)).rev().take(COLLAPSED_CLOSED_CONTEXT).collect();
    let mut lead: Vec<&'a TodoItem> = lead.into_iter().rev().collect();
    let selected = select_within_cap(&open, &is_matched, cap);
    lead.extend(selected.items);
    CollapsedSelection { items: lead, summary: selected.summary }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-agent select_collapsed_todos`
Expected: PASS (all 4 tests).

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/tools/todo.rs
git commit -m "feat(todo): port walking-viewport collapse policy from omp"
```

---

### Task A2: Tree glyphs + styled `format_todo_line`

**Files:**
- Modify: `oxicode-vtui/src/theme/mod.rs` (or wherever `ThemeStyles`/theme constants live — locate via `grep "pub struct ThemeStyles"` at task start; add three glyph constants, not full theme entries, since these are structural not color)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (add `format_todo_line`, replacing the marker logic inside the current `render_todo_pane`)

**Interfaces:**
- Consumes: `TodoItem` (`oxicode_agent::tools::todo::TodoItem`), `ThemeStyles` (`active_styles()`).
- Produces: `fn format_todo_line(todo: &TodoItem, matched: bool, styles: &ThemeStyles) -> Line<'static>`, three glyph constants `TREE_BRANCH = "├─"`, `TREE_VERTICAL = "│ "`, `TREE_HOOK = "└─"` (plain `&str` constants in `main_loop.rs` — no new theme infra needed since box-drawing glyphs are not themed colors, just characters; confirmed no existing tree-glyph module in `oxicode-vtui/src` in Task A3's grep).

- [ ] **Step 1: Write the failing test**

```rust
// in oxicode-cli/src/tui_vt/main_loop.rs, inside `mod tests`
#[test]
fn format_todo_line_shows_block_reason_and_notes_marker() {
    let styles = active_styles();
    let todo = oxicode_agent::tools::todo::TodoItem {
        content: "Wire OAuth".into(),
        status: oxicode_agent::tools::todo::TodoStatus::Blocked,
        notes: Some(vec!["waiting on vendor".into()]),
        block_reason: Some("vendor sandbox pending".into()),
    };
    let line = format_todo_line(&todo, false, &styles);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("Wire OAuth"));
    assert!(text.contains("blocked: vendor sandbox pending"));
    assert!(text.contains("·1"));
}

#[test]
fn format_todo_line_abandoned_is_strikethrough() {
    let styles = active_styles();
    let todo = oxicode_agent::tools::todo::TodoItem {
        content: "Drop this".into(),
        status: oxicode_agent::tools::todo::TodoStatus::Abandoned,
        notes: None,
        block_reason: None,
    };
    let line = format_todo_line(&todo, false, &styles);
    assert!(line.spans.iter().any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT)));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli format_todo_line`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement**

```rust
use oxicode_agent::tools::todo::{TodoItem, TodoStatus};

/// Format one todo row: marker + content + status-specific suffix + notes
/// marker. Ports omp's `#formatTodoLine` (`interactive-mode.ts:2326-2341`).
fn format_todo_line(todo: &TodoItem, matched: bool, styles: &ThemeStyles) -> Line<'static> {
    let notes_marker = match todo.notes.as_ref().map(|n| n.len()).unwrap_or(0) {
        0 => String::new(),
        n => format!(" ·{n}"),
    };
    let (marker, color, strike, suffix) = match todo.status {
        TodoStatus::Completed => ("✓", styles.foreground, true, String::new()),
        TodoStatus::InProgress => ("▸", styles.primary.get_fg_color().unwrap_or(styles.foreground), false, String::new()),
        TodoStatus::Abandoned => ("☐", styles.error.get_fg_color().unwrap_or(styles.foreground), true, String::new()),
        TodoStatus::Blocked => {
            let reason = todo
                .block_reason
                .as_deref()
                .map(|r| format!(" (blocked: {r})"))
                .unwrap_or_else(|| " (blocked)".to_string());
            ("☐", styles.info.get_fg_color().unwrap_or(styles.foreground), false, reason)
        }
        TodoStatus::Pending if matched => ("☐", styles.primary.get_fg_color().unwrap_or(styles.foreground), false, String::new()),
        TodoStatus::Pending => ("☐", styles.secondary.get_fg_color().unwrap_or(styles.foreground), false, String::new()),
    };
    let mut text_style = Style::default().fg(color_from_anstyle(Some(color)));
    if strike {
        text_style = text_style.add_modifier(Modifier::CROSSED_OUT);
    }
    Line::from(vec![
        Span::styled(format!("{marker} "), Style::default().fg(color_from_anstyle(Some(color)))),
        Span::styled(format!("{}{}{}", todo.content, suffix, notes_marker), text_style),
    ])
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli format_todo_line`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(todo): render blocked reason, notes marker, abandoned strikethrough"
```

---

### Task A3: Phase-tree HUD renderer (replaces flatten + flat list)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs`
  - `RenderState` struct: replace `pub todo_items: Vec<(String, TodoStatus)>` with `pub todo_phases: Vec<TodoPhase>` and add `pub todo_expanded: bool` (default `false`)
  - Remove `flatten_todo_items` and its test `flatten_todo_items_preserves_order_and_status`
  - Rewrite `render_todo_pane` to consume `&[TodoPhase]`
  - Update the two call sites (`snapshot.todo_items = flatten_todo_items(...)` → `snapshot.todo_phases = provider.get_phases()`; `if !state.todo_items.is_empty()` → `if !state.todo_phases.is_empty()`)

**Interfaces:**
- Consumes: `select_collapsed_todos` (Task A1), `format_todo_line` (Task A2), `TodoPhase`/`TodoItem` (`oxicode_agent::tools::todo`).
- Produces: `render_todo_pane(frame: &mut Frame<'_>, area: Rect, phases: &[TodoPhase], expanded: bool, is_matched: impl Fn(&TodoItem) -> bool) -> u16` — now returns the row count consumed (needed by Task A4's compact-mode caller and any future caller that must reserve space), matching `render_queue_pane`'s existing return-height convention (`main_loop.rs:5824`).

- [ ] **Step 1: Write the failing test**

```rust
// in oxicode-cli/src/tui_vt/main_loop.rs, inside `mod tests`
fn phase(name: &str, tasks: Vec<(&str, TodoStatus)>) -> TodoPhase {
    TodoPhase {
        name: name.into(),
        tasks: tasks.into_iter().map(|(c, s)| oxicode_agent::tools::todo::TodoItem {
            content: c.into(), status: s, notes: None, block_reason: None,
        }).collect(),
    }
}

#[test]
fn render_todo_pane_multi_phase_shows_roman_header_and_progress() {
    let phases = vec![
        phase("Foundation", vec![("a", TodoStatus::Completed), ("b", TodoStatus::Completed)]),
        phase("Auth", vec![("c", TodoStatus::Completed), ("d", TodoStatus::InProgress), ("e", TodoStatus::Pending)]),
    ];
    let backend = ratatui::backend::TestBackend::new(60, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| {
        render_todo_pane(f, f.area(), &phases, false, |_| false);
    }).unwrap();
    let text = terminal.backend().buffer().content.iter().map(|c| c.symbol()).collect::<String>();
    assert!(text.contains("II. Auth"));
    assert!(text.contains("1/3"));
}

#[test]
fn render_todo_pane_single_phase_has_no_roman_header() {
    let phases = vec![phase("Todos", vec![("a", TodoStatus::Pending)])];
    let backend = ratatui::backend::TestBackend::new(60, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| {
        render_todo_pane(f, f.area(), &phases, false, |_| false);
    }).unwrap();
    let text = terminal.backend().buffer().content.iter().map(|c| c.symbol()).collect::<String>();
    assert!(!text.contains("I. Todos"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli render_todo_pane`
Expected: FAIL — signature mismatch (old `render_todo_pane` takes `&[(String, TodoStatus)]`).

- [ ] **Step 3: Implement**

```rust
const TREE_BRANCH: &str = "├─";
const TREE_VERTICAL: &str = "│ ";
const TREE_HOOK: &str = "└";
const SUBSEQUENT_STAGE_CAP: usize = 4;
const ACTIVE_TASK_CAP: usize = 5;

fn active_phase_index(phases: &[TodoPhase]) -> usize {
    phases
        .iter()
        .position(|p| p.tasks.iter().any(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress)))
        .unwrap_or_else(|| phases.len().saturating_sub(1))
}

fn closed_count(tasks: &[oxicode_agent::tools::todo::TodoItem]) -> usize {
    tasks.iter().filter(|t| matches!(t.status, TodoStatus::Completed | TodoStatus::Abandoned)).count()
}

/// Render the sticky todo HUD: phase tree + progress spine. Ports omp's
/// `#renderTodoList` (`interactive-mode.ts:2529-2643`). Returns rows used.
fn render_todo_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    phases: &[TodoPhase],
    expanded: bool,
    is_matched: impl Fn(&oxicode_agent::tools::todo::TodoItem) -> bool,
) -> u16 {
    let phases: Vec<&TodoPhase> = phases.iter().filter(|p| !p.tasks.is_empty()).collect();
    if phases.is_empty() {
        return 0;
    }
    let styles = active_styles();
    let multi_phase = phases.len() > 1;
    let active_idx = active_phase_index(&phases.iter().map(|p| (*p).clone()).collect::<Vec<_>>());

    let render_tasks = |phase: &TodoPhase| -> Vec<Line<'static>> {
        if expanded {
            phase.tasks.iter().map(|t| format_todo_line(t, is_matched(t), &styles)).collect()
        } else {
            let sel = oxicode_agent::tools::todo::select_collapsed_todos(&phase.tasks, &is_matched, ACTIVE_TASK_CAP);
            let mut lines: Vec<Line<'static>> = sel.items.iter().map(|t| format_todo_line(t, is_matched(t), &styles)).collect();
            if let Some(summary) = sel.summary {
                lines.push(Line::from(Span::styled(summary, Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())))));
            }
            lines
        }
    };

    let base_idx = if expanded { 0 } else { active_idx };
    let phase_slice: &[&TodoPhase] = if expanded {
        &phases[base_idx..]
    } else {
        &phases[base_idx..(base_idx + 1 + SUBSEQUENT_STAGE_CAP).min(phases.len())]
    };
    let hidden_stages = phases.len() - base_idx - phase_slice.len();

    let mut content_lines: Vec<Line<'static>> = Vec::new();
    let mut spine_filled: Vec<bool> = Vec::new();
    for (i, phase) in phase_slice.iter().enumerate() {
        let one_based = base_idx + i + 1;
        let is_active = base_idx + i == active_idx;
        let done = closed_count(&phase.tasks);
        let header_text = if multi_phase {
            format!("{} · {done}/{}", phase_display_name(&phase.name, one_based), phase.tasks.len())
        } else {
            phase.name.clone()
        };
        let header_style = if is_active {
            Style::default().fg(color_from_anstyle(styles.primary.get_fg_color())).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color_from_anstyle(Some(styles.secondary.get_fg_color().unwrap_or(styles.foreground))))
        };
        content_lines.push(Line::from(Span::styled(header_text, header_style)));
        spine_filled.push(false);
        if is_active || expanded {
            for line in render_tasks(phase) {
                content_lines.push(line);
                spine_filled.push(false);
            }
        }
    }
    if hidden_stages > 0 {
        content_lines.push(Line::from(Span::styled(
            format!("… {hidden_stages} more stage{}", if hidden_stages == 1 { "" } else { "s" }),
            Style::default().fg(color_from_anstyle(styles.secondary.get_fg_color())),
        )));
        spine_filled.push(false);
    }

    let total: usize = phases.iter().map(|p| p.tasks.len()).sum();
    let closed: usize = phases.iter().map(|p| closed_count(&p.tasks)).sum();
    let path_len = spine_filled.len() + 1; // +1 for the closing hook row
    let mut filled = if total == 0 { 0 } else { (closed * path_len) / total };
    if closed > 0 {
        filled = filled.max(1);
    }
    if closed < total {
        filled = filled.min(path_len.saturating_sub(1));
    }

    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
        "TODO",
        Style::default().fg(color_from_anstyle(styles.primary.get_fg_color())).add_modifier(Modifier::BOLD),
    ))];
    for (i, content) in content_lines.into_iter().enumerate() {
        let glyph = if i == 0 { TREE_BRANCH } else { TREE_VERTICAL };
        let glyph_color = if i < filled { styles.primary.get_fg_color() } else { styles.secondary.get_fg_color() };
        let mut spans = vec![Span::styled(format!(" {glyph}"), Style::default().fg(color_from_anstyle(glyph_color)))];
        spans.extend(content.spans);
        lines.push(Line::from(spans));
    }
    let hook_color = if content_lines_len_is_filled(filled, spine_filled.len()) { styles.primary.get_fg_color() } else { styles.secondary.get_fg_color() };
    lines.push(Line::from(Span::styled(format!(" {TREE_HOOK}"), Style::default().fg(color_from_anstyle(hook_color)))));

    let height = lines.len() as u16;
    let render_area = Rect { x: area.x, y: area.y, width: area.width, height };
    frame.render_widget(Paragraph::new(lines), render_area);
    height
}

fn content_lines_len_is_filled(filled: usize, total_rows: usize) -> bool {
    filled > total_rows
}

/// "I. Foundation", "II. Auth", … Reuses `roman_numeral` already in `todo.rs`.
fn phase_display_name(name: &str, one_based: usize) -> String {
    format!("{}. {name}", oxicode_agent::tools::todo::roman_numeral(one_based))
}
```

Note: `roman_numeral` is currently a private `fn` in `todo.rs` (line 567) —
change it to `pub fn` as part of this task (it already has zero external
callers to break).

Update `RenderState`:

```rust
// replace:
// pub todo_items: Vec<(String, TodoStatus)>,
pub todo_phases: Vec<oxicode_agent::tools::todo::TodoPhase>,
pub todo_expanded: bool,
```

and the two call sites:

```rust
// was: snapshot.todo_items = flatten_todo_items(&provider.get_phases());
snapshot.todo_phases = provider.get_phases();

// was: if !state.todo_items.is_empty() { render_todo_pane(frame, pinned_area, &state.todo_items); }
if !state.todo_phases.is_empty() {
    let used = render_todo_pane(frame, pinned_area, &state.todo_phases, state.todo_expanded, |_| false);
    pinned_area.y = pinned_area.y.saturating_add(used);
    pinned_area.height = pinned_area.height.saturating_sub(used);
}
```

(The `|_| false` matched-closure placeholder is replaced with the real
subagent-matching closure in Task C2 — leaving it `false` here keeps this
task's diff isolated to the renderer shape change, matching the "commit
frequently" constraint.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli render_todo_pane`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs oxicode-agent/src/tools/todo.rs
git commit -m "feat(todo): phase-tree HUD renderer with progress spine, drop flat list"
```

---

### Task A4: Compact mode (<18 rows)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (near the `render_todo_pane` call site in the main frame function, ~line 4385)

**Interfaces:**
- Consumes: `frame.area().height`, `RenderState.todo_phases`.
- Produces: `fn render_todo_compact_line(phases: &[TodoPhase]) -> Line<'static>`; the frame-render call site branches on height.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn render_todo_compact_line_shows_counts_and_active_task() {
    let phases = vec![phase("Auth", vec![("a", TodoStatus::Completed), ("b", TodoStatus::InProgress)])];
    let line = render_todo_compact_line(&phases);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("TODO 1/2"));
    assert!(text.contains("b"));
}

#[test]
fn render_todo_compact_line_all_done_shows_done_marker() {
    let phases = vec![phase("Auth", vec![("a", TodoStatus::Completed)])];
    let line = render_todo_compact_line(&phases);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("done"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli render_todo_compact_line`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement**

```rust
const TODO_COMPACT_ROWS_THRESHOLD: u16 = 18;

fn next_actionable_task(phases: &[TodoPhase]) -> Option<&oxicode_agent::tools::todo::TodoItem> {
    let mut first_pending = None;
    for phase in phases {
        for task in &phase.tasks {
            if task.status == TodoStatus::InProgress {
                return Some(task);
            }
            if first_pending.is_none() && task.status == TodoStatus::Pending {
                first_pending = Some(task);
            }
        }
    }
    first_pending
}

fn render_todo_compact_line(phases: &[TodoPhase]) -> Line<'static> {
    let styles = active_styles();
    let total: usize = phases.iter().map(|p| p.tasks.len()).sum();
    let closed: usize = phases.iter().map(|p| closed_count(&p.tasks)).sum();
    let mut spans = vec![Span::styled(
        format!("TODO {closed}/{total} "),
        Style::default().fg(color_from_anstyle(styles.primary.get_fg_color())).add_modifier(Modifier::BOLD),
    )];
    match next_actionable_task(phases) {
        Some(task) => spans.extend(format_todo_line(task, false, &styles).spans),
        None => spans.push(Span::styled("✓ done", Style::default().fg(color_from_anstyle(styles.foreground.into())))),
    }
    Line::from(spans)
}
```

Call-site branch (replacing the plain `render_todo_pane` call from Task A3):

```rust
if !state.todo_phases.is_empty() {
    if frame.area().height < TODO_COMPACT_ROWS_THRESHOLD {
        let line = render_todo_compact_line(&state.todo_phases);
        frame.render_widget(Paragraph::new(vec![line]), Rect { height: 1, ..pinned_area });
        pinned_area.y = pinned_area.y.saturating_add(1);
        pinned_area.height = pinned_area.height.saturating_sub(1);
    } else {
        let used = render_todo_pane(frame, pinned_area, &state.todo_phases, state.todo_expanded, |_| false);
        pinned_area.y = pinned_area.y.saturating_add(used);
        pinned_area.height = pinned_area.height.saturating_sub(used);
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli render_todo_compact_line`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(todo): compact single-line HUD under 18 terminal rows"
```

---

## Group C — Subagent linkage

### Task C1: `reconcile_with_subagents` pure function

**Files:**
- Modify: `oxicode-agent/src/tools/todo.rs` (near `todo_matches_any_description`, line ~500)

**Interfaces:**
- Consumes: `todo_matches_any_description` (existing).
- Produces: `pub fn reconcile_with_subagents(phases: &[TodoPhase], completed_descriptions: &[String]) -> (Vec<TodoPhase>, bool)`. Consumed by Task C2.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn reconcile_closes_matching_open_task() {
    let phases = vec![TodoPhase {
        name: "Auth".into(),
        tasks: vec![task("implement authentication module", TodoStatus::Pending)],
    }];
    let (updated, mutated) = reconcile_with_subagents(&phases, &["authentication module".to_string()]);
    assert!(mutated);
    assert_eq!(updated[0].tasks[0].status, TodoStatus::Completed);
}

#[test]
fn reconcile_clears_block_reason_on_close() {
    let mut t = task("implement authentication module", TodoStatus::Blocked);
    t.block_reason = Some("waiting on subagent".into());
    let phases = vec![TodoPhase { name: "Auth".into(), tasks: vec![t] }];
    let (updated, mutated) = reconcile_with_subagents(&phases, &["authentication module".to_string()]);
    assert!(mutated);
    assert_eq!(updated[0].tasks[0].status, TodoStatus::Completed);
    assert!(updated[0].tasks[0].block_reason.is_none());
}

#[test]
fn reconcile_does_not_touch_already_closed_or_unmatched() {
    let phases = vec![TodoPhase {
        name: "Auth".into(),
        tasks: vec![task("unrelated task", TodoStatus::Pending), task("done already", TodoStatus::Completed)],
    }];
    let (_updated, mutated) = reconcile_with_subagents(&phases, &["authentication module".to_string()]);
    assert!(!mutated);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-agent reconcile_`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement**

```rust
/// Auto-complete open todos whose content matches a subagent that finished
/// successfully. Ports omp's `#reconcileTodosWithSubagents`
/// (`interactive-mode.ts:2369-2404`). Idempotent: never touches an already
/// closed task. Failed/aborted subagents are the caller's responsibility to
/// exclude from `completed_descriptions` — this function only matches.
pub fn reconcile_with_subagents(phases: &[TodoPhase], completed_descriptions: &[String]) -> (Vec<TodoPhase>, bool) {
    if completed_descriptions.is_empty() {
        return (phases.to_vec(), false);
    }
    let mut mutated = false;
    let updated = phases
        .iter()
        .map(|phase| TodoPhase {
            name: phase.name.clone(),
            tasks: phase
                .tasks
                .iter()
                .map(|task| {
                    if !matches!(task.status, TodoStatus::Pending | TodoStatus::InProgress | TodoStatus::Blocked) {
                        return task.clone();
                    }
                    if !todo_matches_any_description(&task.content, completed_descriptions) {
                        return task.clone();
                    }
                    mutated = true;
                    TodoItem {
                        content: task.content.clone(),
                        status: TodoStatus::Completed,
                        notes: task.notes.clone(),
                        block_reason: None,
                    }
                })
                .collect(),
        })
        .collect();
    (updated, mutated)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-agent reconcile_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/tools/todo.rs
git commit -m "feat(todo): auto-reconcile open todos with completed subagents"
```

---

### Task C2: Wire matched-highlight + reconcile into the frame loop

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (the block around line 1405-1409 that refreshes `snapshot.todo_phases`)

**Interfaces:**
- Consumes: `reconcile_with_subagents` (C1), `AgentPoolProvider::list_agents()` (`oxicode_agent::tools::AgentPoolProvider`, already on `RenderState` or reachable the same way the Hub overlay reaches it — locate the exact field name via `grep "agent_pool" oxicode-cli/src/tui_vt/main_loop.rs` at task start, since the design doc's earlier grep found `AgentPoolProvider` defined but did not confirm the `RenderState` field name).
- Produces: the `snapshot.todo_phases` refresh block now also reconciles and computes a live `is_matched` closure passed to both `render_todo_pane` and `render_todo_compact_line` call sites (replacing the `|_| false` placeholders from Tasks A3/A4).

- [ ] **Step 1: Write the failing test**

Add an integration-style test in `main_loop.rs`'s `mod tests` using a fake
`TodoStateProvider` and a fake `AgentPoolProvider` (follow the existing fake
pattern used for `TodoStateProvider` tests near
`flatten_todo_items_preserves_order_and_status`, before its removal in Task
A3 — check `git log -p` on that test if it's already gone by the time this
task runs) asserting that after one frame refresh with a completed subagent
whose description matches an open todo, `snapshot.todo_phases` shows that
task `Completed`.

```rust
#[test]
fn frame_refresh_reconciles_todo_with_completed_subagent() {
    let todo_state = std::sync::Arc::new(crate::store::todo_state::TodoState::with_phases(vec![
        oxicode_agent::tools::todo::TodoPhase {
            name: "Auth".into(),
            tasks: vec![oxicode_agent::tools::todo::TodoItem {
                content: "implement authentication module".into(),
                status: oxicode_agent::tools::todo::TodoStatus::Pending,
                notes: None,
                block_reason: None,
            }],
        },
    ]));
    let pool = std::sync::Arc::new(FakeAgentPool::with_completed("authentication module"));
    let phases = refresh_todo_phases(&todo_state, &pool);
    assert_eq!(phases[0].tasks[0].status, oxicode_agent::tools::todo::TodoStatus::Completed);
}
```

(`FakeAgentPool` is a small test-only struct implementing
`AgentPoolProvider` with a hardcoded `list_agents()` — write it inline in
`mod tests` next to the fake `TodoStateProvider`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli frame_refresh_reconciles_todo_with_completed_subagent`
Expected: FAIL — `refresh_todo_phases` not found.

- [ ] **Step 3: Implement**

```rust
/// Pull the latest todo phases from `provider`, auto-reconciling against
/// `pool`'s completed sub-agents, and commit the reconciled result back
/// through the provider when it changed (avoids needless writes on every
/// frame). Returns the phases to display this frame.
fn refresh_todo_phases(
    provider: &std::sync::Arc<dyn TodoStateProvider>,
    pool: &std::sync::Arc<dyn oxicode_agent::tools::AgentPoolProvider>,
) -> Vec<oxicode_agent::tools::todo::TodoPhase> {
    let phases = provider.get_phases();
    let completed: Vec<String> = pool
        .list_agents()
        .into_iter()
        .filter(|a| a.kind == oxicode_agent::tools::AgentKind::Task && a.status == oxicode_agent::tools::AgentHubStatus::Idle)
        .filter_map(|a| a.current_task)
        .collect();
    let (updated, mutated) = oxicode_agent::tools::todo::reconcile_with_subagents(&phases, &completed);
    if mutated {
        // Fire-and-forget: the TUI frame loop is sync; apply as a `Done`-only
        // op batch would require async, so reconcile via a direct setter.
        // `TodoState` (the concrete provider) needs a sync `set_phases` — add
        // one alongside `with_phases`/`get_phases` in `todo_state.rs` if it
        // doesn't already exist (`apply_ops` is async and op-based, not a
        // fit for a bulk-replace reconcile).
        provider.set_phases_sync(updated.clone());
    }
    updated
}

fn build_matched_closure<'a>(
    pool: &'a std::sync::Arc<dyn oxicode_agent::tools::AgentPoolProvider>,
) -> impl Fn(&oxicode_agent::tools::todo::TodoItem) -> bool + 'a {
    let active_descs: Vec<String> = pool
        .list_agents()
        .into_iter()
        .filter(|a| a.kind == oxicode_agent::tools::AgentKind::Task && a.status == oxicode_agent::tools::AgentHubStatus::Running)
        .filter_map(|a| a.current_task)
        .collect();
    move |t| !active_descs.is_empty() && oxicode_agent::tools::todo::todo_matches_any_description(&t.content, &active_descs)
}
```

Add `fn set_phases_sync(&self, phases: Vec<TodoPhase>)` to the
`TodoStateProvider` trait (`oxicode-agent/src/tools.rs`) and implement it on
`TodoState` (`oxicode-cli/src/store/todo_state.rs`) as a plain
`*self.phases.write() = phases` — no async needed, this is a local
`RwLock`, unlike `apply_ops` which is `async` only because the trait method
signature is shared with future non-local providers.

Replace the `snapshot.todo_phases = provider.get_phases();` line from Task
A3 with `snapshot.todo_phases = refresh_todo_phases(provider, agent_pool);`
(the `RenderState`'s existing `agent_pool` field — confirm its exact name at
task start), and thread `build_matched_closure(agent_pool)` into both
`render_todo_pane(...)` and `render_todo_compact_line(...)` call sites,
replacing `|_| false`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli frame_refresh_reconciles_todo_with_completed_subagent`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/tools.rs oxicode-cli/src/store/todo_state.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(todo): wire subagent-matched highlight and auto-reconcile into HUD"
```

---

## Group E — Settings

### Task E1: New todo settings fields

**Files:**
- Modify: `oxicode-cli/src/store/settings.rs` (the `omp-adoption-2` feature-flag block, near `todo_panel_enabled` at line ~241)

**Interfaces:**
- Produces: `pub enum TodoEagerMode { Off, Preferred, Always }` (with `Default` = `Off`), `Settings.todo_eager_mode: TodoEagerMode`, `Settings.todo_reminders_enabled: bool`, `Settings.todo_reminders_max: u32`, `Settings.todo_clear_delay_secs: i64`. Consumed by Task E2 and Group D.

- [ ] **Step 1: Write the failing test**

```rust
// in oxicode-cli/src/store/settings.rs, inside `mod tests`
#[test]
fn todo_settings_default_preserve_current_behavior() {
    let s = Settings::default();
    assert_eq!(s.todo_eager_mode, TodoEagerMode::Off);
    assert!(s.todo_reminders_enabled);
    assert_eq!(s.todo_reminders_max, 3);
    assert_eq!(s.todo_clear_delay_secs, 60);
}

#[test]
fn todo_eager_mode_round_trips_through_toml() {
    let toml_str = r#"todo_eager_mode = "always""#;
    let parsed: TodoEagerMode = toml::from_str(&format!("v = \"always\"\n")).map(|t: toml::Value| {
        TodoEagerMode::deserialize(t["v"].clone()).unwrap()
    }).unwrap();
    assert_eq!(parsed, TodoEagerMode::Always);
    let _ = toml_str; // documents the field name; full Settings round-trip covered by existing settings tests
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli todo_settings_default_preserve_current_behavior`
Expected: FAIL — fields not found.

- [ ] **Step 3: Implement**

```rust
/// How strongly to auto-create a todo list on the first turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoEagerMode {
    /// Model decides; no automatic todo list. (default)
    #[default]
    Off,
    /// Suggests a todo list on the first message (reminder, not forced).
    Preferred,
    /// Forces a todo list on the first message via `ToolChoice::Named("todo")`
    /// when the resolved model's provider supports it.
    Always,
}

fn default_todo_reminders_max() -> u32 {
    3
}

fn default_todo_clear_delay_secs() -> i64 {
    60
}
```

and, in the `omp-adoption-2` block next to `todo_panel_enabled`:

```rust
/// How strongly to auto-create a todo list on the first turn. Default: off.
#[serde(default)]
pub todo_eager_mode: TodoEagerMode,

/// Remind the agent to finish open todos before it stops. Default: true.
#[serde(default = "default_true")]
pub todo_reminders_enabled: bool,

/// Max stop-time todo reminders per run. Default: 3.
#[serde(default = "default_todo_reminders_max")]
pub todo_reminders_max: u32,

/// Seconds after every todo closes before the HUD auto-clears. Default: 60.
#[serde(default = "default_todo_clear_delay_secs")]
pub todo_clear_delay_secs: i64,
```

Add the same three fields with matching defaults to `Settings::default()`'s
struct literal (next to `todo_panel_enabled: true`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli todo_settings_default_preserve_current_behavior`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/store/settings.rs
git commit -m "feat(settings): add todo_eager_mode/reminders/clear_delay settings"
```

---

### Task E2: Thread settings into `AgentLoopConfig`, replace the hardcoded constant

**Files:**
- Modify: `oxicode-agent/src/agent_loop/config.rs` (add fields to `AgentLoopConfig`)
- Modify: `oxicode-agent/src/agent_loop/mod.rs` (the `build_stop_reminder` call site, line ~1240-1245)
- Modify: `oxicode-cli/src/app/agent_session_runtime.rs` (wherever `AgentLoopConfig` is constructed from `Settings` — locate via `grep "AgentLoopConfig {" oxicode-cli/src/app/agent_session_runtime.rs`)

**Interfaces:**
- Consumes: `Settings.todo_reminders_enabled`/`todo_reminders_max` (E1).
- Produces: `AgentLoopConfig.todo_reminders_enabled: bool`, `AgentLoopConfig.todo_reminders_max: u32`.

- [ ] **Step 1: Write the failing test**

```rust
// in oxicode-agent/src/agent_loop tests (find the existing test module that
// exercises build_stop_reminder / MAX_TODO_STOP_REMINDERS wiring, or add a
// focused test next to `stop_reminder_dedups_unchanged_open_set` in
// oxicode-agent/src/tools/todo.rs since the constant itself moves there)
#[test]
fn stop_reminder_respects_configured_max_below_default() {
    let mut state = StopReminderState::default();
    let phases = open_task_phases();
    assert!(build_stop_reminder(&phases, &mut state, 1).is_some());
    // second call, same open set -> deduped regardless of max
    assert!(build_stop_reminder(&phases, &mut state, 1).is_none());
    assert_eq!(state.count(), 1);
}
```

(This test already passes against the existing `build_stop_reminder`
signature — it documents that the *caller* now supplies a configurable
`max` instead of the constant. The real behavior change is the call site.)

- [ ] **Step 2: Run to verify current wiring**

Run: `cargo test -p oxicode-agent stop_reminder_respects_configured_max_below_default`
Expected: PASS already (this test doesn't need new code — it's here to pin
the contract before the call-site change). Confirms no regression once E2
step 3 lands.

- [ ] **Step 3: Implement**

```rust
// oxicode-agent/src/agent_loop/config.rs — add to AgentLoopConfig:
/// Whether to inject stop-time reminders for incomplete todos. Default true
/// (mirrors the always-on behavior before this setting existed).
pub todo_reminders_enabled: bool,
/// Max stop-time todo reminders per run. Default `MAX_TODO_STOP_REMINDERS`.
pub todo_reminders_max: u32,
```

```rust
// oxicode-agent/src/agent_loop/mod.rs, replacing the block at line 1240-1245:
if self.config.todo_reminders_enabled
    && let Some(provider) = self.config.todo.as_ref()
    && let Some(text) = build_stop_reminder(&provider.get_phases(), &mut todo_reminder_state, self.config.todo_reminders_max)
{
```

```rust
// oxicode-cli/src/app/agent_session_runtime.rs, at each AgentLoopConfig { .. } construction:
todo_reminders_enabled: settings.todo_reminders_enabled,
todo_reminders_max: settings.todo_reminders_max,
```

Also give `AgentLoopConfig`'s `Default`/test-fixture constructors (check
`oxicode-agent/src/tests.rs` and any `AgentLoopConfig::default()`-style
helper) the values `todo_reminders_enabled: true, todo_reminders_max: MAX_TODO_STOP_REMINDERS`
so existing tests that don't care about this setting keep passing unchanged.

- [ ] **Step 4: Run to verify pass**

Run: `cargo nextest run -p oxicode-agent -p oxicode-cli`
Expected: PASS, no regressions in existing agent-loop / stop-reminder tests.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/agent_loop/config.rs oxicode-agent/src/agent_loop/mod.rs oxicode-cli/src/app/agent_session_runtime.rs
git commit -m "feat(todo): thread reminders settings into AgentLoopConfig"
```

---

## Group B — `/todo` slash command

### Task B1: Fuzzy match helpers + quote-aware tokenizer

**Files:**
- Modify: `oxicode-agent/src/tools/todo.rs` (near `resolve_targets`, line ~300)

**Interfaces:**
- Produces: `pub fn tokenize_quoted(input: &str) -> Vec<String>`, `pub fn find_task_fuzzy<'a>(phases: &'a [TodoPhase], query: &str) -> Option<(&'a TodoItem, &'a TodoPhase)>`, `pub fn find_phase_fuzzy<'a>(phases: &'a [TodoPhase], query: &str) -> Option<&'a TodoPhase>`. Consumed by Task B2.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn tokenize_quoted_respects_double_quotes() {
    assert_eq!(tokenize_quoted(r#"auth "wire oauth" now"#), vec!["auth", "wire oauth", "now"]);
}

#[test]
fn tokenize_quoted_handles_escaped_chars() {
    assert_eq!(tokenize_quoted(r#"a\ b"#), vec!["a b"]);
}

#[test]
fn find_phase_fuzzy_prefers_exact_then_prefix_then_substring() {
    let phases = vec![
        TodoPhase { name: "Authentication".into(), tasks: vec![] },
        TodoPhase { name: "Auth UI".into(), tasks: vec![] },
    ];
    assert_eq!(find_phase_fuzzy(&phases, "Authentication").unwrap().name, "Authentication");
    assert_eq!(find_phase_fuzzy(&phases, "auth ").is_none(), true); // ambiguous prefix -> None
}

#[test]
fn find_task_fuzzy_prefers_single_substring_match() {
    let phases = vec![TodoPhase {
        name: "Auth".into(),
        tasks: vec![task("Wire OAuth providers", TodoStatus::Pending)],
    }];
    let (t, p) = find_task_fuzzy(&phases, "oauth").unwrap();
    assert_eq!(t.content, "Wire OAuth providers");
    assert_eq!(p.name, "Auth");
}

#[test]
fn find_task_fuzzy_ambiguous_prefers_active_status() {
    let phases = vec![TodoPhase {
        name: "Auth".into(),
        tasks: vec![
            task("Wire OAuth providers", TodoStatus::Completed),
            task("Wire OAuth refresh", TodoStatus::InProgress),
        ],
    }];
    // Both match "wire oauth", but only one is active -> pick it.
    let (t, _) = find_task_fuzzy(&phases, "wire oauth").unwrap();
    assert_eq!(t.content, "Wire OAuth refresh");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-agent tokenize_quoted find_phase_fuzzy find_task_fuzzy`
Expected: FAIL — functions not found.

- [ ] **Step 3: Implement**

```rust
/// Quote-aware tokenizer: splits on whitespace, respects `"…"` groups, and
/// honors backslash escapes. Ports omp's `tokenize`
/// (`todo-command-controller.ts:37-58`).
pub fn tokenize_quoted(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
            }
            continue;
        }
        if ch == '"' {
            in_quote = !in_quote;
            continue;
        }
        if !in_quote && ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Exact (case-insensitive) -> unique prefix -> unique substring. Ambiguous
/// or no match -> `None`. Ports omp's `findPhaseFuzzy`
/// (`todo-command-controller.ts:81-92`).
pub fn find_phase_fuzzy<'a>(phases: &'a [TodoPhase], query: &str) -> Option<&'a TodoPhase> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    if let Some(p) = phases.iter().find(|p| p.name.to_lowercase() == q) {
        return Some(p);
    }
    let prefix: Vec<&TodoPhase> = phases.iter().filter(|p| p.name.to_lowercase().starts_with(&q)).collect();
    if prefix.len() == 1 {
        return Some(prefix[0]);
    }
    let sub: Vec<&TodoPhase> = phases.iter().filter(|p| p.name.to_lowercase().contains(&q)).collect();
    if sub.len() == 1 { Some(sub[0]) } else { None }
}

/// Exact content match -> unique substring match -> if ambiguous, prefer a
/// single in_progress/pending hit. Ports omp's `findTaskFuzzy`
/// (`todo-command-controller.ts:94-113`).
pub fn find_task_fuzzy<'a>(phases: &'a [TodoPhase], query: &str) -> Option<(&'a TodoItem, &'a TodoPhase)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    for phase in phases {
        for task in &phase.tasks {
            if task.content.to_lowercase() == q {
                return Some((task, phase));
            }
        }
    }
    let matches: Vec<(&TodoItem, &TodoPhase)> = phases
        .iter()
        .flat_map(|phase| phase.tasks.iter().map(move |t| (t, phase)))
        .filter(|(t, _)| t.content.to_lowercase().contains(&q))
        .collect();
    if matches.len() == 1 {
        return Some(matches[0]);
    }
    let active: Vec<(&TodoItem, &TodoPhase)> = matches
        .into_iter()
        .filter(|(t, _)| matches!(t.status, TodoStatus::InProgress | TodoStatus::Pending))
        .collect();
    if active.len() == 1 { Some(active[0]) } else { None }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-agent tokenize_quoted find_phase_fuzzy find_task_fuzzy`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/tools/todo.rs
git commit -m "feat(todo): port fuzzy phase/task matching + quote-aware tokenizer"
```

---

### Task B2: `/todo` command — show/help/append/start/done/drop/rm

**Files:**
- Create: `oxicode-cli/src/tui_vt/slash/todo_command.rs`
- Modify: `oxicode-cli/src/tui_vt/slash/mod.rs` (add `pub(crate) mod todo_command;` — check exact module declaration file/pattern via `grep "mod sessions" oxicode-cli/src/tui_vt/slash/` since `SessionsCommand` lives directly in `registry.rs` today; if the convention truly is "one file, `registry.rs`", put `TodoCommand` there instead of a new file per Task Right-Sizing's "follow established patterns" constraint — this is a build-time judgment call, not a design decision, resolve by checking whether any `SlashCommand` impl already lives outside `registry.rs`/`commands.rs`)
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs` (`register_all`, add `TodoCommand`)

**Interfaces:**
- Consumes: `find_task_fuzzy`/`find_phase_fuzzy`/`tokenize_quoted` (B1), `phases_to_markdown` (existing), `TodoOp`/`apply_ops` via `TodoStateProvider` (`ctx.session`'s todo provider — locate the accessor, likely `ctx.session.todo_provider()` mirroring `AgentSessionHandle::todo_provider()` already confirmed in `agent_session.rs:487-491`).
- Produces: `struct TodoCommand;` implementing `SlashCommand` with `name() == "todo"`.

- [ ] **Step 1: Write the failing test**

```rust
// in registry.rs's `mod tests` (or todo_command.rs's own `mod tests` if split)
#[test]
fn todo_command_show_reports_no_todos_when_empty() {
    let mut ctx = test_slash_ctx(); // reuse whatever harness SessionsCommand's tests use
    let outcome = TodoCommand.execute("", &mut ctx);
    assert_eq!(outcome, SlashOutcome::Handled);
    assert!(ctx.last_reply().unwrap().contains("No todos"));
}

#[test]
fn todo_command_append_creates_phase_and_task() {
    let mut ctx = test_slash_ctx();
    TodoCommand.execute("append Auth Wire OAuth", &mut ctx);
    let phases = ctx.session.todo_provider().unwrap().get_phases();
    assert_eq!(phases[0].name, "Auth");
    assert_eq!(phases[0].tasks[0].content, "Wire OAuth");
}

#[test]
fn todo_command_done_with_no_arg_marks_all_completed() {
    let mut ctx = test_slash_ctx_with_phase("Auth", vec![("a", oxicode_agent::tools::todo::TodoStatus::Pending)]);
    TodoCommand.execute("done", &mut ctx);
    let phases = ctx.session.todo_provider().unwrap().get_phases();
    assert_eq!(phases[0].tasks[0].status, oxicode_agent::tools::todo::TodoStatus::Completed);
}

#[test]
fn todo_command_unknown_verb_errors_with_usage() {
    let mut ctx = test_slash_ctx();
    TodoCommand.execute("bogus", &mut ctx);
    assert!(ctx.last_reply().unwrap().contains("Usage: /todo"));
}
```

(`test_slash_ctx`/`test_slash_ctx_with_phase`/`ctx.last_reply()` are small
test helpers — write them once in `registry.rs`'s `mod tests` if they don't
already exist there for `SessionsCommand`'s tests; reuse if they do.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli todo_command_`
Expected: FAIL — `TodoCommand` not found.

- [ ] **Step 3: Implement**

```rust
const TODO_USAGE: &str = "Usage: /todo <verb> [args]\n\
  /todo                              Show current todos\n\
  /todo expand                       Expand the sticky HUD\n\
  /todo collapse                     Collapse the sticky HUD\n\
  /todo copy                         Copy todos as Markdown to clipboard\n\
  /todo export [<path>]              Write todos to file (default: TODO.md)\n\
  /todo import [<path>]              Replace todos from file (default: TODO.md)\n\
  /todo append [<phase>] <task...>   Append a task; phase fuzzy-matched or auto-created\n\
  /todo start  <task>                Mark task in_progress (fuzzy content match)\n\
  /todo done   [<task|phase>]        Mark task/phase/all completed\n\
  /todo drop   [<task|phase>]        Mark task/phase/all abandoned\n\
  /todo rm     [<task|phase>]        Remove task/phase/all";

struct TodoCommand;

impl SlashCommand for TodoCommand {
    fn name(&self) -> &'static str {
        "todo"
    }

    fn description(&self) -> &'static str {
        "Show or mutate the todo list"
    }

    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let trimmed = args.trim();
        let Some(provider) = ctx.session.todo_provider() else {
            ctx.reply(InlineMessageKind::Error, "Todo not configured");
            return SlashOutcome::Handled;
        };
        if trimmed.is_empty() {
            return show_current(&provider, ctx);
        }
        let (verb, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
        let rest = rest.trim();
        match verb.to_ascii_lowercase().as_str() {
            "help" | "?" => {
                ctx.reply(InlineMessageKind::Info, TODO_USAGE);
                SlashOutcome::Handled
            }
            "expand" => {
                ctx.state.todo_expanded = true;
                SlashOutcome::Handled
            }
            "collapse" => {
                ctx.state.todo_expanded = false;
                SlashOutcome::Handled
            }
            "append" => append(rest, &provider, ctx),
            "start" => start(rest, &provider, ctx),
            "done" => mutate(rest, &provider, ctx, oxicode_agent::tools::todo::TodoStatus::Completed),
            "drop" => mutate(rest, &provider, ctx, oxicode_agent::tools::todo::TodoStatus::Abandoned),
            "rm" => remove(rest, &provider, ctx),
            _ => {
                ctx.reply(InlineMessageKind::Error, format!("Unknown /todo verb \"{verb}\".\n{TODO_USAGE}"));
                SlashOutcome::Handled
            }
        }
    }
}

fn show_current(provider: &std::sync::Arc<dyn TodoStateProvider>, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    let phases = provider.get_phases();
    if phases.is_empty() {
        ctx.reply(InlineMessageKind::Info, "No todos. Use /todo append <task> to start one.");
    } else {
        ctx.reply(InlineMessageKind::Info, oxicode_agent::tools::todo::phases_to_markdown(&phases).trim_end());
    }
    SlashOutcome::Handled
}

fn append(rest: &str, provider: &std::sync::Arc<dyn TodoStateProvider>, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    let tokens = oxicode_agent::tools::todo::tokenize_quoted(rest);
    if tokens.is_empty() {
        ctx.reply(InlineMessageKind::Error, "Usage: /todo append [<phase>] <task...>");
        return SlashOutcome::Handled;
    }
    let (phase_name, content) = if tokens.len() == 1 {
        (None, tokens[0].clone())
    } else {
        (Some(tokens[0].clone()), tokens[1..].join(" "))
    };
    let target = phase_name
        .as_ref()
        .and_then(|name| oxicode_agent::tools::todo::find_phase_fuzzy(&provider.get_phases(), name).map(|p| p.name.clone()))
        .or(phase_name)
        .unwrap_or_else(|| "Todos".to_string());
    let ops = vec![oxicode_agent::tools::todo::TodoOp::Append { phase: target.clone(), items: vec![content.clone()] }];
    apply_and_report(provider, ops, ctx, &format!("Appended to {target}: {content}"));
    SlashOutcome::Handled
}

fn start(rest: &str, provider: &std::sync::Arc<dyn TodoStateProvider>, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    if rest.is_empty() {
        ctx.reply(InlineMessageKind::Error, "Usage: /todo start <task>");
        return SlashOutcome::Handled;
    }
    let phases = provider.get_phases();
    match oxicode_agent::tools::todo::find_task_fuzzy(&phases, rest) {
        Some((task, _)) => {
            let content = task.content.clone();
            let ops = vec![oxicode_agent::tools::todo::TodoOp::Start { task: Some(content.clone()), phase: None }];
            apply_and_report(provider, ops, ctx, &format!("Started: {content}"));
        }
        None => ctx.reply(InlineMessageKind::Error, format!("No task matched \"{rest}\". Use /todo to list current tasks.")),
    }
    SlashOutcome::Handled
}

fn mutate(
    rest: &str,
    provider: &std::sync::Arc<dyn TodoStateProvider>,
    ctx: &mut SlashCtx<'_>,
    status: oxicode_agent::tools::todo::TodoStatus,
) -> SlashOutcome {
    use oxicode_agent::tools::todo::{TodoOp, TodoStatus};
    let (op_all, label): (fn() -> TodoOp, &str) = match status {
        TodoStatus::Completed => (|| TodoOp::Done { task: None, phase: None }, "Marked all tasks completed."),
        _ => (|| TodoOp::Drop { task: None, phase: None }, "Marked all tasks abandoned."),
    };
    if rest.trim().is_empty() {
        apply_and_report(provider, vec![op_all()], ctx, label);
        return SlashOutcome::Handled;
    }
    let phases = provider.get_phases();
    if let Some((task, _)) = oxicode_agent::tools::todo::find_task_fuzzy(&phases, rest) {
        let content = task.content.clone();
        let op = match status {
            TodoStatus::Completed => TodoOp::Done { task: Some(content.clone()), phase: None },
            _ => TodoOp::Drop { task: Some(content.clone()), phase: None },
        };
        let msg = format!("{}: {content}", if status == TodoStatus::Completed { "Marked completed" } else { "Marked abandoned" });
        apply_and_report(provider, vec![op], ctx, &msg);
        return SlashOutcome::Handled;
    }
    if let Some(phase) = oxicode_agent::tools::todo::find_phase_fuzzy(&phases, rest) {
        let name = phase.name.clone();
        let op = match status {
            TodoStatus::Completed => TodoOp::Done { task: None, phase: Some(name.clone()) },
            _ => TodoOp::Drop { task: None, phase: Some(name.clone()) },
        };
        let msg = format!("{}: {name}", if status == TodoStatus::Completed { "Marked phase completed" } else { "Marked phase abandoned" });
        apply_and_report(provider, vec![op], ctx, &msg);
        return SlashOutcome::Handled;
    }
    ctx.reply(InlineMessageKind::Error, format!("No task or phase matched \"{rest}\"."));
    SlashOutcome::Handled
}

fn remove(rest: &str, provider: &std::sync::Arc<dyn TodoStateProvider>, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    use oxicode_agent::tools::todo::TodoOp;
    if rest.trim().is_empty() {
        apply_and_report(provider, vec![TodoOp::Rm { task: None, phase: None }], ctx, "Cleared all todos.");
        return SlashOutcome::Handled;
    }
    let phases = provider.get_phases();
    if let Some((task, _)) = oxicode_agent::tools::todo::find_task_fuzzy(&phases, rest) {
        let content = task.content.clone();
        apply_and_report(provider, vec![TodoOp::Rm { task: Some(content.clone()), phase: None }], ctx, &format!("Removed: {content}"));
        return SlashOutcome::Handled;
    }
    if let Some(phase) = oxicode_agent::tools::todo::find_phase_fuzzy(&phases, rest) {
        let name = phase.name.clone();
        apply_and_report(provider, vec![TodoOp::Rm { task: None, phase: Some(name.clone()) }], ctx, &format!("Removed phase: {name}"));
        return SlashOutcome::Handled;
    }
    ctx.reply(InlineMessageKind::Error, format!("No task or phase matched \"{rest}\"."));
    SlashOutcome::Handled
}

/// `execute` on `SlashCommand` is synchronous; `TodoStateProvider::apply_ops`
/// is `async` (shared with the agent-tool caller). Block on it with the
/// current-thread runtime handle the way `CompactCommand` (per AGENTS.md's
/// "Adding a TUI Slash Command") already bridges sync-to-async — confirm the
/// exact helper name at task start (`grep "tokio::spawn\|block_in_place" oxicode-cli/src/tui_vt/slash/registry.rs`)
/// and use it here instead of a bare `tokio::runtime::Handle::current().block_on`
/// if the codebase already has one, for consistency.
fn apply_and_report(provider: &std::sync::Arc<dyn TodoStateProvider>, ops: Vec<oxicode_agent::tools::todo::TodoOp>, ctx: &mut SlashCtx<'_>, message: &str) {
    let provider = provider.clone();
    let result = tokio::runtime::Handle::current().block_on(provider.apply_ops(ops));
    match result {
        Ok(_) => ctx.reply(InlineMessageKind::Info, message),
        Err(e) => ctx.reply(InlineMessageKind::Error, e.to_string()),
    }
}
```

Register in `register_all` (`registry.rs`): `registry.builtins.push(Box::new(TodoCommand));`

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli todo_command_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/
git commit -m "feat(todo): add /todo slash command (show/append/start/done/drop/rm)"
```

---

### Task B3: `/todo` export / import (Markdown round-trip to file)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/todo_command.rs` (or wherever B2 landed)

**Interfaces:**
- Consumes: `phases_to_markdown`/`markdown_to_phases` (existing in `todo.rs`).
- Produces: `export`/`import` verb handling inside `TodoCommand::execute`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn todo_command_export_writes_markdown_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = test_slash_ctx_with_cwd_and_phase(dir.path(), "Auth", vec![("a", oxicode_agent::tools::todo::TodoStatus::Pending)]);
    TodoCommand.execute(&format!("export {}", dir.path().join("out.md").display()), &mut ctx);
    let content = std::fs::read_to_string(dir.path().join("out.md")).unwrap();
    assert!(content.contains("- [ ] a"));
}

#[test]
fn todo_command_import_replaces_todos_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.md");
    std::fs::write(&path, "## Auth\n- [x] done task\n- [ ] open task\n").unwrap();
    let mut ctx = test_slash_ctx();
    TodoCommand.execute(&format!("import {}", path.display()), &mut ctx);
    let phases = ctx.session.todo_provider().unwrap().get_phases();
    assert_eq!(phases[0].tasks[1].content, "open task");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli todo_command_export todo_command_import`
Expected: FAIL — verbs not handled (falls through to "Unknown /todo verb").

- [ ] **Step 3: Implement**

```rust
"export" => export(rest, ctx),
"import" => import(rest, &provider, ctx),
```

```rust
fn resolve_todo_path(rest: &str, cwd: &std::path::Path) -> std::path::PathBuf {
    if rest.trim().is_empty() {
        cwd.join("TODO.md")
    } else {
        let p = std::path::PathBuf::from(rest.trim());
        if p.is_absolute() { p } else { cwd.join(p) }
    }
}

fn export(rest: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    let Some(provider) = ctx.session.todo_provider() else {
        ctx.reply(InlineMessageKind::Error, "Todo not configured");
        return SlashOutcome::Handled;
    };
    let phases = provider.get_phases();
    if phases.is_empty() {
        ctx.reply(InlineMessageKind::Warning, "No todos to export.");
        return SlashOutcome::Handled;
    }
    let target = resolve_todo_path(rest, &ctx.session.cwd()); // confirm exact cwd accessor name at task start
    match std::fs::write(&target, oxicode_agent::tools::todo::phases_to_markdown(&phases)) {
        Ok(()) => ctx.reply(InlineMessageKind::Info, format!("Wrote todos to {}", target.display())),
        Err(e) => ctx.reply(InlineMessageKind::Error, format!("Failed to write todos: {e}")),
    }
    SlashOutcome::Handled
}

fn import(rest: &str, provider: &std::sync::Arc<dyn TodoStateProvider>, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    let target = resolve_todo_path(rest, &ctx.session.cwd());
    let content = match std::fs::read_to_string(&target) {
        Ok(c) => c,
        Err(e) => {
            ctx.reply(InlineMessageKind::Error, format!("Failed to read todos: {e}"));
            return SlashOutcome::Handled;
        }
    };
    match oxicode_agent::tools::todo::markdown_to_phases(&content) {
        Ok(phases) => {
            let task_count: usize = phases.iter().map(|p| p.tasks.len()).sum();
            provider.set_phases_sync(phases.clone()); // set_phases_sync added in Task C2
            ctx.reply(InlineMessageKind::Info, format!("Imported {} phase(s), {task_count} task(s) from {}", phases.len(), target.display()));
        }
        Err(e) => ctx.reply(InlineMessageKind::Error, format!("Could not parse {}:\n  {e}", target.display())),
    }
    SlashOutcome::Handled
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli todo_command_export todo_command_import`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/
git commit -m "feat(todo): add /todo export and import Markdown round-trip"
```

---

### Task B4: `/todo copy` (clipboard, no new crate)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/todo_command.rs`

**Interfaces:**
- Produces: `fn copy_to_clipboard(text: &str) -> std::io::Result<()>` (platform shell-out per §7.1's decision — `pbcopy`/`xclip -selection clipboard`/`clip`), wired to the `"copy"` verb.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
#[cfg(target_os = "macos")]
fn copy_to_clipboard_uses_pbcopy_without_erroring() {
    // Smoke test only — asserts the subprocess call succeeds; does not
    // assert clipboard contents (no cross-platform read-back API without a
    // new dependency, and CI runners may lack a clipboard/display session).
    assert!(copy_to_clipboard("hello").is_ok());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli copy_to_clipboard_uses_pbcopy_without_erroring`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement**

```rust
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("pbcopy");
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard"]);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = Command::new("clip");

    let mut child = cmd.stdin(Stdio::piped()).spawn()?;
    child.stdin.as_mut().expect("piped stdin").write_all(text.as_bytes())?;
    child.wait()?;
    Ok(())
}
```

Wire `"copy"` in `TodoCommand::execute`:

```rust
"copy" => {
    let phases = provider.get_phases();
    if phases.is_empty() {
        ctx.reply(InlineMessageKind::Warning, "No todos to copy.");
    } else {
        match copy_to_clipboard(&oxicode_agent::tools::todo::phases_to_markdown(&phases)) {
            Ok(()) => ctx.reply(InlineMessageKind::Info, "Copied todos as Markdown to clipboard."),
            Err(e) => ctx.reply(InlineMessageKind::Error, e.to_string()),
        }
    }
    SlashOutcome::Handled
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli copy_to_clipboard_uses_pbcopy_without_erroring` (on macOS; on Linux/Windows CI this test is `cfg`-gated out — no cross-platform assertion needed per Step 1's note).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/todo_command.rs
git commit -m "feat(todo): add /todo copy via platform clipboard shell-out"
```

---

## Group F — Message visibility primitive

### Task F1: `visible` flag on `UserMessage`, skipped in transcript rendering

**Files:**
- Modify: `oxicode-ai/src/messages.rs` (`UserMessage` struct + `new` constructor + new `hidden` constructor)
- Modify: `oxicode-cli/src/store/session.rs` (`convert_to_session_entry`/`convert_from_session_entry` — persist the field)
- Modify: wherever `oxicode-cli` builds `TranscriptDisplayItem`s from session `Message`s (locate exact file at task start per the design doc's §7.2 instruction: grep `TranscriptDisplayItem` in `oxicode-cli/src/tui_vt/`)

**Interfaces:**
- Produces: `UserMessage.visible: bool` (`#[serde(default = "default_true_field")]`, i.e. `true` unless explicitly `false`), `UserMessage::hidden(content) -> Self` (sets `visible: false`). `UserMessage::new` unchanged in signature, now also sets `visible: true`.

- [ ] **Step 1: Write the failing tests**

```rust
// oxicode-ai/src/messages.rs, in `mod tests`
#[test]
fn user_message_new_defaults_visible_true() {
    let m = UserMessage::new("hi");
    assert!(m.visible);
}

#[test]
fn user_message_hidden_sets_visible_false() {
    let m = UserMessage::hidden("system nudge");
    assert!(!m.visible);
}

#[test]
fn user_message_deserializes_missing_visible_as_true() {
    let json = r#"{"role":"user","content":"hi","timestamp":0}"#;
    let m: UserMessage = serde_json::from_str(json).unwrap();
    assert!(m.visible); // backward-compat: old session files have no `visible` field
}
```

```rust
// oxicode-cli/src/store/session.rs or the transcript-builder file, wherever located
#[test]
fn hidden_user_message_is_skipped_in_transcript_display() {
    let hidden = oxicode_ai::UserMessage::hidden("nudge");
    let visible = oxicode_ai::UserMessage::new("real input");
    let items = build_transcript_display_items(&[
        oxicode_ai::Message::User(hidden),
        oxicode_ai::Message::User(visible),
    ]); // exact function name confirmed at task start
    assert_eq!(items.len(), 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai user_message_` && `cargo test -p oxicode-cli hidden_user_message_is_skipped`
Expected: FAIL — `visible` field / `hidden` constructor / skip logic not present.

- [ ] **Step 3: Implement**

```rust
// oxicode-ai/src/messages.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub role: UserRole,
    pub content: MessageContent,
    pub timestamp: i64,
    /// Whether this message renders in the transcript. `false` for
    /// synthetic reminders/nudges the model must see but the human should
    /// not (they are not something the human typed). Always sent to the
    /// provider regardless of this flag — it is display-only metadata.
    /// Missing on old session files deserializes to `true` (backward compat).
    #[serde(default = "default_visible")]
    pub visible: bool,
}

fn default_visible() -> bool {
    true
}

impl UserMessage {
    pub fn new(content: impl Into<MessageContent>) -> Self {
        Self { role: UserRole::User, content: content.into(), timestamp: chrono::Utc::now().timestamp_millis(), visible: true }
    }

    /// A message sent to the model but not rendered in the transcript.
    pub fn hidden(content: impl Into<MessageContent>) -> Self {
        Self { role: UserRole::User, content: content.into(), timestamp: chrono::Utc::now().timestamp_millis(), visible: false }
    }
}
```

For the session/transcript side: add `visible: bool` (same default) to
whatever struct persists a user message entry
(`SessionMessageEntry`/`AgentMessage`'s user variant — confirm exact shape
at task start), and add one `if !visible { continue; }` (or equivalent
filter) at the single point session entries become
`TranscriptDisplayItem`s. If that grep finds more than one call site,
factor a shared `is_display_visible(entry) -> bool` helper rather than
duplicating the check.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai user_message_` && `cargo test -p oxicode-cli hidden_user_message_is_skipped`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/messages.rs oxicode-cli/src/store/session.rs
git commit -m "feat(messages): add UserMessage.visible for transcript-hidden synthetic turns"
```

---

## Group D — Session policy

### Task D1: Stop-reminder banner (fixes the fake-user-turn bug)

**Files:**
- Modify: `oxicode-agent/src/agent_loop/mod.rs` (the injection at line ~1251, plus emit a new event)
- Modify: `oxicode-agent/src/agent_loop/mod.rs` or wherever `AgentEvent` is defined (add `TodoReminder` variant)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (handle the new event, commit a banner via `render_committed_chunk`)

**Interfaces:**
- Consumes: `UserMessage::hidden` (F1), `build_stop_reminder`'s open-task list (currently only returns formatted text — extend to also return the raw `Vec<TodoItem>` the banner needs, or have the call site re-derive "open tasks" from `provider.get_phases()` directly since it already has that value at line 1242).
- Produces: `AgentEvent::TodoReminder { open: Vec<oxicode_agent::tools::todo::TodoItem>, attempt: u32, max: u32 }`.

- [ ] **Step 1: Write the failing test**

```rust
// oxicode-agent/src/agent_loop tests
#[tokio::test]
async fn stop_reminder_injects_hidden_message_and_emits_event() {
    // Reuse the existing MockProvider-based test harness for run_loop that
    // already exercises build_stop_reminder (find it via
    // `grep -rl "MockProvider" oxicode-agent/tests/` at task start) and add
    // an assertion that:
    // 1. the injected Message::User has `visible == false`
    // 2. an AgentEvent::TodoReminder was emitted with the matching open list
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-agent stop_reminder_injects_hidden_message_and_emits_event`
Expected: FAIL — `visible` still `true`, no `TodoReminder` event.

- [ ] **Step 3: Implement**

```rust
// AgentEvent enum, wherever it's defined:
TodoReminder { open: Vec<oxicode_agent::tools::todo::TodoItem>, attempt: u32, max: u32 },
```

```rust
// agent_loop/mod.rs, replacing lines ~1246-1252:
if self.config.todo_reminders_enabled
    && let Some(provider) = self.config.todo.as_ref()
    && let phases = provider.get_phases()
    && let Some(text) = build_stop_reminder(&phases, &mut todo_reminder_state, self.config.todo_reminders_max)
{
    let open: Vec<_> = phases
        .iter()
        .flat_map(|p| p.tasks.iter())
        .filter(|t| matches!(t.status, oxicode_agent::tools::todo::TodoStatus::Pending | oxicode_agent::tools::todo::TodoStatus::InProgress))
        .cloned()
        .collect();
    emit(AgentEvent::TodoReminder { open: open.clone(), attempt: todo_reminder_state.count(), max: self.config.todo_reminders_max });
    events.push(AgentEvent::TodoReminder { open, attempt: todo_reminder_state.count(), max: self.config.todo_reminders_max });
    pending_messages = vec![Message::User(UserMessage::hidden(text))];
    continue;
}
```

```rust
// main_loop.rs, wherever AgentEvent is matched to update RenderState (locate
// the existing match arms handling e.g. AgentEvent::TurnStart):
AgentEvent::TodoReminder { open, attempt, max } => {
    let header = format!("⚠ {} incomplete todo{} — reminder {attempt}/{max}", open.len(), if open.len() == 1 { "" } else { "s" });
    let mut lines = vec![Line::from(Span::styled(header, Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)))];
    for t in &open {
        lines.push(Line::from(format!("  ☐ {}", t.content)));
    }
    commit_banner_lines(lines); // reuses render_committed_chunk's insertion path — implement as a thin wrapper if one doesn't already exist for ad-hoc banners
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-agent stop_reminder_injects_hidden_message_and_emits_event`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/agent_loop/mod.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "fix(todo): stop reminder no longer masquerades as a user turn; add banner"
```

---

### Task D2: Mid-run nudge

**Files:**
- Modify: `oxicode-agent/src/agent_loop/mod.rs` (per-turn mutation counter + injection)

**Interfaces:**
- Consumes: `UserMessage::hidden` (F1), the existing tool-result loop (wherever `after_tool_call`/tool-name tracking already happens per-turn — locate via `grep "AfterToolCallHook\|tool_name ==" oxicode-agent/src/agent_loop/mod.rs`).
- Produces: `MidRunNudgeState { mutations_since_todo_touch: u32, nudge_count: u32 }` (mirrors `StopReminderState`'s shape), reset each `run_loop` invocation same as `todo_reminder_state`.

- [ ] **Step 1: Write the failing test**

```rust
// oxicode-agent/src/tools/todo.rs, next to StopReminderState
#[test]
fn mid_run_nudge_fires_after_threshold_mutations_without_todo_touch() {
    let mut state = MidRunNudgeState::default();
    for _ in 0..11 {
        state.record_tool_result("edit", false);
        assert!(!state.should_nudge());
    }
    state.record_tool_result("edit", false);
    assert!(state.should_nudge());
}

#[test]
fn mid_run_nudge_resets_on_todo_touch() {
    let mut state = MidRunNudgeState::default();
    for _ in 0..12 {
        state.record_tool_result("edit", false);
    }
    assert!(state.should_nudge());
    state.record_tool_result("todo", false);
    assert!(!state.should_nudge());
}

#[test]
fn mid_run_nudge_caps_at_two_per_cycle() {
    let mut state = MidRunNudgeState::default();
    for _ in 0..12 { state.record_tool_result("edit", false); }
    assert!(state.take_nudge().is_some());
    for _ in 0..12 { state.record_tool_result("edit", false); }
    assert!(state.take_nudge().is_some());
    for _ in 0..12 { state.record_tool_result("edit", false); }
    assert!(state.take_nudge().is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-agent mid_run_nudge_`
Expected: FAIL — `MidRunNudgeState` not found.

- [ ] **Step 3: Implement**

```rust
const MID_RUN_NUDGE_MUTATION_THRESHOLD: u32 = 12;
const MID_RUN_NUDGE_MAX_PER_CYCLE: u32 = 2;
const MUTATING_TOOLS: &[&str] = &["bash", "eval", "edit", "write", "ast_edit"];

/// Tracks mutating-tool-call volume since the last `todo` touch, to fire a
/// hidden mid-run reconciliation nudge. Ports omp's `TodoTracker`'s nudge
/// half (`todo-tracker.ts:15-16, 110-116`).
#[derive(Debug, Default)]
pub struct MidRunNudgeState {
    mutations_since_touch: u32,
    nudge_count: u32,
}

impl MidRunNudgeState {
    pub fn record_tool_result(&mut self, tool_name: &str, is_error: bool) {
        if tool_name == "todo" {
            self.mutations_since_touch = 0;
        } else if !is_error && MUTATING_TOOLS.contains(&tool_name) {
            self.mutations_since_touch += 1;
        }
    }

    pub fn should_nudge(&self) -> bool {
        self.mutations_since_touch >= MID_RUN_NUDGE_MUTATION_THRESHOLD && self.nudge_count < MID_RUN_NUDGE_MAX_PER_CYCLE
    }

    /// Consumes the nudge budget and resets the mutation counter, returning
    /// the reminder text to inject, or `None` if not due.
    pub fn take_nudge(&mut self) -> Option<String> {
        if !self.should_nudge() {
            return None;
        }
        self.nudge_count += 1;
        self.mutations_since_touch = 0;
        Some(
            "You've made several file changes without touching your todo list. \
             Reconcile it now: mark finished tasks done, update in-progress ones, \
             and add anything new before continuing."
                .to_string(),
        )
    }
}
```

Wire `MidRunNudgeState::default()` alongside `todo_reminder_state` in
`run_loop`, call `.record_tool_result(name, is_error)` at the same point the
loop already inspects tool results per-call (find via the grep above), and
after each turn settles, if `state.take_nudge()` returns `Some(text)`,
inject `Message::User(UserMessage::hidden(text))` into `pending_messages`
the same way the stop reminder does.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-agent mid_run_nudge_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/tools/todo.rs oxicode-agent/src/agent_loop/mod.rs
git commit -m "feat(todo): add mid-run reconciliation nudge after 12 untouched mutations"
```

---

### Task D3: `ToolChoice` type + `StreamOptions` field (no-op passthrough)

**Files:**
- Modify: `oxicode-ai/src/providers/options.rs` (add `tool_choice` field to `StreamOptions`)
- Create: `oxicode-ai/src/tools.rs` addition or a small new `oxicode-ai/src/tool_choice.rs` (check whether `Tool` already lives in a file worth extending vs. a new one — prefer extending `tools.rs` since `ToolChoice` is tightly coupled to `Tool`)

**Interfaces:**
- Produces: `pub enum ToolChoice { Auto, Named(String) }` (`#[oxicode_stable(since = "<next-version>")]`, `Default = Auto`), `StreamOptions.tool_choice: Option<ToolChoice>`.

- [ ] **Step 1: Write the failing test**

```rust
// oxicode-ai/src/providers/options.rs, in `mod tests`
#[test]
fn stream_options_default_has_no_tool_choice() {
    let opts = StreamOptions::default();
    assert!(opts.tool_choice.is_none());
}

#[test]
fn tool_choice_named_round_trips_through_serde() {
    let tc = ToolChoice::Named("todo".to_string());
    let json = serde_json::to_string(&tc).unwrap();
    let back: ToolChoice = serde_json::from_str(&json).unwrap();
    assert_eq!(back, tc);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai tool_choice`
Expected: FAIL — type not found.

- [ ] **Step 3: Implement**

```rust
// oxicode-ai/src/tools.rs (near the existing `Tool` type)
/// Forces or leaves-to-the-model which tool the next assistant turn must
/// call. `Auto` is the existing default behavior (model decides freely,
/// including calling no tool). `Named` forces exactly one tool by name —
/// only honored by providers with native forced-tool-choice support; owned
/// (in-band XML) dialects and providers without the feature silently treat
/// it as `Auto` (see each provider's mapping for the exact behavior).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    Named(String),
}
```

```rust
// oxicode-ai/src/providers/options.rs
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct StreamOptions {
    // ...existing fields...
    /// Forces the next assistant turn's tool choice. `None`/`Auto` = no
    /// change from today's behavior. See [`crate::tools::ToolChoice`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<crate::tools::ToolChoice>,
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/tools.rs oxicode-ai/src/providers/options.rs
git commit -m "feat(ai): add ToolChoice type and StreamOptions.tool_choice (no provider wiring yet)"
```

---

### Task D4: OpenAI + OpenAI Responses `tool_choice` mapping

**Files:**
- Modify: `oxicode-ai/src/providers/openai.rs` (`build_tools`, and wherever the request body is assembled to also read `options.tool_choice`)
- Modify: `oxicode-ai/src/providers/openai_responses.rs` (same)

**Interfaces:**
- Consumes: `ToolChoice` (D3).
- Produces: request-body `"tool_choice"` field: omitted for `Auto`/`None`, `{"type":"function","function":{"name":"todo"}}` for `Named("todo")` (Chat Completions shape); Responses API uses `{"type":"function","name":"todo"}` — confirm exact shape against OpenAI's Responses API docs at task start, the two request shapes differ slightly.

- [ ] **Step 1: Write the failing tests**

```rust
// oxicode-ai/src/providers/openai.rs, in `mod tests`
#[test]
fn build_request_body_omits_tool_choice_when_auto() {
    let body = build_request_body_for_test(&context_with_tools(), &StreamOptions::default());
    assert!(!body.as_object().unwrap().contains_key("tool_choice"));
}

#[test]
fn build_request_body_forces_named_tool_choice() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    let body = build_request_body_for_test(&context_with_tools(), &opts);
    assert_eq!(body["tool_choice"], serde_json::json!({"type": "function", "function": {"name": "todo"}}));
}
```

(`build_request_body_for_test`/`context_with_tools` — reuse or add a small
test helper mirroring however `openai.rs`'s existing `build_tools` tests
construct a `Context`; check the file's existing `mod tests` first.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai -p providers::openai tool_choice`
Expected: FAIL — field not read/mapped yet.

- [ ] **Step 3: Implement**

```rust
fn build_tool_choice(tool_choice: Option<&crate::tools::ToolChoice>) -> Option<JsonValue> {
    match tool_choice {
        None | Some(crate::tools::ToolChoice::Auto) => None,
        Some(crate::tools::ToolChoice::Named(name)) => Some(json!({"type": "function", "function": {"name": name}})),
    }
}
```

Insert `if let Some(tc) = build_tool_choice(options.tool_choice.as_ref()) { body["tool_choice"] = tc; }`
at the point `body` is assembled in the existing request-building function
(locate the exact function via the `build_tools` call site in `openai.rs`).
Mirror for `openai_responses.rs` with its shape:
`json!({"type": "function", "name": name})`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/providers/openai.rs oxicode-ai/src/providers/openai_responses.rs
git commit -m "feat(ai): map ToolChoice::Named to OpenAI/Responses forced tool_choice"
```

---

### Task D5: Anthropic `tool_choice` mapping

**Files:**
- Modify: `oxicode-ai/src/providers/anthropic.rs`

**Interfaces:** Same shape as D4, Anthropic's forced-tool-choice body is `{"type": "tool", "name": "todo"}`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn anthropic_body_forces_named_tool_choice() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    let body = build_request_body_for_test(&context_with_tools(), &opts);
    assert_eq!(body["tool_choice"], serde_json::json!({"type": "tool", "name": "todo"}));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai anthropic_body_forces_named_tool_choice`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
fn build_tool_choice(tool_choice: Option<&crate::tools::ToolChoice>) -> Option<JsonValue> {
    match tool_choice {
        None | Some(crate::tools::ToolChoice::Auto) => None,
        Some(crate::tools::ToolChoice::Named(name)) => Some(json!({"type": "tool", "name": name})),
    }
}
```

Insert at the same point `build_anthropic_tools` result is placed into the
request body.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai anthropic_body_forces_named_tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/providers/anthropic.rs
git commit -m "feat(ai): map ToolChoice::Named to Anthropic forced tool_choice"
```

---

### Task D6: Google / Vertex `tool_choice` mapping

**Files:**
- Modify: `oxicode-ai/src/providers/google_shared.rs` (`convert_tools` call sites in `google.rs`/`vertex.rs`)

**Interfaces:** Gemini's forced-function-calling shape:
`{"function_calling_config": {"mode": "ANY", "allowed_function_names": ["todo"]}}` under `tool_config`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn google_body_forces_named_tool_choice_via_tool_config() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    let body = build_request_body_for_test(&context_with_tools(), &opts);
    assert_eq!(
        body["tool_config"]["function_calling_config"],
        serde_json::json!({"mode": "ANY", "allowed_function_names": ["todo"]})
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai google_body_forces_named_tool_choice`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// google_shared.rs
pub fn build_tool_config(tool_choice: Option<&crate::tools::ToolChoice>) -> Option<JsonValue> {
    match tool_choice {
        None | Some(crate::tools::ToolChoice::Auto) => None,
        Some(crate::tools::ToolChoice::Named(name)) => Some(json!({
            "function_calling_config": {"mode": "ANY", "allowed_function_names": [name]}
        })),
    }
}
```

Insert `if let Some(tc) = build_tool_config(options.tool_choice.as_ref()) { body["tool_config"] = tc; }`
in both `google.rs` and `vertex.rs` wherever they assemble the request body
around their shared `convert_tools` call.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai google_body_forces_named_tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/providers/google_shared.rs oxicode-ai/src/providers/google.rs oxicode-ai/src/providers/vertex.rs
git commit -m "feat(ai): map ToolChoice::Named to Gemini tool_config.function_calling_config"
```

---

### Task D7: Azure `tool_choice` mapping

**Files:**
- Modify: `oxicode-ai/src/providers/azure.rs`

**Interfaces:** Azure OpenAI uses the same Chat Completions shape as D4
(`{"type": "function", "function": {"name": "todo"}}`) — this is the
smallest task in the group, essentially a copy of D4's helper.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn azure_body_forces_named_tool_choice() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    let body = build_request_body_for_test(&context_with_tools(), &opts);
    assert_eq!(body["tool_choice"], serde_json::json!({"type": "function", "function": {"name": "todo"}}));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai azure_body_forces_named_tool_choice`
Expected: FAIL.

- [ ] **Step 3: Implement**

Reuse D4's `build_tool_choice` helper shape (copy into `azure.rs` next to
`build_tools`, since `oxicode-ai` providers are intentionally
independent — confirm there's no shared `openai_shapes.rs` utility module
before duplicating; if one exists, put the helper there instead and import
it from both `openai.rs` and `azure.rs`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai azure_body_forces_named_tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/providers/azure.rs
git commit -m "feat(ai): map ToolChoice::Named to Azure OpenAI forced tool_choice"
```

---

### Task D8: Bedrock `tool_choice` mapping

**Files:**
- Modify: `oxicode-ai/src/providers/bedrock.rs` (`build_bedrock_tool_config`)

**Interfaces:** Bedrock Converse API forced-tool-choice shape:
`{"toolChoice": {"tool": {"name": "todo"}}}`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn bedrock_tool_config_forces_named_tool_choice() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    let config = build_bedrock_tool_config_for_test(&[test_tool()], &opts);
    assert_eq!(config["toolChoice"], serde_json::json!({"tool": {"name": "todo"}}));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-ai bedrock_tool_config_forces_named_tool_choice`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
fn build_bedrock_tool_choice(tool_choice: Option<&crate::tools::ToolChoice>) -> Option<JsonValue> {
    match tool_choice {
        None | Some(crate::tools::ToolChoice::Auto) => None,
        Some(crate::tools::ToolChoice::Named(name)) => Some(json!({"tool": {"name": name}})),
    }
}
```

Extend `build_bedrock_tool_config` to accept `Option<&ToolChoice>` and, when
`Some`, insert the `toolChoice` key into its returned JSON object alongside
the existing `tools` key.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai bedrock_tool_config_forces_named_tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/providers/bedrock.rs
git commit -m "feat(ai): map ToolChoice::Named to Bedrock Converse toolChoice"
```

---

### Task D9: Ollama and owned (in-band XML) dialects ignore `tool_choice`

**Files:**
- Modify: `oxicode-ai/src/providers/ollama.rs` (explicit no-op, with a comment, not silent omission)
- Modify: `oxicode-ai/src/agent_loop/streaming.rs` or wherever the owned-dialect branch lives (confirm `tool_choice` is never read on that path — it already isn't, since owned dialects never call any `build_*_tool_choice` helper; this task's job is to add a regression test proving a `Named` choice on an owned-dialect model does not error).

**Interfaces:** No new production code expected on the Ollama/owned-dialect
side beyond a doc comment — this task is regression-test coverage for the
"silently ignore, never error" contract stated in the design.

- [ ] **Step 1: Write the failing test**

```rust
// oxicode-ai/src/providers/ollama.rs
#[test]
fn ollama_ignores_tool_choice_without_erroring() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    let body = build_request_body_for_test(&context_with_tools(), &opts);
    // No tool_choice-shaped key should appear; the request must still be
    // well-formed (build succeeds, doesn't panic/error).
    assert!(!body.as_object().unwrap().keys().any(|k| k.contains("tool_choice") || k.contains("toolChoice")));
}
```

```rust
// wherever the owned/XML-dialect request path is unit-tested (find via
// `grep -rl "owned dialect" oxicode-ai/src/dialect/` at task start)
#[test]
fn owned_dialect_request_ignores_forced_tool_choice() {
    let mut opts = StreamOptions::default();
    opts.tool_choice = Some(ToolChoice::Named("todo".into()));
    // Build the owned-dialect request the same way an XML-tool-calling
    // model would and assert it succeeds (no panic, no ProviderError).
}
```

- [ ] **Step 2: Run to verify current behavior**

Run: `cargo test -p oxicode-ai ollama_ignores_tool_choice owned_dialect_request_ignores_forced_tool_choice`
Expected: PASS already for the owned-dialect test (no code path reads
`tool_choice` there); the Ollama test may already pass too since `ollama.rs`
never reads `options.tool_choice` — if so this task adds pure regression
coverage, no production diff. If Ollama's `build_tools`-equivalent
accidentally starts reading a `tool_choice`-shaped field added by a
copy-paste from D4-D8, this test catches it.

- [ ] **Step 3: (only if Step 2 failed) Implement**

Add nothing — the omission itself is correct. If Step 2 unexpectedly failed
(some other task's diff leaked a `tool_choice` write into `ollama.rs`),
remove that write here.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-ai ollama_ignores_tool_choice owned_dialect_request_ignores_forced_tool_choice`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-ai/src/providers/ollama.rs
git commit -m "test(ai): pin Ollama/owned-dialect tool_choice no-op contract"
```

---

### Task D10: Eager todo prelude

**Files:**
- Create: `oxicode-agent/src/agent_loop/todo_policy.rs`
- Modify: `oxicode-agent/src/agent_loop/mod.rs` (module declaration + call the prelude builder on the first turn)
- Modify: `oxicode-agent/src/agent_loop/config.rs` (`AgentLoopConfig.todo_eager_mode: TodoEagerModeForLoop` — `oxicode-agent` cannot depend on `oxicode-cli`'s `Settings::TodoEagerMode`, so mirror a small 3-value enum here and have `oxicode-cli` convert when constructing `AgentLoopConfig`, the same pattern already used for other settings crossing the crate boundary — confirm the existing pattern via `grep "pub enum.*Mode" oxicode-agent/src/agent_loop/config.rs`)

**Interfaces:**
- Consumes: `UserMessage::hidden` (F1), `ToolChoice::Named` (D3), the resolved `Model` (to check provider support — a model "supports" forcing if its provider is one of openai/openai-responses/anthropic/google/vertex/azure/bedrock, i.e. not ollama and not an owned/XML dialect; expose this as `fn provider_supports_tool_choice(api: oxicode_ai::Api) -> bool` next to the `Api` enum or in `todo_policy.rs`).
- Produces: `pub fn build_eager_todo_prelude(prompt_text: Option<&str>, mode: TodoEagerMode, has_existing_phases: bool, is_subagent: bool, model_supports_forcing: bool) -> Option<(Message, Option<ToolChoice>)>`.

- [ ] **Step 1: Write the failing tests**

```rust
// oxicode-agent/src/agent_loop/todo_policy.rs, in `mod tests`
#[test]
fn eager_prelude_none_when_mode_off() {
    assert!(build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Off, false, false, true).is_none());
}

#[test]
fn eager_prelude_none_when_phases_already_exist() {
    assert!(build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Always, true, false, true).is_none());
}

#[test]
fn eager_prelude_none_for_subagent() {
    assert!(build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Always, false, true, true).is_none());
}

#[test]
fn eager_prelude_none_when_prompt_looks_like_a_question() {
    assert!(build_eager_todo_prelude(Some("what does this function do?"), TodoEagerMode::Always, false, false, true).is_none());
}

#[test]
fn eager_prelude_preferred_never_forces_tool_choice() {
    let (_, choice) = build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Preferred, false, false, true).unwrap();
    assert!(choice.is_none());
}

#[test]
fn eager_prelude_always_forces_tool_choice_when_model_supports_it() {
    let (_, choice) = build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Always, false, false, true).unwrap();
    assert_eq!(choice, Some(oxicode_ai::tools::ToolChoice::Named("todo".into())));
}

#[test]
fn eager_prelude_always_falls_back_to_reminder_only_when_model_cannot_force() {
    let (_, choice) = build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Always, false, false, false).unwrap();
    assert!(choice.is_none());
}

#[test]
fn eager_prelude_message_is_hidden() {
    let (msg, _) = build_eager_todo_prelude(Some("build a login page"), TodoEagerMode::Always, false, false, true).unwrap();
    match msg {
        Message::User(u) => assert!(!u.visible),
        _ => panic!("expected a hidden user message"),
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-agent eager_prelude_`
Expected: FAIL — module/function not found.

- [ ] **Step 3: Implement**

```rust
//! Eager todo-list creation policy for the first agent turn. Ports the
//! eager-prelude half of omp's `TodoTracker` (`todo-tracker.ts`); the
//! reminders/mid-run-nudge half lives in `agent_loop/mod.rs` next to
//! `build_stop_reminder`/`MidRunNudgeState` to avoid a second todo-state
//! owner.

use crate::messages_reexport::{Message, UserMessage}; // adjust to the actual re-export path used elsewhere in agent_loop
use oxicode_ai::tools::ToolChoice;

/// Mirrors `oxicode_cli`'s `Settings::TodoEagerMode` without a crate
/// dependency in the other direction; `oxicode-cli` converts when building
/// `AgentLoopConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TodoEagerMode {
    #[default]
    Off,
    Preferred,
    Always,
}

const QUESTION_PROMPT_PREFIXES: &[&str] = &[
    "what", "which", "when", "where", "why", "how", "who", "whom", "whose",
    "do", "does", "did", "can", "could", "would", "will", "should",
    "is", "are", "am", "may", "shall",
];

/// Whether `text` reads as a question rather than a task request. Ports
/// omp's `QUESTION_PROMPT_RE` + non-ASCII fallback (`todo-tracker.ts:24-30`).
fn looks_like_a_question(text: &str) -> bool {
    let trimmed = text.trim_end();
    if !(trimmed.ends_with('?') || trimmed.ends_with('!')) {
        return false;
    }
    // Non-ASCII prose ending in "?"/"!" is treated as a genuine question
    // regardless of the English word list (CJK, Spanish "¿…?", etc.) — the
    // punctuation alone is the reliable signal there.
    if trimmed.chars().any(|c| !c.is_ascii()) {
        return true;
    }
    let first_word = trimmed.trim_start().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    QUESTION_PROMPT_PREFIXES.contains(&first_word.as_str())
}

/// Builds the first-turn eager-todo message + optional forced tool choice.
/// Returns `None` when eager mode is off, a plan already exists, this is a
/// sub-agent, or the prompt looks like a question rather than a task.
pub fn build_eager_todo_prelude(
    prompt_text: Option<&str>,
    mode: TodoEagerMode,
    has_existing_phases: bool,
    is_subagent: bool,
    model_supports_forcing: bool,
) -> Option<(Message, Option<ToolChoice>)> {
    if mode == TodoEagerMode::Off || has_existing_phases || is_subagent {
        return None;
    }
    if let Some(text) = prompt_text
        && looks_like_a_question(text)
    {
        return None;
    }
    let text = "Before starting, create a todo list with the `todo` tool covering the \
                full scope of this request, then begin working through it."
        .to_string();
    let message = Message::User(UserMessage::hidden(text));
    let choice = if mode == TodoEagerMode::Always && model_supports_forcing {
        Some(ToolChoice::Named("todo".to_string()))
    } else {
        None
    };
    Some((message, choice))
}
```

Wire into `run_loop`: on `first_turn` (the existing `let mut first_turn = true;` flag at line 773), before the first provider call, call
`build_eager_todo_prelude` with `self.config.todo_eager_mode`,
`self.config.todo.as_ref().map(|p| !p.get_phases().is_empty()).unwrap_or(true)`,
`self.config.agent_kind == AgentKind::Sub` (confirm exact field name), and
`provider_supports_tool_choice(model.api)`; on `Some((message, choice))`,
push `message` into the outgoing messages and set `choice` on the next
`StreamOptions` passed to `Provider::stream`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-agent eager_prelude_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/agent_loop/todo_policy.rs oxicode-agent/src/agent_loop/mod.rs oxicode-agent/src/agent_loop/config.rs
git commit -m "feat(todo): eager first-turn todo prelude with optional forced tool_choice"
```

---

### Task D11: Wire `todo_eager_mode` end-to-end from settings

**Files:**
- Modify: `oxicode-cli/src/app/agent_session_runtime.rs` (convert `Settings::TodoEagerMode` → `oxicode_agent::agent_loop::todo_policy::TodoEagerMode` when building `AgentLoopConfig`)

**Interfaces:**
- Consumes: `Settings.todo_eager_mode` (E1), `todo_policy::TodoEagerMode` (D10).

- [ ] **Step 1: Write the failing test**

```rust
// oxicode-cli/src/app/agent_session_runtime.rs, in `mod tests`
#[test]
fn settings_todo_eager_mode_converts_to_loop_enum() {
    assert_eq!(convert_todo_eager_mode(crate::store::settings::TodoEagerMode::Always), oxicode_agent::agent_loop::todo_policy::TodoEagerMode::Always);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli settings_todo_eager_mode_converts_to_loop_enum`
Expected: FAIL — `convert_todo_eager_mode` not found.

- [ ] **Step 3: Implement**

```rust
fn convert_todo_eager_mode(mode: crate::store::settings::TodoEagerMode) -> oxicode_agent::agent_loop::todo_policy::TodoEagerMode {
    match mode {
        crate::store::settings::TodoEagerMode::Off => oxicode_agent::agent_loop::todo_policy::TodoEagerMode::Off,
        crate::store::settings::TodoEagerMode::Preferred => oxicode_agent::agent_loop::todo_policy::TodoEagerMode::Preferred,
        crate::store::settings::TodoEagerMode::Always => oxicode_agent::agent_loop::todo_policy::TodoEagerMode::Always,
    }
}
```

Set `todo_eager_mode: convert_todo_eager_mode(settings.todo_eager_mode)` at
each `AgentLoopConfig { .. }` construction (same sites touched in Task E2).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli settings_todo_eager_mode_converts_to_loop_enum`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/app/agent_session_runtime.rs
git commit -m "feat(todo): wire todo_eager_mode setting into the agent loop"
```

---

### Task D12: Auto-clear timer

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (`RenderState` gains a clear-deadline field; the todo-refresh block checks it)

**Interfaces:**
- Consumes: `Settings.todo_clear_delay_secs` (E1) — threaded into `RenderState` construction the same way `todo_panel_enabled` already is (locate that exact wiring point at task start).
- Produces: `RenderState.todo_clear_deadline: Option<std::time::Instant>`; HUD-only clear, does not mutate `TodoState`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn todo_auto_clear_fires_after_delay_when_all_closed() {
    let phases = vec![phase("Auth", vec![("a", TodoStatus::Completed)])];
    let mut state = RenderState::default();
    state.todo_phases = phases;
    sync_todo_clear_timer(&mut state, 0); // 0-second delay = instant per spec §6.6
    assert!(state.todo_phases.is_empty());
}

#[test]
fn todo_auto_clear_does_not_fire_while_open_tasks_remain() {
    let phases = vec![phase("Auth", vec![("a", TodoStatus::Pending)])];
    let mut state = RenderState::default();
    state.todo_phases = phases.clone();
    sync_todo_clear_timer(&mut state, 0);
    assert_eq!(state.todo_phases, phases);
}

#[test]
fn todo_auto_clear_negative_delay_disables_clearing() {
    let phases = vec![phase("Auth", vec![("a", TodoStatus::Completed)])];
    let mut state = RenderState::default();
    state.todo_phases = phases.clone();
    sync_todo_clear_timer(&mut state, -1);
    assert_eq!(state.todo_phases, phases);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p oxicode-cli todo_auto_clear_`
Expected: FAIL — `sync_todo_clear_timer` not found.

- [ ] **Step 3: Implement**

```rust
fn is_todo_list_settled(phases: &[TodoPhase]) -> bool {
    let mut seen_task = false;
    for phase in phases {
        for task in &phase.tasks {
            if !matches!(task.status, TodoStatus::Completed | TodoStatus::Abandoned) {
                return false;
            }
            seen_task = true;
        }
    }
    seen_task
}

/// HUD-only auto-clear: does not touch the underlying `TodoState`, so a
/// `/todo` or `todo` tool call after clearing still sees the historical
/// phases. `delay_secs < 0` disables clearing entirely.
fn sync_todo_clear_timer(state: &mut RenderState, delay_secs: i64) {
    if delay_secs < 0 || !is_todo_list_settled(&state.todo_phases) {
        state.todo_clear_deadline = None;
        return;
    }
    if delay_secs == 0 {
        state.todo_phases.clear();
        state.todo_clear_deadline = None;
        return;
    }
    let deadline = state.todo_clear_deadline.get_or_insert_with(|| std::time::Instant::now() + std::time::Duration::from_secs(delay_secs as u64));
    if std::time::Instant::now() >= *deadline {
        state.todo_phases.clear();
        state.todo_clear_deadline = None;
    }
}
```

Call `sync_todo_clear_timer(&mut snapshot, settings.todo_clear_delay_secs)`
immediately after the `refresh_todo_phases` call from Task C2, each frame.
Add `pub todo_clear_deadline: Option<std::time::Instant>` to `RenderState`
(defaulting to `None`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxicode-cli todo_auto_clear_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(todo): HUD auto-clear timer after the list settles"
```

---

## Final verification (run once, after all tasks land)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxicode-cli -- -D warnings   # native-browser default feature
cargo build -p oxicode-agent --features native-browser
cargo nextest run --workspace
```

Then a manual TUI smoke test per the design spec's §8: run `cargo run -p
oxicode-cli`, create a multi-phase todo list via the agent, verify the tree
spine/roman headers/collapse-expand/compact mode (resize below 18 rows),
block a task with a reason, drive `/todo` through each verb, and confirm a
completed sub-agent auto-closes its matching todo.
