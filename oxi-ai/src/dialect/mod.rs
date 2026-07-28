//! Owned (in-band) tool-calling dialects.
//!
//! This module is the Rust port of omp's `@oh-my-pi/pi-ai/dialect` package. It
//! lets a model that has **no native tool-calling support** still drive the
//! agent loop: the tool catalog is injected into the system prompt as text,
//! prior tool calls/results are re-encoded as text in the history, and the
//! model's text output is parsed back into canonical [`ToolCall`] blocks.
//!
//! # Architecture (omp three-piece contract)
//!
//! 1. **Prompt injection** — [`render_inband_tool_prompt`] appends the tool
//!    catalog plus the dialect's format guide to the system prompt.
//! 2. **History encoding** — [`encode_inband_tool_history`] rewrites prior
//!    assistant tool calls and tool results into the dialect's text form so the
//!    model sees a coherent transcript (and prefix caching stays stable).
//! 3. **Output parsing** — [`Dialect::parse`] turns the model's text back into
//!    [`ScanSegment`]s (text / thinking / tool calls) the loop can execute.
//!
//! # Dialects
//!
//! [`Dialect`] enumerates the 11 wire dialects omp knows. This first delivery
//! implements the **XML** dialect fully (the documented fallback,
//! `FALLBACK_DIALECT` in omp). Dialects without a dedicated implementation
//! fall back to XML — this mirrors omp's fallback semantics rather than being a
//! placeholder: XML's `<invoke>/<parameter>` grammar is the generic envelope.
//! Streaming scanners and agent-loop wiring land in a follow-up.
//!
//! [`ToolCall`]: crate::ToolCall

mod coercion;
mod history;
mod render;
mod xml;

pub use coercion::{ToolArgShape, build_arg_shapes, is_string_only_schema};
pub use history::encode_inband_tool_history;
pub use render::{render_inband_tool_prompt, render_tool_catalog};

use crate::messages::{AssistantMessage, ToolCall};
use crate::tools::Tool;

/// An owned tool-calling dialect.
///
/// Mirrors omp's `Dialect` union (`@oh-my-pi/pi-catalog/identity`). The variant
/// selects the text grammar used to render and parse in-band tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// GLM (ChatGLM) XML dialect.
    Glm,
    /// Hermes / Qwen JSON-fence dialect.
    Hermes,
    /// Kimi (Moonshot) section dialect.
    Kimi,
    /// Generic XML dialect — the fallback (`<invoke>/<parameter>`).
    Xml,
    /// Anthropic `<function_calls>` / `<antml:invoke>` dialect.
    Anthropic,
    /// DeepSeek DSML dialect.
    Deepseek,
    /// OpenAI / GPT-OSS Harmony control-token dialect.
    Harmony,
    /// Qwen3 dialect.
    Qwen3,
    /// Google Gemini `<start_of_turn>` dialect.
    Gemini,
    /// Google Gemma `<|turn>` dialect.
    Gemma,
    /// MiniMax `<minimax:tool_call>` dialect.
    Minimax,
}

/// The dialect used when a model family has no dedicated grammar.
pub const FALLBACK_DIALECT: Dialect = Dialect::Xml;

impl Dialect {
    /// Parse a dialect from its kebab/short name (e.g. `"xml"`, `"qwen3"`).
    ///
    /// Returns `None` for unknown names. Accepts the canonical lower-case
    /// identifier omp uses on the wire and in `PI_DIALECT`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "glm" => Dialect::Glm,
            "hermes" => Dialect::Hermes,
            "kimi" => Dialect::Kimi,
            "xml" => Dialect::Xml,
            "anthropic" => Dialect::Anthropic,
            "deepseek" => Dialect::Deepseek,
            "harmony" => Dialect::Harmony,
            "qwen3" | "qwen" => Dialect::Qwen3,
            "gemini" => Dialect::Gemini,
            "gemma" => Dialect::Gemma,
            "minimax" => Dialect::Minimax,
            _ => return None,
        })
    }

    /// The canonical wire identifier (matches omp's `Dialect` string values).
    pub fn as_str(self) -> &'static str {
        match self {
            Dialect::Glm => "glm",
            Dialect::Hermes => "hermes",
            Dialect::Kimi => "kimi",
            Dialect::Xml => "xml",
            Dialect::Anthropic => "anthropic",
            Dialect::Deepseek => "deepseek",
            Dialect::Harmony => "harmony",
            Dialect::Qwen3 => "qwen3",
            Dialect::Gemini => "gemini",
            Dialect::Gemma => "gemma",
            Dialect::Minimax => "minimax",
        }
    }

    /// Resolve the dialect a model id prefers, by family token.
    ///
    /// Mirrors omp's `preferredDialect(modelId)`. The match is a substring
    /// scan over the lower-cased model id; unknown families fall back to
    /// [`FALLBACK_DIALECT`].
    pub fn preferred_for_model(model_id: &str) -> Self {
        let id = model_id.to_ascii_lowercase();
        // Order matters: more specific families first (gemma before gemini).
        if id.contains("anthropic") || id.contains("claude") {
            Dialect::Anthropic
        } else if id.contains("glm") {
            Dialect::Glm
        } else if id.contains("gemma") {
            Dialect::Gemma
        } else if id.contains("gemini") {
            Dialect::Gemini
        } else if id.contains("kimi") || id.contains("moonshot") {
            Dialect::Kimi
        } else if id.contains("qwen") {
            Dialect::Qwen3
        } else if id.contains("deepseek") {
            Dialect::Deepseek
        } else if id.contains("minimax") {
            Dialect::Minimax
        } else if id.contains("gpt-oss") || id.contains("openai") || id.contains("gpt-") {
            Dialect::Harmony
        } else {
            FALLBACK_DIALECT
        }
    }

    /// The dialect's format-guide prompt fragment (injected after the catalog).
    pub fn prompt(self) -> String {
        // Only XML carries a dedicated prompt in this delivery; every dialect
        // falls back to the XML guide, matching omp's fallback semantics.
        xml::xml_prompt()
    }

    /// Render a batch of (parallel) tool calls as one text block.
    pub fn render_tool_calls(self, calls: &[ToolCall], tools: &[Tool]) -> String {
        xml::render_tool_calls(calls, tools)
    }

    /// Render a run of tool results as one text block.
    pub fn render_tool_results(self, results: &[RenderedToolResult]) -> String {
        xml::render_tool_results(results)
    }

    /// Render a thinking/reasoning block in the dialect's envelope.
    pub fn render_thinking(self, text: &str) -> String {
        xml::render_thinking(text)
    }

    /// Parse completed model text into segments (text / thinking / tool calls).
    ///
    /// This is the batch entry point. Streaming (incremental) parsing lands in
    /// a follow-up; the agent loop will call this on the accumulated assistant
    /// text once the turn completes.
    pub fn parse(self, text: &str, tools: &[Tool]) -> Vec<ScanSegment> {
        xml::parse(text, tools)
    }

    /// Re-materialize in-band tool-call text on an assistant message as native
    /// [`ToolCall`] content blocks, leaving any prose as text blocks.
    ///
    /// Returns the rewritten message. When the text contains no tool calls the
    /// message is returned unchanged.
    pub fn parse_assistant_message(
        self,
        message: &AssistantMessage,
        tools: &[Tool],
    ) -> AssistantMessage {
        xml::parse_assistant_message(message, tools)
    }
}

