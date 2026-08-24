//! Model registry for oxicode-ai
//!
//! Provides a centralized registry of available LLM models.
//! Supports both static built-in models and dynamic runtime registration
//! for custom OpenAI-compatible providers.

use crate::{Api, CompatSettings, Cost, InputModality, MaxTokensField, Model, ThinkingFormat};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Extract the model name after the last '/', or return the whole id if no '/' is present.
fn extract_model_name(id: &str) -> &str {
    id.rsplit_once('/').map(|(_, name)| name).unwrap_or(id)
}

/// Return provider-specific compatibility defaults.
///
/// Internal helper used by `add_*_models()` functions so that every model
/// from the same provider gets the same `compat` baseline.
fn default_compat_for_provider(provider: &str) -> Option<CompatSettings> {
    match provider {
        "openai" | "openai-responses" | "openai-completions" => Some(CompatSettings {
            thinking_format: Some(ThinkingFormat::OpenAI),
            max_tokens_field: Some(MaxTokensField::MaxCompletionTokens),
            ..CompatSettings::default()
        }),
        "openrouter" => Some(CompatSettings {
            thinking_format: Some(ThinkingFormat::OpenRouter),
            requires_tool_result_name: true,
            ..CompatSettings::default()
        }),
        "deepseek" => Some(CompatSettings {
            thinking_format: Some(ThinkingFormat::DeepSeek),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            ..CompatSettings::default()
        }),
        "zai" => Some(CompatSettings {
            thinking_format: Some(ThinkingFormat::Zai),
            ..CompatSettings::default()
        }),
        // azure-openai already has explicit CompatSettings in add_azure_models()
        // All other providers: use defaults (return None)
        _ => None,
    }
}

/// Global model registry (static built-in models)
static STATIC_MODELS: LazyLock<HashMap<String, Model>> = LazyLock::new(|| {
    let mut map = HashMap::new();

    // OpenAI models
    add_openai_models(&mut map);

    // Anthropic models
    add_anthropic_models(&mut map);

    // Google models
    add_google_models(&mut map);

    // DeepSeek models
    add_deepseek_models(&mut map);

    // Mistral models
    add_mistral_models(&mut map);

    // Groq models
    add_groq_models(&mut map);

    // Cerebras models
    add_cerebras_models(&mut map);

    // xAI models
    add_xai_models(&mut map);

    // OpenRouter models
    add_openrouter_models(&mut map);

    // Azure OpenAI models
    add_azure_models(&mut map);

    // ZAI models
    add_zai_models(&mut map);
    // MiniMax models
    add_minimax_models(&mut map);

    // models.dev is the source of truth for numeric metadata. The static
    // registry exists to carry hand-maintained transport quirks (compat
    // settings, base URLs) — its hand-written numbers drift (e.g.
    // gemini-2.5-pro listed at 2M while models.dev reports 1_048_576).
    // Resolution prefers static over catalog, so refresh the numbers here
    // or the stale hand values would shadow the catalog forever.
    refresh_numerics_from_catalog(&mut map);

    map
});

/// Overwrite hand-maintained numeric metadata (context window, max output
/// tokens, costs) with models.dev catalog values for every static entry
/// the catalog knows. See the call site for why this runs at init.
fn refresh_numerics_from_catalog(map: &mut HashMap<String, Model>) {
    for (key, model) in map.iter_mut() {
        let Some((provider, id)) = key.split_once('/') else {
            continue;
        };
        let Some(entry) = crate::model_db::get_model_entry(provider, id) else {
            continue;
        };
        if entry.context_window > 0 {
            model.context_window = entry.context_window as usize;
        }
        if entry.max_tokens > 0 {
            model.max_tokens = entry.max_tokens as usize;
        }
        if entry.cost_input > 0.0 {
            model.cost.input = entry.cost_input;
        }
        if entry.cost_output > 0.0 {
            model.cost.output = entry.cost_output;
        }
        if entry.cost_cache_read > 0.0 {
            model.cost.cache_read = entry.cost_cache_read;
        }
        if entry.cost_cache_write > 0.0 {
            model.cost.cache_write = entry.cost_cache_write;
        }
    }
}

