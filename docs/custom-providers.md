# Provider selection in oxicode

oxicode ships **eight built-in providers** (`openai`, `openai-responses`,
`anthropic`, `google`, `vertex`, `azure`, `bedrock`, `ollama`) and
allows new OpenAI-compatible providers to be registered either through
the SDK's `ProviderRegistry::register` API or via the
`oxicode config add-provider` CLI command. The implementation is
extensible; the **selection** in normal operation is profile-driven.

## Resolution order

When the agent starts, oxicode resolves which provider + model to use
in this order:

1. **Environment override** — `OXICODE_PROVIDER` and `OXICODE_MODEL`
   (or `--provider` / `--model`). When both are set, the active agent
   uses them directly. This is the only override that does not require
   a Foundation installation.
2. **Explicit profile** — `--profile <id>` or `OXICODE_PROFILE=<id>`.
   oxicode reads the matching record from `~/.oxi/foundation/v1/profiles.json`
   and resolves its Keychain credential before the agent starts.
3. **Role-compatible profile** — when a role is requested
   (`coding.primary`, `assistant.general`, …), oxicode picks the
   first profile whose `roles` list contains the requested role.
   Multiple matches fail with a typed error.
4. **Compatibility import** — one-time legacy import, gated by
   `OXICODE_FOUNDATION_MIGRATION=1`. Reads a single legacy profile
   from the host's compatibility shim and writes a structured migration
   marker. Disabled by default.

A failed profile does not silently select a different remote provider;
the agent engine refuses to start with a typed error.

## Credentials

Profile credentials are **Keychain locators** — a `{ service, account }`
pair that oxicode resolves on demand. The OS Keychain is the only
durable credential store. No profile, environment dump, log, or
diagnostic carries a secret value.

```
{ "credential": { "service": "dev.oxi.foundation", "account": "personal-coding" } }
```

oxicode reads the value through `foundation::credentials::KeychainAuthProvider`,
which returns a typed `Unavailable` / `Locked` / `NotFound` error when
the locator cannot be resolved. Credentials are never exposed through
`Debug` or `Display`.

### Legacy import

A one-time importer at `foundation::credentials::legacy_import` moves a
plaintext `~/.oxicode/auth.json` API key into the Keychain. The
importer:

1. Requires explicit acknowledgement before reading the legacy file.
2. Writes the Keychain record (the only durable owner after import).
3. Writes a structured migration marker to the profile.
4. Optionally archives the legacy file outside the active credential
   path. The archive is **never automatic** — the user is asked.

The importer is the only code path that reads `~/.oxicode/auth.json`
under the Foundation host.

## Custom (OpenAI-compatible) providers

`oxicode config add-provider` registers an OpenAI-compatible endpoint
into the `ProviderRegistry`. The settings file is non-secret; only the
**environment variable name** pointing at the API key is recorded.

```toml
[[custom_provider]]
name = "minimax"
base_url = "https://api.minimax.chat/v1"
api_key_env = "MINIMAX_API_KEY"
api = "openai_completions"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Unique identifier, used as `name/model` in commands |
| `base_url` | yes | OpenAI-compatible endpoint, including `/v1` |
| `api_key_env` | yes | Environment variable name holding the API key |
| `api` | no | `openai_completions` (default) or `openai_responses` |

The API key is read from the environment, never from a plaintext
`~/.oxicode/auth.json` under the Foundation host. A custom provider
without an environment variable fails fast at agent start.

## Selection examples

```bash
# Explicit env override (no Foundation needed)
export OXICODE_PROVIDER=anthropic
export OXICODE_MODEL=claude-sonnet-4-20250514
oxicode

# Explicit profile id
oxicode --profile personal-coding

# Role-compatible (Foundation auto-select)
oxicode --request-role coding.primary

# Custom OpenAI-compatible endpoint
export MINIMAX_API_KEY="sk-..."
oxicode -m minimax/MiniMax-M1 "Hello"
```

## Compatibility matrix

| Host | Owns |
|---|---|
| **oxibrain** | durable memory |
| **oxicode** | code execution, workspace policy, tool invocation |
| **oxios** | orchestration, experience (embeds `oxicode-sdk`) |

oxios does not spawn the oxicode CLI for normal operation; it embeds
`oxicode-sdk` directly. The custom provider API supports both
embedding models.

## References

- [Oxi Foundation v1 contract](superpowers/specs/2026-08-17-oxi-foundation-contract.md)
- [oxicode-cli/ARCHITECTURE.md § Oxi Foundation host](../oxicode-cli/ARCHITECTURE.md)
- [oxicode-ai provider registry](../oxicode-ai/src/providers/register_builtins.rs)
