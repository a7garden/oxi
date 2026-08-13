# TUI `/model` Slash Command — Make It Actually Pick — Design

**Date:** 2026-08-13
**Status:** Approved (autonomous execution per user delegation; user is away and cannot answer questions)
**Scope:** Slash-command surface only. No protocol changes, no SDK contract changes.

## Problem

`/model` (no args) in the TUI shows only the current model as a transcript line:
> `Current model: anthropic/claude-sonnet-4-5`

The user reports this as "조회용" (read-only). They want a picker that lists
every model the user can actually call — i.e. every provider that has an
API key configured — and lets the user switch to one with a single
selection.

### Root cause

`oxicode-cli/src/tui_vt/main_loop.rs:3057` hardcodes

```rust
scoped_models: Vec::new(),
```

when building the TUI's `AgentSession` options. `ScopedModel` is a
user-curated cycling set (config-based) and nothing populates it in the
TUI. The `ModelCommand::execute` path in
`oxicode-cli/src/tui_vt/slash/registry.rs:603-677` therefore always
hits the `is_empty()` branch and never opens the picker overlay.

The overlay-submission handler in `main_loop.rs:1771-1777` is correctly
wired and would consume the picker if a list were shown. The dead path
is upstream (no rows).

## Goal

`/model` (no args) opens an overlay picker with these rules:

1. **Default view: "models you can use."** Filter the catalog to
   providers where `shared_auth_storage().has(provider)` is `true`
   (this includes both key-based and OAuth credentials — `has()`
   checks the `credentials` map, populated by both paths).
2. **Current model is always present**, even if its provider has no
   key (the session is already using it; do not silently hide it).
3. **No providers with keys, no current model row from a keyed
   provider → fall through to the full catalog** so the picker is
   never empty in a fresh TUI.
4. **Searchable** — same `InlineListSearchConfig` UX as `/models` and
   `/providers`. Search by provider / model id / display name.
5. **Selection switches the model** end-to-end via the existing
   `OverlaySubmission::Selection(InlineListSelection::Model(idx))`
   handler at `main_loop.rs:1771`.

## Non-goals

- No changes to `ScopedModel`, cycling, or `Ctrl+P`. The cycling
  set is a separate feature.
- No new SDK port. `ModelCatalog.search_sync` already returns the
  full snapshot; filtering is a client-side concern.
- No changes to the RPC `get_commands` surface; the command name and
  description stay the same.
- No changes to the `product-meta.toml` OAuth list or the
  `AuthStorage` schema.

## Audit of other slash commands

I grep'd every `SlashOutcome::Handled` arm in
`oxicode-cli/src/tui_vt/slash/`. The other commands fall into three
groups, all of which are functional:

| Group | Commands | Why they work |
|---|---|---|
| Pure state ops | `quit`, `clear`, `compact`, `cancel`, `find`, `vim`, `shortcuts`, `theme`, `settings`, `sessions`, `status`, `handoff`, `agents` | Operate on `session`/`state`/`handle` directly; no provider/catalog data dependency. |
| Catalog read | `tools`, `mcp`, `info`, `export` | Read-only diagnostics. |
| Auth + catalog | `providers` (incl. `remove`/`add`/`run-oauth`), `models`, file commands | All have working side-effects. |

`/model` is the only non-functional command. There is no other
"Vec::new() stub" or "empty-list fallback" in the TUI slash surface.

### `/models` (full catalog) — borderline

`/models` works but shows the entire 5000+ row catalog. It is not
broken — the description says "Browse the full model catalog" — so it
is out of scope. After this change, `/model` and `/models` are clearly
distinguished: `/model` = "models I can call" (default), `/models` =
"browse everything" (escape hatch for power users).

### 1. `ModelCommand::execute` — picker, not a transcript line

`oxicode-cli/src/tui_vt/slash/registry.rs:603-677`. The `""` arm is
rewritten to compute a `Vec<CatalogModelEntry>` (owned) and hand it to
the overlay builder:

```rust
"" => {
    let Some(catalog) = ctx.state.catalog.as_ref() else {
        // Catalog never loaded — keep the existing read-only message.
        ctx.reply(
            InlineMessageKind::Info,
            format!("Current model: {}", ctx.session.model_id()),
        );
        return SlashOutcome::Handled;
    };

    let auth = crate::store::auth_storage::shared_auth_storage();
    let current = ctx.session.model_id();
    let (cur_provider, cur_model_id) = super::commands::split_model_id(&current);

    let (rows, used_fallback) = model_picker_rows(
        catalog,
        &auth,
        cur_provider,
        cur_model_id,
    );

    let keyed_provider_count = rows
        .iter()
        .filter(|e| auth.has(&e.provider) && e.provider != cur_provider)
        .map(|e| e.provider.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let filter_label = if used_fallback {
        "Showing full catalog — no providers with keys configured yet".to_string()
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
                "{} · {} in / {} out",
                super::commands::fmt_ctx(e.context_window),
                super::commands::fmt_cost(e.cost_input),
                super::commands::fmt_cost(e.cost_output),
            );
            if e.reasoning { sub.push_str(" · reasoning"); }
            if e.supports_vision { sub.push_str(" · vision"); }
            let badge = if id == current { Some("active".to_string()) }
                        else if used_fallback { None }
                        else if !auth.has(&e.provider) { Some("no-key".to_string()) }
                        else { None };
            InlineListItem {
                title: id.clone(),
                subtitle: Some(sub),
                badge,
                indent: 0,
                selection: Some(InlineListSelection::Model(i)),
                search_value: Some(format!("{} {} {}", e.provider, e.model_id, e.name)),
            }
        })
        .collect();

    let total = items.len();
    let search = InlineListSearchConfig {
        label: "Filter models".into(),
        placeholder: Some("Type to filter (provider / model / name)\u{2026}".into()),
    };
    ctx.handle.show_list_modal(
        format!("Models ({total})"),
        vec![format!("{filter_label} — Enter to switch, Esc to close")],
        items,
        None,
        Some(search),
    );
}
```

