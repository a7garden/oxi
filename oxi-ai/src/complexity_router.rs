//! Complexity-based model routing for oxi-ai
//!
//! This module provides a router that classifies task complexity
//! and selects appropriate models based on capability and cost requirements.

use crate::model_db::{self, ModelEntry};
use crate::{Complexity, Context, Message, MessageContent};

/// Routes tasks to models based on estimated complexity.
pub trait ComplexityRouter: Send + Sync {
    /// Classify the complexity of the given context.
    fn classify(&self, context: &Context) -> Complexity;

    /// Pick the best models for a given complexity.
    fn route(
        &self,
        complexity: Complexity,
        prefer_cost_efficient: bool,
    ) -> Vec<&'static ModelEntry>;
}

/// Default implementation of ComplexityRouter.
///
/// Uses keyword analysis, token counting, and system prompt hints
/// to determine task complexity, then routes to appropriate models.
#[derive(Debug, Clone, Default)]
pub struct DefaultRouter {
    _private: (),
}

impl DefaultRouter {
    /// Create a new DefaultRouter
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Extract text from MessageContent
    fn extract_content_text(&self, content: &MessageContent) -> String {
        match content {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// Get the last user message text from the context
    fn get_last_user_message_text(&self, context: &Context) -> Option<String> {
        context.messages.iter().rev().find_map(|msg| {
            if let Message::User(user_msg) = msg {
                let text = self.extract_content_text(&user_msg.content);
                if !text.is_empty() {
                    Some(text)
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    /// Count tokens in text using the high-level estimator
    fn count_tokens(&self, text: &str) -> usize {
        crate::high_level::tokens::estimate(text)
    }

    /// Analyze text for complexity keywords and return a base complexity score
    /// Returns 0-4 mapping to Complexity tiers (0=Trivial, 1=Simple, 2=Moderate, etc.)
    fn analyze_keywords(&self, text: &str) -> i32 {
        let lower = text.to_lowercase();

        // Check for complex/research keywords FIRST (more specific patterns)
        // Complex keywords: score 3
        let complex_keywords = [
            "build a",
            "build the",
            "create a service",
            "write a full",
            "implement a complete",
            "implement a full",
            "microservice",
            "distributed system",
            "concurrent",
            "parallel processing",
            "full-stack",
            "full stack",
            "end-to-end",
            "enterprise",
            "complete application",
            "complete system",
        ];
        let has_complex = complex_keywords.iter().any(|kw| lower.contains(*kw));

        // Research keywords: score 4
        let research_keywords = [
            "analyze deeply",
            "research",
            "evaluate thoroughly",
            "investigate",
            "compare and contrast",
            "benchmark",
            "comprehensive analysis",
            "thorough",
            "in-depth",
            "deep research",
            "study of",
        ];
        let has_research = research_keywords.iter().any(|kw| lower.contains(*kw));

        // Moderate keywords: score 2
        let moderate_keywords = [
            "architect",
            "design a",
            "refactor",
            "implement",
            "create a class",
            "optimize",
            "debug",
            "review code",
            "parse",
            "validate",
            "schema",
            "api",
            "build a",
        ];
        let has_moderate = moderate_keywords.iter().any(|kw| lower.contains(*kw));

        // Simple keywords: score 1
        let simple_keywords = [
            "explain",
            "write function",
            "fix typo",
            "list",
            "describe",
            "define",
            "convert",
            "calculate",
            "simple",
        ];
        let has_simple = simple_keywords.iter().any(|kw| lower.contains(*kw));

        // Trivial keywords: score 0
        let trivial_keywords = [
            "translate",
            "summarize",
            "spell check",
            "format",
            "capitalize",
            "lowercase",
            "uppercase",
            "trim",
            "count words",
        ];
        let has_trivial = trivial_keywords.iter().any(|kw| lower.contains(*kw));

        // Return the highest matching score (research > complex > moderate > simple > trivial)
        if has_research {
            4
        } else if has_complex {
            3
        } else if has_moderate {
            2
        } else if has_simple {
            1
        } else if has_trivial {
            0
        } else {
            1 // Default to simple
        }
    }

    /// Analyze system prompt for complexity hints
    fn analyze_system_prompt(&self, system_prompt: Option<&str>) -> i32 {
        let Some(prompt) = system_prompt else {
            return 0;
        };

        let lower = prompt.to_lowercase();

        // System prompts with "research" or "deep analysis" suggest higher complexity
        if lower.contains("research")
            || lower.contains("deep analysis")
            || lower.contains("thorough")
        {
            return 2;
        }

        // "helpful assistant" without specific guidance suggests trivial/simple
        if lower.contains("helpful assistant")
            && !lower.contains("expert")
            && !lower.contains("advanced")
        {
            return 0;
        }

        // "expert" or "senior" suggests higher complexity
        if lower.contains("expert")
            || lower.contains("senior developer")
            || lower.contains("architect")
        {
            return 1;
        }

        0
    }

    /// Convert keyword score to Complexity enum
    /// Score maps directly to complexity tier (0=Trivial, 1=Simple, 2=Moderate, etc.)
    fn score_to_complexity(&self, score: i32) -> Complexity {
        match score {
            0 => Complexity::Trivial,
            1 => Complexity::Simple,
            2 => Complexity::Moderate,
            3 => Complexity::Complex,
            _ => Complexity::Research,
        }
    }

    /// Get models filtered by complexity tier
    fn get_models_for_complexity(&self, complexity: Complexity) -> Vec<&'static ModelEntry> {
        let complexity_tier = complexity.cost_tier();

        // Model mapping per complexity level
        // We search for models by name/id pattern
        let patterns: Vec<&str> = match complexity {
            Complexity::Trivial => vec!["haiku", "gpt-4o-mini", "mini"],
            Complexity::Simple => vec!["haiku", "sonnet", "gpt-4o-mini", "mini"],
            Complexity::Moderate => vec!["sonnet", "opus", "gpt-4o", "gpt-4.1"],
            Complexity::Complex => vec!["opus", "gemini-2.5-pro", "gpt-4.1", "claude-sonnet"],
            Complexity::Research => vec![
                "opus-4.5",
                "opus-4.6",
                "gemini-3-pro",
                "gemini-2.5-pro",
                "claude-opus",
            ],
        };

        // Collect matching models from the database
        let mut candidates: Vec<&'static ModelEntry> = Vec::new();

        for pattern in &patterns {
            let matches = model_db::search_models(pattern);
            for model in matches {
                // Prefer models that have been updated more recently (higher version numbers)
                // Also filter by relevance to complexity tier
                if self.model_suitable_for_tier(model, complexity_tier)
                    && !candidates.contains(&model)
                {
                    candidates.push(model);
                }
            }
        }

        // Deduplicate and limit
        candidates.truncate(20);
        candidates
    }

    /// Check if a model is suitable for a given complexity tier
    fn model_suitable_for_tier(&self, model: &ModelEntry, tier: u8) -> bool {
        match tier {
            // Trivial: fast, cheap models
            0 => {
                // Prefer models without reasoning (faster, cheaper)
                !model.supports_reasoning() || model.cost_input < 0.5
            }
            // Simple: moderate capability
            1 => !model.supports_reasoning() || model.cost_input < 1.5,
            // Moderate: good capability
            2 => {
                // Mid-range models
                model.cost_input < 5.0 || model.supports_reasoning()
            }
            // Complex: high capability
            3 => {
                // High-end models
                model.supports_reasoning() || model.cost_input < 15.0
            }
            // Research: top tier only
            _ => {
                // Best models for research
                model.supports_reasoning()
                    || model.context_window >= 200_000
                    || model.name.to_lowercase().contains("pro")
                    || model.name.to_lowercase().contains("opus")
            }
        }
    }

    /// Sort candidates by cost efficiency
    fn sort_by_cost(&self, candidates: &mut [&'static ModelEntry]) {
        candidates.sort_by(|a, b| {
            let cost_a = a.cost_input + a.cost_output;
            let cost_b = b.cost_input + b.cost_output;
            cost_a
                .partial_cmp(&cost_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Sort candidates by capability (reasoning > context_window > cost)
    fn sort_by_capability(&self, candidates: &mut [&'static ModelEntry]) {
        candidates.sort_by(|a, b| {
            // Primary: reasoning capability
            let a_reasoning = if a.supports_reasoning() { 1 } else { 0 };
            let b_reasoning = if b.supports_reasoning() { 1 } else { 0 };
            if a_reasoning != b_reasoning {
                return b_reasoning.cmp(&a_reasoning);
            }

            // Secondary: context window size
            let a_context = a.context_window;
            let b_context = b.context_window;
            if a_context != b_context {
                return b_context.cmp(&a_context);
            }

            // Tertiary: output capability
            let a_output = a.max_tokens;
            let b_output = b.max_tokens;
            if a_output != b_output {
                return b_output.cmp(&a_output);
            }

            // Quaternary: cost (prefer cheaper for same capability)
            let cost_a = a.cost_input + a.cost_output;
            let cost_b = b.cost_input + b.cost_output;
            cost_a
                .partial_cmp(&cost_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
}

impl ComplexityRouter for DefaultRouter {
    fn classify(&self, context: &Context) -> Complexity {
        // Get last user message text
        let last_user_text = self.get_last_user_message_text(context);

        let Some(text) = last_user_text else {
            // No user message - check system prompt
            let prompt_score = self.analyze_system_prompt(context.system_prompt.as_deref());
            if !context.tools.is_empty() {
                let bumped = (prompt_score + 1).min(4);
                return self.score_to_complexity(bumped);
            }
            return self.score_to_complexity(prompt_score);
        };

        // Count tokens
        let token_count = self.count_tokens(&text);

        // Analyze keywords for complexity
        let keyword_score = self.analyze_keywords(&text);

        // Determine base score based on keywords and token count
        // For short trivial inputs, don't bump (they're genuinely simple)
        let base_score = if token_count < 100 {
            // Short inputs: trust keyword detection
            // But ensure trivial keywords don't get bumped
            keyword_score
        } else if token_count > 2000 {
            // Very long inputs: increase complexity
            (keyword_score + 2).min(4)
        } else if token_count > 500 {
            // Medium inputs: slightly increase complexity
            (keyword_score + 1).min(4)
        } else {
            keyword_score
        };

        // Analyze system prompt for hints (can increase complexity)
        let system_score = self.analyze_system_prompt(context.system_prompt.as_deref());
        let final_score = if system_score > base_score {
            system_score
        } else {
            base_score
        };

        // If context has tools, bump complexity by 1 (capped at Research)
        let final_score = if !context.tools.is_empty() {
            (final_score + 1).min(4)
        } else {
            final_score
        };

        self.score_to_complexity(final_score)
    }

    fn route(
        &self,
        complexity: Complexity,
        prefer_cost_efficient: bool,
    ) -> Vec<&'static ModelEntry> {
        // Get candidates for this complexity
        let mut candidates = self.get_models_for_complexity(complexity);

        // Filter to models that support the complexity tier
        let tier = complexity.cost_tier();
        candidates.retain(|m| self.model_suitable_for_tier(m, tier));

        // Sort based on preference
        if prefer_cost_efficient {
            self.sort_by_cost(&mut candidates);
        } else {
            self.sort_by_capability(&mut candidates);
        }

        // Return top 3 candidates
        candidates.truncate(3);
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, UserMessage};

    fn create_context_with_user_message(text: &str) -> Context {
        let mut ctx = Context::new();
        ctx.add_message(Message::User(UserMessage::new(text.to_string())));
        ctx
    }

    #[test]
    fn test_trivial_keywords() {
        let router = DefaultRouter::new();

        // Trivial keywords should be detected
        let ctx = create_context_with_user_message("Please translate this to Spanish");
        assert_eq!(router.classify(&ctx), Complexity::Trivial);

        let ctx = create_context_with_user_message("Summarize this text for me");
        assert_eq!(router.classify(&ctx), Complexity::Trivial);

        // "spell check" as a phrase should be trivial
        let ctx = create_context_with_user_message("spell check this document");
        assert_eq!(router.classify(&ctx), Complexity::Trivial);
    }

    #[test]
    fn test_simple_keywords() {
        let router = DefaultRouter::new();

        let ctx = create_context_with_user_message("Explain how this code works");
        assert_eq!(router.classify(&ctx), Complexity::Simple);

        let ctx = create_context_with_user_message("Write a function to reverse a string");
        assert_eq!(router.classify(&ctx), Complexity::Simple);

        let ctx = create_context_with_user_message("List all files in the directory");
        assert_eq!(router.classify(&ctx), Complexity::Simple);
    }

    #[test]
    fn test_moderate_keywords() {
        let router = DefaultRouter::new();

        let ctx = create_context_with_user_message("Architect a REST API service");
        assert_eq!(router.classify(&ctx), Complexity::Moderate);

        let ctx = create_context_with_user_message("Design a database schema");
        assert_eq!(router.classify(&ctx), Complexity::Moderate);

        let ctx = create_context_with_user_message("Refactor this module");
        assert_eq!(router.classify(&ctx), Complexity::Moderate);
    }

    #[test]
    fn test_complex_keywords() {
        let router = DefaultRouter::new();

        // Complex keywords should be detected
        let ctx = create_context_with_user_message(
            "Build a complete microservices architecture with distributed tracing",
        );
        assert!(router.classify(&ctx) >= Complexity::Complex);

        let ctx = create_context_with_user_message(
            "Implement a full-stack application with authentication and database",
        );
        assert!(router.classify(&ctx) >= Complexity::Complex);
    }

    #[test]
    fn test_research_keywords() {
        let router = DefaultRouter::new();

        let ctx = create_context_with_user_message(
            "Analyze deeply the performance characteristics of this system",
        );
        assert_eq!(router.classify(&ctx), Complexity::Research);

        let ctx = create_context_with_user_message(
            "Conduct a comprehensive research study on machine learning",
        );
        assert_eq!(router.classify(&ctx), Complexity::Research);
    }

    #[test]
    fn test_tools_bump_complexity() {
        let router = DefaultRouter::new();

        let mut ctx = create_context_with_user_message("List files");
        assert_eq!(router.classify(&ctx), Complexity::Simple);

        // Add a tool - should bump complexity
        ctx.add_tool(crate::Tool::new(
            "list_files",
            "List files",
            serde_json::json!({}),
        ));
        assert_eq!(router.classify(&ctx), Complexity::Moderate);
    }

    #[test]
    fn test_token_count_affects_complexity() {
        let router = DefaultRouter::new();

        // Test single character - very short
        let ctx = create_context_with_user_message("a");
        let complexity = router.classify(&ctx);
        assert!(
            complexity >= Complexity::Simple,
            "Short text should be at least Simple, got {:?}",
            complexity
        );

        // Test "explain" keyword alone
        let ctx = create_context_with_user_message("explain this");
        let complexity = router.classify(&ctx);
        assert_eq!(complexity, Complexity::Simple, "'explain' should be Simple");

        // Very long text should increase complexity (use enough to exceed 500 tokens)
        // "Explain this code in detail. " is ~7 words, ~10 chars, ~13 tokens
        // Need ~40+ repetitions to exceed 500 tokens
        let long_text = "Explain this code in detail. ".repeat(100);
        let ctx = create_context_with_user_message(&long_text);
        let complexity = router.classify(&ctx);
        assert!(
            complexity >= Complexity::Moderate,
            "Long text should be at least Moderate, got {:?}",
            complexity
        );
    }

    #[test]
    fn test_routing_trivial() {
        let router = DefaultRouter::new();

        let models = router.route(Complexity::Trivial, true);
        assert!(!models.is_empty());
        assert!(models.len() <= 3);
    }

    #[test]
    fn test_routing_research() {
        let router = DefaultRouter::new();

        let models = router.route(Complexity::Research, false);
        assert!(!models.is_empty());
        assert!(models.len() <= 3);

        // Research models should support reasoning
        for model in &models {
            assert!(
                model.supports_reasoning() || model.context_window >= 200_000,
                "Model {} should support reasoning or have large context",
                model.name
            );
        }
    }

    #[test]
    fn test_cost_efficient_sorting() {
        let router = DefaultRouter::new();

        let models = router.route(Complexity::Moderate, true);

        if models.len() > 1 {
            // Verify cost sorting
            for i in 1..models.len() {
                let prev_cost = models[i - 1].cost_input + models[i - 1].cost_output;
                let curr_cost = models[i].cost_input + models[i].cost_output;
                assert!(
                    prev_cost <= curr_cost,
                    "Cost-efficient sorting failed: {:?} > {:?}",
                    prev_cost,
                    curr_cost
                );
            }
        }
    }

    #[test]
    fn test_capability_sorting() {
        let router = DefaultRouter::new();

        let models = router.route(Complexity::Complex, false);

        if models.len() > 1 {
            // First model should have reasoning if any do
            let any_reasoning = models.iter().any(|m| m.supports_reasoning());
            if any_reasoning {
                assert!(
                    models[0].supports_reasoning(),
                    "First model should support reasoning when sorting by capability"
                );
            }
        }
    }

    #[test]
    fn test_system_prompt_analysis() {
        let router = DefaultRouter::new();

        let mut ctx = Context::new();
        ctx.set_system_prompt("You are a helpful assistant.");
        ctx.add_message(Message::User(UserMessage::new("Hello")));

        // Simple system prompt should not increase complexity
        let complexity = router.classify(&ctx);
        assert!(complexity <= Complexity::Simple);

        let mut ctx = Context::new();
        ctx.set_system_prompt(
            "You are an expert senior software architect conducting thorough deep analysis.",
        );
        ctx.add_message(Message::User(UserMessage::new("Hello")));

        // Expert system prompt should increase complexity
        let complexity = router.classify(&ctx);
        assert!(complexity >= Complexity::Moderate);
    }

    #[test]
    fn test_empty_context() {
        let router = DefaultRouter::new();

        let ctx = Context::new();
        let complexity = router.classify(&ctx);
        // Empty context defaults to Trivial
        assert_eq!(complexity, Complexity::Trivial);
    }

    #[test]
    fn test_default_router() {
        let router = DefaultRouter::default();
        let ctx = create_context_with_user_message("translate this text");
        let complexity = router.classify(&ctx);
        // "translate" is a trivial keyword
        assert_eq!(complexity, Complexity::Trivial);
    }

    #[test]
    fn test_complexity_trait_object() {
        use std::sync::Arc;

        let router: Arc<dyn ComplexityRouter> = Arc::new(DefaultRouter::new());
        let ctx = create_context_with_user_message("refactor this code");
        let complexity = router.classify(&ctx);
        assert_eq!(complexity, Complexity::Moderate);

        let models = router.route(complexity, true);
        assert!(!models.is_empty());
    }
}
