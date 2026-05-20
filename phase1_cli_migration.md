# Phase 2: oxi-cli Migration — Step 1 Complete

## Summary

Added `oxi-sdk` as a dependency of `oxi-cli`. No code changes were made to `App::new()` or any other source file — the refactor is deferred to Phase 3 as instructed.

## Change

**File:** `oxi-cli/Cargo.toml`

Added line after existing oxi-tui dependency:
```toml
oxi-sdk = { version = "0.12.0", path = "../oxi-sdk" }
```

This gives `oxi-cli` access to:
- `OxiBuilder` and `Oxi` engine
- `AgentBuilder` for fluent agent construction
- All re-exported types from `oxi-ai` and `oxi-agent` via the SDK's prelude

## Verification

```
$ cargo check --workspace --lib 2>&1 | grep '^error' | wc -l
0
```

Workspace compiles cleanly. The only warnings are pre-existing ones in `oxi-sdk` (unused `mut` qualifiers in `agent_builder.rs`).

## What's Next (Phase 3)

- Refactor `App::new()` in `oxi-cli/src/lib.rs` to optionally use `OxiBuilder` for provider resolution
- Replace `register_custom_providers()` call with engine-based provider creation
- Keep the public CLI API identical