fn add_openai_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("openai/gpt-4o", "GPT-4o", true, 2.5, 10.0),
        ("openai/gpt-4o-mini", "GPT-4o Mini", true, 0.15, 0.60),
        ("openai/gpt-4-turbo", "GPT-4 Turbo", true, 10.0, 30.0),
        ("openai/gpt-4", "GPT-4", false, 30.0, 60.0),
        ("openai/gpt-3.5-turbo", "GPT-3.5 Turbo", false, 0.5, 1.5),
        ("openai/o1-preview", "OpenAI o1 Preview", true, 15.0, 60.0),
        ("openai/o1-mini", "OpenAI o1 Mini", true, 15.0, 60.0),
        ("openai/o1", "OpenAI o1", true, 15.0, 60.0),
        ("openai/o3", "OpenAI o3", true, 15.0, 60.0),
        ("openai/o3-mini", "OpenAI o3 Mini", true, 15.0, 60.0),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "openai".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                reasoning,
                input: if reasoning {
                    vec![InputModality::Text]
                } else {
                    vec![InputModality::Text, InputModality::Image]
                },
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: input_cost * 0.5,
                    cache_write: input_cost * 7.5,
                },
                context_window: 128_000,
                max_tokens: 32_000,
                headers: Default::default(),
                compat: default_compat_for_provider("openai"),
            },
        );
    }
}

fn add_anthropic_models(map: &mut HashMap<String, Model>) {
    let models = [
        (
            "anthropic/claude-sonnet-4-20250514",
            "Claude Sonnet 4",
            true,
            3.0,
            15.0,
        ),
        (
            "anthropic/claude-opus-4-20250514",
            "Claude Opus 4",
            true,
            15.0,
            75.0,
        ),
        (
            "anthropic/claude-3-5-sonnet-20241022",
            "Claude 3.5 Sonnet",
            true,
            3.0,
            15.0,
        ),
        (
            "anthropic/claude-3-5-haiku-20241022",
            "Claude 3.5 Haiku",
            false,
            0.8,
            4.0,
        ),
        (
            "anthropic/claude-3-opus",
            "Claude 3 Opus",
            false,
            15.0,
            75.0,
        ),
        (
            "anthropic/claude-3-sonnet",
            "Claude 3 Sonnet",
            false,
            3.0,
            15.0,
        ),
        (
            "anthropic/claude-3-haiku",
            "Claude 3 Haiku",
            false,
            0.25,
            1.25,
        ),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::AnthropicMessages,
                provider: "anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                reasoning,
                input: vec![InputModality::Text, InputModality::Image],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: input_cost * 0.1,
                    cache_write: input_cost * 1.25,
                },
                context_window: 200_000,
                max_tokens: 8192,
                headers: Default::default(),
                compat: default_compat_for_provider("anthropic"),
            },
        );
    }
}

fn add_google_models(map: &mut HashMap<String, Model>) {
    let models = [
        (
            "google/gemini-2.0-flash",
            "Gemini 2.0 Flash",
            0.0,
            0.0,
            1_000_000,
        ),
        (
            "google/gemini-2.5-flash",
            "Gemini 2.5 Flash",
            0.0,
            0.0,
            1_000_000,
        ),
        (
            "google/gemini-2.5-pro",
            "Gemini 2.5 Pro",
            1.25,
            5.0,
            2_000_000,
        ),
        (
            "google/gemini-1.5-flash",
            "Gemini 1.5 Flash",
            0.0,
            0.0,
            1_000_000,
        ),
        (
            "google/gemini-1.5-pro",
            "Gemini 1.5 Pro",
            1.25,
            5.0,
            2_000_000,
        ),
        ("google/gemini-pro", "Gemini Pro", 0.125, 0.5, 32_000),
    ];

    for (id, name, input_cost, output_cost, ctx) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::GoogleGenerativeAi,
                provider: "google".to_string(),
                base_url: "https://generativelanguage.googleapis.com".to_string(),
                reasoning: false,
                input: vec![InputModality::Text, InputModality::Image],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: ctx,
                max_tokens: 8192,
                headers: Default::default(),
                compat: default_compat_for_provider("google"),
            },
        );
    }
}