The `id` and `next`/`cycle` arms stay as they are. `id` is unchanged
(switch to a specific `provider/model`); `next`/`cycle` keeps the
"no scoped models configured" warning because `ScopedModel` is
out of scope.

### 1a. `model_picker_rows` — pure helper, unit-tested

Extracted from the inline logic so the rules are testable without
building a `SlashCtx`. Returns `(Vec<CatalogModelEntry>, bool)` —
the bool is `used_fallback` and tells the caller to drop the
"no-key" badge decoration.

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
    catalog: &dyn oxicode_sdk::ports::catalog::ModelCatalog,
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

`ModelCatalog` is the SDK trait (already in `oxicode-cli` deps via
the SDK re-export path). `AuthStorage` is reached via
`crate::store::auth_storage::AuthStorage` — `Arc<AuthStorage>` is
what `shared_auth_storage()` returns, and the function takes `&Arc`
to avoid an unnecessary clone.


### 2. Imports

Add to `registry.rs` (the file already imports
`InlineListSearchConfig` indirectly through `commands.rs`; add to
`registry.rs` if not present):

```rust
use oxicode_vtui::tui::core::{InlineListSearchConfig, InlineMessageKind, InlineListItem, InlineListSelection};
```

`CatalogModelEntry` is reached via `ctx.state.catalog.as_ref().unwrap().search_sync(...)`; the catalog port lives in `oxicode_sdk` (already in deps for `oxicode-cli`).

### 3. `is_empty()` branch — keep it as the **catalog-missing** fallback

The original `if models.is_empty()` check referred to `scoped_models`.
The new `""` arm checks for `ctx.state.catalog.as_ref()`. If the
catalog never loaded (rare; `ModelCatalog` is registered as a
required port in `bootstrap.rs`), show the same
"Current model: …" message and return. No silent data loss.

### 4. Test coverage

`oxicode-cli/src/tui_vt/slash/registry.rs::tests` already has
`builtins_register_expected_commands` etc. Add:

- `model_picker_filters_by_keyed_providers` — a unit test on a
  helper `fn model_picker_rows(catalog, auth, current_model_id) -> (Vec<...>, PickerMeta)`
  (extracted from the inline logic so it is testable without a
  `SlashCtx`). Asserts: keyed providers' models are in the list;
  the current model is at index 0; an unkeyed provider is excluded;
  when neither is keyed nor current, the full catalog is returned
  with `used_fallback = true`.
- `model_picker_includes_active_row_even_without_key` — switches
  the active model to a provider whose key was removed mid-session;
  the row still appears in the picker.

## Files to change

| File | Change |
|---|---|
| `oxicode-cli/src/tui_vt/slash/registry.rs` | Rewrite `ModelCommand::execute` `""` arm to use the catalog + auth filter; extract the row-builder into a small private function for unit testing; add the two tests above. |
| `CHANGELOG.md` | New `### Fixed` entry under `[Unreleased]`: "TUI `/model` is now a model picker (was: display-only). Lists every model from providers with a stored API key, pins the active model to the top, and falls back to the full catalog when no providers are keyed." |

No changes to `main_loop.rs`, `bootstrap.rs`, `agent_session.rs`, the
SDK, or any other crate.

## Rollout

1. Implement + tests in one commit on the current branch.
2. `cargo fmt --all`
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo nextest run -p oxicode-cli`
5. Manual TUI smoke: launch, `/model`, confirm picker appears, select
   a different model, confirm `Model: …` updates in the footer.
6. Update `CHANGELOG.md` `[Unreleased]`.

## Out-of-scope follow-ups (deferred — not part of this change)

- **`/model` pinned to a single provider.** The user sometimes wants
  "models for `anthropic` only". Could be `/model anthropic` or
  `/model @anthropic`. Defer until users ask.
- **Scope the picker to the current provider's models when only one
  provider is keyed.** Would hide the ability to switch providers
  without `/providers add` first. Keep the current behavior (show
  every keyed provider's models) so the picker is always a way to
  discover cross-provider switching.
- **Per-model auth capability flags** ("oauth" badge on rows whose
  provider uses OAuth). `AuthStorage` has the data (`has_oauth_with_refresh`); defer.