/// A tool result flattened to text for dialect rendering.
///
/// Mirrors omp's `DialectToolResult`.
#[derive(Debug, Clone)]
pub struct RenderedToolResult {
    /// The tool call id this result answers.
    pub id: String,
    /// The tool name.
    pub name: String,
    /// Position within a parallel result run (0-based).
    pub index: usize,
    /// Flattened text content.
    pub text: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

/// A parsed segment of in-band model output.
///
/// Mirrors the terminal events of omp's `InbandScanEvent` stream, collapsed to
/// the three segment kinds the agent loop consumes.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanSegment {
    /// Visible prose text.
    Text(String),
    /// Reasoning / chain-of-thought.
    Thinking(String),
    /// A re-materialized tool call.
    ToolCall(ToolCall),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect_round_trips_through_str() {
        for d in [
            Dialect::Glm,
            Dialect::Hermes,
            Dialect::Kimi,
            Dialect::Xml,
            Dialect::Anthropic,
            Dialect::Deepseek,
            Dialect::Harmony,
            Dialect::Qwen3,
            Dialect::Gemini,
            Dialect::Gemma,
            Dialect::Minimax,
        ] {
            assert_eq!(Dialect::from_name(d.as_str()), Some(d), "{d:?}");
        }
    }

    #[test]
    fn dialect_from_name_is_case_insensitive_and_trims() {
        assert_eq!(Dialect::from_name("  XML "), Some(Dialect::Xml));
        assert_eq!(Dialect::from_name("Qwen"), Some(Dialect::Qwen3));
        assert_eq!(Dialect::from_name("nope"), None);
    }

    #[test]
    fn preferred_dialect_matches_families() {
        assert_eq!(
            Dialect::preferred_for_model("claude-sonnet-4"),
            Dialect::Anthropic
        );
        assert_eq!(
            Dialect::preferred_for_model("anthropic/claude-opus"),
            Dialect::Anthropic
        );
        assert_eq!(
            Dialect::preferred_for_model("gemini-2.5-pro"),
            Dialect::Gemini
        );
        assert_eq!(Dialect::preferred_for_model("gemma-3-27b"), Dialect::Gemma);
        assert_eq!(Dialect::preferred_for_model("qwen3-coder"), Dialect::Qwen3);
        assert_eq!(
            Dialect::preferred_for_model("deepseek-v3"),
            Dialect::Deepseek
        );
        assert_eq!(
            Dialect::preferred_for_model("gpt-oss-120b"),
            Dialect::Harmony
        );
        assert_eq!(Dialect::preferred_for_model("glm-4.6"), Dialect::Glm);
        assert_eq!(Dialect::preferred_for_model("kimi-k2"), Dialect::Kimi);
        assert_eq!(Dialect::preferred_for_model("minimax-m2"), Dialect::Minimax);
        // Unknown family falls back.
        assert_eq!(
            Dialect::preferred_for_model("llama-3.3-70b"),
            FALLBACK_DIALECT
        );
    }

    #[test]
    fn gemma_wins_over_gemini_prefix() {
        // "gemma" must be checked before "gemini" — both contain "gem".
        assert_eq!(Dialect::preferred_for_model("gemma-2-9b"), Dialect::Gemma);
    }
}
