# Brief 02: Dependency Maintenance

**Area:** Unmaintained crate warnings from `cargo audit`
**Severity:** 🟢 Normal
**Estimated scope:** 3 unmaintained crate warnings, 0 vulnerability advisories

---

## Context

`cargo audit` reports 3 unmaintained crate warnings and 0 actual vulnerability advisories. This is a clean security posture. The unmaintained warnings are transitive dependencies — none are directly depended upon by oxicode crates.

Current state:

| Crate | Version | Advisory ID | Dependency Path | Impact |
|-------|---------|-------------|-----------------|--------|
| `number_prefix` | 0.4.0 | RUSTSEC-2025-0119 | `indicatif 0.17.11` ← `self_update 0.41.0` ← `oxicode-cli` | Number formatting; unmaintained since 2025-11 |
| `paste` | 1.0.15 | RUSTSEC-2024-0436 | `rav1e 0.8.1` / `boa_string 0.20.0` ← `oxibrowser-core` ← `oxicode-agent` / `oxicode-cli` | Procedural macro crate; unmaintained since 2024-10 |

None of these are direct security vulnerabilities. They are **informational warnings** about crates that are no longer maintained by their authors.

---

## Objective

Resolve or acknowledge all 3 unmaintained crate warnings from `cargo audit`.

This does NOT mean:
- ❌ Removing dependencies that are needed (self_update, oxibrowser-core)
- ❌ Forking and maintaining these crates ourselves
- ❌ Pinning to older versions that may have actual vulnerabilities

It DOES mean:
- ✅ `cargo audit 2>&1 | grep "Warning:" | wc -l` is reduced from 3 to 2 (fxhash resolved by replacing a3s-search with oxibrowser search)
- ✅ For warnings that cannot be resolved, document the reasoning

---

## Approach

### Phase 1: Assess each dependency (read-only)

For each unmaintained crate:

1. ~~**`fxhash`**~~: **Resolved.** Replaced `a3s-search` with `oxibrowser` search module (v0.14.1+). `fxhash` is no longer a transitive dependency.
2. **`number_prefix`**: Check if `indicatif` or `self_update` has a newer version that drops this dependency.
3. **`paste`**: Check if `rav1e` or `boa_*` crates have newer versions that replaced `paste`.

Commands:
```bash
cargo update --dry-run 2>&1 | head -40
cargo update -p fxhash --dry-run 2>&1
cargo update -p number_prefix --dry-run 2>&1
cargo update -p paste --dry-run 2>&1
```

### Phase 2: Update if possible

For each crate where a newer parent dependency exists:
```bash
cargo update -p <parent-crate>
```

Verify the update doesn't break anything:
```bash
cargo check --workspace
cargo nextest run --workspace
```

### Phase 3: Document or suppress

For warnings that cannot be resolved (no newer version available):

Option A: Add to `[advisories]` in `.cargo/audit.toml` (or create it) with a comment:
```toml
[advisories]
ignore = [
    # RUSTSEC-2025-0057: RESOLVED — a3s-search removed, fxhash no longer in tree.
    # Keeping entry as documentation; cargo audit will not flag it.
]
```

Option B: If the dependency path is optional (e.g., `oxibrowser-core` is behind a feature gate), ensure the feature is opt-in and document that it pulls in unmaintained transitive deps.

### Phase 4: Verify

1. `cargo audit 2>&1` — verify warnings are resolved or explicitly acknowledged.
2. `cargo check --workspace` — still compiles.
3. `cargo nextest run --workspace` — tests still pass.

---

## Constraints

- **Do not** remove functionality (web search, self-update, browser) to eliminate transitive deps.
- **Do not** fork unmaintained crates — that's a maintenance burden that exceeds the risk.
- **Preserve** exact dependency versions in `Cargo.lock` unless intentionally updating.
- If suppressing warnings, **document why** in the audit config file.

## Verification

1. `cargo audit 2>&1 | grep "Warning:" | wc -l` — count is 0 or each remaining warning is documented
2. `cargo check --workspace` — clean
3. `cargo nextest run --workspace` — 2131 tests pass
