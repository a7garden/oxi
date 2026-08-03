# Catalog tooling

Scripts for managing `oxicode-ai/data/catalog/` — the 3-tier hybrid catalog.

## `port-openclaw.py`

Idempotent. Port openclaw's static `modelCatalog` from
`/tmp/openclaw-upstream/extensions/` into oxicode's `data/catalog/openclaw/`.

```bash
# Edit the OPENCLAW_EXT and OXICODE_* constants in the script for your paths.
python3 scripts/catalog/port-openclaw.py
```

Updates existing oxicode files (anthropic.toml, mistral.toml, etc.) with
new model IDs from openclaw, never overwriting oxicode-curated values.

## `backfill-prices.py`

Idempotent. Backfill `cost_input`/`cost_output` for openclaw models
where upstream shipped `0.0` and we have an official vendor API key
(venice, novita). Reads vendor APIs, updates `data/catalog/openclaw/*.toml`.

```bash
python3 scripts/catalog/backfill-prices.py
```

## `add-license-headers.py`

Idempotent. Add MIT/source-attribution header to every openclaw TOML
file in `data/catalog/openclaw/`.

```bash
python3 scripts/catalog/add-license-headers.py
```

## `convert-models.py`

Idempotent. Convert the legacy `oxicode-ai/src/model_db.rs` static arrays
into per-provider TOML files. Used once during the Layer 1 migration.
Kept for reference; the resulting TOML files are already committed.

```bash
python3 scripts/catalog/convert-models.py
```

## Round-trip workflow

```text
        ┌──────────────────────────────────────────┐
        │  Layer 1: data/catalog/*.toml            │
        │  (committed, source of truth)            │
        └──────────────────────────────────────────┘
                    ↑                     ↑
       port-openclaw.py          backfill-prices.py
                    │                     │
        ┌──────────────────────────────────────────┐
        │  openclaw upstream                       │
        │  /tmp/openclaw-upstream/extensions/      │
        └──────────────────────────────────────────┘
                    ↓
        vendor APIs (venice, novita)
```

Layer 2 (`~/.oxicode/catalog/overrides.toml`) and Layer 3 (runtime
discovery) are runtime-only and don't need scripts.
