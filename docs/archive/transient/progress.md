# Progress

## Status
In Progress

## Tasks

### Phase 2: Apply Rust 2024 let chains across the entire oxi workspace
- [x] Task 1: Identified 15 safe conversion candidates across all workspace crates
- [x] Task 2: Converted nested `if let` → `if let A && let B { }` let chains
- [x] Task 3: Verified all conversions — no semantic changes, no test failures
- [x] Verification: cargo fmt, clippy, nextest (2116/2116 passed)

### Phase 3: Replace `once_cell::sync::Lazy` with `std::sync::LazyLock` and remove unused dependencies
- [x] Task 1: Replace `once_cell::sync::Lazy` in oxi-ai (4 statics across 3 files)
- [x] Task 2: Remove unused dependencies (once_cell, lazy_static, tokio-test)
- [x] Verification: cargo fmt, clippy, nextest, cargo tree checks all pass

## Files Changed (Phase 2)
- `scripts/generate-models.rs` — 2 conversions (anthropic + openai model lookups)
- `oxi-ai/benches/sse_parsing.rs` — 1 conversion (delta + content)
- `oxi-ai/src/providers/openai.rs` — 2 conversions (idx+Text, idx+Thinking)
- `oxi-ai/src/providers/anthropic.rs` — 2 conversions (idx+Text, idx+Thinking)
- `oxi-ai/src/providers/mistral.rs` — 1 conversion (idx+Text)
- `oxi-ai/src/providers/azure.rs` — 1 conversion (idx+Text)
- `oxi-ai/src/providers/openai_responses.rs` — 1 conversion (idx+Text)
- `oxi-ai/src/providers/google_shared.rs` — 2 conversions (idx+Thinking, idx+Text)
- `oxi-ai/src/providers/bedrock.rs` — 2 conversions (idx+Text, idx+Thinking)
- `oxi-cli/src/main.rs` — 1 conversion (branch_info + parent_session_id)
- `oxi-cli/src/skills/mod.rs` — 1 conversion (frontmatter parsing)

## Files Changed (Phase 3)
- `oxi-ai/src/model_registry.rs` — `Lazy` → `LazyLock` (STATIC_MODELS, GLOBAL_REGISTRY)
- `oxi-ai/src/env_api_keys.rs` — `Lazy` → `LazyLock` (VERTEX_ADC_CHECK)
- `oxi-ai/src/providers/mod.rs` — `Lazy` → `LazyLock` (CUSTOM_PROVIDERS)
- `oxi-ai/Cargo.toml` — removed `once_cell = "1"`, removed `tokio-test = "0.4"`
- `oxi-cli/Cargo.toml` — removed `once_cell = "1"`, removed `lazy_static = "1.4"`

## Notes
- Phase 2: 24 additional candidates were NOT converted due to else branches, extra statements between if-lets, or semantic changes. See `phase2-let-chains.md` for full analysis.
- `async-trait` was not present in oxi-cli Cargo.toml (already removed or never added)
- `once_cell` and `lazy_static` still appear as transitive deps (from tracing-subscriber, etc.) but no workspace member declares them directly
- MSRV 1.96 is well above `LazyLock` stabilization (1.80)
- All 2116 tests pass, clippy clean, fmt clean
