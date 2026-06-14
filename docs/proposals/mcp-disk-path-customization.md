# oxi-sdk MCP Disk-Path Customization for SDK Consumers

**Author:** oxios project
**Date:** 2026-06-13
**Status:** Draft — Codebase-verified
**Target:** oxi-sdk / oxi-agent (oxi project)
**Related:** `docs/proposals/sdk-consumer-requirements.md`, oxios RFC-023 (MCP delegation)

---

## Background

oxi 0.33.0 shipped a first-class MCP system (`oxi_agent::mcp`) that oxios wants to
adopt wholesale, replacing its hand-rolled `oxios-mcp` crate. The oxi implementation
is strictly better on every axis the oxios implementation cared about: standard
Content-Length framing, lifecycle modes (lazy/eager/keep-alive), disk-backed
metadata cache, consent manager, full protocol (resources/prompts/sampling),
and a deadlock-free mpsc-based lifecycle loop.

The SDK layer already exposes the right hooks for the **in-memory** parts:

```rust
// oxi-sdk/src/builder.rs
impl OxiBuilder {
    pub fn with_mcp_config(self, config: oxi_agent::mcp::McpConfig) -> Self { ... }
    pub fn with_mcp(self, enabled: bool) -> Self { ... }
}
```

This lets oxios inject a programmatic `McpConfig` built from its own
`~/.oxios/config.toml`, **without** oxi auto-discovering `~/.config/oxi/mcp.json`.

The single remaining gap is the **disk state**: the metadata cache and consent
manager still resolve their file paths via hardcoded defaults
(`~/.config/oxi/mcp-cache.json`, `~/.config/oxi/mcp-consent.json`). oxios needs
those files under `~/.oxios/` so that:

1. oxios has a single source of truth under `~/.oxios/` (config + state).
2. oxi CLI users and oxios users don't share/scribble on each other's MCP cache
   and consent state by accident.
3. The pattern is symmetric with how oxios already handles provider credentials
   (`oxios-kernel/src/credential.rs` injects keys into `OxiBuilder` instead of
   letting oxi read its own files).

This is the same class of request as `sdk-consumer-requirements.md`: an additive,
non-breaking SDK surface change that lets a consumer self-host its state.

> **Note on `~/.oxi/auth.json`:** oxios *intentionally* shares oxi CLI's auth
> store (`~/.oxi/auth.json`) so that `oxi auth login` works for both. That path
> is not in scope here — only the MCP-specific cache/consent files.

---

## Request 1: `McpManager::spawn_with_paths(config, cache_path, consent_path)`

**Priority:** High — blocks oxios MCP delegation
**Effort estimate:** Low (extract a private helper, add one public constructor)

### Problem

`McpManager` has three construction paths today:

| Constructor | Config source | Cache path | Consent path |
|-------------|---------------|------------|--------------|
| `spawn()` | `config::load_mcp_config()` (auto-discover) | `MetadataCache::new()` → hardcoded | `ConsentManager::new()` → hardcoded |
| `spawn_with_config(cfg)` | injected ✅ | `MetadataCache::new()` → hardcoded ❌ | `ConsentManager::new()` → hardcoded ❌ |
| `new_no_spawn()` | auto-discover | hardcoded | hardcoded |

`spawn_with_config` was added specifically for SDK consumers (its doc says
*"used by the SDK `OxiBuilder::with_mcp_config`"*), but it only parameterizes
the **config object**, not the **disk paths**. A consumer that injects its own
config still finds cache/consent files appearing under `~/.config/oxi/`.

### Current state (verified code)

```rust
// oxi-agent/src/mcp/mod.rs:137-148
pub fn spawn_with_config(mcp_config: McpConfig) -> Arc<Self> {
    let cache = MetadataCache::new();          // ← default_cache_path() hardcoded
    let _ = cache.load();
    let consent = ConsentManager::new();        // ← default_consent_path() hardcoded
    let _ = consent.load();
    // ...
}
```

