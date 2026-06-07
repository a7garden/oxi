//! Demonstrates how to implement a custom tool for the agent loop.
//!
//! Run with: cargo run -p oxi-agent --example custom_tool

use async_trait::async_trait;
use oxi_agent::{AgentTool, AgentToolResult, ToolContext};
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// A simple custom tool that echoes back the input parameters.
struct EchoTool;

#[async_trait]
impl AgentTool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn label(&self) -> &str {
        "Echo"
    }

    fn description(&self) -> &str {
        "Echoes back the input parameters as JSON."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to echo back"
                }
            },
            "required": ["message"]
        })
    }

    fn essential(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, String> {
        let msg = params
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("no message");
        Ok(AgentToolResult::success(json!({"echo": msg}).to_string()))
    }
}

fn main() {
    println!("Custom Tool Example");
    println!("===================");
    println!();
    println!("This example shows how to create a custom AgentTool.");
    println!();
    println!("The EchoTool returns its input parameters when called.");
    println!();
    println!("To use it with a real provider:");
    println!("  1. Create a Provider via oxi_ai::create_builtin_provider");
    println!("  2. Build an Agent with the provider, model, and tool registry");
    println!("  3. Register: agent.tools().register_arc(Arc::new(echo_tool))");
    println!("  4. Call agent.run(context) to start the agent loop");
    println!();
    println!("Tool name: {}", EchoTool.name());
    println!("Tool description: {}", EchoTool.description());

    // Demonstrate a direct tool call
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        let tool = EchoTool;
        tool.execute(
            "test-call-1",
            json!({"message": "Hello from oxi!"}),
            None,
            &ToolContext::default(),
        )
        .await
    });

    match result {
        Ok(r) => println!("Tool result: {}", r.output),
        Err(e) => println!("Tool error: {e}"),
    }
}
