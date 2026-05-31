# Brief 01: Documentation Debt

**Area:** Public API documentation and doc-test coverage across all 6 workspace crates
**Severity:** 🟡 High
**Estimated scope:** 39 ignored doc-tests, 34 stub doc comments (`/// TODO`), 1 broken doc-test, 0 `#[deny(missing_docs)]` annotations

---

## Context

oxi is a workspace crate intended for public consumption (published to crates.io). Public API documentation quality directly impacts adoption and integrator experience. Currently, 39 doc-tests are explicitly `ignore`d and 34 public items have placeholder `/// TODO` doc comments with no actual content. One doc-test in `oxi-tui/src/keybindings/mod.rs` is **broken** (references undefined `event` variable) and fails on `cargo test --doc`.

The project uses `#[doc]` attributes on 28 items but has zero `#![deny(missing_docs)]` enforcement.

Current state:

| File | Finding | Count |
|------|---------|-------|
| `oxi-ai/src/fallback_chain.rs` | Ignored doc-tests | 14 |
| `oxi-ai/src/multi_provider.rs` | Ignored doc-tests | 6 |
| `oxi-ai/src/transform.rs` | Ignored doc-tests | 2 |
| `oxi-ai/src/model_db.rs` | Ignored doc-tests | 2 |
| `oxi-ai/src/providers/mod.rs` | Ignored doc-tests | 1 |
| `oxi-ai/src/secret.rs` | Ignored doc-tests | 1 |
| `oxi-ai/src/lib.rs` | Ignored doc-tests | 1 |
| `oxi-agent/src/agent.rs` | Ignored doc-tests | 1 |
| `oxi-agent/src/agent_loop/mod.rs` | Ignored doc-tests | 1 |
| `oxi-agent/src/tools.rs` | Ignored doc-tests | 2 |
| `oxi-agent/src/tools/browse/tab_guard.rs` | Ignored doc-tests | 1 |
| `oxi-sdk/src/multi_provider.rs` | Ignored doc-tests | 3 |
| `oxi-sdk/src/tool_factory.rs` | Ignored doc-tests | 2 |
| `oxi-sdk/src/builder.rs` | Ignored doc-tests | 1 |
| `oxi-sdk/src/agent_builder.rs` | Ignored doc-tests | 1 |
| `oxi-sdk/src/kernel_bridge.rs` | Ignored doc-tests | 1 |
| `oxi-cli/src/cli.rs` | Ignored doc-tests | 1 |
| `oxi-cli/src/extensions/loading.rs` | Ignored doc-tests | 1 |
| `oxi-store/src/settings.rs` | Ignored doc-tests | 1 |
| `oxi-tui/src/widgets/completion.rs` | Ignored doc-tests | 1 |
| `oxi-tui/src/widgets/table_list.rs` | Ignored doc-tests | 1 |
| **`oxi-tui/src/keybindings/mod.rs`** | **BROKEN doc-test** (references `event` — undefined) | **1** |
| `oxi-cli/src/extensions/mod.rs` | Stub `/// TODO: document.` | 34 |
| `oxi-agent/src/tools.rs` | Stub `/// TODO` / `/// TODO: document this function.` | 5 |
| `oxi-agent/src/types.rs` | Stub `/// TODO.` | 4 |
| `oxi-agent/src/agent_loop/mod.rs` | Stub `/// TODO` / `/// TODO: document this function.` | 11 |
| `oxi-agent/src/tools/tool_definition_wrapper.rs` | Stub `/// TODO: document this function.` | 5 |

---

## Objective

1. **Fix the broken doc-test** in `oxi-tui/src/keybindings/mod.rs` so `cargo test --doc` passes clean.
2. **Resolve all 34 stub `/// TODO` doc comments** — either write real documentation or mark the items `#[doc(hidden)]` if they are internal.
3. **Enable or unignore doc-tests** for items where the examples are straightforward enough to compile and run.

This does NOT mean:
- ❌ Adding `#![deny(missing_docs)]` globally (too broad for this brief)
- ❌ Rewriting all doc comments from scratch
- ❌ Adding documentation to private/internal items
- ❌ Creating new documentation files (guides, tutorials, etc.)

It DOES mean:
- ✅ `cargo test --workspace --doc` passes with 0 failures
- ✅ Every `/// TODO` stub in the files listed above is replaced with real documentation
- ✅ Ignored doc-tests are either fixed and un-ignored, or the `ignore` is justified with a comment explaining why

---

## Approach

### Phase 1: Triage (read-only)

1. Run `cargo test --workspace --doc 2>&1` and record all failures and ignored tests.
2. For each ignored doc-test, read the surrounding code and determine:
   - **Category A:** Can be made runnable with minimal fixes (missing imports, etc.)
   - **Category B:** Needs infrastructure not available in doc-tests (async runtime, network, files)
   - **Category C:** The example is wrong/outdated and needs rewriting
3. For each `/// TODO` stub, read the item's signature, type constraints, and usage to determine what the documentation should say.

### Phase 2: Fix broken doc-test

1. Fix `oxi-tui/src/keybindings/mod.rs` line 11 — the `event` variable is undefined. Replace with a valid `crossterm::event::KeyEvent` construction or use `no_run`.
2. Verify: `cargo test -p oxi-tui --doc` passes.

### Phase 3: Resolve stub doc comments

1. For each `/// TODO` in the files listed above, write a one-sentence description of what the item does, based on its signature and usage.
2. Focus on **public API items** (pub fn, pub struct, pub trait methods). Private items can be left alone.
3. For `oxi-cli/src/extensions/mod.rs` (34 stubs in one file), this is likely an extension trait — document each method's purpose and parameters.

### Phase 4: Address ignored doc-tests

1. For **Category A** items: fix imports and remove `ignore`.
2. For **Category B** items: add a comment above the `ignore` attribute explaining why (e.g., `// ignore: requires tokio runtime`).
3. For **Category C** items: rewrite the example or change to `no_run` / `text` if compilation isn't practical.

### Phase 5: Verify

1. `cargo test --workspace --doc` — zero failures, documented list of remaining intentionally-ignored tests.
2. `rg "/// TODO" --type rust | grep -v target/` — zero remaining stubs in the files listed above.
3. `cargo doc --workspace --no-deps` — builds without warnings.

---

## Constraints

- **Do not** add `#![deny(missing_docs)]` — that's a separate policy decision.
- **Do not** change public API signatures while fixing docs.
- **Preserve** all existing doc-tests that currently pass.
- **Respect** the convention that doc comments describe *what* and *why*, not implementation details.

## Verification

1. `cargo test --workspace --doc 2>&1 | grep "FAILED"` — must be empty
2. `rg "/// TODO\b" --type rust | grep -v target/ | grep -v examples/` — must be empty
3. `cargo doc --workspace --no-deps 2>&1 | grep "warning:" | grep -v "generated"` — no new warnings
