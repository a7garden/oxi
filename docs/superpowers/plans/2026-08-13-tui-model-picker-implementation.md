# TUI `/model` Slash Command — Make It Actually Pick — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the TUI `/model` slash command from a read-only transcript line into a searchable picker that lists every model from providers with a stored API key, pins the active model to the top, and falls back to the full catalog when no providers are keyed.

**Architecture:** Replace the `""` arm of `ModelCommand::execute` in `oxicode-cli/src/tui_vt/slash/registry.rs` to use the existing `RenderState.catalog` (an `Arc<dyn ModelCatalog>`) and `shared_auth_storage()`. Extract a pure helper `model_picker_rows` so the filter rules are unit-testable without building a `SlashCtx`. The existing overlay-submission handler in `main_loop.rs:1771` (which already calls `session.set_model`) is reused unchanged.

**Tech Stack:** Rust 2024 edition, ratatui (via `oxicode_vtui`), existing `oxicode_sdk::CatalogModelEntry`/`ModelCatalog` port, existing `crate::store::auth_storage::AuthStorage`. No new dependencies.

## Global Constraints

- **Cargo fmt** before every commit (`cargo fmt --all`).
- **Clippy** must pass: `cargo clippy --workspace --all-targets -- -D warnings`.
- **Tests** with `cargo nextest run -p oxicode-cli` (this crate is the only one that changes).
- **CHANGELOG.md** `[Unreleased]` section gets a `### Fixed` entry.
- **Pre-commit hooks** mirror CI; if installed they auto-run fmt/clippy on every commit.
- The `[Unreleased]` header sits at line 8 of `CHANGELOG.md` (after the `# Changelog` intro block).
- No SDK/protocol changes. No new port. No public API additions.
- `ModelCatalog` and `CatalogModelEntry` are re-exported from `oxicode_sdk` and are already in the dep graph for `oxicode-cli`.

## File Structure

| File | Role |
|---|---|
| `oxicode-cli/src/tui_vt/slash/registry.rs` | **Modify** — rewrite the `ModelCommand::execute` `""` arm; add `model_picker_rows` helper; add two unit tests in the existing `mod tests`. |
| `oxicode-cli/src/tui_vt/slash/commands.rs` | **Read only** — borrow `super::commands::fmt_ctx`/`fmt_cost`/`split_model_id`. |
| `oxicode-cli/src/tui_vt/main_loop.rs` | **Unchanged** — `OverlaySubmission::Selection(InlineListSelection::Model(idx))` already wired at line 1771. |
| `CHANGELOG.md` | **Modify** — add `### Fixed` entry under `[Unreleased]`. |

No new files. No crate split.

---

### Task 1: Add the `model_picker_rows` helper + unit tests (TDD)

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs:1015-1027` (append tests to the existing `mod tests`).
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs:594-677` (`ModelCommand` struct + `execute` — first just add the helper next to it; the picker rewrite comes in Task 2).

**Interfaces:**
- Consumes: `Arc<dyn ModelCatalog>` (the catalog port), `Arc<AuthStorage>` (from `shared_auth_storage()`), and the current model id (split into `cur_provider` / `cur_model_id` by the caller using `super::commands::split_model_id`).
- Produces: `fn model_picker_rows(catalog: &Arc<dyn ModelCatalog>, auth: &Arc<AuthStorage>, cur_provider: &str, cur_model_id: &str) -> (Vec<CatalogModelEntry>, bool)`. The bool is `used_fallback`.

- [ ] **Step 1: Add the test cases (write them first)**

Append the following two test cases to the existing `#[cfg(test)] mod tests` block at the end of `registry.rs` (the block currently ends at line 1026 with the `dispatch_help_is_intercepted` test). Use the existing `super::*` import.

