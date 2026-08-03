# Brief 05: Error Handling Consistency

**Area:** anyhow vs thiserror usage conventions across workspace crates
**Severity:** 🟢 Normal
**Estimated scope:** 4 crates use both anyhow + thiserror, 1 crate uses anyhow only (oxicode-tui), mixed patterns

---

## Context

oxicode uses a mix of `anyhow::Result` for application code and `thiserror` for library error types. This is the standard Rust pattern — but the convention is not documented, and there are inconsistencies:

- **oxicode-ai**: Uses `thiserror` for `ProviderError`, `CircuitBreakerError` + `anyhow` for internal code. ✅ Correct pattern.
- **oxicode-agent**: Uses `thiserror` for `ToolError` + `anyhow` for agent code. ✅ Correct pattern.
- **oxicode-store**: Uses both, but session and settings errors use `anyhow` rather than typed errors.
- **oxicode-sdk**: Uses `thiserror` for `SdkError` + `anyhow` for builders. ✅ Correct pattern.
- **oxicode-tui**: Uses `anyhow` only — no typed errors. ⚠️ TUI is a leaf crate so this is acceptable.
- **oxicode-cli**: Uses both — appropriate for the integration layer.

The AGENTS.md convention says:
> "Prefer `anyhow::Result` for application code, custom error enums (`thiserror`) for library crates."

This is the right convention but it's not enforced and has some gaps — particularly in `oxicode-store` where session/settings errors that propagate to the CLI layer are `anyhow` rather than typed.

Current state:

| Crate | `thiserror` error types | `anyhow` usage | Consistent? |
|-------|------------------------|----------------|-------------|
| oxicode-ai | `ProviderError`, `CircuitBreakerError`, `ToolError` | Internal providers, transform | ✅ |
| oxicode-agent | `ToolError`, `AgentError` | Agent loop, tools | ✅ |
| oxicode-store | (none explicitly typed) | Session, settings, auth | ⚠️ Could benefit from typed errors |
| oxicode-tui | (none) | Widget rendering | ✅ (leaf crate) |
| oxicode-sdk | `SdkError` | Builders, lifecycle | ✅ |
| oxicode-cli | (none — consumes others) | All handlers | ✅ (application layer) |

---

## Objective

Document the error handling convention and ensure `oxicode-store` has typed errors for its public API, since it is a library crate consumed by `oxicode-cli` and potentially third-party code.

This does NOT mean:
- ❌ Converting all `anyhow` to `thiserror` everywhere
- ❌ Adding error types to `oxicode-tui` (it's a leaf widget crate)
- ❌ Adding error types to `oxicode-cli` (it's the application layer)
- ❌ Changing the error types in oxicode-ai, oxicode-agent, or oxicode-sdk (they're already correct)
- ❌ Creating a shared error crate

It DOES mean:
- ✅ `oxicode-store` has typed errors for session, settings, and auth operations
- ✅ The convention is documented in AGENTS.md or CONTRIBUTING.md
- ✅ All existing tests pass

---

## Approach

### Phase 1: Survey (read-only)

1. List all public functions in `oxicode-store/src/` that return `anyhow::Result`:
   ```bash
   rg "pub (async )?fn \w+.*->.*Result" oxicode-store/src/ | grep -v test
   ```
2. Identify which errors should be typed (public API functions consumed by other crates) vs which can stay `anyhow` (internal helpers).
3. Check what errors `oxicode-cli` matches on from `oxicode-store` — if it pattern-matches on error messages, that's a sign typed errors are needed.

### Phase 2: Add typed errors to oxicode-store (if warranted)

1. Create `oxicode-store/src/error.rs` with:
   ```rust
   #[derive(Debug, thiserror::Error)]
   pub enum StoreError {
       #[error("session not found: {0}")]
       SessionNotFound(Uuid),
       #[error("settings parse error: {0}")]
       SettingsParse(#[from] toml::de::Error),
       #[error("auth storage error: {0}")]
       AuthStorage(String),
       #[error("I/O error: {0}")]
       Io(#[from] std::io::Error),
   }
   ```
2. Update public session/settings/auth functions to return `Result<T, StoreError>`.
3. Internal functions can continue using `anyhow`.

### Phase 3: Document convention

Add to AGENTS.md under "Code Style":
```markdown
### Error Handling Convention
- **Library crates** (oxicode-ai, oxicode-agent, oxicode-store, oxicode-sdk): Use `thiserror` for public error types.
  Internal functions may use `anyhow`.
- **Application crates** (oxicode-cli): Use `anyhow::Result` everywhere.
- **Leaf crates** (oxicode-tui): `anyhow` is acceptable.
```

### Phase 4: Verify

1. `cargo check --workspace` — compiles
2. `cargo nextest run --workspace` — 2131 tests pass
3. `cargo clippy --workspace -- -D warnings` — clean

---

## Constraints

- **Do not** change error types in oxicode-ai, oxicode-agent, or oxicode-sdk — they're already consistent.
- **Do not** create a workspace-level shared error crate.
- **Preserve** all existing test assertions.
- **Do not** change public function signatures in oxicode-store unless the return type changes from `anyhow::Result<T>` to `Result<T, StoreError>` — no behavioral changes.

## Verification

1. `cargo nextest run --workspace` — 2131 tests pass
2. `cargo clippy --workspace -- -D warnings` — clean
3. `rg "pub.*fn.*->.*anyhow::Result" oxicode-store/src/ | grep -v test` — reduced or zero
