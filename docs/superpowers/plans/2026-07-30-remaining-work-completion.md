# Remaining Work Completion Implementation Plan

> **For agentic workers:** This plan executes the 2026-07-27 omp-realignment residuals against the 2026-07-29 tape-cutover baseline.

**Goal:** Complete every still-load-bearing alignment item found by the 2026-07-30 code/document audit, while preserving explicit non-goals and verifying each delivered path.

**Architecture:** Keep provider transport, catalog metadata, and auth concerns separate. Keep tape rendering as the production main-screen path, with overlays as the only alternate-screen path. Add plugin runtime policy as a package-manager boundary, consume personas/prompts at session construction, and make memory commands invoke real backend/pipeline operations.

**Tech Stack:** Rust 2024, Tokio, reqwest, tokio-tungstenite, ratatui/crossterm, rusqlite, oxi-mnemopi, cargo-nextest.

## Global Constraints

- Do not reintroduce `Provider::name()` or a second provider identity source.
- Do not add a runtime `OXI_TAPE_RENDER` dual path.
- Do not hardcode TUI glyphs or SGR colors in tape components.
- Preserve TOML settings and atomic file writes.
- Keep OS desktop capture/input, collab-web, stats dashboard, and wire protocol outside this alignment unless an existing real backend already exists.
- Every implementation task adds behavior tests before production code, runs focused tests, and skips project-wide validation until the final gate.
- Final gate: `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p oxi-sdk --features native-browser -- -D warnings`, `cargo fmt --all -- --check`, and `cargo nextest run --workspace`.

## Tasks

1. Implement OpenAI Codex Responses and Google Gemini CLI transports or explicit provider-specific adapters, with catalog protocol mapping and fixtures.
2. Replace the debug tool's harness-only scaffold with a real DAP JSON transport boundary, retaining explicit errors when no adapter is available.
3. Wire tape rich content (LaTeX block/inline, Mermaid, image protocol metadata/fallback) and bracketed-paste parser into production paths.
4. Add package RuntimeConfig, ProjectPluginOverrides, Doctor checks, and shell-metachar validation at package boundaries.
5. Consume FilePersonaProvider and move remaining user-facing prompt fragments into embedded Markdown templates; remove orphan prompt assets.
6. Complete memory/Mnemopi E2E coverage and make `/memory clear`, `/memory enqueue`, and diagnostics call real backend/pipeline operations.
7. Remove stale language-policy/dead-tape/provider documentation and unreachable language overlay branches after code changes.

## Acceptance

- No current provider dispatch arm silently returns `None` for a catalog-supported built-in API.
- Debug execution either speaks DAP to a configured adapter or returns a typed actionable unavailable error; it never claims a real action occurred when it did not.
- Production tape renders or safely falls back for all existing rich content variants and raw input paste is handled without data loss.
- Package enablement/overrides/doctor/validation are persisted and tested.
- Persona files affect the constructed system prompt and embedded prompt assets are consumed.
- Memory command operations mutate/query the configured backend and pipeline with deterministic tests.
- Stale claims are corrected; no code comments say tape is standalone or language policy remains when it does not.