```rust
    /// `model_picker_rows` filters the catalog to providers with stored
    /// API keys, pins the active model at index 0, and falls back to the
    /// full catalog when nothing is keyed.
    #[test]
    fn model_picker_filters_by_keyed_providers() {
        use crate::store::auth_storage::AuthStorage;
        use oxicode_sdk::ports::catalog::{
            CatalogModelEntry, CatalogSource, ModelCatalog,
        };
        use std::sync::Arc;

        // Tiny in-memory catalog with two providers and three models.
        #[derive(Debug)]
        struct TwoProviderCatalog;
        impl ModelCatalog for TwoProviderCatalog {
            fn search_sync(&self, _pattern: &str) -> Vec<CatalogModelEntry> {
                vec![
                    CatalogModelEntry {
                        provider: "anthropic".into(),
                        model_id: "claude-sonnet".into(),
                        name: "Claude Sonnet".into(),
                        context_window: 200_000,
                        cost_input: 3.0,
                        cost_output: 15.0,
                        reasoning: false,
                        supports_vision: true,
                        source: CatalogSource::Embedded,
                    },
                    CatalogModelEntry {
                        provider: "anthropic".into(),
                        model_id: "claude-opus".into(),
                        name: "Claude Opus".into(),
                        context_window: 200_000,
                        cost_input: 15.0,
                        cost_output: 75.0,
                        reasoning: false,
                        supports_vision: true,
                        source: CatalogSource::Embedded,
                    },
                    CatalogModelEntry {
                        provider: "google".into(),
                        model_id: "gemini-2.5-pro".into(),
                        name: "Gemini 2.5 Pro".into(),
                        context_window: 1_000_000,
                        cost_input: 1.25,
                        cost_output: 5.0,
                        reasoning: true,
                        supports_vision: true,
                        source: CatalogSource::Embedded,
                    },
                ]
            }
            // All async methods fall back to defaults; not exercised here.
        }
        let catalog: Arc<dyn ModelCatalog> = Arc::new(TwoProviderCatalog);

        // Only anthropic has a key; the active model is an anthropic one.
        let auth = AuthStorage::new_for_test();
        auth.set_for_test("anthropic", "test-anthropic-key");

        let (rows, used_fallback) =
            model_picker_rows(&catalog, &auth, "anthropic", "claude-sonnet");

        assert!(!used_fallback, "keyed providers exist — no fallback expected");
        // Active row pinned to index 0, the other keyed provider row
        // follows. google's models are excluded (no key).
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].provider, "anthropic");
        assert_eq!(rows[0].model_id, "claude-sonnet");
        assert_eq!(rows[1].model_id, "claude-opus");
    }

    /// When no providers are keyed AND there is no active model match
    /// in the catalog, the helper returns the full catalog with
    /// `used_fallback = true`.
    #[test]
    fn model_picker_falls_back_when_unkeyed_and_no_active_match() {
        use crate::store::auth_storage::AuthStorage;
        use oxicode_sdk::ports::catalog::{
            CatalogModelEntry, CatalogSource, ModelCatalog,
        };
        use std::sync::Arc;

        #[derive(Debug)]
        struct EmptyCatalog;
        impl ModelCatalog for EmptyCatalog {
            fn search_sync(&self, _pattern: &str) -> Vec<CatalogModelEntry> {
                vec![CatalogModelEntry {
                    provider: "openai".into(),
                    model_id: "gpt-4o".into(),
                    name: "GPT-4o".into(),
                    context_window: 128_000,
                    cost_input: 2.5,
                    cost_output: 10.0,
                    reasoning: false,
                    supports_vision: true,
                    source: CatalogSource::Embedded,
                }]
            }
        }
        let catalog: Arc<dyn ModelCatalog> = Arc::new(EmptyCatalog);

        // No keys at all.
        let auth = AuthStorage::new_for_test();

        // Active model id is not in the catalog (impossible in practice
        // but the helper must handle it).
        let (rows, used_fallback) =
            model_picker_rows(&catalog, &auth, "anthropic", "claude-not-in-catalog");

        assert!(used_fallback, "no keys, no active match — fallback expected");
        // The full catalog is returned.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model_id, "gpt-4o");
    }
}

- [ ] **Step 2: Add the `set_for_test` test seam on `AuthStorage`**

`AuthStorage` already has `Default::default()` (creates an empty in-memory
store at `auth_storage.rs:1076`) and a public `set_api_key(provider, key)`
method (`auth_storage.rs:743`). The test seam is one new method:

Open `oxicode-cli/src/store/auth_storage.rs` and add the following method
inside the existing `impl AuthStorage` block, right after the existing
`pub fn has(&self, provider: &str) -> bool` at line 916:

```rust
    /// **Test-only:** create a fresh, hermetic `AuthStorage` with no
    /// persisted file backing. Equivalent to `AuthStorage::default()`
    /// but spelled for the test seam. Do not call from production code.
    #[cfg(test)]
    pub fn new_for_test() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    /// **Test-only:** insert a placeholder API key for `provider` so
    /// `has(provider)` returns true. Bypasses the secure-prompt
    /// validation flow. Do not call from production code.
    #[cfg(test)]
    pub fn set_for_test(&self, provider: &str, key: &str) {
        self.set_api_key(provider, key.to_string());
    }
