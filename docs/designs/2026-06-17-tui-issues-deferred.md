# Issue Feature — Deferred Enhancements

Three improvements deferred from the 2026-06-17 polish pass. Each requires
a small design decision; documented here so the next pass can pick them up
without re-deriving the tradeoffs.

---

## 1. Filter overlay (priority / label / text)

### Problem
`FileIssueStore::summary()` already exposes lock count, top priority, and
issue count. The `IssueFilter` struct already accepts `priority`, `label`,
and `text` fields. The overlay only toggles `status` (Open ↔ All) via `f`.

For projects with 50+ issues, "filter to `priority=critical` or
`label=auth`" is the primary navigation need.

### Design

**Entry:** Press `/` while in list view (same convention as fzf, vim,
lazygit).

**UI:** A small bottom-aligned input modal — single text field with a
hint bar showing accepted syntax:

```
┌─ filter: ─────────────────────────────────────────────────┐
│ priority=critical label=auth                              │
├──────────────────────────────────────────────────────────┤
│ Enter: apply · Esc: cancel · Ctrl+U: clear                │
└──────────────────────────────────────────────────────────┘
```

**Parsing:** Reuse `parse_new_opts`-style bracket tokenizer from
`tui/slash.rs` (it already handles `priority=high` and `labels=a,b`).
Translate directly into `IssueFilter` fields:

```rust
IssueFilter {
    status: self.status_filter.into(),
    priority: opts.get("priority").and_then(parse_priority),
    label: opts.get("label").map(String::from),
    text: opts.get("text").map(String::from),
    ..Default::default()
}
```

**State:** Add `custom_filter: Option<IssueFilter>` to `IssuesPanelOverlay`.
`refresh()` applies the union: `filter = custom_filter ∪ status_filter`.

**Cost:** ~80 lines + tests. Reuses `parse_new_opts` from slash.rs
(extract to `store/issues.rs::parse_filter_opts`).

---

## 2. Undo for close

### Problem
`close` is destructive and irreversible. If a session closes the wrong
issue (e.g. selecting off-by-one), the user has no recourse except manual
file edit. This is hostile to keyboard-driven workflows where misclicks
happen.

### Three options

#### Option A — `archived: bool` flag (recommended)
Add `archived: bool` to `IssueMeta` (default false). `close` sets
`status: Closed, archived: true`. New issues default `archived: false`.

UI shows archived issues in a separate `/issue archived` overlay (or
greyed-out in the list). Restore via `/issue reopen <id>`.

- Pro: Zero data loss. Visible undo. Cheap (~20 lines schema).
- Con: Issue files grow a new field; v5 → v6 migration needed.

#### Option B — `oxicode issue reopen <id>` command
No schema change. `close` already sets `closed_at`. `reopen` flips
`status` back to `Open` and clears `closed_at`.

- Pro: No migration. Minimal code (~10 lines in main.rs + slash.rs).
- Con: No bulk undo. User must remember to type the exact id.

#### Option C — Persistent undo log
Add `~/.oxicode/issues/.undo.jsonl` with `{"op": "close", "id": 12,
"prev": {...}}` entries. `oxicode issue undo` pops the most recent.

- Pro: Works for any destructive op. Future-proof.
- Con: New file format. Cleanup needed on commit. ~80 lines.

### Recommendation
**Option B** for the next pass — 10-line change, no migration, addresses
90% of the failure mode. **Option A** if we later want a visible undo UI.
Skip Option C unless we add other destructive ops.

---

## 3. Body markdown rendering

### Problem
Issue bodies are markdown (per `FileIssueStore` spec), but the detail
view renders them as plain text. Headers, bullets, and code fences all
display as literal `#`, `-`, ``` characters.

### Existing infrastructure
`oxicode_tui/src/widgets/chat/markdown.rs` has a private
`render_markdown(content: &str, styles: &ThemeStyles) -> Vec<Line<'static>>`
function that already handles headers, lists, bold/italic, code spans,
and tables. It's used by the chat widget but isn't `pub`.

### Design

1. **Make `render_markdown` `pub`.** Add to `oxicode_tui::widgets::chat::markdown`
   re-export list. ~3 lines.
2. **Use it for the body in `render_detail`.** Replace the plain-text
   body section with:
   ```rust
   let body_lines = render_markdown(&issue.body, &styles);
   let wrapped = hard_wrap_lines(&body_lines, body_inner.width as usize);
   ```
   This gives styled headers, code blocks, etc. while keeping the
   scrollbar position tracking accurate.
3. **Theme integration.** Pass the panel's `Theme` through to
   `render_markdown` so headers use the configured accent color.

### Cost
~30 lines. Touches:
- `oxicode-tui/src/widgets/chat/markdown.rs` (expose)
- `oxicode-cli/src/tui/overlay/issues_panel/render.rs` (use)

### Risk
The private `render_markdown` may not be efficient for long bodies
(no streaming). Add a `body_max_lines: usize` cap (similar to
`MAX_BODY_RENDER_LINES`) to avoid runaway allocations.

---

## Cross-cutting note

These three items all touch `tui/overlay/issues_panel/render.rs`. Doing
them in one PR is more efficient than three small ones — the module
is now stable (after the recent split into `mod/state/render/input`)
and a follow-up PR won't churn the structure again.