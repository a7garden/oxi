//! LSP tool — Language Server Protocol operations.
//!
//! Delegates to `LspProvider` capability (injected via `ToolContext.lsp`).
//! When `None`, returns an error indicating LSP is not configured.
//! The actual LSP client lives in the (future) `oxi-lsp` crate.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{AgentTool, AgentToolResult, LspAction, ToolContext, ToolError};

/// `lsp` agent tool — IDE-grade code intelligence.
///
/// Requires `LspProvider` capability. Supports diagnostics, definition,
/// references, hover, rename, and symbols.
pub struct LspTool;

#[async_trait]
impl AgentTool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn label(&self) -> &str {
        "LSP"
    }

    fn essential(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Language Server Protocol operations: diagnostics, definition, references, \
         hover, rename, symbols. Requires LSP enabled in settings."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "diagnostics", "definition", "references", "hover",
                        "rename", "symbols", "status",
                        "code_actions", "type_definition", "implementation",
                        "file_rename"
                    ]
                },
                "file": {"type": "string", "description": "File path"},
                "old_path": {"type": "string", "description": "Existing path for file_rename"},
                "new_path": {"type": "string", "description": "Target path for file_rename"},
                "line": {"type": "integer", "description": "1-based line number"},
                "symbol": {"type": "string", "description": "Symbol name (optional, for disambiguation)"},
                "new_name": {"type": "string", "description": "New name for rename"},
                "apply": {"type": "boolean", "description": "Apply changes (vs preview)"},
                "query": {"type": "string", "description": "Search query for symbols"}
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let provider = ctx
            .lsp
            .as_ref()
            .ok_or("LSP not configured. Enable LSP in settings or install language servers.")?;

        let action = parse_lsp_action(&params)?;

        let result = provider.execute_action(&action).await?;

        Ok(AgentToolResult::success(result))
    }
}

/// Parse JSON params into an `LspAction`.
fn parse_lsp_action(params: &Value) -> Result<LspAction, ToolError> {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: action")?;

    let file = params
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let line = params
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(1);

    let symbol = params
        .get("symbol")
        .and_then(|v| v.as_str())
        .map(String::from);

    match action {
        "diagnostics" => Ok(LspAction::Diagnostics { file }),
        "definition" => Ok(LspAction::Definition { file, line, symbol }),
        "references" => Ok(LspAction::References { file, line, symbol }),
        "hover" => Ok(LspAction::Hover { file, line, symbol }),
        "rename" => {
            let new_name = params
                .get("new_name")
                .and_then(|v| v.as_str())
                .ok_or("rename requires 'new_name'")?
                .to_string();
            let sym = symbol.clone().unwrap_or_default();
            let apply = params
                .get("apply")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(LspAction::Rename {
                file,
                line,
                symbol: sym,
                new_name,
                apply,
            })
        }
        "symbols" => {
            let query = params
                .get("query")
                .and_then(|v| v.as_str())
                .map(String::from);
            Ok(LspAction::Symbols { file, query })
        }
        "status" => Ok(LspAction::Status),
        "code_actions" => Ok(LspAction::CodeActions { file, line, symbol }),
        "type_definition" => Ok(LspAction::TypeDefinition { file, line, symbol }),
        "implementation" => Ok(LspAction::Implementation { file, line, symbol }),
        "file_rename" => {
            let old_path = params
                .get("old_path")
                .and_then(|v| v.as_str())
                .ok_or("file_rename requires 'old_path'")?
                .to_string();
            let new_path = params
                .get("new_path")
                .and_then(|v| v.as_str())
                .ok_or("file_rename requires 'new_path'")?
                .to_string();
            let apply = params
                .get("apply")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(LspAction::FileRename {
                old_path,
                new_path,
                apply,
            })
        }
        other => Err(format!("Unknown LSP action: '{}'", other)),
    }
}
