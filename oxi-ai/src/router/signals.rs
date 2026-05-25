//! Signal extraction for routing decisions.

#![allow(missing_docs)]

//! Three signal types are extracted from the conversation context:
//! - **StructuralSignal**: message count, tool-call density, context size.
//! - **BehavioralSignal**: conversation phase, recent tool-use patterns.
//! - **ContextBudgetSignal**: token-budget pressure and cost constraints.

use crate::messages::{ContentBlock, Message};

use super::types::{RouterPhase, RoutingDecision};

// ── Structural Signal ─────────────────────────────────────────────────────────

/// Signals derived from the structure of the conversation.
#[derive(Debug, Clone, Default)]
pub struct StructuralSignal {
    pub message_count: usize,
    pub tool_call_count: usize,
    pub tool_result_count: usize,
    pub estimated_tokens: usize,
    pub user_message_count: usize,
}

impl StructuralSignal {
    /// Extract structural signals from the conversation messages.
    pub fn extract(messages: &[Message]) -> Self {
        let mut signal = Self {
            message_count: messages.len(),
            ..Default::default()
        };
        let mut total_chars: usize = 0;

        for msg in messages {
            match msg {
                Message::User(u) => {
                    signal.user_message_count += 1;
                    total_chars += match &u.content {
                        crate::messages::MessageContent::Text(s) => s.len(),
                        crate::messages::MessageContent::Blocks(blocks) => blocks
                            .iter()
                            .map(|b| match b {
                                ContentBlock::Text(t) => t.text.len(),
                                ContentBlock::Image(img) => img.data.len() / 4,
                                _ => 16,
                            })
                            .sum(),
                    };
                }
                Message::Assistant(a) => {
                    for block in &a.content {
                        match block {
                            ContentBlock::Text(t) => total_chars += t.text.len(),
                            ContentBlock::Thinking(t) => total_chars += t.thinking.len(),
                            ContentBlock::ToolCall(_) => signal.tool_call_count += 1,
                            ContentBlock::Image(img) => total_chars += img.data.len() / 4,
                            ContentBlock::Unknown(v) => total_chars += v.to_string().len(),
                        }
                    }
                }
                Message::ToolResult(t) => {
                    signal.tool_result_count += 1;
                    for block in &t.content {
                        if let ContentBlock::Text(txt) = block {
                            total_chars += txt.text.len();
                        }
                    }
                }
            }
        }

        signal.estimated_tokens = total_chars / 4;
        signal
    }

    /// Normalize to `[0, 1]` for scoring.
    pub fn normalized(&self) -> f64 {
        let msg_factor = (self.message_count as f64).ln_1p() / 10.0_f64.ln_1p();
        let tool_factor = (self.tool_call_count as f64).ln_1p() / 20.0_f64.ln_1p();
        let token_factor = (self.estimated_tokens as f64).ln_1p() / 100_000.0_f64.ln_1p();
        (0.3 * msg_factor + 0.4 * tool_factor + 0.3 * token_factor).clamp(0.0, 1.0)
    }
}

// ── Behavioral Signal ─────────────────────────────────────────────────────────

/// Signals derived from recent conversation behavior.
#[derive(Debug, Clone, Default)]
pub struct BehavioralSignal {
    pub phase: RouterPhase,
    pub recent_tool_count: usize,
    pub phase_transitions: usize,
    pub is_question: bool,
}

impl BehavioralSignal {
    /// Extract behavioral signals from context and decision history.
    pub fn extract(messages: &[Message], history: &[RoutingDecision]) -> Self {
        let phase = Self::detect_phase(messages);
        let recent_tool_count = Self::count_recent_tools(messages, 10);
        let phase_transitions = Self::count_phase_transitions(history);
        let is_question = Self::detect_question(messages);

        Self {
            phase,
            recent_tool_count,
            phase_transitions,
            is_question,
        }
    }

