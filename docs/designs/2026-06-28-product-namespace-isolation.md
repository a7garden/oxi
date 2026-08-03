# Product Namespace Isolation (`OXICODE_HOME` Unification)

**Date:** 2026-06-28
**Status:** Implemented
**Scope:** `oxicode-ai` (leaf), `oxicode-sdk` (delegation)

## Problem

`oxicode-ai` is the dependency **leaf** (zero `oxicode-*` deps) — a reusable library
embedded by multiple products (`oxicode-cli`, `oxios`, downstream forks). It
hardcoded the `~/.oxicode/` directory for three runtime reads:

| Site | Read | Escape hatch? |
|---|---|---|
| `catalog/override_.rs:67` | `~/.oxicode/catalog/overrides.toml` (provider override probe) | **none** |
| `catalog/models_dev.rs:416` | `~/.oxicode/cache/models-dev.json` | `OXICODE_MODELS_DEV_CACHE_PATH` (file-level) |
| `oauth.rs:126` | `~/.oxicode/auth.json` (auth store) | **none** |

Because `oxicode-sdk` **already** resolved a product home via `$OXICODE_HOME → ~/.oxicode`
(`ports/fs/path.rs`), the workspace was *internally inconsistent*: setting
`OXICODE_HOME=/custom` redirected `oxicode-cli`'s own sessions/settings/auth
(`oxicode-sdk` paths) but the leaf library silently kept reading `~/.oxicode/`.

Concretely, `get_builtin_providers()` (`providers/register_builtins.rs:184`)
caches providers in a process-global `OnceLock`, and its init closure calls
`load_overrides()` → `find_override_files()` → the hardcoded `~/.oxicode/` probe.
So **every embedder** of `oxicode-ai` involuntarily inherits the `oxicode-cli`
override namespace on the provider hot path — a library-layer coupling smell.

## Root cause

Two parallel resolutions of the same concept ("the oxicode product home") lived in
different crates and disagreed:

```
oxicode-sdk:  $OXICODE_HOME → ~/.oxicode        (respected)
oxicode-ai:   ~/.oxicode  (hardcoded)        (ignored $OXICODE_HOME)
```

## Design

**Lift the existing `OXICODE_HOME` convention to the leaf.** oxicode-ai becomes the
single source of truth; oxicode-sdk delegates to it.

### Why env-var, not a typed global

A product identity is **process-global**: one binary is one product, with one
home namespace, for its entire lifetime. An environment variable is the
natural representation:

- Set at process spawn, **before** any library code runs.
- Readable inside any lazy initializer — including the `OnceLock` that caches
  `get_builtin_providers()`. The env var is already resolved by the time the
  first catalog lookup fires.
- Avoids introducing a second, mutable, init-ordered global (`init_namespace()`
  + `current_namespace()`) alongside the already-established `OXICODE_HOME`
  convention — two ways to do the same thing breeds precedence ambiguity.

This deliberately **does not** extend `ProductMeta` (materialize.rs), which
carries provider HTTP headers from `data/catalog/product-meta.toml` — a
distinct concern. Conflating request headers with filesystem namespace would
create a god-object.

### Implementation

**`oxicode-ai/src/product_env.rs`** (new) — the canonical resolver:

```rust
fn resolve_home(oxicode_home: Option<&str>, user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = oxicode_home.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));           // $OXICODE_HOME wins
    }
    user_home.map(|h| h.join(".oxicode"))            // fallback $HOME/.oxicode
}

pub fn home_dir() -> std::io::Result<PathBuf> { /* wires env → resolve_home */ }
pub fn catalog_override_dir() -> Option<PathBuf> { /* <home>/catalog */ }
pub fn cache_dir() -> Option<PathBuf> { /* <home>/cache */ }
pub fn auth_path() -> Option<PathBuf> { /* <home>/auth.json */ }
```

The pure `resolve_home` is split out so it is **testable without racing the
process-global environment** under parallel test runners.

**Three call-sites routed through `product_env`:**

- `override_.rs` probe → `product_env::catalog_override_dir().join("overrides.toml")`
- `models_dev.rs` cache → `product_env::cache_dir().join("models-dev.json")`
  (still after the more-specific `OXICODE_MODELS_DEV_CACHE_PATH` check)
- `oauth.rs` auth → `product_env::auth_path()` (+ parent-dir creation)

**`oxicode-sdk/src/ports/fs/path.rs`** — `home_dir()` now delegates to
`oxicode_ai::product_env::home_dir()`. One resolution path across both crates; the
dead duplicate logic is removed.

### What an embedder does

```text
OXICODE_HOME=~/.oxios   # oxios: its own ~/.oxios/{catalog,cache,auth.json}
```

No `oxicode-ai` API to learn, no init-ordering contract, no code change in the
embedder beyond setting one variable. The leaf library and the SDK both honor
it uniformly.

## Backward compatibility

- `OXICODE_HOME` unset (the common case, incl. all existing `oxicode-cli` users) →
  `~/.oxicode/`, identical to prior behavior.
- `OXICODE_CATALOG_OVERRIDE` (single-file escape) and `OXICODE_MODELS_DEV_*` family
  are unchanged — they remain the more-specific knobs on top of the product
  home default.

## Verification

- `product_env` unit tests: `resolve_home` precedence (OXICODE_HOME wins, empty
  falls through, default `.oxicode`, both-absent → `None`), subpath composition,
  live `home_dir()` smoke test. All pass under parallel runners (no env
  mutation).
- `override_` / `models_dev` / `oauth` module tests: 32 pass.
- Workspace `cargo clippy -D warnings` + `clippy-native-browser` + `nextest`.
- Regression: `find_override_files` honors `OXICODE_HOME` end-to-end (tempdir +
  override file + env var round-trip).

## Non-goals

- Separating the base catalog from overrides inside the `get_builtin_providers`
  `OnceLock` — too invasive for the foundation layer and would change the
  semantics of every `get_provider_*` lookup. The probe location is the only
  thing changed.
- A programmatic (`init_namespace()`) injection knob. `OXICODE_HOME` covers the
  programmatic case via `std::env::set_var` at startup; a parallel typed API
  would duplicate it. Add only if a concrete embedder needs typed control
  without env.