fn add_deepseek_models(map: &mut HashMap<String, Model>) {
    // Legacy models (to be retired 2026-07-24)
    let legacy_models = [
        (
            "deepseek/deepseek-chat",
            "DeepSeek Chat",
            false,
            0.27,
            1.1,
            64_000,
            8192,
        ),
        (
            "deepseek/deepseek-chat-v3",
            "DeepSeek Chat V3",
            false,
            0.27,
            1.1,
            64_000,
            8192,
        ),
        (
            "deepseek/deepseek-reasoner",
            "DeepSeek Reasoner",
            true,
            0.55,
            2.19,
            64_000,
            8192,
        ),
        (
            "deepseek/deepseek-coder",
            "DeepSeek Coder",
            false,
            0.27,
            1.1,
            64_000,
            8192,
        ),
    ];

    for (id, name, reasoning, input_cost, output_cost, ctx, max_out) in legacy_models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "deepseek".to_string(),
                base_url: "https://api.deepseek.com".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.1,
                    cache_write: 1.0,
                },
                context_window: ctx,
                max_tokens: max_out,
                headers: Default::default(),
                compat: default_compat_for_provider("deepseek"),
            },
        );
    }

    // V4 models (released 2026-04-24)
    let v4_models = [
        // deepseek-v4-flash: 284B total / 13B active, $0.14/M input, $0.28/M output
        (
            "deepseek/deepseek-v4-flash",
            "DeepSeek V4 Flash",
            true,
            0.14,
            0.28,
            1_000_000,
            384_000,
        ),
        // deepseek-v4-pro: 1.6T total / 49B active, $0.435/M input, $0.87/M output
        (
            "deepseek/deepseek-v4-pro",
            "DeepSeek V4 Pro",
            true,
            0.435,
            0.87,
            1_000_000,
            384_000,
        ),
    ];

    for (id, name, reasoning, input_cost, output_cost, ctx, max_out) in v4_models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "deepseek".to_string(),
                base_url: "https://api.deepseek.com".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    // V4 cache pricing: flash $0.0028, pro $0.003625 per 1M tokens
                    cache_read: if input_cost < 0.2 { 0.0028 } else { 0.003625 },
                    cache_write: 0.0, // DeepSeek does not charge extra for cache writes
                },
                context_window: ctx,
                max_tokens: max_out,
                headers: Default::default(),
                compat: default_compat_for_provider("deepseek"),
            },
        );
    }
}

fn add_mistral_models(map: &mut HashMap<String, Model>) {
    let models = [
        (
            "mistral/mistral-large-latest",
            "Mistral Large",
            false,
            2.0,
            6.0,
        ),
        (
            "mistral/mistral-medium-latest",
            "Mistral Medium",
            false,
            0.5,
            1.5,
        ),
        (
            "mistral/mistral-small-latest",
            "Mistral Small",
            false,
            0.2,
            0.6,
        ),
        ("mistral/mistral-nemo", "Mistral Nemo", false, 0.15, 0.15),
        ("mistral/codestral", "Codestral", false, 0.3, 0.9),
        (
            "mistral/codestral-mamba",
            "Codestral Mamba",
            false,
            0.25,
            0.25,
        ),
        (
            "mistral/open-mixtral-8x22b",
            "Mixtral 8x22B",
            false,
            0.45,
            1.4,
        ),
        (
            "mistral/open-mixtral-8x7b",
            "Mixtral 8x7B",
            false,
            0.24,
            0.24,
        ),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "mistral".to_string(),
                base_url: "https://api.mistral.ai".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 128_000,
                max_tokens: 32_000,
                headers: Default::default(),
                compat: default_compat_for_provider("mistral"),
            },
        );
    }
}

