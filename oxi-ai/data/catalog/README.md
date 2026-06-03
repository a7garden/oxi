# oxi catalog (3-tier hybrid)

oxi ships with a catalog of LLM providers and models. The catalog is
organized as **three layers**, each fully overridable by the next:

| Layer | Source | Mutable? | Offline? |
|-------|--------|----------|----------|
| **Layer 1** — built-in | `data/catalog/*.toml` (compiled into the binary) | No (read-only) | Yes |
| **Layer 2** — user override | `OXI_CATALOG_OVERRIDE`, `~/.oxi/catalog/overrides.toml`, `.oxi/catalog.local.toml` | Yes (user-owned) | Yes |
| **Layer 3** — runtime discovery | `GET {base_url}/v1/models` for ollama/lmstudio/vllm/sglang | Yes (transient) | No (network) |

## Layer 1: built-in catalog

This is what ships in the binary. The TOML is parsed at compile time
(`include_str!`) so there's zero runtime IO.

```
data/catalog/
  providers.toml             # 71 provider metadata entries
  models/
    anthropic.toml           # 1 file per provider (~30 files, 957 models)
    openai.toml
    ...
  openclaw/                  # ported from openclaw (MIT), see file headers
    chutes.toml
    venice.toml
    ...
```

To **add a new built-in provider**:

1. Create `data/catalog/models/<provider>.toml` (no Rust changes needed —
   `build.rs` will pick it up).
2. Add the provider metadata to `data/catalog/providers.toml`.
3. `cargo build -p oxi-ai` to verify.

The schema is:

```toml
provider = "<canonical_id>"

[[model]]
id = "<model_id>"
name = "<Display Name>"
api = "<api_string>"             # see API table below
provider = "<canonical_id>"
reasoning = <bool>
input = ["text", "image"]        # only text/image supported today
cost_input = 0.0                 # USD per million tokens
cost_output = 0.0
cost_cache_read = 0.0
cost_cache_write = 0.0
context_window = 200000          # tokens
max_tokens = 8192                # max output tokens
```

Supported `api` strings:

| `api` | Used by |
|-------|---------|
| `openai-completions` | openai, groq, deepseek, together, fireworks, cerebras, xai, github-copilot, chutes, ... |
| `openai-responses` | openai-codex |
| `anthropic-messages` | anthropic, anthropic-vertex |
| `google-generative-ai` | google |
| `google-vertex` | google-vertex |
| `mistral-conversations` | mistral |
| `azure-openai-responses` | azure-openai-responses |
| `bedrock-converse-stream` | amazon-bedrock |

## Layer 2: user override

Three locations, in increasing priority:

1. `OXI_CATALOG_OVERRIDE` env var (path to a TOML file)
2. `~/.oxi/catalog/overrides.toml` (global)
3. `.oxi/catalog.local.toml` (project-local, relative to cwd)

Use cases:

- **Custom pricing** for negotiated enterprise rates
- **Internal AI gateway** that's not in the built-in catalog
- **Local-only model** that the open-source community hasn't added yet
- **Hiding** a built-in model you don't want available

### Example: `~/.oxi/catalog/overrides.toml`

```toml
# Custom pricing for Anthropic (your negotiated rate)
[[provider]]
id = "anthropic"
display_name = "Anthropic (Enterprise)"
api = "anthropic-messages"
env_key = "ANTHROPIC_API_KEY"
auth_method = "x-api-key"
category = "primary"
description = "Anthropic Claude models (negotiated rate)"

# Add a new provider that doesn't exist in the built-in catalog
[[provider]]
id = "my-company-gateway"
display_name = "My Company AI Gateway"
api = "openai-completions"
env_key = "MY_GATEWAY_API_KEY"
auth_method = "bearer"
category = "enterprise"
description = "Internal AI gateway for company use"
base_url = "https://gateway.example.com/v1"
aliases = ["my-gw"]

# Add new models to existing providers
[[model]]
id = "claude-haiku-4-5-custom"
name = "Claude Haiku 4.5 (custom pricing)"
api = "anthropic-messages"
provider = "anthropic"
reasoning = true
input = ["text", "image"]
cost_input = 0.5       # your negotiated rate, not vendor's
cost_output = 2.0
cost_cache_read = 0.05
cost_cache_write = 0.5
context_window = 200000
max_tokens = 8192
```

### Merge semantics

| Scenario | Behavior |
|----------|----------|
| Override `provider.id` matches built-in | **Replaces** entire entry (not field-level merge) |
| Override `provider.id` is new | **Appends** to provider list |
| Override `(model.provider, model.id)` matches built-in | **Replaces** entire model entry |
| Override `(model.provider, model.id)` is new | **Appends** to that provider's model list |

### Failure handling

Files that fail to parse or have wrong types are **silently skipped** with a
warning log. Set `OXI_CATALOG_DEBUG=1` for verbose output.

## Layer 3: runtime discovery

For providers that expose an OpenAI-compatible `/v1/models` endpoint, oxi
can fetch them at startup. The default targets are:

| Provider | URL | Why |
|----------|-----|-----|
| ollama | `http://localhost:11434/v1` | Local model server |
| lmstudio | `http://localhost:1234/v1` | Local model server |
| vllm | `http://localhost:8000/v1` | Self-hosted LLM |
| sglang | `http://localhost:30000/v1` | Self-hosted LLM |

Each endpoint is queried in parallel with a 5-second timeout. Failures
are silent — if Ollama isn't running, you just don't have its models.

