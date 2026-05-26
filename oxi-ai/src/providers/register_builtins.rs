//! Built-in provider registration.
//!
//! Defines all built-in providers with comprehensive metadata: names, aliases,
//! API key environment variables, base URLs, auth methods, and provider-specific
//! headers. The provider factory is data-driven from this metadata rather than
//! using hardcoded match arms.

use crate::Api;

// ---------------------------------------------------------------------------
// Auth method
// ---------------------------------------------------------------------------

/// How a provider passes its API key in HTTP headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// `Authorization: Bearer <key>` — most OpenAI-compatible providers.
    Bearer,
    /// `x-api-key: <key>` — Anthropic and Anthropic-compatible providers.
    XApiKey,
    /// `api-key: <key>` — Azure OpenAI.
    ApiKey,
    /// No API key header (uses other auth like OAuth, SigV4).
    None,
}

// ---------------------------------------------------------------------------
// Provider metadata
// ---------------------------------------------------------------------------

/// Metadata for a built-in provider.
#[derive(Debug, Clone)]
pub struct BuiltinProvider {
    /// Primary provider name (e.g. "openai")
    pub name: &'static str,
    /// Display name (e.g. "OpenAI")
    pub display_name: &'static str,
    /// Alternative names that resolve to this provider
    pub aliases: &'static [&'static str],
    /// API type used by this provider
    pub api: Api,
    /// Environment variable(s) that may hold the API key (in priority order)
    pub env_key: &'static str,
    /// Additional environment variables to check
    pub extra_env_keys: &'static [&'static str],
    /// Default base URL for the API
    pub base_url: &'static str,
    /// Whether this provider is enabled by default
    pub default_enabled: bool,
    /// How to pass the API key
    pub auth_method: AuthMethod,
    /// Extra HTTP headers required by this provider
    pub extra_headers: &'static [(&'static str, &'static str)],
    /// Provider category for UI grouping ("primary", "chinese", "enterprise", "open",
    /// "coding", "cloud", "specialized")
    pub category: &'static str,
    /// Short human-readable description shown in the provider selection UI
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// Built-in provider definitions
// ---------------------------------------------------------------------------