fn add_groq_models(map: &mut HashMap<String, Model>) {
    let models = [
        (
            "groq/llama-3.3-70b-versatile",
            "Llama 3.3 70B Versatile",
            false,
            0.0,
            0.0,
        ),
        (
            "groq/llama-3.1-70b-versatile",
            "Llama 3.1 70B Versatile",
            false,
            0.0,
            0.0,
        ),
        (
            "groq/llama-3.1-8b-instant",
            "Llama 3.1 8B Instant",
            false,
            0.0,
            0.0,
        ),
        (
            "groq/llama-3-70b-versatile",
            "Llama 3 70B Versatile",
            false,
            0.0,
            0.0,
        ),
        (
            "groq/llama-3-8b-versatile",
            "Llama 3 8B Versatile",
            false,
            0.0,
            0.0,
        ),
        ("groq/mixtral-8x7b-32768", "Mixtral 8x7B", false, 0.0, 0.0),
        ("groq/gemma2-9b-it", "Gemma 2 9B", false, 0.0, 0.0),
        ("groq/gemma-7b-it", "Gemma 7B", false, 0.0, 0.0),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "groq".to_string(),
                base_url: "https://api.groq.com/openai/v1".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 128_000,
                max_tokens: 8192,
                headers: Default::default(),
                compat: default_compat_for_provider("groq"),
            },
        );
    }
}

fn add_cerebras_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("cerebras/llama-3.3-70b", "Llama 3.3 70B", false, 0.0, 0.0),
        ("cerebras/llama-3.1-8b", "Llama 3.1 8B", false, 0.0, 0.0),
        ("cerebras/qwen-2.5-32b", "Qwen 2.5 32B", false, 0.0, 0.0),
        ("cerebras/qwen-2.5-7b", "Qwen 2.5 7B", false, 0.0, 0.0),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "cerebras".to_string(),
                base_url: "https://api.cerebras.ai".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 128_000,
                max_tokens: 8192,
                headers: Default::default(),
                compat: default_compat_for_provider("cerebras"),
            },
        );
    }
}

fn add_xai_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("xai/grok-2", "Grok 2", false, 5.0, 15.0),
        ("xai/grok-2-mini", "Grok 2 Mini", false, 0.3, 0.5),
        ("xai/grok-1", "Grok 1", false, 5.0, 15.0),
        ("xai/grok-1.5", "Grok 1.5", false, 5.0, 15.0),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "xai".to_string(),
                base_url: "https://api.x.ai/v1".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 131_072,
                max_tokens: 8192,
                headers: Default::default(),
                compat: default_compat_for_provider("xai"),
            },
        );
    }
}

fn add_openrouter_models(map: &mut HashMap<String, Model>) {
    let models = [
        (
            "openrouter/anthropic/claude-3.5-sonnet",
            "Claude 3.5 Sonnet",
            false,
            3.0,
            15.0,
        ),
        (
            "openrouter/anthropic/claude-3-opus",
            "Claude 3 Opus",
            false,
            15.0,
            75.0,
        ),
        (
            "openrouter/google/gemini-pro-1.5",
            "Gemini Pro 1.5",
            false,
            1.25,
            5.0,
        ),
        (
            "openrouter/meta-llama/llama-3-70b",
            "Llama 3 70B",
            false,
            0.65,
            2.75,
        ),
        (
            "openrouter/meta-llama/llama-3-8b",
            "Llama 3 8B",
            false,
            0.2,
            0.2,
        ),
        (
            "openrouter/mistralai/mistral-large",
            "Mistral Large",
            false,
            2.0,
            6.0,
        ),
        (
            "openrouter/deepseek/deepseek-chat",
            "DeepSeek Chat",
            false,
            0.27,
            1.1,
        ),
        ("openrouter/qwen/qwen-2-72b", "Qwen 2 72B", false, 0.9, 0.9),
        (
            "openrouter/nousresearch/hermes-3-llama-3-70b",
            "Hermes 3 70B",
            false,
            0.5,
            1.5,
        ),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "openrouter".to_string(),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 128_000,
                max_tokens: 32_000,
                headers: [
                    ("HTTP-Referer".to_string(), "https://oxicode-ai".to_string()),
                    ("X-Title".to_string(), "oxicode-ai".to_string()),
                ]
                .into_iter()
                .collect(),
                compat: default_compat_for_provider("openrouter"),
            },
        );
    }
}

