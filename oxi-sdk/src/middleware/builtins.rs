//! Built-in middleware implementations

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use tracing::Level;

use crate::middleware::{
    Middleware, MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewareResult,
};

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Rate limit middleware — limits calls per minute.
pub struct RateLimitMiddleware {
    max_calls_per_minute: usize,
    counters: Arc<RwLock<HashMap<String, (usize, u64)>>>,
}

impl RateLimitMiddleware {
    /// Create a rate limiter allowing at most `max_calls_per_minute` calls per minute.
    pub fn new(max_calls_per_minute: usize) -> Self {
        Self {
            max_calls_per_minute,
            counters: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Middleware for RateLimitMiddleware {
    fn name(&self) -> &str {
        "rate_limit"
    }
    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::BeforeTool]
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        Box::pin(async move {
            let agent_id = ctx.agent_id.clone();
            let max = self.max_calls_per_minute;
            let counters = Arc::clone(&self.counters);
            let now = current_time_ms();
            let allowed = {
                let mut c = counters.write();
                let entry = c.entry(agent_id.clone()).or_insert((0, now));
                if now - entry.1 >= 60_000 {
                    entry.0 = 1;
                    entry.1 = now;
                    true
                } else {
                    entry.0 += 1;
                    entry.1 = now;
                    entry.0 <= max
                }
            };
            if allowed {
                MiddlewareResult::pass()
            } else {
                MiddlewareResult::block(format!("Rate limit exceeded for {}", ctx.agent_id))
            }
        })
    }
}

/// Logging middleware — logs middleware events.
pub struct LoggingMiddleware {
    _level: Level,
}

impl LoggingMiddleware {
    /// Create a logging middleware that emits at the given tracing `level`.
    pub fn new(level: Level) -> Self {
        Self { _level: level }
    }
}

impl Middleware for LoggingMiddleware {
    fn name(&self) -> &str {
        "logging"
    }
    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![
            MiddlewarePhase::BeforeTool,
            MiddlewarePhase::AfterTool,
            MiddlewarePhase::AfterRun,
        ]
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        Box::pin(async move {
            match &ctx.data {
                MiddlewareData::BeforeTool { tool_name, .. } => {
                    tracing::info!(agent = %ctx.agent_id, tool = %tool_name, "BeforeTool")
                }
                MiddlewareData::AfterTool {
                    tool_name, result, ..
                } => {
                    tracing::info!(agent = %ctx.agent_id, tool = %tool_name, result = %result, "AfterTool")
                }
                MiddlewareData::AfterRun {
                    response,
                    success,
                    duration_ms,
                } => {
                    tracing::info!(agent = %ctx.agent_id, success = %success, duration_ms = %duration_ms, response = %truncate(response, 100), "AfterRun")
                }
                _ => {}
            }
            MiddlewareResult::pass()
        })
    }
}

/// Token budget middleware — tracks and enforces token budgets.
pub struct TokenBudgetMiddleware {
    max_tokens: usize,
    usage: Arc<AtomicU64>,
    cost_tracker: Option<Arc<crate::observability::CostTracker>>,
    cost_budget: Option<f64>,
}

impl TokenBudgetMiddleware {
    /// Create a token budget that terminates the agent once cumulative usage exceeds `max_tokens`.
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            usage: Arc::new(AtomicU64::new(0)),
            cost_tracker: None,
            cost_budget: None,
        }
    }
    /// Create with cost tracker integration for cost-based budget enforcement.
    pub fn with_cost_tracker(
        max_tokens: usize,
        tracker: Arc<crate::observability::CostTracker>,
        budget: f64,
    ) -> Self {
        Self {
            max_tokens,
            usage: Arc::new(AtomicU64::new(0)),
            cost_tracker: Some(tracker),
            cost_budget: Some(budget),
        }
    }
}

impl Middleware for TokenBudgetMiddleware {
    fn name(&self) -> &str {
        "token_budget"
    }
    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::AfterLlm]
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        Box::pin(async move {
            if let MiddlewareData::AfterLlm {
                response_text,
                tokens_used,
            } = &ctx.data
            {
                // Track token usage from the LLM response if available
                if let Some(usage) = tokens_used {
                    self.usage.fetch_add(usage.total(), Ordering::SeqCst);
                } else {
                    // Fallback: estimate from response length
                    let len = response_text.len() as u64;
                    self.usage.fetch_add(len, Ordering::SeqCst);
                }

                // Check token budget
                if self.usage.load(Ordering::SeqCst) > self.max_tokens as u64 {
                    return MiddlewareResult::terminate(format!(
                        "Token budget exceeded for {}",
                        ctx.agent_id
                    ));
                }

                // Check cost budget if cost tracker is attached
                if let Some(tracker) = &self.cost_tracker {
                    if let Some(budget) = self.cost_budget
                        && tracker.agent_cost(&ctx.agent_id) > budget
                    {
                        return MiddlewareResult::terminate(format!(
                            "Cost budget exceeded for {}",
                            ctx.agent_id
                        ));
                    }
                    if tracker.is_over_budget(&ctx.agent_id) {
                        return MiddlewareResult::terminate(format!(
                            "Agent budget exceeded for {}",
                            ctx.agent_id
                        ));
                    }
                }
            }
            MiddlewareResult::pass()
        })
    }
}

/// Content filter middleware — blocks content matching patterns.
pub struct ContentFilterMiddleware {
    blocked: Vec<String>,
}

impl ContentFilterMiddleware {
    /// Create a content filter blocking any content matching the given patterns.
    pub fn new(blocked: Vec<String>) -> Self {
        Self { blocked }
    }
}

impl Middleware for ContentFilterMiddleware {
    fn name(&self) -> &str {
        "content_filter"
    }
    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::AfterLlm, MiddlewarePhase::BeforeTool]
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        Box::pin(async move {
            match &ctx.data {
                MiddlewareData::AfterLlm { response_text, .. } => {
                    for pat in &self.blocked {
                        if response_text.contains(pat) {
                            return MiddlewareResult::block(format!(
                                "Content blocked for {}",
                                ctx.agent_id
                            ));
                        }
                    }
                }
                MiddlewareData::BeforeTool { params, .. } => {
                    let s = serde_json::to_string(params).unwrap_or_default();
                    for pat in &self.blocked {
                        if s.contains(pat) {
                            return MiddlewareResult::block(format!(
                                "Content blocked for {}",
                                ctx.agent_id
                            ));
                        }
                    }
                }
                _ => {}
            }
            MiddlewareResult::pass()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentFilterMiddleware, RateLimitMiddleware};
    use crate::middleware::{Middleware, MiddlewareContext, MiddlewareData, MiddlewarePhase};

    #[tokio::test]
    async fn test_rate_limit() {
        let mw = RateLimitMiddleware::new(5);
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "a1",
            MiddlewareData::BeforeTool {
                tool_name: "read".into(),
                params: serde_json::json!({}),
            },
        );
        for _ in 0..5 {
            assert!(mw.handle(&ctx).await.is_continue());
        }
        assert!(mw.handle(&ctx).await.is_block());
    }

    #[tokio::test]
    async fn test_content_filter() {
        let mw = ContentFilterMiddleware::new(vec!["rm -rf".into()]);
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "a1",
            MiddlewareData::BeforeTool {
                tool_name: "bash".into(),
                params: serde_json::json!({"cmd": "rm -rf /"}),
            },
        );
        assert!(mw.handle(&ctx).await.is_block());
    }
}
