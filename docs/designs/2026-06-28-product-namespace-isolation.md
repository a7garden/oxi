# Product Namespace Isolation (`OXI_HOME` Unification)

**Date:** 2026-06-28
**Status:** Implemented
**Scope:** `oxi-ai` (leaf), `oxi-sdk` (delegation)

## Problem

`oxi-ai` is the dependency **leaf** (zero `oxi-*` deps) — a reusable library
embedded by multiple products (`oxi-cli`, `oxios`, downstream forks). It
hardcoded the `~/.oxi/` directory for three runtime reads:

| Site | Read | Escape hatch? |
|---|---|---|
| `catalog/override_.rs:67` | `~/.oxi/catalog/overrides.toml` (provider override probe) | **none** |
| `catalog/models_dev.rs:416` | `~/.oxi/cache/models-dev.json` | `OXI_MODELS_DEV_CACHE_PATH` (file-level) |
| `oauth.rs:126` | `~/.oxi/auth.json` (auth store) | **none** |

Because `oxi-sdk` **already** resolved a product home via `$OXI_HOME → ~/.oxi`
(`ports/fs/path.rs`), the workspace was *internally inconsistent*: setting
`OXI_HOME=/custom` redirected `oxi-cli`'s own sessions/settings/auth
(`oxi-sdk` paths) but the leaf library silently kept reading `~/.oxi/`.

Concretely, `get_builtin_providers()` (`providers/register_builtins.rs:184`)
caches providers in a process-global `OnceLock`, and its init closure calls
`load_overrides()` → `find_override_files()` → the hardcoded `~/.oxi/` probe.
So **every embedder** of `oxi-ai` involuntarily inherits the `oxi-cli`
override namespace on the provider hot path — a library-layer coupling smell.

## Root cause

Two parallel resolutions of the same concept ("the oxi product home") lived in
different crates and disagreed:

```
oxi-sdk:  $OXI_HOME → ~/.oxi        (respected)
oxi-ai:   ~/.oxi  (hardcoded)        (ignored $OXI_HOME)
```

## Design

**Lift the existing `OXI_HOME` convention to the leaf.** oxi-ai becomes the
single source of truth; oxi-sdk delegates to it.

### Why env-var, not a typed global

A product identity is **process-global**: one binary is one product, with one
home namespace, for its entire lifetime. An environment variable is the
natural representation:

- Set at process spawn, **before** any library code runs.
- Readable inside any lazy initializer — including the `OnceLock` that caches
  `get_builtin_providers()`. The env var is already resolved by the time the
  first catalog lookup fires.
- Avoids introducing a second, mutable, init-ordered global (`init_namespace()`
  + `current_namespace()`) alongside the already-established `OXI_HOME`
  convention — two ways to do the same thing breeds precedence ambiguity.

This deliberately **does not** extend `ProductMeta` (materialize.rs), which
carries provider HTTP headers from `data/catalog/product-meta.toml` — a
distinct concern. Conflating request headers with filesystem namespace would
create a god-object.

### Implementation

**`oxi-ai/src/product_env.rs`** (new) — the canonical resolver:

```rust
fn resolve_home(oxi_home: Option<&str>, user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = oxi_home.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));           // $OXI_HOME wins
    }
    user_home.map(|h| h.join(".oxi"))            // fallback $HOME/.oxi
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
  (still after the more-specific `OXI_MODELS_DEV_CACHE_PATH` check)
- `oauth.rs` auth → `product_env::auth_path()` (+ parent-dir creation)

**`oxi-sdk/src/ports/fs/path.rs`** — `home_dir()` now delegates to
`oxi_ai::product_env::home_dir()`. One resolution path across both crates; the
dead duplicate logic is removed.

### What an embedder does

```text
OXI_HOME=~/.oxios   # oxios: its own ~/.oxios/{catalog,cache,auth.json}
```

No `oxi-ai` API to learn, no init-ordering contract, no code change in the
embedder beyond setting one variable. The leaf library and the SDK both honor
it uniformly.

## Backward compatibility

- `OXI_HOME` unset (the common case, incl. all existing `oxi-cli` users) →
  `~/.oxi/`, identical to prior behavior.
- `OXI_CATALOG_OVERRIDE` (single-file escape) and `OXI_MODELS_DEV_*` family
  are unchanged — they remain the more-specific knobs on top of the product
  home default.

## Verification

- `product_env` unit tests: `resolve_home` precedence (OXI_HOME wins, empty
  falls through, default `.oxi`, both-absent → `None`), subpath composition,
  live `home_dir()` smoke test. All pass under parallel runners (no env
  mutation).
- `override_` / `models_dev` / `oauth` module tests: 32 pass.
- Workspace `cargo clippy -D warnings` + `clippy-native-browser` + `nextest`.
- Regression: `find_override_files` honors `OXI_HOME` end-to-end (tempdir +
  override file + env var round-trip).

## Non-goals

- Separating the base catalog from overrides inside the `get_builtin_providers`
  `OnceLock` — too invasive for the foundation layer and would change the
  semantics of every `get_provider_*` lookup. The probe location is the only
  thing changed.
- A programmatic (`init_namespace()`) injection knob. `OXI_HOME` covers the
  programmatic case via `std::env::set_var` at startup; a parallel typed API
  would duplicate it. Add only if a concrete embedder needs typed control
  without env.