fn add_azure_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("azure-openai/gpt-4o", "GPT-4o", false, 2.5, 10.0),
        ("azure-openai/gpt-4o-mini", "GPT-4o Mini", false, 0.15, 0.60),
        ("azure-openai/gpt-4-turbo", "GPT-4 Turbo", false, 10.0, 30.0),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::AzureOpenAiResponses,
                provider: "azure-openai".to_string(),
                base_url: "https://{your-resource-name}.openai.azure.com".to_string(),
                reasoning,
                input: vec![InputModality::Text, InputModality::Image],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 128_000,
                max_tokens: 32_000,
                headers: Default::default(),
                compat: Some(crate::CompatSettings {
                    supports_store: false,
                    supports_developer_role: false,
                    supports_reasoning_effort: false,
                    supports_usage_in_streaming: false,
                    max_tokens_field: Some(crate::MaxTokensField::MaxCompletionTokens),
                    requires_tool_result_name: true,
                    requires_assistant_after_tool_result: false,
                    requires_thinking_as_text: false,
                    thinking_format: None,
                }),
            },
        );
    }
}

fn add_zai_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("zai/glm-4.7", "GLM-4.7", true, 0.0, 0.0),
        ("zai/glm-5-turbo", "GLM-5-Turbo", true, 0.0, 0.0),
        ("zai/glm-5.1", "GLM-5.1", true, 0.0, 0.0),
        ("zai/glm-5v-turbo", "GLM-5V-Turbo", true, 0.0, 0.0),
        ("zai/glm-4.5-air", "GLM-4.5-Air", true, 0.0, 0.0),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::OpenAiCompletions,
                provider: "zai".to_string(),
                base_url: "https://api.z.ai/api/coding/paas/v4".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                context_window: 200_000,
                max_tokens: 131_072,
                headers: Default::default(),
                compat: default_compat_for_provider("zai"),
            },
        );
    }
}

fn add_minimax_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("minimax/MiniMax-M2.7", "MiniMax-M2.7", true, 0.0, 0.0),
        (
            "minimax/MiniMax-M2.7-highspeed",
            "MiniMax-M2.7-highspeed",
            true,
            0.0,
            0.0,
        ),
    ];

    for (id, name, reasoning, input_cost, output_cost) in models {
        map.insert(
            id.to_string(),
            Model {
                id: extract_model_name(id).to_string(),
                name: name.to_string(),
                api: Api::AnthropicMessages,
                provider: "minimax".to_string(),
                base_url: "https://api.minimax.io".to_string(),
                reasoning,
                input: vec![InputModality::Text],
                cost: Cost {
                    input: input_cost,
                    output: output_cost,
                    cache_read: 0.06,
                    cache_write: 0.375,
                },
                context_window: 204_800,
                max_tokens: 131_072,
                headers: Default::default(),
                compat: default_compat_for_provider("minimax"),
            },
        );
    }
}

/// Lightweight model registry for SDK/engine usage.
///
/// Stores model metadata (provider, base_url, API type, costs) without
/// authentication details. For CLI usage with auth integration, see
/// `oxicode_store::CliModelRegistry`.
#[derive(Default)]
pub struct ModelRegistry {
    static_models: HashMap<String, Model>,
    dynamic_models: parking_lot::RwLock<HashMap<String, Model>>,
}

