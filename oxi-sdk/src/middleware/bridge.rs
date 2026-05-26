// MiddlewareBridge — converts MiddlewarePipeline to AgentHooks

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oxi_agent::AgentHooks;

use crate::middleware::{MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewarePipeline};

/// Build `AgentHooks` from a `MiddlewarePipeline`.
///
/// Maps MiddlewarePhase to the agent's BeforeTool and AfterTool hooks.
/// When a middleware calls MiddlewareResult::terminate, sets `terminate_flag`.
pub fn build_hooks(
    pipeline: Arc<MiddlewarePipeline>,
    agent_id: String,
    terminate_flag: Arc<AtomicBool>,
) -> AgentHooks {
    let before_tool_call = Some(std::boxed::Box::new({
        let pipeline = Arc::clone(&pipeline);
        let agent_id = agent_id.clone();
        let terminate_flag = Arc::clone(&terminate_flag);

        move |ctx: &oxi_agent::BeforeToolCallContext| -> oxi_agent::BeforeToolCallResult {
            let mw_ctx = MiddlewareContext::new(
                MiddlewarePhase::BeforeTool,
                &agent_id,
                MiddlewareData::BeforeTool {
                    tool_name: ctx.tool_name.clone(),
                    params: ctx.args.clone(),
                },
            );

            // Use block_on to run the async pipeline synchronously from sync callback
            let rt = tokio::runtime::Handle::current();
            let result = rt.block_on(pipeline.execute(&mw_ctx));

            match result.action {
                crate::middleware::MiddlewareAction::Continue => oxi_agent::BeforeToolCallResult {
                    block: false,
                    reason: None,
                },
                crate::middleware::MiddlewareAction::Block => oxi_agent::BeforeToolCallResult {
                    block: true,
                    reason: result.reason,
                },
                crate::middleware::MiddlewareAction::Terminate => {
                    terminate_flag.store(true, Ordering::SeqCst);
                    oxi_agent::BeforeToolCallResult {
                        block: true,
                        reason: result.reason,
                    }
                }
            }
        }
    })
        as std::boxed::Box<
            dyn Fn(&oxi_agent::BeforeToolCallContext) -> oxi_agent::BeforeToolCallResult
                + Send
                + Sync,
        >);

    let after_tool_call = Some(std::boxed::Box::new({
        let pipeline = Arc::clone(&pipeline);
        let agent_id = agent_id.clone();
        let terminate_flag = Arc::clone(&terminate_flag);

        move |ctx: &oxi_agent::AfterToolCallContext| -> oxi_agent::AfterToolCallResult {
            let mw_ctx = MiddlewareContext::new(
                MiddlewarePhase::AfterTool,
                &agent_id,
                MiddlewareData::AfterTool {
                    tool_name: ctx.tool_name.clone(),
                    params: serde_json::Value::Null,
                    result: ctx.result.clone(),
                },
            );

            let rt = tokio::runtime::Handle::current();
            let result = rt.block_on(pipeline.execute(&mw_ctx));

            if matches!(
                result.action,
                crate::middleware::MiddlewareAction::Terminate
            ) {
                terminate_flag.store(true, Ordering::SeqCst);
            }

            oxi_agent::AfterToolCallResult::default()
        }
    })
        as std::boxed::Box<
            dyn Fn(&oxi_agent::AfterToolCallContext) -> oxi_agent::AfterToolCallResult
                + Send
                + Sync,
        >);

    let should_stop_after_turn = Some(Arc::new({
        let flag = terminate_flag;

        move |_ctx: &oxi_agent::ShouldStopAfterTurnContext| -> bool { flag.load(Ordering::SeqCst) }
    })
        as Arc<dyn Fn(&oxi_agent::ShouldStopAfterTurnContext) -> bool + Send + Sync>);

    AgentHooks {
        before_tool_call,
        after_tool_call,
        should_stop_after_turn,
        get_steering_messages: None,
        get_follow_up_messages: None,
        tool_execution: oxi_agent::ToolExecutionMode::Parallel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_returns_valid_hooks() {
        let pipeline = Arc::new(MiddlewarePipeline::new());
        let terminate_flag = Arc::new(AtomicBool::new(false));

        let hooks = build_hooks(pipeline, "test-agent".into(), terminate_flag);

        assert!(hooks.before_tool_call.is_some());
        assert!(hooks.after_tool_call.is_some());
        assert!(hooks.should_stop_after_turn.is_some());
    }
}