```

Test code then uses `AuthStorage::new_for_test()` to construct and
`auth.set_for_test("anthropic", "test-key")` to insert. No new fields,
no in-memory constructor needed.
- [ ] **Step 3: Run the tests to verify they fail (compile error / assertion)**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- model_picker`
Expected: compile error pointing at the missing `model_picker_rows` function. The test is a TDD scaffold for the helper that Task 2 will add.

- [ ] **Step 4: Add the `model_picker_rows` helper**

In `oxicode-cli/src/tui_vt/slash/registry.rs`, just below the existing `ModelCommand` impl (after line 677), add:

```rust
/// Build the rows for the `/model` picker.
///
/// Rules:
/// 1. Models from every provider where `auth.has(p)` is true are
///    included, **except** the active provider's models (the active
///    row is pinned at the top below).
/// 2. The active model is always present and pinned at index 0,
///    even if its provider has no key (e.g. key removed mid-session).
/// 3. If neither (1) nor (2) produces a row, fall back to the full
///    catalog and set `used_fallback = true` so the caller can drop
///    "no-key" badges (every row would be "no-key" in that state and
///    the footer already explains the fallback).
fn model_picker_rows(
    catalog: &std::sync::Arc<dyn oxicode_sdk::ports::catalog::ModelCatalog>,
    auth: &std::sync::Arc<crate::store::auth_storage::AuthStorage>,
    cur_provider: &str,
    cur_model_id: &str,
) -> (Vec<oxicode_sdk::CatalogModelEntry>, bool) {
    let all = catalog.search_sync("");

    let mut keyed: Vec<_> = all
        .iter()
        .filter(|e| auth.has(&e.provider) && e.provider != cur_provider)
        .cloned()
        .collect();

    let current_entry = all
        .iter()
        .find(|e| e.provider == cur_provider && e.model_id == cur_model_id)
        .cloned();

    let mut rows = Vec::with_capacity(keyed.len() + 1);
    if let Some(ce) = current_entry {
        rows.push(ce); // active row pinned to top
    }
    rows.append(&mut keyed);

    if rows.is_empty() {
        (all, true)
    } else {
        (rows, false)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p oxicode-cli --no-fail-fast -- model_picker`
Expected: 2 tests pass.

- [ ] **Step 6: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy -p oxicode-cli --all-targets -- -D warnings
```

Expected: clean. If clippy complains about an unused import or `must_use` on the helper, fix inline.

- [ ] **Step 7: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/registry.rs oxicode-cli/src/store/auth_storage.rs
git commit -m "feat(tui): add /model picker row helper (catalog + auth filter)

Pure helper that the /model slash command will use to compute its
picker rows. Filters the catalog to providers with stored API keys,
pins the active model at index 0, and falls back to the full catalog
when no providers are keyed (so a fresh TUI is never empty).

Unit tests cover the keyed-provider filter and the unkeyed
fallback. The slash command itself still shows the old transcript
line — that change lands in the next commit."
```

---