/// All built-in providers, defined statically.
///
/// Base URLs follow opencode's profiles where available to ensure consistency
/// with well-tested endpoints.
static BUILTIN_PROVIDERS: &[BuiltinProvider] = &[
    // ── Primary providers ──────────────────────────────────────────────
    BuiltinProvider {
        name: "openai",
        display_name: "OpenAI",
        aliases: &["openai"],
        api: Api::OpenAiCompletions,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "primary",
        description: "GPT, o-series models",
    },
    BuiltinProvider {
        name: "openai-responses",
        display_name: "OpenAI Responses API",
        aliases: &["openai-responses"],
        api: Api::OpenAiResponses,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "primary",
        description: "GPT Responses API with hosted tools",
    },
    BuiltinProvider {
        name: "openai-completions",
        display_name: "OpenAI Completions API (Legacy)",
        aliases: &["openai-completions", "completions"],
        api: Api::OpenAiCompletions,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "primary",
        description: "Legacy Completions API",
    },
    BuiltinProvider {
        name: "anthropic",
        display_name: "Anthropic",
        aliases: &["anthropic"],
        api: Api::AnthropicMessages,
        env_key: "ANTHROPIC_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.anthropic.com",
        default_enabled: true,
        auth_method: AuthMethod::XApiKey,
        extra_headers: &[],
        category: "primary",
        description: "Claude models with extended thinking",
    },
    BuiltinProvider {
        name: "google",
        display_name: "Google AI",
        aliases: &["google"],
        api: Api::GoogleGenerativeAi,
        env_key: "GOOGLE_API_KEY",
        extra_env_keys: &["GEMINI_API_KEY"],
        base_url: "https://generativelanguage.googleapis.com",
        default_enabled: true,
        auth_method: AuthMethod::None, // Google uses URL-param auth
        extra_headers: &[],
        category: "primary",
        description: "Gemini models with thinking",
    },
    BuiltinProvider {
        name: "vertex",
        display_name: "Google Vertex AI",
        aliases: &["vertex", "google-vertex"],
        api: Api::GoogleVertex,
        env_key: "GOOGLE_APPLICATION_CREDENTIALS",
        extra_env_keys: &["GOOGLE_CLOUD_PROJECT"],
        base_url: "https://us-central1-aiplatform.googleapis.com",
        default_enabled: true,
        auth_method: AuthMethod::None, // Vertex uses OAuth Bearer
        extra_headers: &[],
        category: "specialized",
        description: "Gemini via Google Cloud Vertex AI",
    },
    BuiltinProvider {
        name: "mistral",
        display_name: "Mistral",
        aliases: &["mistral"],
        api: Api::MistralConversations,
        env_key: "MISTRAL_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.mistral.ai",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "primary",
        description: "Mistral and Codestral models",
    },
    BuiltinProvider {
        name: "azure",
        display_name: "Azure OpenAI",
        aliases: &["azure", "azure-openai"],
        api: Api::AzureOpenAiResponses,
        env_key: "AZURE_OPENAI_API_KEY",
        extra_env_keys: &["AZURE_API_KEY"],
        base_url: "", // Configured per deployment (resource name)
        default_enabled: true,
        auth_method: AuthMethod::ApiKey,
        extra_headers: &[],
        category: "cloud",
        description: "OpenAI models via Azure Cognitive Services",
    },
    BuiltinProvider {
        name: "bedrock",
        display_name: "Amazon Bedrock",
        aliases: &["bedrock", "amazon-bedrock", "aws-bedrock"],
        api: Api::BedrockConverseStream,
        env_key: "AWS_ACCESS_KEY_ID",
        extra_env_keys: &["AWS_PROFILE", "AWS_REGION"],
        base_url: "", // Uses AWS SDK resolution
        default_enabled: true,
        auth_method: AuthMethod::None, // Bedrock uses AWS SigV4
        extra_headers: &[],
        category: "specialized",
        description: "Multi-model via AWS Bedrock ConverseStream",
    },
    // ── OpenAI-compatible providers ─────────────────────────────────────
    // All use Api::OpenAiCompletions with custom base URLs.
    // Base URLs match opencode's openai-compatible-profile.ts.
    BuiltinProvider {
        name: "deepseek",
        display_name: "DeepSeek",
        aliases: &["deepseek"],
        api: Api::OpenAiCompletions,
        env_key: "DEEPSEEK_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.deepseek.com",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "DeepSeek-V3, DeepSeek-R1",
    },
    BuiltinProvider {
        name: "groq",
        display_name: "Groq",
        aliases: &["groq"],
        api: Api::OpenAiCompletions,
        env_key: "GROQ_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.groq.com/openai/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "open",
        description: "Fast Llama, Mixtral, Gemma inference",
    },
    BuiltinProvider {
        name: "cerebras",
        display_name: "Cerebras",
        aliases: &["cerebras"],
        api: Api::OpenAiCompletions,
        env_key: "CEREBRAS_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.cerebras.ai/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("X-Cerebras-3rd-Party-Integration", "opencode")],
        category: "open",
        description: "Ultra-fast open model inference",
    },
    BuiltinProvider {
        name: "xai",
        display_name: "xAI (Grok)",
        aliases: &["xai", "grok"],
        api: Api::OpenAiCompletions,
        env_key: "XAI_API_KEY",
        extra_env_keys: &["GROK_API_KEY"],
        base_url: "https://api.x.ai/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "primary",
        description: "Grok models from xAI",
    },
    BuiltinProvider {
        name: "openrouter",
        display_name: "OpenRouter",
        aliases: &["openrouter"],
        api: Api::OpenAiCompletions,
        env_key: "OPENROUTER_API_KEY",
        extra_env_keys: &[],
        base_url: "https://openrouter.ai/api/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "open",
        description: "Unified gateway to 200+ models",
    },
    BuiltinProvider {
        name: "fireworks",
        display_name: "Fireworks AI",
        aliases: &["fireworks"],
        api: Api::OpenAiCompletions,
        env_key: "FIREWORKS_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.fireworks.ai/inference/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "open",
        description: "Fast open-source model serving",
    },
    BuiltinProvider {
        name: "togetherai",
        display_name: "Together AI",
        aliases: &["togetherai", "together"],
        api: Api::OpenAiCompletions,
        env_key: "TOGETHER_API_KEY",
        extra_env_keys: &["TOGETHER_AI_API_KEY"],
        base_url: "https://api.together.xyz/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "open",
        description: "Open-source model hosting (Llama, Mixtral, etc.)",
    },
    BuiltinProvider {
        name: "deepinfra",
        display_name: "DeepInfra",
        aliases: &["deepinfra"],
        api: Api::OpenAiCompletions,
        env_key: "DEEPINFRA_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.deepinfra.com/v1/openai",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "open",
        description: "Open model inference marketplace",
    },
    BuiltinProvider {
        name: "cloudflare",
        display_name: "Cloudflare Workers AI",
        aliases: &["cloudflare", "workers-ai"],
        api: Api::OpenAiCompletions,
        env_key: "CLOUDFLARE_API_KEY",
        extra_env_keys: &["CF_API_KEY"],
        // Requires account ID: https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1
        base_url: "https://api.cloudflare.com/client/v4/accounts",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "cloud",
        description: "Cloudflare Workers AI",
    },
    BuiltinProvider {
        name: "copilot",
        display_name: "GitHub Copilot",
        aliases: &["copilot", "github-copilot"],
        api: Api::OpenAiCompletions,
        env_key: "GITHUB_COPILOT_TOKEN",
        extra_env_keys: &["GITHUB_TOKEN"],
        base_url: "https://api.githubcopilot.com",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "GitHub Copilot models (GPT-4, Claude)",
    },
    BuiltinProvider {
        name: "codex",
        display_name: "GitHub Codex",
        aliases: &["codex", "github-codex", "copilot-codex"],
        api: Api::OpenAiCompletions,
        env_key: "GITHUB_COPILOT_TOKEN",
        extra_env_keys: &["GITHUB_TOKEN"],
        base_url: "https://api.githubcopilot.com",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "GitHub Codex coding agent",
    },
    // ── Anthropic-compatible providers ──────────────────────────────────
    // These expose the Anthropic Messages API at a custom endpoint.
    BuiltinProvider {
        name: "minimax",
        display_name: "MiniMax",
        aliases: &["minimax"],
        api: Api::AnthropicMessages,
        env_key: "MINIMAX_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.minimax.io/anthropic",
        default_enabled: true,
        auth_method: AuthMethod::XApiKey,
        extra_headers: &[],
        category: "chinese",
        description: "MiniMax-M2.7, abab models",
    },
    BuiltinProvider {
        name: "minimax-cn",
        display_name: "MiniMax (China)",
        aliases: &["minimax-cn"],
        api: Api::AnthropicMessages,
        env_key: "MINIMAX_CN_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.minimaxi.com/anthropic",
        default_enabled: true,
        auth_method: AuthMethod::XApiKey,
        extra_headers: &[],
        category: "chinese",
        description: "MiniMax China region endpoint",
    },
    // ── Other providers ─────────────────────────────────────────────────
    BuiltinProvider {
        name: "zai",
        display_name: "ZAI",
        aliases: &["zai"],
        api: Api::OpenAiCompletions,
        env_key: "ZAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.z.ai/api/coding/paas/v4",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "Z.AI GLM models (coding plan)",
    },
    BuiltinProvider {
        name: "zai-coding-global",
        display_name: "ZAI Coding Plan (Global)",
        aliases: &["zai-coding-global"],
        api: Api::OpenAiCompletions,
        env_key: "ZAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.z.ai/api/coding/paas/v4",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "Z.AI GLM Coding Plan — prompt-based pricing, optimized for coding",
    },
    BuiltinProvider {
        name: "zai-coding-cn",
        display_name: "ZAI Coding Plan (China)",
        aliases: &["zai-coding-cn"],
        api: Api::OpenAiCompletions,
        env_key: "ZAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.z.ai/api/coding/paas/v4",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "Z.AI GLM Coding Plan — China region",
    },
    BuiltinProvider {
        name: "zai-global",
        display_name: "ZAI General API (Global)",
        aliases: &["zai-global"],
        api: Api::OpenAiCompletions,
        env_key: "ZAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.z.ai/api/paas/v4",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "Z.AI GLM General API — token-based pricing",
    },
    BuiltinProvider {
        name: "zai-cn",
        display_name: "ZAI General API (China)",
        aliases: &["zai-cn"],
        api: Api::OpenAiCompletions,
        env_key: "ZAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.z.ai/api/paas/v4",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "Z.AI GLM General API — China region, token-based pricing",
    },
    BuiltinProvider {
        name: "cloudflare-ai-gateway",
        display_name: "Cloudflare AI Gateway",
        aliases: &["cloudflare-ai-gateway", "cf-ai-gateway"],
        api: Api::OpenAiCompletions,
        env_key: "CLOUDFLARE_API_TOKEN",
        extra_env_keys: &["CF_AIG_TOKEN"],
        // Requires account ID + gateway ID in URL at runtime
        base_url: "",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "cloud",
        description: "Serverless AI via Cloudflare",
    },
    BuiltinProvider {
        name: "cloudflare-workers-ai",
        display_name: "Cloudflare Workers AI",
        aliases: &["cloudflare-workers-ai"],
        api: Api::OpenAiCompletions,
        env_key: "CLOUDFLARE_API_KEY",
        extra_env_keys: &["CF_API_KEY"],
        // Requires account ID: https://api.cloudflare.com/client/v4/accounts/{id}/ai/v1
        base_url: "",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "cloud",
        description: "Serverless AI via Cloudflare",
    },
    BuiltinProvider {
        name: "huggingface",
        display_name: "Hugging Face",
        aliases: &["huggingface", "hf"],
        api: Api::OpenAiCompletions,
        env_key: "HUGGINGFACE_API_KEY",
        extra_env_keys: &["HF_API_KEY"],
        base_url: "https://router.huggingface.co/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "open",
        description: "Open model inference hub",
    },
    BuiltinProvider {
        name: "moonshotai",
        display_name: "Moonshot AI (Kimi)",
        aliases: &["moonshotai", "moonshot", "kimi"],
        api: Api::OpenAiCompletions,
        env_key: "MOONSHOT_API_KEY",
        extra_env_keys: &["MOONSHOT_AI_API_KEY", "KIMI_API_KEY"],
        base_url: "https://api.moonshot.cn/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "Kimi models from Moonshot AI",
    },
    BuiltinProvider {
        name: "moonshotai-cn",
        display_name: "Moonshot AI (China)",
        aliases: &["moonshotai-cn", "moonshot-cn"],
        api: Api::OpenAiCompletions,
        env_key: "MOONSHOT_CN_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.moonshotai.com/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "Kimi models — China region endpoint",
    },
    BuiltinProvider {
        name: "vercel-ai-gateway",
        display_name: "Vercel AI Gateway",
        aliases: &["vercel-ai-gateway", "vercel"],
        api: Api::OpenAiCompletions,
        env_key: "VERCEL_API_KEY",
        extra_env_keys: &[],
        base_url: "https://sdk.vercel.ai/api",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "cloud",
        description: "Vercel AI Gateway",
    },
    BuiltinProvider {
        name: "xiaomi",
        display_name: "Xiaomi",
        aliases: &["xiaomi"],
        api: Api::OpenAiCompletions,
        env_key: "XIAOMI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://xiaomi.ai/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "chinese",
        description: "Xiaomi MiMo models",
    },
    BuiltinProvider {
        name: "kimi-coding",
        display_name: "Kimi Coding",
        aliases: &["kimi-coding"],
        api: Api::OpenAiCompletions,
        env_key: "KIMI_CODING_API_KEY",
        extra_env_keys: &["KIMI_API_KEY"],
        base_url: "https://api.kimi.ai/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "Kimi Coding Plan — optimized for coding",
    },
    BuiltinProvider {
        name: "openai-codex",
        display_name: "OpenAI Codex",
        aliases: &["openai-codex"],
        api: Api::OpenAiResponses,
        env_key: "OPENAI_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.openai.com/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "OpenAI Codex coding agent",
    },
    BuiltinProvider {
        name: "opencode-go",
        display_name: "OpenCode Go",
        aliases: &["opencode-go"],
        api: Api::OpenAiCompletions,
        env_key: "OPENCODE_GO_API_KEY",
        extra_env_keys: &[],
        base_url: "https://api.opencode.ai/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "specialized",
        description: "OpenCode Go Gateway",
    },
    // ── Enterprise / Gateway providers ──────────────────────────────────
    BuiltinProvider {
        name: "google-vertex-anthropic",
        display_name: "Google Vertex AI (Anthropic)",
        aliases: &["google-vertex-anthropic", "vertex-anthropic"],
        api: Api::AnthropicMessages,
        env_key: "GOOGLE_APPLICATION_CREDENTIALS",
        extra_env_keys: &["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_LOCATION"],
        // Base URL computed at runtime from project + location
        base_url: "",
        default_enabled: true,
        auth_method: AuthMethod::None, // Uses Google OAuth Bearer
        extra_headers: &[],
        category: "cloud",
        description: "Claude via Google Cloud Vertex AI",
    },
    BuiltinProvider {
        name: "azure-cognitive-services",
        display_name: "Azure Cognitive Services",
        aliases: &["azure-cognitive-services"],
        api: Api::OpenAiCompletions,
        env_key: "AZURE_COGNITIVE_SERVICES_API_KEY",
        extra_env_keys: &["AZURE_COGNITIVE_SERVICES_RESOURCE_NAME"],
        base_url: "", // Computed from resource name at runtime
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "cloud",
        description: "OpenAI via Azure (alias)",
    },
    BuiltinProvider {
        name: "baseten",
        display_name: "Baseten",
        aliases: &["baseten"],
        api: Api::OpenAiCompletions,
        env_key: "BASETEN_API_KEY",
        extra_env_keys: &[],
        base_url: "https://inference.baseten.co/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "open",
        description: "Baseten model inference",
    },
    BuiltinProvider {
        name: "nvidia",
        display_name: "NVIDIA NIM",
        aliases: &["nvidia", "nim"],
        api: Api::OpenAiCompletions,
        env_key: "NVIDIA_API_KEY",
        extra_env_keys: &[],
        base_url: "https://integrate.api.nvidia.com/v1",
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "enterprise",
        description: "LLM Gateway proxy",
    },
    BuiltinProvider {
        name: "llmgateway",
        display_name: "LLM Gateway",
        aliases: &["llmgateway", "llm-gateway"],
        api: Api::OpenAiCompletions,
        env_key: "LLMGATEWAY_API_KEY",
        extra_env_keys: &[],
        base_url: "", // Configured per deployment
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "enterprise",
        description: "LLM Gateway proxy",
    },
    BuiltinProvider {
        name: "gitlab",
        display_name: "GitLab AI",
        aliases: &["gitlab", "gitlab-ai"],
        api: Api::OpenAiCompletions,
        env_key: "GITLAB_TOKEN",
        extra_env_keys: &["GITLAB_INSTANCE_URL"],
        base_url: "", // Computed from GITLAB_INSTANCE_URL at runtime
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "enterprise",
        description: "GitLab Duo Workflow",
    },
    BuiltinProvider {
        name: "sap-ai-core",
        display_name: "SAP AI Core",
        aliases: &["sap-ai-core", "sap"],
        api: Api::OpenAiCompletions,
        env_key: "AICORE_SERVICE_KEY",
        extra_env_keys: &[],
        base_url: "", // Configured from service key at runtime
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[],
        category: "enterprise",
        description: "SAP AI Core",
    },
    BuiltinProvider {
        name: "zenmux",
        display_name: "ZenMux",
        aliases: &["zenmux"],
        api: Api::OpenAiCompletions,
        env_key: "ZENMUX_API_KEY",
        extra_env_keys: &[],
        base_url: "", // Configured per deployment
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "enterprise",
        description: "Kilo Gateway",
    },
    BuiltinProvider {
        name: "kilo",
        display_name: "Kilo",
        aliases: &["kilo"],
        api: Api::OpenAiCompletions,
        env_key: "KILO_API_KEY",
        extra_env_keys: &[],
        base_url: "", // Configured per deployment
        default_enabled: true,
        auth_method: AuthMethod::Bearer,
        extra_headers: &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")],
        category: "enterprise",
        description: "Kilo Gateway",
    },
];

