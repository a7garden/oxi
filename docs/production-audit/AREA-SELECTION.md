# Area Selection — oxicode

**Date:** 2026-05-31
**Version:** 0.24.0 / 0.25.0 (crate versions)
**Survey summary:** oxicode is a healthy Rust workspace with 290 source files, 113K lines of Rust code across 6 crates. Build is clean (0 clippy warnings, `cargo check` passes). All 2131 tests pass with zero failures. The project has strong CI (fmt, clippy, audit, deny, nextest, doc-tests, release build). Release profile is properly configured (LTO, strip, panic=abort). The main areas for improvement are documentation debt (39 ignored doc-tests, 34 stub `TODO` doc comments), dependency maintenance (3 unmaintained crate warnings from `cargo audit`), and scattered production `expect()`/`unwrap()` calls in streaming/provider paths.

## Areas

| # | Area | Severity | Trigger Findings | Brief Path |
|---|------|----------|------------------|------------|
| 01 | Documentation Debt | 🟡 High | 39 ignored doc-tests, 34 stub doc comments (`/// TODO`), 1 broken doc-test (keybindings) | 01-doc-debt/BRIEF.md |
| 02 | Dependency Maintenance | 🟢 Normal | 3 unmaintained crate warnings: `fxhash`, `number_prefix`, `paste` | 02-dep-maintenance/BRIEF.md |
| 03 | Code Quality — Production Expects | 🟢 Normal | 7 `expect()` in streaming.rs, 10 in bedrock.rs — most are infallible but should be audited | 03-code-quality/BRIEF.md |
| 04 | Observability Gaps | 🟢 Normal | 244 tracing calls but no structured exporter, no metrics export, no health endpoint | 04-observability/BRIEF.md |
| 05 | Error Handling Consistency | 🟢 Normal | Mixed anyhow/thiserror usage across crates, no clear convention documented | 05-error-handling/BRIEF.md |
| 06 | Extension Safety | 🟡 High | 15 `unsafe` blocks — most in extension loading (FFI), some in TUI overlays | 06-extension-safety/BRIEF.md |
