// reduce — pure state-update function.

use oxi_agent::events::AgentEvent;

use crate::emitter::PagerEvent;
use crate::state::{ModalKind, PagerState};

#[derive(Debug)]
pub enum PagerAction {
    Render,
    SendToAgent(AgentCmd),
    SendToTerminal(TermCmd),
    PlaySound(Sound),
    ScheduleTick(u64),
    OpenModal(ModalKind, ModalCtx),
    CloseModal,
    Quit(ExitReason),
}

#[derive(Debug, Clone)]
pub enum AgentCmd {
    SubmitUserMessage { text: String },
    Cancel,
    ApproveTool { call_id: String },
    DenyTool { call_id: String, reason: String },
}

#[derive(Debug, Clone)]
pub enum TermCmd { Stub }

#[derive(Debug, Clone)]
pub enum Sound { Stub }

#[derive(Debug, Default)]
pub struct ModalCtx {
    pub payload: Option<Box<dyn std::any::Any + Send + Sync>>,
}

#[derive(Debug)]
pub enum ExitReason { UserQuit, AgentDone, Error(String) }

pub fn reduce(state: &mut PagerState, event: PagerEvent) -> Vec<PagerAction> {
    match event {
        PagerEvent::Agent(agent_ev) => reduce_agent(state, *agent_ev),
        PagerEvent::Input(_) => Vec::new(),
        PagerEvent::Tick => { state.status.tick(); vec![PagerAction::Render] }
        PagerEvent::Background(_) => Vec::new(),
    }
}

fn reduce_agent(state: &mut PagerState, event: AgentEvent) -> Vec<PagerAction> {
    use AgentEvent::*;
    match event {
        MessageUpdate { delta, .. } => {
            if let Some(text) = delta { state.scrollback.append_token(&text); }
            vec![PagerAction::Render]
        }
        MessageStart { .. } => { state.scrollback.begin_assistant(); vec![PagerAction::Render] }
        MessageEnd { .. } => { state.scrollback.end_assistant(); vec![PagerAction::Render] }
        ToolExecutionStart { tool_name, tool_call_id, .. } => {
            state.scrollback.begin_tool_call(&tool_name, &tool_call_id);
            vec![PagerAction::Render]
        }
        ToolExecutionEnd { .. } => { state.scrollback.end_tool_call(""); vec![PagerAction::Render] }
        ToolError { error, .. } => { state.status.set_error(error); vec![PagerAction::Render] }
        Error { .. } => vec![PagerAction::Render],
        AgentStart { .. } | AgentEnd { .. } => vec![PagerAction::Render],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PagerState;

    #[test]
    fn reduce_tick_triggers_render() {
        let mut state = PagerState::default();
        let actions = reduce(&mut state, PagerEvent::Tick);
        assert!(actions.iter().any(|a| matches!(a, PagerAction::Render)));
    }

    #[test]
    fn reduce_agent_start_triggers_render() {
        let mut state = PagerState::default();
        let actions = reduce(&mut state, PagerEvent::Agent(Box::new(
            AgentEvent::AgentStart { prompts: vec![], session_id: None },
        )));
        assert!(actions.iter().any(|a| matches!(a, PagerAction::Render)));
    }
}