// ---------------------------------------------------------------------------
// API-to-provider mappings
// ---------------------------------------------------------------------------

/// Mapping from API identifier to the primary provider name.
static API_TO_PROVIDER: &[(&str, Api)] = &[
    ("anthropic-messages", Api::AnthropicMessages),
    ("openai-completions", Api::OpenAiCompletions),
    ("mistral-conversations", Api::MistralConversations),
    ("openai-responses", Api::OpenAiResponses),
    ("azure-openai-responses", Api::AzureOpenAiResponses),
    ("google-generative-ai", Api::GoogleGenerativeAi),
    ("google-vertex", Api::GoogleVertex),
    ("bedrock-converse-stream", Api::BedrockConverseStream),
];

// ---------------------------------------------------------------------------
// Registry access functions
// ---------------------------------------------------------------------------

/// Get all built-in providers.
pub fn get_builtin_providers() -> &'static [BuiltinProvider] {
    BUILTIN_PROVIDERS
}

/// Look up a built-in provider by name or alias.
pub fn get_builtin_provider(name: &str) -> Option<&'static BuiltinProvider> {
    BUILTIN_PROVIDERS
        .iter()
        .find(|p| p.name == name || p.aliases.contains(&name))
}

/// Get the environment variable name for a provider.
pub fn get_provider_env_key(name: &str) -> Option<&'static str> {
    get_builtin_provider(name).map(|p| p.env_key)
}

