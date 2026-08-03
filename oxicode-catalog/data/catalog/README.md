# oxicode catalog — models.dev as source of truth

oxicode's provider/model catalog is powered by [models.dev](https://models.dev)
(MIT, the same source used by opencode). The catalog data flows through a
single source of truth with progressive enhancement layers:

| Layer | Source | Mutable? | Offline? |
|-------|--------|----------|----------|
| **SNAP** — embedded snapshot | `data/catalog/_snapshot.json.gz` (compiled into the binary) | No (read-only) | Yes |
| **LIVE** — runtime cache | `~/.oxicode/cache/models-dev.json` (ETag-aware conditional GET) | Yes (auto-refresh) | No (network on first refresh) |
| **Layer 2** — user override | `~/.oxicode/catalog/overrides.toml`, `.oxicode/catalog.local.toml` | Yes (user-owned) | Yes |
| **LOCAL** — runtime discovery | `GET {base_url}/v1/models` for ollama/lmstudio/vllm/sglang | Yes (transient) | No (network) |

## How it works

1. **SNAP** (`_snapshot.json.gz`): A gzip'd copy of
   `https://models.dev/api.json` (202KB compressed, ~5277 models across 145
   providers). Included at compile time via `include_bytes!`. This ensures
   oxicode works fully offline on first run.

2. **LIVE** (runtime cache): On each run, oxicode checks the local cache mtime.
   If within the mtime window (default 1 hour), no HTTP request is made. If
   older, a conditional GET (`If-None-Match` ETag) is sent — a `304 Not
   Modified` response costs ~0 bytes. Run `oxicode refresh` to force an update.

3. **Materialize**: Both SNAP and LIVE are deserialized into `MdCatalog`,
   then [`materialize()`](../../src/catalog/materialize.rs) converts them to
   oxicode's internal `BuiltinProviderEntry` / `BuiltinModelEntry` using
   [`protocol_for(npm)`](../../src/catalog/models_dev.rs) to map the
   models.dev `npm` field to oxicode's `Api` enum (7 match arms).

4. **Layer 2** (overrides): User-supplied `overrides.toml` takes highest
   precedence — same `(provider, id)` replaces built-in, new entries append.

5. **LOCAL**: Runtime `/v1/models` discovery for local servers (ollama,
   lmstudio, etc.) is layered on top via the `runtime` module.

## File layout

```
data/catalog/
  _snapshot.json.gz          # SNAP: models.dev snapshot (gzip'd, ~202KB)
  providers.toml             # Provider metadata for create_builtin_provider()
  product-meta.toml          # oxicode-specific extra HTTP headers (9 providers)
  README.md                  # This file
```

> **Note**: `data/catalog/models/` and `data/catalog/openclaw/` have been
> removed — model data now comes exclusively from models.dev via the
> embedded snapshot.

## Updating the snapshot

The snapshot is committed to the repo. To refresh it (e.g., before a
release):

```bash
# Download the latest api.json
curl -sL https://models.dev/api.json -o /tmp/api.json

# Compress and replace the snapshot
gzip -c /tmp/api.json > data/catalog/_snapshot.json.gz
```

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `OXICODE_MODELS_DEV` | `auto` | `auto`/`on`/`off` — enable/disable models.dev |
| `OXICODE_MODELS_DEV_URL` | `https://models.dev` | Enterprise mirror URL |
| `OXICODE_MODELS_DEV_DISABLE_FETCH` | (unset) | `1` = air-gapped (no live fetch) |
| `OXICODE_MODELS_DEV_MTIME_WINDOW` | `3600` | Seconds before conditional GET (default 1h) |
| `OXICODE_MODELS_DEV_FORCE_REFRESH` | (unset) | `1` = force conditional GET on next access |
| `OXICODE_MODELS_DEV_CACHE_PATH` | `~/.oxicode/cache/models-dev.json` | Cache location |
| `OXICODE_CATALOG_SNAPSHOT` | (unset) | Build-time: inject specific snapshot (CI/release) |

## Attribution

Model catalog data © [models.dev](https://models.dev) (MIT License).
See <https://github.com/sst/models.dev>.

The `product-meta.toml` file contains oxicode-specific product metadata
(HTTP-Referer headers, etc.) that models.dev does not provide.
