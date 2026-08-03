# Model DB Tooling

Tools for managing `oxicode-ai/src/model_db.rs` — a static database of 544 models across 28 providers.

## Round-trip workflow

```
model_db.rs ──extract──→ models.json ──generate──→ model_db.rs
```

Both directions are lossless. All 544 models, per-model API overrides, and provider metadata are preserved.

## Extract (Rust → JSON)

One-time migration or sync from hand-edited `model_db.rs`:

```bash
python3 scripts/extract-models.py
# Output: scripts/models.json (544 models, 28 providers)
```

## Generate (JSON → Rust)

Regenerate `model_db.rs` from JSON:

```bash
cargo run --manifest-path scripts/Cargo.toml --bin generate-models < scripts/models.json > oxicode-ai/src/model_db.rs
# Verify:
cargo check -p oxicode-ai
```

## Adding models

1. Edit `scripts/models.json` — add models under the appropriate provider, or add a new provider block.
2. Regenerate: `cargo run --manifest-path scripts/Cargo.toml --bin generate-models < scripts/models.json > oxicode-ai/src/model_db.rs`
3. Verify: `cargo check -p oxicode-ai && cargo test -p oxicode-ai`

## JSON schema

```json
{
  "providers": [
    {
      "name": "provider-name",
      "api": "api-variant",
      "models": [
        {
          "id": "model-id",
          "name": "Human-readable name",
          "api": "optional-per-model-override",
          "reasoning": false,
          "input": ["text", "image"],
          "cost_input": 3.0,
          "cost_output": 15.0,
          "cost_cache_read": 0.3,
          "cost_cache_write": 3.75,
          "context_window": 200000,
          "max_tokens": 8192
        }
      ]
    }
  ]
}
```

### Per-model API overrides

When a provider hosts models with different APIs (e.g., GitHub Copilot serves both OpenAI and Anthropic models), add an `"api"` field to the individual model:

```json
{
  "name": "github-copilot",
  "api": "openai-responses",
  "models": [
    { "id": "gpt-4o", "name": "GPT-4o", ... },
    { "id": "claude-sonnet-4", "name": "Claude Sonnet 4", "api": "anthropic-messages", ... }
  ]
}
```

### API variants

| JSON value                | Rust enum variant          |
|---------------------------|----------------------------|
| `openai-completions`      | `Api::OpenAiCompletions`   |
| `openai-responses`        | `Api::OpenAiResponses`     |
| `anthropic-messages`      | `Api::AnthropicMessages`   |
| `google-generative-ai`    | `Api::GoogleGenerativeAi`  |
| `google-vertex`           | `Api::GoogleVertex`        |
| `mistral-conversations`   | `Api::MistralConversations`|
| `azure-openai-responses`  | `Api::AzureOpenAiResponses`|
| `bedrock-converse-stream` | `Api::BedrockConverseStream`|

### Input modalities

| JSON value | Rust enum variant      |
|------------|------------------------|
| `text`     | `InputModality::Text`  |
| `image`    | `InputModality::Image` |

## Notes

- This is a standalone code generation tool — it is **not** part of the workspace.
- The generated `model_db.rs` is committed to the repo. Regenerate only when model data changes.
- All costs are in USD per million tokens.
- `extract-models.py` requires Python 3.6+ (no external dependencies).
