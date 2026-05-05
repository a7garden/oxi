//! Important default values used across the application.

/// Default thinking level for the agent.
pub const DEFAULT_THINKING_LEVEL: &str = "medium";

/// Default maximum iterations the agent will run before stopping.
pub const DEFAULT_MAX_ITERATIONS: u32 = 10;

/// Default tool timeout in seconds.
pub const DEFAULT_TOOL_TIMEOUT_SECONDS: u64 = 120;

/// Default context window size (tokens).
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

/// Default temperature for model responses.
pub const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Default maximum tokens for model responses.
pub const DEFAULT_MAX_TOKENS: u32 = 16_384;

/// Default compaction threshold ratio.
pub const DEFAULT_COMPACTION_THRESHOLD: f32 = 0.8;

/// Application name used in prompts and display.
pub const APP_NAME: &str = "oxi";