The good news: the path-parameterized constructors already exist and are `pub`:

```rust
// oxi-agent/src/mcp/cache.rs:88
pub fn with_path(cache_path: PathBuf) -> Self { ... }

// oxi-agent/src/mcp/consent.rs:56
pub fn with_path(persist_path: PathBuf) -> Self { ... }
```

`ConsentManager::new()` even delegates to `with_path(default_consent_path())`.
The pieces exist — `spawn_with_config` just doesn't wire them through.

### Desired state

```rust
// oxios usage
let manager = oxi_sdk::mcp::McpManager::spawn_with_paths(
    oxi_mcp_config,
    oxios_home.join("mcp-cache.json"),
    oxios_home.join("mcp-consent.json"),
);
```

### What oxi-agent needs to add

> **Verified against `oxi-agent/src/mcp/mod.rs` as of 0.33.0.**

Refactor the construction so the eager-connect seeding logic isn't duplicated,
then add one public constructor. Suggested implementation:

```rust
// oxi-agent/src/mcp/mod.rs

impl McpManager {
    /// **Primary constructor.** Spawns the lifecycle task, uses default disk
    /// paths, and eagerly connects to Eager/KeepAlive servers.
    pub fn spawn() -> Arc<Self> {
        Self::spawn_with_paths(
            config::load_mcp_config(),
            None,  // default cache path
            None,  // default consent path
        )
    }

    /// Spawn with a programmatically-supplied config. Disk paths default
    /// to `~/.config/oxi/`. (Backwards-compatible with 0.33.0.)
    pub fn spawn_with_config(mcp_config: McpConfig) -> Arc<Self> {
        Self::spawn_with_paths(mcp_config, None, None)
    }

    /// Spawn with a programmatically-supplied config **and** custom disk
    /// paths for the metadata cache and consent store.
    ///
    /// Pass `None` for either path to use the oxi default
    /// (`~/.config/oxi/mcp-cache.json` / `mcp-consent.json`).
    ///
    /// This is the constructor SDK consumers (e.g. oxios) should use when
    /// they self-host MCP state under their own config directory.
    pub fn spawn_with_paths(
        mcp_config: McpConfig,
        cache_path: Option<PathBuf>,
        consent_path: Option<PathBuf>,
    ) -> Arc<Self> {
        let cache = match cache_path {
            Some(p) => MetadataCache::with_path(p),
            None => MetadataCache::new(),
        };
        let _ = cache.load();

        let consent = match consent_path {
            Some(p) => ConsentManager::with_path(p),
            None => ConsentManager::new(),
        };
        let _ = consent.load();

        // ... rest identical to current spawn_with_config body ...
    }
}
```

`spawn()` and `spawn_with_config(cfg)` become thin wrappers — zero behavior
change for existing callers.

### Acceptance criteria

- [ ] `McpManager::spawn_with_paths(config, Some(path), Some(path))` writes
      cache/consent to the supplied paths (verified by a test using `TempDir`).
- [ ] `McpManager::spawn()` and `spawn_with_config(cfg)` are unchanged in
      observable behavior (existing tests pass unmodified).
- [ ] No public signature is removed or altered (additive only).

---

## Request 2: `OxiBuilder::with_mcp_paths(cache_path, consent_path)`

**Priority:** High — pairs with Request 1 at the SDK layer
**Effort estimate:** Low (two `Option<PathBuf>` fields + one builder method)

### Problem

`OxiBuilder` lets a consumer inject a config object but not disk paths:

```rust
// oxi-sdk/src/builder.rs:504-511
pub fn build(self) -> Oxi {
    let mcp_manager = if self.mcp_enabled {
        Some(match self.mcp_config {
            Some(cfg) => oxi_agent::mcp::McpManager::spawn_with_config(cfg),
            None => oxi_agent::mcp::McpManager::spawn(),
        })
    } else {
        None
    };
    // ...
}
```

A consumer that wants custom disk paths must drop `OxiBuilder` and construct
the `McpManager` itself — defeating the purpose of the SDK surface.