Runtime-discovered models have:
- `provider` = the local server's id (e.g. `ollama`)
- `cost_input/output` = 0.0 (local = free)
- `context_window` = 0 (unknown)
- `reasoning` = false (unknown)
- `input` = `["text"]` (default)

These limitations are why **Layer 1 is the source of truth** when possible.
Layer 3 is for *discovery*, not *accuracy*.

## TOML schema (full)

### `BuiltinProviderEntry`

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | yes | Canonical id, kebab-case (e.g. `amazon-bedrock`) |
| `display_name` | string | yes | Human-readable name |
| `api` | string | yes | One of the API table above |
| `env_key` | string | yes | Env var holding the API key |
| `auth_method` | string | yes | `bearer`, `x-api-key`, `oauth`, `query-param`, `none` |
| `category` | string | yes | `primary`, `secondary`, `enterprise`, `local` |
| `description` | string | yes | One-line description |
| `aliases` | string[] | no | Other names this provider is known by |
| `extra_env_keys` | string[] | no | Additional env vars (OAuth refresh, etc.) |
| `base_url` | string | no | Custom base URL (overrides default) |
| `extra_headers` | string[] | no | HTTP headers required for requests |
| `default_enabled` | bool | no | Whether the provider is on by default |

### `BuiltinModelEntry`

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | yes | Model id as known to the API |
| `name` | string | yes | Human-readable name |
| `api` | string | yes | API protocol (same table as provider) |
| `provider` | string | yes | Provider id (must match a `BuiltinProviderEntry.id`) |
| `reasoning` | bool | yes | Whether the model supports reasoning/thinking |
| `input` | string[] | yes | Supported input modalities: `text`, `image` |
| `cost_input` | float | yes | USD per million input tokens |
| `cost_output` | float | yes | USD per million output tokens |
| `cost_cache_read` | float | yes | USD per million cached read tokens |
| `cost_cache_write` | float | yes | USD per million cached write tokens |
| `context_window` | u32 | yes | Maximum context in tokens |
| `max_tokens` | u32 | yes | Maximum output in tokens |

## Upstream sync

oxi's catalog is informed by two upstream projects, both reviewed for
license compatibility and data quality:

| Upstream | License | Status | What we took |
|----------|---------|--------|--------------|
| **openclaw** (openclaw/openclaw) | MIT | ✅ 13 files in `data/catalog/openclaw/` | Static `modelCatalog` from 13 extension manifests |
| **opencode** (sst/opencode) | MIT | ℹ️ Reviewed, no data to port | They have NO static model metadata — model ids are just branded strings. Their value is in protocol routing, not catalog data. |

### Why opencode contributes no files

opencode's `packages/llm/src/schema/ids.ts` defines `ModelID` as a plain
branded string. There is no `models.ts`, no `MODELS` array, no
`inputCost`/`outputCost`/`contextWindow` fields anywhere in the LLM
package. Pricing is handled at request time by the protocol layer.

What opencode DOES have is the *protocol variants* concept
(`openai-compatible-profile.ts` lists 9 OpenAI-compatible profiles like
`openai-chat`, `anthropic-messages`, `gemini`, etc.). This is **code**,
not data, so it doesn't port into the catalog — but it's worth studying
for future protocol work in oxi.

## Price data quality

The `cost_input` / `cost_output` fields are USD per **million** tokens,
sourced from official vendor documentation. **Not all fields are
verified** — see the breakdown:

| Source | Models | Prices verified | Method |
|--------|--------|-----------------|--------|
| oxi-original (`models/*.toml`) | 957 | ✅ All (hand-curated by oxi team) | Manual research |
| openclaw port (`openclaw/*.toml`) — venice | 38 | ✅ 30/38 (live API `https://api.venice.ai/api/v1/models`) | Verified 2026-Q2 |
| openclaw port (`openclaw/*.toml`) — novita | 6 | ✅ 6/6 (live API `https://api.novita.ai/v3/openai/models`) | Verified 2026-Q2 |
| openclaw port (`openclaw/*.toml`) — other 11 | 121 | ⚠️ 0 verified, all `0.0` from openclaw source | **Needs user-supplied pricing** |

For the 11 unverified providers (`gmi`, `kilocode`, `moonshot`,
`nvidia`, `ollama-cloud`, `qianfan`, `qwen`, `stepfun`, `byteplus`,
`chutes`, `deepinfra`), the openclaw upstream itself shipped zero
prices. We preserve that fidelity. To override:

```toml
# ~/.oxi/catalog/overrides.toml

[[model]]
id = "deepseek-v3.2"
provider = "gmi"
cost_input = 0.5     # your verified rate
cost_output = 2.0
```

## Testing

## Testing

```bash
cargo test -p oxi-ai --lib catalog
```

Coverage:

- `catalog_loads` — basic parse
- `all_providers_have_unique_ids` — no duplicate ids
- `all_provider_aliases_resolve` — every alias maps to a real provider
- `find_provider_by_id_and_alias` — id and alias lookup both work
- `all_providers_have_valid_auth_method` — enum value is in the allowed set
- `all_providers_have_non_empty_env_key` — env_key is set
- `openai_compatible_providers_use_bearer` — OpenAI-compatible providers use `bearer` auth
- `models_index_loads_all_providers` — every TOML file is in the index
- `all_loaded_models_round_trip` — every model parses
- `parse_minimal_override` — Layer 2 TOML parses
- `apply_provider_override_replaces` / `apply_model_override_appends_new` — Layer 2 merge
- `discover_empty_url_returns_empty` / `discover_unreachable_returns_empty` — Layer 3 robustness
