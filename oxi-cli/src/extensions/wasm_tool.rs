//! AgentTool wrapper for WASM extension tools.
//!
//! Each tool exported by a WASM extension is wrapped in a `WasmTool` that
//! implements the `AgentTool` trait. Tool execution is delegated to the
//! WASM extension via `WasmExtensionManager::execute_tool`.

use anyhow::Result;
use async_trait::async_trait;
use oxi_agent::{AgentTool, AgentToolResult, ToolError};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::oneshot;

use super::wasm::WasmExtensionManager;

/// An `AgentTool` backed by a WASM extension.
pub struct WasmTool {
    manager: Arc<WasmExtensionManager>,
    tool_name: String,
    description: String,
    schema: Value,
}

impl WasmTool {
    /// Create a new WasmTool.
    pub fn new(
        manager: Arc<WasmExtensionManager>,
        tool_name: String,
        description: String,
        schema: Value,
    ) -> Self {
        Self {
            manager,
            tool_name,
            description,
            schema,
        }
    }
}

#[async_trait]
impl AgentTool for WasmTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        // Prefix with "WASM" to distinguish from built-in tools
        &self.description
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let manager = self.manager.clone();
        let tool_name = self.tool_name.clone();

        // Extism calls are synchronous — run on a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            manager.execute_tool(&tool_name, params)
        })
        .await
        .map_err(|e| format!("WASM execution panicked: {}", e))?;

        match result {
            Ok(value) => {
                let success = value.get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let output = value.get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no output)")
                    .to_string();

                if success {
                    let mut tool_result = AgentToolResult::success(output);
                    if let Some(meta) = value.get("metadata").cloned() {
                        tool_result = tool_result.with_metadata(meta);
                    }
                    Ok(tool_result)
                } else {
                    Ok(AgentToolResult::error(output))
                }
            }
            Err(e) => Ok(AgentToolResult::error(format!(
                "WASM tool '{}' error: {}",
                self.tool_name, e
            ))),
        }
    }
}