impl ModelRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            static_models: HashMap::new(),
            dynamic_models: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry pre-populated with all built-in static models.
    ///
    /// This loads models from the embedded static database.
    pub fn from_static() -> Self {
        Self {
            static_models: STATIC_MODELS.clone(),
            dynamic_models: RwLock::new(HashMap::new()),
        }
    }

    /// Register a model at runtime.
    ///
    /// If a model with the same `provider/model_id` key already exists,
    /// the new one replaces it.
    pub fn register(&self, model: Model) {
        let key = format!("{}/{}", model.provider, model.id);
        self.dynamic_models.write().insert(key, model);
    }

    /// Unregister a previously registered dynamic model.
    pub fn unregister(&self, provider: &str, model_id: &str) {
        let key = format!("{}/{}", provider, model_id);
        self.dynamic_models.write().remove(&key);
    }

    /// Look up a model by provider and model ID.
    ///
    /// Dynamic models take priority over static ones.
    pub fn lookup(&self, provider: &str, model_id: &str) -> Option<Model> {
        let key = format!("{}/{}", provider, model_id);
        // Dynamic models take priority
        if let Some(m) = self.dynamic_models.read().get(&key) {
            return Some(m.clone());
        }
        // Then static models
        self.static_models.get(&key).cloned()
    }

    /// Get a model by provider/model ID (static models only).
    pub fn get(provider: &str, model_id: &str) -> Option<&'static Model> {
        let key = format!("{}/{}", provider, model_id);
        STATIC_MODELS.get(&key)
    }

    /// Get all models from a provider (static only).
    pub fn get_by_provider(provider: &str) -> Vec<&'static Model> {
        STATIC_MODELS
            .values()
            .filter(|m| m.provider == provider)
            .collect()
    }

    /// Get all available models (static only).
    pub fn all() -> Vec<&'static Model> {
        STATIC_MODELS.values().collect()
    }

    /// Get all dynamically registered models.
    pub fn dynamic_models(&self) -> Vec<Model> {
        self.dynamic_models.read().values().cloned().collect()
    }

    /// Get all registered model IDs as `provider/model` strings.
    pub fn model_ids(&self) -> Vec<String> {
        let static_ids: Vec<String> = self.static_models.keys().cloned().collect();
        let dynamic_ids: Vec<String> = self.dynamic_models.read().keys().cloned().collect();
        static_ids.into_iter().chain(dynamic_ids).collect()
    }

    /// Search models by pattern (static only).
    pub fn search(pattern: &str) -> Vec<&'static Model> {
        let pattern_lower = pattern.to_lowercase();
        STATIC_MODELS
            .values()
            .filter(|m| {
                m.id.to_lowercase().contains(&pattern_lower)
                    || m.name.to_lowercase().contains(&pattern_lower)
            })
            .collect()
    }
}

// ── Global registry instance ────────────────────────────────────────

/// Global model registry instance (for convenience functions).
static GLOBAL_REGISTRY: LazyLock<ModelRegistry> = LazyLock::new(ModelRegistry::from_static);

// ── Convenience functions using global registry ─────────────────────

/// Register a model at runtime.
///
/// Call this during startup for each custom provider's model.
/// If a model with the same `provider/model_id` key already exists,
/// the new one replaces it.
pub fn register_model(model: Model) {
    GLOBAL_REGISTRY.register(model);
}

/// Unregister a previously registered dynamic model.
pub fn unregister_model(provider: &str, model_id: &str) {
    GLOBAL_REGISTRY.unregister(provider, model_id);
}

/// Look up a model by provider and model ID, checking both dynamic and static registries.
///
/// Dynamic models take priority over static ones.
pub fn lookup_model(provider: &str, model_id: &str) -> Option<Model> {
    GLOBAL_REGISTRY.lookup(provider, model_id)
}

/// Convenience function to get a model (static registry only – use [`lookup_model`] for dynamic too)
pub fn get_model(provider: &str, model_id: &str) -> Option<&'static Model> {
    ModelRegistry::get(provider, model_id)
}

/// Get all available providers
pub fn get_providers() -> Vec<&'static str> {
    let mut providers: Vec<&'static str> = STATIC_MODELS
        .values()
        .map(|m| m.provider.as_str())
        .collect();
    providers.sort();
    providers.dedup();
    providers
}

/// Get all models from a provider
pub fn get_models(provider: &str) -> Vec<&'static Model> {
    ModelRegistry::get_by_provider(provider)
}