### Task 2: Wire the helper into `ModelCommand::execute`

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs:594-677` (`ModelCommand` struct + `execute` method).

**Interfaces:**
- Consumes: `model_picker_rows(...)` from Task 1.
- Produces: a list-modal overlay that the existing `OverlaySubmission::Selection(InlineListSelection::Model(idx))` handler in `main_loop.rs:1771` already consumes.

- [ ] **Step 1: Update the imports**

The current `use` block at lines 15-17 is:

```rust
use oxicode_vtui::tui::core::{
    InlineHandle, InlineListItem, InlineListSelection, InlineMessageKind,
};
```

Add `InlineListSearchConfig` so the picker becomes searchable (matching `/models` and `/providers` UX):

```rust
use oxicode_vtui::tui::core::{
    InlineHandle, InlineListItem, InlineListSearchConfig, InlineListSelection,
    InlineMessageKind,
};
```

- [ ] **Step 2: Replace the `""` arm of `ModelCommand::execute`**

Find the current `"" =>` arm (starts at line 604, ends at line 659) inside `ModelCommand::execute`. Replace it with:

```rust
            "" => {
                let Some(catalog) = ctx.state.catalog.as_ref() else {
                    // Catalog never loaded — keep the existing read-only
                    // message so the user still gets *some* answer.
                    ctx.reply(
                        InlineMessageKind::Info,
                        format!("Current model: {}", ctx.session.model_id()),
                    );
                    return SlashOutcome::Handled;
                };

                let auth = crate::store::auth_storage::shared_auth_storage();
                let current = ctx.session.model_id();
                let (cur_provider, cur_model_id) =
                    super::commands::split_model_id(&current);

                let (rows, used_fallback) = model_picker_rows(
                    catalog,
                    &auth,
                    cur_provider,
                    cur_model_id,
                );

                let keyed_provider_count = rows
                    .iter()
                    .filter(|e| {
                        auth.has(&e.provider) && e.provider != cur_provider
                    })
                    .map(|e| e.provider.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                let filter_label = if used_fallback {
                    "Showing full catalog — no providers with keys configured yet"
                        .to_string()
                } else {
                    format!(
                        "Showing models from {keyed_provider_count} keyed provider{}",
                        if keyed_provider_count == 1 { "" } else { "s" },
                    )
                };

                ctx.state.overlay_model_ids = rows
                    .iter()
                    .map(|e| format!("{}/{}", e.provider, e.model_id))
                    .collect();

                let items: Vec<InlineListItem> = rows
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let id = format!("{}/{}", e.provider, e.model_id);
                        let mut sub = format!(
                            "{} \u{00b7} {} in / {} out",
                            super::commands::fmt_ctx(e.context_window),
                            super::commands::fmt_cost(e.cost_input),
                            super::commands::fmt_cost(e.cost_output),
                        );
                        if e.reasoning {
                            sub.push_str(" \u{00b7} reasoning");
                        }
                        if e.supports_vision {
                            sub.push_str(" \u{00b7} vision");
                        }
                        let badge = if id == current {
                            Some("active".to_string())
                        } else if used_fallback {
                            None
                        } else if !auth.has(&e.provider) {
                            Some("no-key".to_string())
                        } else {
                            None
                        };
                        InlineListItem {
                            title: id.clone(),
                            subtitle: Some(sub),
                            badge,
                            indent: 0,
                            selection: Some(InlineListSelection::Model(i)),
                            search_value: Some(format!(
                                "{} {} {}",
                                e.provider, e.model_id, e.name,
                            )),
                        }
                    })
                    .collect();

                let total = items.len();
                let search = InlineListSearchConfig {
                    label: "Filter models".into(),
                    placeholder: Some(
                        "Type to filter (provider / model / name)\u{2026}".into(),
                    ),
                };
                ctx.handle.show_list_modal(
                    format!("Models ({total})"),
                    vec![format!(
                        "{filter_label} \u{2014} Enter to switch, Esc to close"
                    )],
                    items,
                    None,
                    Some(search),
                );
            }