### Current state (consumer code, projected)

Without this request, oxios would have to do something awkward like:

```rust
// Not possible today through the builder — would require manual Oxi assembly.
let mcp = McpManager::spawn_with_paths(cfg, cache, consent);
// then somehow inject into Oxi... but OxiBuilder::build() always spawns its own.
```

### Desired state

```rust
// oxios engine assembly — symmetric with with_mcp_config
let oxi = OxiBuilder::new()
    .with_mcp_config(oxi_mcp_config)
    .with_mcp_paths(
        oxios_home.join("mcp-cache.json"),
        oxios_home.join("mcp-consent.json"),
    )
    .build();
```

### What oxi-sdk needs to add

> **Verified against `oxi-sdk/src/builder.rs` as of 0.33.0.**
> Relevant existing fields: `mcp_config: Option<McpConfig>` (line 158),
> `mcp_enabled: bool` (line 161), `Oxi::mcp()` accessor (lines 74-76).

```rust
// oxi-sdk/src/builder.rs

pub struct OxiBuilder {
    // ... existing fields ...
    mcp_config: Option<oxi_agent::mcp::McpConfig>,
    mcp_enabled: bool,
    // ── NEW ──
    mcp_cache_path: Option<PathBuf>,
    mcp_consent_path: Option<PathBuf>,
}

impl OxiBuilder {
    // in new():
    mcp_cache_path: None,
    mcp_consent_path: None,

    /// Set custom disk paths for the MCP metadata cache and consent store.
    ///
    /// Only takes effect when MCP is enabled. When unset, oxi uses its
    /// default paths (`~/.config/oxi/`). Intended for SDK consumers that
    /// self-host MCP state under their own config directory.
    ///
    /// Combine with [`OxiBuilder::with_mcp_config`] to also inject a
    /// programmatic config (otherwise oxi auto-discovers from its standard
    /// config file locations).
    pub fn with_mcp_paths(
        mut self,
        cache_path: PathBuf,
        consent_path: PathBuf,
    ) -> Self {
        self.mcp_cache_path = Some(cache_path);
        self.mcp_consent_path = Some(consent_path);
        self
    }

    // in build():
    let mcp_manager = if self.mcp_enabled {
        // If either path is set, go through spawn_with_paths regardless of
        // whether a config was supplied.
        if self.mcp_cache_path.is_some() || self.mcp_consent_path.is_some() {
            let cfg = self.mcp_config.unwrap_or_default();
            Some(oxi_agent::mcp::McpManager::spawn_with_paths(
                cfg,
                self.mcp_cache_path,
                self.mcp_consent_path,
            ))
        } else {
            Some(match self.mcp_config {
                Some(cfg) => oxi_agent::mcp::McpManager::spawn_with_config(cfg),
                None => oxi_agent::mcp::McpManager::spawn(),
            })
        }
    } else {
        None
    };
}
```

`McpConfig` already `impl Default` (added in 0.33.0 per CHANGELOG), so
`unwrap_or_default()` is safe when only paths are supplied.

### Acceptance criteria

- [ ] `OxiBuilder::with_mcp_paths(a, b).build()` produces an `Oxi` whose
      `mcp()` manager reads/writes cache and consent at `a` and `b`.
- [ ] `OxiBuilder` without `with_mcp_paths` behaves exactly as in 0.33.0.
- [ ] `with_mcp_config` and `with_mcp_paths` compose (config + paths together).
- [ ] `with_mcp(false)` still disables MCP entirely regardless of paths/config.

---

## Request 3 (optional): Re-export `MetadataCache`

**Priority:** Low — convenience for advanced consumers
**Effort estimate:** Trivial (one `pub use`)

### Problem

`MetadataCache` is not in the oxi-sdk re-export block, although its sibling
`ConsentManager` is:

```rust
// oxi-sdk/src/lib.rs:591-596
pub use oxi_agent::mcp::{
    ConsentManager, ConsentState, DirectToolDef, DirectToolsConfig, LifecycleMode,
    McpCallResult, McpConfig, ..., ToolPrefix,
};   // ← MetadataCache absent
```