/// Get all dynamically registered models.
pub fn dynamic_models() -> Vec<Model> {
    GLOBAL_REGISTRY.dynamic_models()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_model() {
        let model = get_model("openai", "gpt-4o");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider, "openai");
        // Note: gpt-4o has reasoning enabled
    }

    #[test]
    fn test_get_providers() {
        let providers = get_providers();
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"google"));
        assert!(providers.contains(&"deepseek"));
        assert!(providers.contains(&"mistral"));
        assert!(providers.contains(&"groq"));
    }

    #[test]
    fn test_deepseek_model() {
        let model = get_model("deepseek", "deepseek-chat");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider, "deepseek");
        assert_eq!(model.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn test_deepseek_v4_models() {
        let flash = get_model("deepseek", "deepseek-v4-flash");
        assert!(flash.is_some(), "deepseek-v4-flash should be registered");
        let flash = flash.unwrap();
        assert_eq!(flash.provider, "deepseek");
        assert_eq!(flash.context_window, 1_000_000);
        assert_eq!(flash.max_tokens, 384_000);
        assert!(flash.reasoning);

        let pro = get_model("deepseek", "deepseek-v4-pro");
        assert!(pro.is_some(), "deepseek-v4-pro should be registered");
        let pro = pro.unwrap();
        assert_eq!(pro.provider, "deepseek");
        assert_eq!(pro.context_window, 1_000_000);
        assert_eq!(pro.max_tokens, 384_000);
        assert!(pro.reasoning);
        // V4 Pro is more expensive than V4 Flash
        assert!(pro.cost.input > flash.cost.input);
    }

    #[test]
    fn test_search_models() {
        let results = ModelRegistry::search("gpt");
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|m| m.name.to_lowercase().contains("gpt"))
        );
    }

    #[test]
    fn test_model_registry_instance() {
        let registry = ModelRegistry::from_static();
        assert!(registry.lookup("openai", "gpt-4o").is_some());
        assert!(registry.lookup("fake", "fake-model").is_none());
    }

    #[test]
    fn static_registry_numerics_match_catalog() {
        // Every static entry the models.dev catalog knows must carry the
        // catalog's numeric metadata. Before `refresh_numerics_from_catalog`,
        // hand-written values (gemini-2.5-pro 2M, zai/glm-4.7 200k, …)
        // shadowed the catalog in `resolve_model_from_id` (static wins
        // over catalog) and drifted from reality.
        let mut checked = 0;
        for provider in get_providers() {
            for model in get_models(provider) {
                let Some(entry) = crate::model_db::get_model_entry(provider, &model.id) else {
                    continue; // catalog-only model, nothing to compare
                };
                if entry.context_window > 0 {
                    assert_eq!(
                        model.context_window, entry.context_window as usize,
                        "{provider}/{} context window must match models.dev",
                        model.id
                    );
                }
                if entry.max_tokens > 0 {
                    assert_eq!(
                        model.max_tokens, entry.max_tokens as usize,
                        "{provider}/{} max output tokens must match models.dev",
                        model.id
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 20, "expected a meaningful overlap, got {checked}");
    }

    #[test]
    fn gemini_2_5_pro_uses_models_dev_window() {
        // Concrete regression: the hand-written entry said 2_000_000 while
        // models.dev (and Google's own docs for the GA API) say 1_048_576.
        let m = get_model("google", "gemini-2.5-pro").expect("static gemini-2.5-pro");
        assert_eq!(m.context_window, 1_048_576);
    }

    #[test]
    fn test_model_registry_register_dynamic() {
        let registry = ModelRegistry::new();
        let custom_model = Model {
            id: "custom-model".to_string(),
            name: "Custom Model".to_string(),
            api: Api::OpenAiCompletions,
            provider: "custom".to_string(),
            base_url: "https://custom.example.com".to_string(),
            reasoning: false,
            input: vec![InputModality::Text],
            cost: Cost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.5,
                cache_write: 5.0,
            },
            context_window: 100_000,
            max_tokens: 8192,
            headers: Default::default(),
            compat: None,
        };
        registry.register(custom_model.clone());
        assert!(registry.lookup("custom", "custom-model").is_some());
    }
}
