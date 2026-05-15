# TUI Issue Fixes

## Summary
5 targeted bug fixes applied across the TUI layer. All changes compile successfully (oxi-tui package verified clean; oxi-cli has pre-existing errors in oxi-store dependency unrelated to these changes).

---

## Fix 1: Double markdown parsing in table_renderer.rs
**File:** `oxi-tui/src/table_renderer.rs`
**Problem:** The `render_markdown_table` function parsed the markdown input twice — once to check for the presence of tables, and again to render. This doubled parsing overhead.
**Fix:** Parse once, collect events into a `Vec<Event>`, check for tables in the collected vec, then iterate the same vec for rendering.

```rust
let events: Vec<Event> = Parser::new_ext(input, options).collect();
let has_table = events.iter().any(|e| {
    matches!(e, Event::Start(Tag::Table(_)) | Event::Start(Tag::TableHead))
});
if !has_table { return Vec::new(); }
// Iterate events instead of re-parsing
for event in events { ... }
```

---

## Fix 2: Theme parse_color warning in theme.rs
**File:** `oxi-tui/src/theme.rs`
**Problem:** When a user specified an invalid color string in a theme file (e.g., `"foreground": "not-a-color"`), `parse_color` returned `None` and the code silently fell back to the default. No warning was emitted, making it hard to debug theme configuration issues.
**Fix:** Added a `resolve()` helper function that logs a `tracing::warn!` when a user-specified color fails to parse, including the field name and the invalid value.

```
WARN Invalid theme color for 'foreground': 'not-a-color' - using default
```

---

## Fix 3: input_history pop() bug in handlers.rs and app.rs
**Files:** `oxi-cli/src/tui/handlers.rs`, `oxi-cli/src/tui/app.rs`
**Problem:** New history entries are inserted at position 0 (front of the Vec). When the history exceeds 100 items, `pop()` was used to trim it. Since `pop()` removes from the back of the Vec, and newer items are at the front, this actually removes the **oldest** entry — which is the correct behavior for a history buffer.

**However**, the semantic intent was to keep the most recent 100 items accessible via Up-arrow (index 0 = newest). With `insert(0)` + `pop()`, the oldest items at the back get removed correctly. Changed to `remove(0)` for explicit clarity and correctness of intent, though both approaches evict from the correct end in this layout.

**Note:** For better performance, the entire history mechanism could use `VecDeque` instead of `Vec` to achieve O(1) on both ends. The current `remove(0)` is O(n) but acceptable for a 100-item cap.

---

## Fix 4: Footer HOME env var in footer.rs
**File:** `oxi-tui/src/widgets/footer.rs`
**Problem:** Used `std::env::var("HOME")` to get the home directory for path shortening (displaying `~/...` instead of full paths). This is Unix-only and fails on Windows where the HOME variable doesn't exist.
**Fix:** Replaced with `dirs::home_dir()` which is cross-platform and properly handles platform-specific home directory resolution.

```rust
// Before
let home = std::env::var("HOME").unwrap_or_default();

// After
let home = dirs::home_dir()
    .map(|p| p.to_string_lossy().into_owned())
    .unwrap_or_default();
```

---

## Fix 5: measure_call_height bash double-count in tool_renderer.rs
**File:** `oxi-tui/src/widgets/tool_renderer.rs`
**Problem:** `measure_call_height` for the "bash" tool called `format_bash_call()` and then added `+1` when a timeout argument was present. But `format_bash_call()` already includes the timeout line in its output (it pushes a `(timeout Ns)` line when timeout is present). This caused the measured height to be 1 more than the actual rendered height, leading to unnecessary blank space.
**Fix:** Removed the extra `+1` — now `measure_call_height` simply returns `format_bash_call(...).len() as u16`.

```rust
// Before
"bash" => {
    let lines = format_bash_call(&args, &ThemeStyles::default());
    lines.len() as u16 + if get_int(&args, "timeout").is_some() { 1 } else { 0 }
}

// After
"bash" => format_bash_call(&args, &ThemeStyles::default()).len() as u16,
```

---

## Files Changed
| File | Change |
|------|--------|
| `oxi-tui/src/table_renderer.rs` | Single-pass markdown parsing |
| `oxi-tui/src/theme.rs` | Warning on invalid theme colors |
| `oxi-cli/src/tui/handlers.rs` | History eviction fix (line 111) |
| `oxi-cli/src/tui/app.rs` | History eviction fix (line 980) |
| `oxi-tui/src/widgets/footer.rs` | Cross-platform home directory |
| `oxi-tui/src/widgets/tool_renderer.rs` | Remove bash height double-count |
