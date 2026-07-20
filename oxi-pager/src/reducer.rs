// reduce — pure state-update function. PR-2 ships a stub that returns
// an empty action list; PR-5 fills in the full body per spec §4.
//
// The reducer is `&mut PagerState -> Vec<PagerAction>`, no async, no
// external calls. This keeps the lock-guard-borrowed-across-await
// pattern safe (AGENTS.md pitfall).

use crate::emitter::PagerEvent;
use crate::state::{ModalKind, PagerState};

/// A command the main loop should execute after `reduce` returns.
#[derive(Debug)]
pub enum PagerAction {
    /// Trigger a render pass.
    Render,
    /// Send a command to the agent.
    SendToAgent(AgentCmd),
    /// Execute a raw terminal operation.
    SendToTerminal(TermCmd),
    /// Play a sound (1차 no-op).
    PlaySound(Sound),
    /// Reschedule the next tick.
    ScheduleTick(u64),
    /// Open a modal overlay.
    OpenModal(ModalKind, ModalCtx),
    /// Close the current modal.
    CloseModal,
    /// Quit the TUI.
    Quit(ExitReason),
}

#[derive(Debug, Clone)]
pub enum AgentCmd {
    /// Submit a user message to the agent.
    SubmitUserMessage { text: String },
    /// Cancel the in-flight agent run.
    Cancel,
    /// Approve a tool call.
    ApproveTool { call_id: String },
    /// Deny a tool call.
    DenyTool { call_id: String, reason: String },
}

#[derive(Debug, Clone)]
pub enum TermCmd {
    /// Reserved for OSC 8 / cursor / etc. — see PR-7.
    Stub,
}

#[derive(Debug, Clone)]
pub enum Sound {
    Stub,
}

#[derive(Debug)]
pub struct ModalCtx {
    /// Opaque context passed to the overlay factory. PR-6 will type this.
    pub payload: Option<Box<dyn std::any::Any + Send + Sync>>,
}

#[derive(Debug, Clone)]
pub enum ExitReason {
    UserQuit,
    AgentDone,
    Error(String),
}

/// Pure state-update function. PR-2 stub: returns no actions.
pub fn reduce(_state: &mut PagerState, _event: PagerEvent) -> Vec<PagerAction> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PagerState;

    #[test]
    fn reduce_stub_returns_empty_for_any_event() {
        let mut state = PagerState::default();
        let actions = reduce(&mut state, PagerEvent::Tick);
        assert!(actions.is_empty(), "PR-2 reducer is a no-op");
    }
}