```

The `next`/`cycle` and `id` arms of the match stay exactly as they are. The `id` arm already calls `session.set_model(id)`. The `next`/`cycle` arm still emits the "No scoped models configured to cycle" warning because `ScopedModel` is out of scope (deferred to follow-up).

- [ ] **Step 3: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy -p oxicode-cli --all-targets -- -D warnings
```

Expected: clean. If the borrow checker complains on the `let Some(catalog) = ctx.state.catalog.as_ref() else { ... }` pattern, drop the binding name and call `ctx.state.catalog.as_ref().unwrap()` since the `Some(...) else` early-return guard makes the unwrap safe — or just rewrite as `if ctx.state.catalog.is_none() { ... return; } let catalog = ctx.state.catalog.as_ref().unwrap();` if rustfmt prefers.

- [ ] **Step 4: Run all `oxicode-cli` tests**

Run: `cargo nextest run -p oxicode-cli`
Expected: all tests pass, including the two from Task 1.

- [ ] **Step 5: Smoke-test the wired command (build the binary)**

Run: `cargo build -p oxicode-cli`
Expected: compiles clean.

Then run a quick scripted check that the binary still starts and responds to `/model`:

```bash
target/debug/oxicode --help         # should print usage; no TTY needed
target/debug/oxicode models 2>&1 | head -20   # should print model rows
```

Expected: `--help` prints the usage; `models` lists catalog rows.
This proves the binary builds and the CLI surface is intact. A full
TUI smoke test requires an interactive terminal; if the implementer
can run the TUI they should also do `/model` → confirm a picker
overlay opens. The TUI smoke is optional but recommended.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/registry.rs
git commit -m "feat(tui): /model opens a picker filtered by providers with keys

Was: a single transcript line \"Current model: ...\".
Now: a searchable overlay picker that lists every model from
providers with a stored API key, pins the active model to the top
(even if its key was removed mid-session), and falls back to the
full catalog when no providers are keyed. Selection still routes
through the existing OverlaySubmission::Selection(InlineListSelection::Model)
handler in main_loop.rs, so model switching is end-to-end.

Mirrors the /models and /providers picker UX (search bar, badges,
empty-state footer)."
```

---

### Task 3: Update `CHANGELOG.md`

**Files:**
- Modify: `CHANGELOG.md:8-9` (the `[Unreleased]` block).

- [ ] **Step 1: Add a `### Fixed` entry under `[Unreleased]`**

The current `[Unreleased]` block (line 8) is empty. Below the `## [Unreleased]` header, add:

```markdown
## [Unreleased]

### Fixed

- **TUI `/model` is now a picker, not a transcript line.** Was: a single
  read-only `Current model: <id>` line. Now: a searchable overlay that
  lists every model from providers with a stored API key, pins the
  active model at the top of the list (even when its key has been
  removed mid-session), and falls back to the full catalog when no
  providers are keyed so a fresh TUI is never empty. Selection switches
  the model end-to-end through the existing overlay-submission handler.
  The `next`/`cycle` arm of `/model` still emits the
  "No scoped models configured to cycle" warning (`ScopedModel` is the
  config-curated cycling set and is out of scope here).

## [0.74.0] - 2026-08-12
```

(Only the `### Fixed` subsection and a blank line need to land; the rest of the changelog is untouched.)

- [ ] **Step 2: Verify the diff is minimal**

Run: `git diff CHANGELOG.md`
Expected: 7-10 added lines under `[Unreleased]`, nothing else.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): note TUI /model picker fix in Unreleased"
```

---

## Verification (run after all three tasks)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p oxicode-cli
cargo build -p oxicode-cli
```

Expected: every command exits 0. The two new unit tests in Task 1 pass; no other tests regress.

## Rollback

Revert the three commits in reverse order:

```bash
git revert HEAD         # CHANGELOG
git revert HEAD~1       # registry.rs picker wiring
git revert HEAD~2       # registry.rs helper + tests + auth_storage test seams
```

No destructive operations. No DB migrations. No settings migrations.
