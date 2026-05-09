//! WASM tool wrapper — stub

#![allow(dead_code)]
#![allow(unused_imports)]

use async_trait::async_trait;
use serde_json::Value;
use oxi_agent::{AgentTool, AgentToolResult};

pub struct WasmTool;

impl WasmTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl AgentTool for WasmTool {
    fn name(&self) -> &str { "wasm_stub" }
    fn label(&self) -> &str { "WASM tool (disabled)" }
    fn description(&self) -> &str { "WASM extensions are disabled" }
    fn parameters_schema(&self) -> Value { serde_json::json!({}) }
    async fn execute(&self, _id: &str, _params: Value, _signal: Option<tokio::sync::oneshot::Receiver<()>>) -> Result<AgentToolResult, String> {
        Err("WASM extensions disabled".to_string())
    }
}
