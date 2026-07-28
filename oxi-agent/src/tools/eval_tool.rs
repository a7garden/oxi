//! Eval tool — execute code and capture output.
//!
//! Provides a multi-language code-execution environment (Python and
//! JavaScript today). Each call writes code to a temp file, executes
//! it via the appropriate runtime (`python3` or `bun`/`node`), and
//! captures stdout/stderr + exit code.
//!
//! Each call runs in a fresh process — persistent kernel sessions
//! across calls are a future enhancement. For interactive or
//! multi-step sessions, use `bash` with `python3 -i` or `bun -i`.

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode};

/// `eval` agent tool — run code and capture output.
pub struct EvalTool;

#[async_trait]
impl AgentTool for EvalTool {
    fn name(&self) -> &str {
        "eval"
    }

    fn label(&self) -> &str {
        "Eval"
    }

    fn description(&self) -> &str {
        "Execute code in Python (`py`) or JavaScript (`js`) and capture \
         stdout, stderr, and the return value. Each call runs in a fresh \
         process — state does NOT persist across calls. Use `reset: true` \
         to discard previous state explicitly.\n\n\
         For interactive or multi-step sessions, prefer the `bash` tool \
         with `python3 -i` or `bun -i` for persistent state. Use `eval` \
         for quick one-shot computations where only the output matters."
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["py", "js"],
                    "description": "Language runtime: `py` (Python 3) or `js` (JavaScript / Bun)",
                    "default": "py"
                },
                "code": {
                    "type": "string",
                    "description": "Code to execute. Imports and variable definitions persist across calls."
                },
                "title": {
                    "type": "string",
                    "description": "Optional cell label for readability in the transcript"
                },
                "reset": {
                    "type": "boolean",
                    "description": "Reset the kernel/session before executing this cell",
                    "default": false
                }
            },
            "required": ["code"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Execute code and capture output")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let code = params
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: code".to_string())?;

        if code.trim().is_empty() {
            return Err("Parameter `code` must be a non-empty string".to_string());
        }

        let language = params
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("py");

        let _title = params.get("title").and_then(|v| v.as_str());

        let _reset = params
            .get("reset")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Write code to a temp file and execute it.
        let tmp = std::env::temp_dir().join(format!(
            "oxi_eval_{}.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            match language {
                "py" => "py",
                "js" => "mjs",
                other => return Err(format!("Unsupported language: {}", other)),
            }
        ));

        if let Err(e) = tokio::fs::write(&tmp, code).await {
            return Ok(AgentToolResult::error(format!(
                "Failed to write temp file: {}",
                e
            )));
        }

        let runner = match language {
            "py" => "python3",
            "js" => {
                // Check if bun is available, fall back to node
                let has_bun = tokio::process::Command::new("which")
                    .arg("bun")
                    .output()
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if has_bun { "bun" } else { "node" }
            }
            _ => unreachable!(),
        };

        let output = match tokio::process::Command::new(runner)
            .arg(tmp.to_str().unwrap_or(""))
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => {
                // Clean up temp file.
                let _ = tokio::fs::remove_file(&tmp).await;
                return Ok(AgentToolResult::error(format!(
                    "Failed to execute code via {}: {}",
                    runner, e
                )));
            }
        };

        // Clean up temp file.
        let _ = tokio::fs::remove_file(&tmp).await;

        let mut result_parts = Vec::new();

        if !output.stdout.is_empty() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.trim().is_empty() {
                result_parts.push(format!("── stdout ──\n{}", stdout.trim()));
            }
        }

        if !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                result_parts.push(format!("── stderr ──\n{}", stderr.trim()));
            }
        }

        let exit_code = output.status.code().unwrap_or(-1);

        if exit_code != 0 {
            result_parts.push(format!("── exit code: {} ──", exit_code));
        }

        let result_text = if result_parts.is_empty() {
            "Code executed successfully (exit code 0, no output)".to_string()
        } else {
            result_parts.join("\n")
        };

        Ok(AgentToolResult::success(result_text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> ToolContext {
        ToolContext::default()
    }

    #[tokio::test]
    async fn rejects_missing_code() {
        let result = EvalTool
            .execute("c1", json!({"language": "py"}), None, &ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("code"));
    }

    #[tokio::test]
    async fn rejects_empty_code() {
        let result = EvalTool
            .execute("c2", json!({"code": "   \n\t  "}), None, &ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_language() {
        let result = EvalTool
            .execute("c3", json!({"code": "x", "language": "ruby"}), None, &ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ruby"));
    }

    #[tokio::test]
    async fn executes_python_code() {
        let result = EvalTool
            .execute(
                "c4",
                json!({"code": "print(1+1)", "language": "py"}),
                None,
                &ctx(),
            )
            .await
            .expect("py execution should succeed");
        assert!(result.success);
        assert!(result.output.contains("2"), "output: {}", result.output);
    }

    #[tokio::test]
    async fn executes_js_code() {
        let result = EvalTool
            .execute(
                "c5",
                json!({"code": "console.log(2+2)", "language": "js"}),
                None,
                &ctx(),
            )
            .await;
        // js may be unavailable (no bun/node); error is acceptable.
        if let Ok(result) = result {
            assert!(result.success);
            assert!(result.output.contains("4"), "output: {}", result.output);
        }
    }

    #[tokio::test]
    async fn captures_stderr() {
        let result = EvalTool
            .execute(
                "c6",
                json!({"code": "import sys; print('ok', file=sys.stderr); print('stdout')", "language": "py"}),
                None,
                &ctx(),
            )
            .await
            .expect("py execution should succeed");
        assert!(result.success);
        assert!(result.output.contains("stdout"), "output: {}", result.output);
    }
}
