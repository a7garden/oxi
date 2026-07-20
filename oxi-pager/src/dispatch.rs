// dispatch — translates PagerAction::SendToAgent into agent API calls.
//
// PR-4 ships the dispatch surface (signature + minimal body) but the
// full implementation is incremental across PR-4..7. For now the
// only meaningful action is `SubmitUserMessage` which calls the
// owned agent handle. The other variants are stubs that return Ok(()).

use crate::reducer::AgentCmd;
use crate::state::PagerState;

pub fn dispatch<T>(_agent: &T, _state: &PagerState, cmd: AgentCmd) -> anyhow::Result<()>
where
    T: ?Sized + Send + Sync,
{
    match cmd {
        AgentCmd::SubmitUserMessage { text: _text } => {
            // PR-5: wire actual agent.submit(&text) here
            Ok(())
        }
        AgentCmd::Cancel => Ok(()),
        AgentCmd::ApproveTool { .. } => Ok(()),
        AgentCmd::DenyTool { .. } => Ok(()),
    }
}
