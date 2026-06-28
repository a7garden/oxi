//! Concrete [`AdvisorAgent`] adapter over [`crate::Agent`].
//!
//! The SDK exposes the [`AdvisorAgent`] trait (so tests/SDK consumers can
//! hand-roll a fake), but every real consumer that drives an `oxi_agent::Agent`
//! repeats the same 5-method mapping. This adapter removes that boilerplate —
//! the host wiring and SDK consumers both use [`AgentAdvisor::new`].
//!
//! Method mapping (verified against `Agent`):
//! - `prompt` → `Agent::continue_with` (preserves advisor context across turns;
//!   `run_with_channel` auto-resets the cancel flag, so a prior `abort` does
//!   not strand the next prompt)
//! - `abort` → `Agent::cancel`
//! - `reset` → `Agent::reset`
//! - `rollback_to` → `Agent::update_state(|s| s.messages.truncate(count))`
//! - `message_count` → `Agent::state().messages.len()`

use std::sync::Arc;

use async_trait::async_trait;

use crate::Agent;
use crate::advisor::runtime::AdvisorAgent;

/// Hook fired after each successful advisor prompt, with the advisor agent.
/// The host uses it to persist advisor turns to `<session>/__advisor.jsonl`.
pub type AdvisorPromptHook = Arc<dyn Fn(&Agent) + Send + Sync>;

/// An [`AdvisorAgent`] backed by a concrete [`Agent`].
pub struct AgentAdvisor {
    agent: Arc<Agent>,
    on_prompted: Option<AdvisorPromptHook>,
}

impl AgentAdvisor {
    /// Wrap an `Agent` as an advisor-driving [`AdvisorAgent`].
    #[must_use]
    pub fn new(agent: Arc<Agent>) -> Self {
        Self {
            agent,
            on_prompted: None,
        }
    }

    /// Wrap an `Agent` with a hook fired after each successful advisor prompt.
    /// Used by the host to persist advisor turns to `<session>/__advisor.jsonl`
    /// for stats attribution / observability.
    #[must_use]
    pub fn with_post_prompt_hook(
        agent: Arc<Agent>,
        hook: AdvisorPromptHook,
    ) -> Self {
        Self {
            agent,
            on_prompted: Some(hook),
        }
    }

    /// Access the underlying agent (for transcript-recorder / event wiring).
    #[must_use]
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Clone the underlying agent handle (cheap — `Arc` clone).
    #[must_use]
    pub fn into_agent(self) -> Arc<Agent> {
        self.agent
    }
}

#[async_trait]
impl AdvisorAgent for AgentAdvisor {
    async fn prompt(&self, input: String) -> Result<(), String> {
        // continue_with preserves the advisor's conversation state across
        // turns (appends rather than resetting). The Response + events are
        // discarded — the host wires a separate event subscription for the
        // transcript recorder if it wants advisor-turn observability.
        self.agent
            .continue_with(input)
            .await
            .map(|_| {
                if let Some(hook) = &self.on_prompted {
                    hook(&self.agent);
                }
            })
            .map_err(|e| e.to_string())
    }

    fn abort(&self, _reason: &str) {
        // Sets the cancel flag; the next prompt() -> run_with_channel resets
        // it, so abort does not strand subsequent turns.
        self.agent.cancel();
    }

    fn reset(&self) {
        self.agent.reset();
    }

    async fn rollback_to(&self, count: usize) {
        self.agent.update_state(|s| s.messages.truncate(count));
    }

    fn message_count(&self) -> usize {
        self.agent.state().messages.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::config::AgentConfig;
    use oxi_ai::{Message, Provider};

    /// A provider that never actually streams — we only exercise the
    /// non-network methods (reset/rollback_to/message_count) here.
    struct NopProvider;
    impl Provider for NopProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a oxi_ai::Model,
            _context: &'a oxi_ai::Context,
            _options: Option<oxi_ai::StreamOptions>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = oxi_ai::StreamResult> + Send + 'a>>
        {
            // We never call prompt() in these tests; this provider only
            // satisfies Agent construction. Returns an empty stream.
            let s: std::pin::Pin<Box<dyn futures::Stream<Item = oxi_ai::ProviderEvent> + Send>> =
                Box::pin(futures::stream::empty::<oxi_ai::ProviderEvent>());
            Box::pin(async move { Ok(s) })
        }
        fn name(&self) -> &str {
            "nop"
        }
    }

    #[tokio::test]
    async fn message_count_tracks_state() {
        let provider: Arc<dyn Provider> = Arc::new(NopProvider);
        let agent = Arc::new(Agent::new_empty(provider, AgentConfig::default()));
        let advisor = AgentAdvisor::new(Arc::clone(&agent));

        assert_eq!(advisor.message_count(), 0);
        // Mutate state directly and confirm message_count reflects it.
        agent.update_state(|s| {
            s.messages.push(Message::user("hello"));
            s.messages.push(Message::user("world"));
        });
        assert_eq!(advisor.message_count(), 2);
    }

    #[tokio::test]
    async fn rollback_to_truncates_messages() {
        let provider: Arc<dyn Provider> = Arc::new(NopProvider);
        let agent = Arc::new(Agent::new_empty(provider, AgentConfig::default()));
        let advisor = AgentAdvisor::new(Arc::clone(&agent));
        agent.update_state(|s| {
            s.messages.push(Message::user("a"));
            s.messages.push(Message::user("b"));
            s.messages.push(Message::user("c"));
            s.messages.push(Message::user("d"));
        });
        advisor.rollback_to(2).await;
        assert_eq!(advisor.message_count(), 2);
        assert_eq!(agent.state().messages[0].text_content().unwrap(), "a");
    }

    #[tokio::test]
    async fn reset_clears_state() {
        let provider: Arc<dyn Provider> = Arc::new(NopProvider);
        let agent = Arc::new(Agent::new_empty(provider, AgentConfig::default()));
        let advisor = AgentAdvisor::new(Arc::clone(&agent));
        agent.update_state(|s| {
            s.messages.push(Message::user("a"));
        });
        assert_eq!(advisor.message_count(), 1);
        advisor.reset();
        assert_eq!(advisor.message_count(), 0);
    }

    #[test]
    fn agent_accessor_and_into_agent_round_trip() {
        let provider: Arc<dyn Provider> = Arc::new(NopProvider);
        let agent = Arc::new(Agent::new_empty(provider, AgentConfig::default()));
        let cloned = Arc::clone(&agent);
        let advisor = AgentAdvisor::new(cloned);
        // accessor + into_agent both hand back the same underlying Arc.
        assert!(std::ptr::eq(advisor.agent(), Arc::as_ref(&agent)));
        assert!(Arc::ptr_eq(&advisor.into_agent(), &agent));
    }
}
