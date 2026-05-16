# Progress

## Status
In Progress

## Tasks

- [x] Phase 2 Step 1: Add oxi-sdk as dependency of oxi-cli

## Files Changed

- `oxi-cli/Cargo.toml` — added `oxi-sdk = { version = "0.12.0", path = "../oxi-sdk" }` dependency

## Notes

- `cargo check --workspace --lib` passes with 0 errors (some warnings in oxi-sdk, unrelated)
- App::new() refactor deferred to Phase 3 as instructed
