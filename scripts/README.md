# Model DB Generator

Generates `oxi-ai/src/model_db.rs` from `models.json`.

## Usage

Compile and run the generator, piping output to the target file:

```bash
# From the workspace root
cargo run --manifest-path scripts/Cargo.toml --bin generate-models < scripts/models.json > oxi-ai/src/model_db.rs
```

Or run from the `scripts/` directory:

```bash
cd scripts
cargo run --bin generate-models < models.json > ../oxi-ai/src/model_db.rs
```

## Adding models

1. Edit `scripts/models.json` — add models under the appropriate provider, or add a new provider block.
2. Regenerate: `cargo run --manifest-path scripts/Cargo.toml --bin generate-models < scripts/models.json > oxi-ai/src/model_db.rs`
3. Verify: `cargo build -p oxi-ai`

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
