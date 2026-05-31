# Production Audit — Area Briefs

> **Purpose:** One markdown brief per area. Each brief is a self-contained task
> that an agent can read and execute independently in a separate session.
>
> **How to use:** Open a new agent session, point it at the brief file, and let it run.

**Generated:** 2026-05-31
**Project:** oxi
**Version:** 0.24.0 / 0.25.0

## Survey Summary

| Metric | Value |
|--------|-------|
| Language | Rust 2021 |
| Source files | 290 `.rs` files |
| Lines of Rust code | 112,933 (code) / 132,600 (total) |
| Workspace crates | 6 (oxi-ai, oxi-agent, oxi-store, oxi-tui, oxi-sdk, oxi-cli) |
| Tests | 2131 passing, 0 failing |
| Clippy warnings | 0 |
| `cargo audit` vulnerabilities | 0 |
| `cargo audit` unmaintained | 3 warnings |
| Production `unwrap()` | ~11 (non-test, non-example) |
| Production `expect()` | ~90 |
| `unsafe` blocks | 15 |
| Ignored doc-tests | 39 |
| Broken doc-tests | 1 |
| TODO/FIXME/HACK comments | 68 (65 are stub doc comments) |
| Release profile | ✅ LTO, strip, panic=abort, codegen-units=1 |
| CI | ✅ fmt, clippy, audit, deny, nextest, doc-tests, release build |

## Areas

| # | Area | Brief | Severity |
|---|------|-------|----------|
| 01 | Documentation Debt | [01-doc-debt/BRIEF.md](01-doc-debt/BRIEF.md) | 🟡 High |
| 02 | Dependency Maintenance | [02-dep-maintenance/BRIEF.md](02-dep-maintenance/BRIEF.md) | 🟢 Normal |
| 03 | Code Quality — Production Expects | [03-code-quality/BRIEF.md](03-code-quality/BRIEF.md) | 🟢 Normal |
| 04 | Observability Gaps | [04-observability/BRIEF.md](04-observability/BRIEF.md) | 🟢 Normal |
| 05 | Error Handling Consistency | [05-error-handling/BRIEF.md](05-error-handling/BRIEF.md) | 🟢 Normal |
| 06 | Extension Safety | [06-extension-safety/BRIEF.md](06-extension-safety/BRIEF.md) | 🟡 High |

## Guiding Principles (apply to ALL briefs)

1. **No false positives** — Every finding must be verified against actual code.
   If a pattern is intentional and safe, leave it alone and document why.
2. **No over-engineering** — Do not split files or create modules for aesthetics.
   Split only when there is a concrete maintainability or ownership problem.
3. **Respect legacy structure** — Existing module boundaries exist for a reason.
   Do not reorganize directories unless the brief explicitly calls for it.
4. **Beautiful but practical** — Prefer readable, idiomatic code over clever
   abstractions. Simple is better than clean.
5. **Backwards compatible** — All changes must pass existing tests. No API breaks
   unless explicitly scoped in the brief.

## Priority Order

1. **01 Documentation Debt** — Broken doc-test blocks `cargo test --doc`. Fix first.
2. **06 Extension Safety** — `unsafe` blocks without `SAFETY` comments are a maintenance hazard.
3. **03 Code Quality** — Quick audit, potentially zero changes needed.
4. **02 Dependency Maintenance** — Low effort, check for updates and suppress warnings.
5. **05 Error Handling Consistency** — Document convention, optionally add typed errors to oxi-store.
6. **04 Observability Gaps** — Add tracing subscriber init in oxi-cli.

## Quick Wins (< 1 hour each)

- **01**: Fix the broken keybindings doc-test (5 minutes)
- **02**: Run `cargo update` and create `.cargo/audit.toml` (15 minutes)
- **06**: Add `SAFETY:` comments to all 15 `unsafe` blocks (30 minutes)

## Blockers

No blockers for production deployment. All findings are quality improvements, not deployment gates. The project is in strong shape — zero security vulnerabilities, zero test failures, zero clippy warnings, and a well-configured release profile.