/// Get all environment variable names for a provider (primary + extras).
pub fn get_provider_env_keys(name: &str) -> Vec<&'static str> {
    if let Some(p) = get_builtin_provider(name) {
        let mut keys = vec![p.env_key];
        keys.extend_from_slice(p.extra_env_keys);
        keys
    } else {
        vec![]
    }
}

/// Get the API type for a provider by name or alias.
pub fn get_provider_api(name: &str) -> Option<Api> {
    get_builtin_provider(name).map(|p| p.api)
}

/// Get the default base URL for a provider.
pub fn get_provider_base_url(name: &str) -> Option<&'static str> {
    get_builtin_provider(name).map(|p| p.base_url)
}

/// Get all API-to-provider mappings.
pub fn get_api_mappings() -> &'static [(&'static str, Api)] {
    API_TO_PROVIDER
}

/// Get all provider names (primary names only).
pub fn get_all_provider_names() -> Vec<&'static str> {
    BUILTIN_PROVIDERS.iter().map(|p| p.name).collect()
}

/// Get all provider names including aliases.
pub fn get_all_provider_aliases() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = BUILTIN_PROVIDERS
        .iter()
        .flat_map(|p| std::iter::once(p.name).chain(p.aliases.iter().copied()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Resolve a provider name/alias to its canonical name.
pub fn resolve_provider_name(name: &str) -> Option<&'static str> {
    get_builtin_provider(name).map(|p| p.name)
}

/// Check if a provider name or alias is a known built-in.
pub fn is_builtin_provider(name: &str) -> bool {
    get_builtin_provider(name).is_some()
}

// ---------------------------------------------------------------------------
// Data-driven provider factory
// ---------------------------------------------------------------------------

/// Create a built-in provider by name.
///
/// This is the **single source of truth** for provider instantiation. It reads
/// the `BuiltinProvider` metadata and creates the appropriate provider struct
/// with the correct base URL, API key, and extra headers.
///
/// Returns `None` if the name is not a known built-in provider.
pub fn create_builtin_provider(name: &str) -> Option<Box<dyn super::Provider>> {
    let builtin = get_builtin_provider(name)?;

    match builtin.api {
        // ── Anthropic Messages API ──────────────────────────────────────
        Api::AnthropicMessages => {
            let extra_headers: Vec<(String, String)> = builtin
                .extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            // If the provider has its own base URL (MiniMax, etc.), use it
            if !builtin.base_url.is_empty() && builtin.name != "anthropic" {
                Some(Box::new(super::anthropic::AnthropicProvider::with_config(
                    builtin.base_url,
                    None,
                    extra_headers,
                )))
            } else {
                Some(Box::new(super::anthropic::AnthropicProvider::new()))
            }
        }

        // ── Google APIs ─────────────────────────────────────────────────
        Api::GoogleGenerativeAi => Some(Box::new(super::google::GoogleProvider::new())),
        Api::GoogleVertex => Some(Box::new(super::vertex::VertexProvider::new())),

        // ── Mistral ─────────────────────────────────────────────────────
        Api::MistralConversations => Some(Box::new(super::mistral::MistralProvider::new())),

        // ── Azure ───────────────────────────────────────────────────────
        Api::AzureOpenAiResponses => Some(Box::new(super::azure::AzureProvider::new())),

        // ── Bedrock ─────────────────────────────────────────────────────
        Api::BedrockConverseStream => Some(Box::new(super::bedrock::BedrockProvider::new())),

        // ── OpenAI Responses API ────────────────────────────────────────
        Api::OpenAiResponses => Some(Box::new(
            super::openai_responses::OpenAiResponsesProvider::new(),
        )),

        // ── OpenAI Chat Completions API ─────────────────────────────────
        // All OpenAI-compatible providers use OpenAiProvider with custom base
        // URLs and optional extra headers from their BuiltinProvider metadata.
        Api::OpenAiCompletions => {
            let extra_headers: Vec<(String, String)> = builtin
                .extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            if builtin.base_url.is_empty() {
                // OpenAI itself (no custom base URL)
                if extra_headers.is_empty() {
                    Some(Box::new(super::openai::OpenAiProvider::new()))
                } else {
                    Some(Box::new(super::openai::OpenAiProvider::with_config(
                        "https://api.openai.com/v1",
                        None,
                        extra_headers,
                    )))
                }
            } else if extra_headers.is_empty() {
                Some(Box::new(super::openai::OpenAiProvider::with_base_url(
                    builtin.base_url,
                )))
            } else {
                Some(Box::new(super::openai::OpenAiProvider::with_config(
                    builtin.base_url,
                    None,
                    extra_headers,
                )))
            }
        }
    }
}

/// Create a built-in provider with optional credential and base URL overrides.
///
/// This is like [`create_builtin_provider`] but allows injecting an API key
/// and/or base URL at construction time instead of reading from environment
/// variables. When `api_key` is `Some`, it takes precedence over the
/// environment. When `base_url` is `Some`, the provider's default endpoint
/// is overridden.
///
/// Returns `None` if the name is not a known built-in provider.
pub fn create_builtin_provider_with_options(
    name: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Option<Box<dyn super::Provider>> {
    let builtin = get_builtin_provider(name)?;

    // Resolve API key: explicit override > environment variables
    let resolved_key = api_key.map(String::from).or_else(|| {
        std::env::var(builtin.env_key).ok().or_else(|| {
            builtin
                .extra_env_keys
                .iter()
                .find_map(|k| std::env::var(k).ok())
        })
    });

    // Resolve base URL: explicit override > built-in default
    let resolved_base_url = base_url.map(String::from).or_else(|| {
        if builtin.base_url.is_empty() {
            None
        } else {
            Some(builtin.base_url.to_string())
        }
    });

    let extra_headers: Vec<(String, String)> = builtin
        .extra_headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    match builtin.api {
        // ── Anthropic Messages API ──────────────────────────────────────
        Api::AnthropicMessages => {
            if let Some(key) = resolved_key {
                Some(Box::new(super::anthropic::AnthropicProvider::with_config(
                    resolved_base_url
                        .as_deref()
                        .unwrap_or("https://api.anthropic.com"),
                    Some(key),
                    extra_headers,
                )))
            } else if resolved_base_url.is_some() {
                Some(Box::new(super::anthropic::AnthropicProvider::with_config(
                    resolved_base_url
                        .as_deref()
                        .unwrap_or("https://api.anthropic.com"),
                    None,
                    extra_headers,
                )))
            } else {
                // No key and no base URL — fall back to default construction
                // (reads from env at stream time)
                create_builtin_provider(name)
            }
        }

        // ── Google APIs ─────────────────────────────────────────────────
        Api::GoogleGenerativeAi => create_builtin_provider(name),
        Api::GoogleVertex => create_builtin_provider(name),

        // ── Mistral ─────────────────────────────────────────────────────
        Api::MistralConversations => create_builtin_provider(name),

        // ── Azure ───────────────────────────────────────────────────────
        Api::AzureOpenAiResponses => create_builtin_provider(name),

        // ── Bedrock ─────────────────────────────────────────────────────
        Api::BedrockConverseStream => create_builtin_provider(name),

        // ── OpenAI Responses API ────────────────────────────────────────
        Api::OpenAiResponses => create_builtin_provider(name),

        // ── OpenAI Chat Completions API ─────────────────────────────────
        Api::OpenAiCompletions => {
            let url = resolved_base_url
                .as_deref()
                .unwrap_or(if builtin.base_url.is_empty() {
                    "https://api.openai.com/v1"
                } else {
                    builtin.base_url
                });

            if let Some(key) = resolved_key {
                if extra_headers.is_empty() {
                    Some(Box::new(
                        super::openai::OpenAiProvider::with_base_url_and_key(url, Some(key)),
                    ))
                } else {
                    Some(Box::new(super::openai::OpenAiProvider::with_config(
                        url,
                        Some(key),
                        extra_headers,
                    )))
                }
            } else if url != builtin.base_url || !extra_headers.is_empty() {
                // Base URL override or extra headers without explicit key
                Some(Box::new(super::openai::OpenAiProvider::with_config(
                    url,
                    None,
                    extra_headers,
                )))
            } else {
                create_builtin_provider(name)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_builtin_provider_anthropic() {
        let p = create_builtin_provider("anthropic").unwrap();
        assert_eq!(p.name(), "anthropic");
    }

    #[test]
    fn test_create_builtin_provider_openai() {
        let p = create_builtin_provider("openai").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_by_alias() {
        let p = create_builtin_provider("amazon-bedrock").unwrap();
        assert_eq!(p.name(), "bedrock");
    }

    #[test]
    fn test_create_builtin_provider_unknown() {
        assert!(create_builtin_provider("unknown").is_none());
    }

    #[test]
    fn test_create_builtin_provider_deepseek() {
        let p = create_builtin_provider("deepseek").unwrap();
        assert_eq!(p.name(), "openai"); // Uses OpenAI provider with custom base URL
    }

    #[test]
    fn test_create_builtin_provider_minimax() {
        let p = create_builtin_provider("minimax").unwrap();
        assert_eq!(p.name(), "anthropic"); // Uses Anthropic provider with custom base URL
    }

    #[test]
    fn test_create_builtin_provider_minimax_cn() {
        let p = create_builtin_provider("minimax-cn").unwrap();
        assert_eq!(p.name(), "anthropic"); // Uses Anthropic provider with custom base URL
    }

    #[test]
    fn test_create_builtin_provider_togetherai() {
        let p = create_builtin_provider("togetherai").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_openrouter() {
        let p = create_builtin_provider("openrouter").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_cerebras() {
        let p = create_builtin_provider("cerebras").unwrap();
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn test_get_builtin_provider_openai() {
        let p = get_builtin_provider("openai").unwrap();
        assert_eq!(p.name, "openai");
        assert_eq!(p.display_name, "OpenAI");
        assert_eq!(p.api, Api::OpenAiCompletions);
        assert_eq!(p.auth_method, AuthMethod::Bearer);
    }

    #[test]
    fn test_get_builtin_provider_anthropic() {
        let p = get_builtin_provider("anthropic").unwrap();
        assert_eq!(p.name, "anthropic");
        assert_eq!(p.auth_method, AuthMethod::XApiKey);
    }

    #[test]
    fn test_get_builtin_provider_azure() {
        let p = get_builtin_provider("azure").unwrap();
        assert_eq!(p.auth_method, AuthMethod::ApiKey);
    }

    #[test]
    fn test_get_builtin_provider_by_alias() {
        let p = get_builtin_provider("amazon-bedrock").unwrap();
        assert_eq!(p.name, "bedrock");
    }

    #[test]
    fn test_get_builtin_provider_unknown() {
        assert!(get_builtin_provider("unknown-provider").is_none());
    }

    #[test]
    fn test_get_provider_env_key() {
        assert_eq!(get_provider_env_key("openai"), Some("OPENAI_API_KEY"));
        assert_eq!(get_provider_env_key("anthropic"), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn test_get_provider_env_keys_with_extras() {
        let keys = get_provider_env_keys("google");
        assert!(keys.contains(&"GOOGLE_API_KEY"));
        assert!(keys.contains(&"GEMINI_API_KEY"));
    }

    #[test]
    fn test_get_provider_api() {
        assert_eq!(get_provider_api("anthropic"), Some(Api::AnthropicMessages));
        assert_eq!(get_provider_api("vertex"), Some(Api::GoogleVertex));
    }

    #[test]
    fn test_resolve_provider_name() {
        assert_eq!(resolve_provider_name("google-vertex"), Some("vertex"));
        assert_eq!(resolve_provider_name("aws-bedrock"), Some("bedrock"));
        assert_eq!(resolve_provider_name("openai"), Some("openai"));
    }

    #[test]
    fn test_is_builtin_provider() {
        assert!(is_builtin_provider("openai"));
        assert!(is_builtin_provider("deepseek"));
        assert!(is_builtin_provider("togetherai"));
        assert!(!is_builtin_provider("fake-provider"));
    }

    #[test]
    fn test_all_providers_have_env_key() {
        for p in get_builtin_providers() {
            assert!(!p.env_key.is_empty(), "Provider {} has no env key", p.name);
        }
    }

    #[test]
    fn test_all_providers_have_auth_method() {
        for p in get_builtin_providers() {
            // Just verify they all have a valid auth method
            match p.auth_method {
                AuthMethod::Bearer
                | AuthMethod::XApiKey
                | AuthMethod::ApiKey
                | AuthMethod::None => {}
            }
        }
    }

    #[test]
    fn test_get_all_provider_names() {
        let names = get_all_provider_names();
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"bedrock"));
        assert!(names.contains(&"togetherai"));
        assert!(names.len() >= 20);
    }

    #[test]
    fn test_get_all_provider_aliases() {
        let aliases = get_all_provider_aliases();
        assert!(aliases.contains(&"amazon-bedrock"));
        assert!(aliases.contains(&"aws-bedrock"));
        assert!(aliases.contains(&"bedrock"));
        assert!(aliases.contains(&"together"));
    }

    #[test]
    fn test_get_provider_base_url() {
        assert_eq!(
            get_provider_base_url("openai"),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            get_provider_base_url("anthropic"),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            get_provider_base_url("groq"),
            Some("https://api.groq.com/openai/v1")
        );
        assert_eq!(
            get_provider_base_url("togetherai"),
            Some("https://api.together.xyz/v1")
        );
    }

    #[test]
    fn test_minimax_base_url() {
        let p = get_builtin_provider("minimax").unwrap();
        assert_eq!(p.base_url, "https://api.minimax.io/anthropic");
        assert_eq!(p.api, Api::AnthropicMessages);
    }

    #[test]
    fn test_openrouter_extra_headers() {
        let p = get_builtin_provider("openrouter").unwrap();
        assert_eq!(
            p.extra_headers,
            &[("HTTP-Referer", "https://oxi.dev/"), ("X-Title", "oxi")]
        );
    }

    #[test]
    fn test_cerebras_extra_headers() {
        let p = get_builtin_provider("cerebras").unwrap();
        assert_eq!(
            p.extra_headers,
            &[("X-Cerebras-3rd-Party-Integration", "opencode")]
        );
    }

    #[test]
    fn test_create_builtin_provider_with_options_openai() {
        // With explicit API key and base URL
        let p = create_builtin_provider_with_options(
            "openai",
            Some("sk-test-key"),
            Some("https://my-proxy.example.com/v1"),
        );
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "openai");
    }

    #[test]
    fn test_create_builtin_provider_with_options_anthropic() {
        let p = create_builtin_provider_with_options("anthropic", Some("sk-ant-test-key"), None);
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "anthropic");
    }

    #[test]
    fn test_create_builtin_provider_with_options_no_override() {
        // No key or URL — should fall back to default creation
        let p = create_builtin_provider_with_options("deepseek", None, None);
        assert!(p.is_some());
    }

    #[test]
    fn test_create_builtin_provider_with_options_unknown() {
        let p = create_builtin_provider_with_options("nonexistent_provider", None, None);
        assert!(p.is_none());
    }
}