    fn detect_phase(messages: &[Message]) -> RouterPhase {
        let recent = messages.len().saturating_sub(6);
        let recent_msgs = &messages[recent..];

        let tool_calls_in_recent: usize = recent_msgs
            .iter()
            .map(|m| match m {
                Message::Assistant(a) => a
                    .content
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::ToolCall(_)))
                    .count(),
                _ => 0,
            })
            .sum();

        let user_msgs_in_recent: usize = recent_msgs
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .count();

        if tool_calls_in_recent >= 3 {
            RouterPhase::Implementation
        } else if user_msgs_in_recent >= 2 && tool_calls_in_recent == 0 {
            RouterPhase::Planning
        } else {
            RouterPhase::Lightweight
        }
    }

    /// Count tool-result messages in the last `n` messages.
    pub fn count_recent_tools(messages: &[Message], n: usize) -> usize {
        let start = messages.len().saturating_sub(n);
        messages[start..]
            .iter()
            .filter(|m| matches!(m, Message::ToolResult(_)))
            .count()
    }

    /// Count phase changes in the decision history.
    pub fn count_phase_transitions(history: &[RoutingDecision]) -> usize {
        if history.len() < 2 {
            return 0;
        }
        history
            .windows(2)
            .filter(|w| w[0].phase != w[1].phase)
            .count()
    }

    fn detect_question(messages: &[Message]) -> bool {
        messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::User(u) => Some(u.content.as_str().unwrap_or("").to_lowercase()),
                _ => None,
            })
            .map(|text| {
                text.ends_with('?')
                    || text.starts_with("what")
                    || text.starts_with("how")
                    || text.starts_with("why")
                    || text.starts_with("when")
                    || text.starts_with("where")
                    || text.starts_with("who")
                    || text.starts_with("explain")
            })
            .unwrap_or(false)
    }

    /// Normalize to `[0, 1]` for scoring.
    pub fn normalized(&self) -> f64 {
        let phase_weight = self.phase.weight();
        let tool_factor = (self.recent_tool_count as f64 / 10.0).min(1.0);
        (0.5 * phase_weight + 0.3 * tool_factor + 0.2 * self.phase_transitions as f64)
            .clamp(0.0, 1.0)
    }
}

// ── Context / Budget Signal ───────────────────────────────────────────────────

/// Signals derived from token budget and cost constraints.
#[derive(Debug, Clone, Default)]
pub struct ContextBudgetSignal {
    pub estimated_tokens: usize,
    pub accumulated_cost: f64,
    pub budget_limit: Option<f64>,
    pub context_upgrade_threshold: Option<usize>,
}

impl ContextBudgetSignal {
    /// Extract budget signals from token estimate, cost, and config.
    pub fn extract(
        estimated_tokens: usize,
        accumulated_cost: f64,
        budget_limit: Option<f64>,
        context_upgrade_threshold: Option<usize>,
    ) -> Self {
        Self {
            estimated_tokens,
            accumulated_cost,
            budget_limit,
            context_upgrade_threshold,
        }
    }

    /// Returns `true` if context length exceeds the upgrade threshold.
    pub fn should_upgrade_context(&self) -> bool {
        self.context_upgrade_threshold
            .map(|t| self.estimated_tokens > t)
            .unwrap_or(false)
    }

    /// Returns `true` if accumulated cost exceeds budget.
    pub fn is_over_budget(&self) -> bool {
        self.budget_limit
            .map(|l| self.accumulated_cost >= l)
            .unwrap_or(false)
    }

    /// Budget utilization ratio `[0, 1]` (1.0 = at/over limit).
    pub fn budget_utilization(&self) -> f64 {
        self.budget_limit
            .map(|l| (self.accumulated_cost / l).min(1.0))
            .unwrap_or(0.0)
    }

    /// Normalize to `[0, 1]` for scoring (higher = more resource pressure).
    pub fn normalized(&self) -> f64 {
        let token_factor = (self.estimated_tokens as f64 / 200_000.0).min(1.0);
        let budget_factor = self.budget_utilization();
        (0.6 * token_factor + 0.4 * budget_factor).clamp(0.0, 1.0)
    }
}