A consumer that wants to inspect or clear its own cache (e.g. an oxios
"reset MCP state" admin action) must reach through to `oxi_agent::mcp::cache`,
pulling in a transitive dependency on `oxi-agent`.

### What oxi-sdk needs to add

```rust
// oxi-sdk/src/lib.rs — add to the existing re-export block
pub use oxi_agent::mcp::{ConsentManager, MetadataCache, ConsentState, ...};
```

### Acceptance criteria

- [ ] `oxi_sdk::MetadataCache` resolves and exposes `with_path`, `path`,
      `load`, `get_tools`, `cached_servers`, `update`.

---

## Impact Analysis

### For oxi project

| Change | Risk | Scope | Lines |
|--------|------|-------|-------|
| `McpManager::spawn_with_paths` | **Low** — additive; existing constructors become wrappers | `oxi-agent/src/mcp/mod.rs` | ~20 |
| `OxiBuilder::with_mcp_paths` | **Low** — additive; 2 fields + 1 method + build() branch | `oxi-sdk/src/builder.rs` | ~25 |
| Re-export `MetadataCache` | **None** — additive `pub use` | `oxi-sdk/src/lib.rs` | 1 |

> **Risk note:** `spawn()` and `spawn_with_config()` keep their signatures, so
> oxi-cli, oxi-tui, and any direct `oxi-agent` consumer are unaffected. The
> `build()` change in oxi-sdk only adds a branch taken when paths are set —
> the default path is byte-for-byte the same behavior as 0.33.0.

### For oxios project

| Change | Benefit |
|--------|---------|
| Self-hosted MCP cache/consent under `~/.oxios/` | Single source of truth; no cross-product state leakage |
| Drop `oxios-mcp` crate entirely | Removes ~750 LOC of non-standard (JSONL) reimplementation |
| Standard Content-Length framing via oxi | Interop with Claude Desktop / Cursor / official SDK servers |

### Migration path

1. **oxi-agent** adds `McpManager::spawn_with_paths` (Request 1). Existing
   constructors delegate to it.
2. **oxi-sdk** adds `with_mcp_paths` builder method + `MetadataCache` re-export
   (Requests 2 & 3). Ships in next oxi release.
3. **oxios** bumps `oxi-sdk` to the release containing 1–3, switches to
   `with_mcp_config` + `with_mcp_paths`, and removes the `oxios-mcp` crate.
   (Tracked in oxios RFC-023.)

Steps 1–2 land entirely within oxi; step 3 is oxios-side and unblocked once
the new API ships.

### Testing

Each request should include a `TempDir`-based test:

- Request 1: spawn with explicit paths, connect a mock/no-op server, assert
  cache file appears at the supplied path.
- Request 2: build via `OxiBuilder` with `with_mcp_paths`, assert `Oxi::mcp()`
  manager's `cache().path()` matches.
- Request 3: `assert_eq!(oxi_sdk::MetadataCache::with_path(p).path(), p)`.

---

## Summary

| # | Request | Priority | Effort | Files | Blocking oxios |
|---|---------|----------|--------|-------|----------------|
| 1 | `McpManager::spawn_with_paths(config, cache, consent)` | High | Low | `oxi-agent/src/mcp/mod.rs` | Yes |
| 2 | `OxiBuilder::with_mcp_paths(cache, consent)` | High | Low | `oxi-sdk/src/builder.rs` | Yes |
| 3 | Re-export `MetadataCache` | Low | Trivial | `oxi-sdk/src/lib.rs` | No |

All three are additive — no public API is removed or its signature changed.
Together they let SDK consumers (oxios and future ones) self-host MCP disk
state, completing the injection story that `with_mcp_config` started. This is
the direct MCP analogue of the credential-injection pattern oxios already uses
for providers, and the last blocker before oxios can delete its hand-rolled
MCP client.
