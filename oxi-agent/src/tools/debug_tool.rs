//! Debug tool — Debugger integration via DAP (Debug Adapter Protocol).
//!
//! Provides model-driven access to a debugger through the `xd://debug`
//! virtual device, which wraps DAP clients for common debug adapters
//! (`gdb`, `lldb-dap`, `debugpy`, `dlv`).
//!
//! # Status
//!
//! This is a **scaffold**. The tool validates the requested `action`
//! against the supported DAP set and returns a structured pointer
//! describing how the same operation would be issued via the
//! `xd://debug` device. A future change will route `execute` to wrap
//! the device directly, streaming results back as the tool return
//! value.
//!
//! For now, callers wanting real debugger control should use the host
//! harness's `xd://debug` device directly. The agent still observes a
//! regular `AgentToolResult` round-trip — the contract is preserved
//! while the proxy wiring is being built.
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode};

/// `debug` agent tool — DAP-backed debugger integration.
///
/// Surfaces DAP operations (`launch`, `attach`, breakpoint control,
/// stepping, inspection, termination) to the model over the standard
/// agent-loop contract. The long-term backing is the `xd://debug`
/// virtual device; today the tool is a validated scaffold.
pub struct DebugTool;

impl DebugTool {
    /// Canonical list of DAP actions supported by the tool.
    ///
    /// Order is chosen to mirror the natural debugging workflow:
    /// session lifecycle first, then breakpoint control, then execution
    /// control, then inspection, then teardown.
    pub const ACTIONS: &'static [&'static str] = &[
        "sessions",
        "launch",
        "attach",
        "set_breakpoint",
        "remove_breakpoint",
        "continue",
        "pause",
        "step_in",
        "step_over",
        "step_out",
        "threads",
        "stack_trace",
        "scopes",
        "variables",
        "evaluate",
        "terminate",
    ];

    /// Returns `true` if `action` is a recognised DAP action.
    pub fn is_supported_action(action: &str) -> bool {
        Self::ACTIONS.contains(&action)
    }

    /// Per-action guidance describing how to issue the same operation
    /// through the `xd://debug` virtual device right now.
    fn guidance(action: &str) -> &'static str {
        match action {
            "sessions" => "List active debug sessions. Write `{\"action\":\"sessions\"}` to `xd://debug`.",
            "launch" => "Start a new DAP session. Send `{\"action\":\"launch\",\"program\":\"<bin>\",\"args\":[…],\"adapter\":\"<gdb|lldb-dap|debugpy|dlv>\"}` to `xd://debug`.",
            "attach" => "Attach to a running process. Send `{\"action\":\"attach\",\"adapter\":\"<gdb|lldb-dap|debugpy|dlv>\"}` plus the adapter's attach parameters to `xd://debug`.",
            "set_breakpoint" => "Set a source breakpoint. Send `{\"action\":\"set_breakpoint\",\"file\":\"<path>\",\"line\":<n>,\"condition\":\"<expr>\"?}` to `xd://debug`.",
            "remove_breakpoint" => "Remove a previously set breakpoint. Send `{\"action\":\"remove_breakpoint\",\"file\":\"<path>\",\"line\":<n>}` to `xd://debug`.",
            "continue" => "Resume execution on a thread. Send `{\"action\":\"continue\",\"thread_id\":<n>}` to `xd://debug`.",
            "pause" => "Suspend a running thread. Send `{\"action\":\"pause\",\"thread_id\":<n>}` to `xd://debug`.",
            "step_in" => "Step into the current call. Send `{\"action\":\"step_in\",\"thread_id\":<n>}` to `xd://debug`.",
            "step_over" => "Step over the current call. Send `{\"action\":\"step_over\",\"thread_id\":<n>}` to `xd://debug`.",
            "step_out" => "Step out of the current frame. Send `{\"action\":\"step_out\",\"thread_id\":<n>}` to `xd://debug`.",
            "threads" => "List threads in the current session. Send `{\"action\":\"threads\"}` to `xd://debug`.",
            "stack_trace" => "Fetch the stack frames for a thread. Send `{\"action\":\"stack_trace\",\"thread_id\":<n>}` to `xd://debug`.",
            "scopes" => "Fetch the lexical scopes for a frame. Send `{\"action\":\"scopes\",\"frame_id\":<n>}` to `xd://debug`.",
            "variables" => "Fetch variables for a scope or variable reference. Send `{\"action\":\"variables\",\"frame_id\":<n>,\"variable_ref\":<n>?}` to `xd://debug`.",
            "evaluate" => "Evaluate an expression in a frame. Send `{\"action\":\"evaluate\",\"expression\":\"<expr>\",\"frame_id\":<n>}` to `xd://debug`.",
            "terminate" => "End the debug session. Send `{\"action\":\"terminate\"}` to `xd://debug`.",
            _ => "Unknown action.",
        }
    }
}

#[async_trait]
impl AgentTool for DebugTool {
    fn name(&self) -> &str {
        "debug"
    }

    fn label(&self) -> &str {
        "Debug (DAP)"
    }

    fn description(&self) -> &str {
        "Drive a debugger through the Debug Adapter Protocol (DAP). Supports launching and \
         attaching to programs, setting and removing breakpoints, stepping (in/over/out), \
         inspecting threads, stack frames, scopes, variables, and evaluating expressions, and \
         terminating the session.\n\n\
         Status: scaffold. Backed by the `xd://debug` virtual device in the host harness. Until \
         the proxy is wired up, each call validates the action and returns the equivalent \
         `xd://debug` request payload; the host harness will execute the corresponding action \
         directly."
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "attach",
                        "continue",
                        "launch",
                        "pause",
                        "stack_trace",
                        "step_in",
                        "step_over",
                        "step_out",
                        "terminate",
                        "threads",
                        "variables",
                        "evaluate",
                        "scopes",
                        "set_breakpoint",
                        "remove_breakpoint",
                        "sessions"
                    ],
                    "description": "DAP action to perform. Session lifecycle: `sessions`, `launch`, `attach`, `terminate`. Breakpoints: `set_breakpoint`, `remove_breakpoint`. Execution control: `continue`, `pause`, `step_in`, `step_over`, `step_out`. Inspection: `threads`, `stack_trace`, `scopes`, `variables`, `evaluate`."
                },
                "program": {
                    "type": "string",
                    "description": "Path to the debug target binary/script. Required for `launch`; for `attach` use the host/port fields exposed by the adapter instead."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments forwarded to the program under debug. Honoured by `launch`."
                },
                "adapter": {
                    "type": "string",
                    "enum": ["gdb", "lldb-dap", "debugpy", "dlv"],
                    "description": "DAP adapter to use. `gdb`/`lldb-dap` for native binaries, `debugpy` for Python, `dlv` for Go. Defaults to an adapter inferred from the program extension when unset."
                },
                "expression": {
                    "type": "string",
                    "description": "Expression to evaluate. Used by `evaluate` (and as the body of conditional breakpoints when supplied with `condition: false`)."
                },
                "file": {
                    "type": "string",
                    "description": "Source file path. Required for `set_breakpoint` / `remove_breakpoint`; optional elsewhere for context."
                },
                "line": {
                    "type": "number",
                    "description": "Source line (1-based). Required for `set_breakpoint` / `remove_breakpoint`."
                },
                "condition": {
                    "type": "string",
                    "description": "Breakpoint condition expression. When set, the breakpoint only halts when the expression evaluates to truthy. Used with `set_breakpoint`."
                },
                "thread_id": {
                    "type": "number",
                    "description": "Thread id (from `threads`). Required for `continue`, `pause`, `step_in`, `step_over`, `step_out`, and `stack_trace`."
                },
                "frame_id": {
                    "type": "number",
                    "description": "Stack frame id (from `stack_trace`). Required for `scopes`, `variables`, and `evaluate`."
                },
                "variable_ref": {
                    "type": "number",
                    "description": "Variable reference handle (from `variables`). Used to fetch nested members when omitted on the top scope."
                }
            },
            "required": ["action"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Drive a debugger via DAP")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        // A debug session is a single mutable resource shared across
        // the model: two parallel `step` / `set_breakpoint` calls
        // would race on the same DAP client. Force sequential execution.
        ToolExecutionMode::SequentialOnly
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // ── Validate action ─────────────────────────────────────────
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?
            .trim();

        if action.is_empty() {
            return Err("Parameter `action` must be a non-empty string".to_string());
        }

        if !Self::is_supported_action(action) {
            return Err(format!(
                "Unsupported debug action: `{}`. Supported actions: {}",
                action,
                Self::ACTIONS.join(", ")
            ));
        }

        // ── Action-specific required params ────────────────────────
        validate_action_params(action, &params)?;

        // ── Sessions: probe for adapter availability ───────────────
        if action == "sessions" {
            let adapters = [
                ("lldb-dap", "lldb-dap"),
                ("gdb", "gdb"),
                ("debugpy", "debugpy"),
                ("dlv", "dlv"),
            ];
            let mut available = Vec::new();
            let mut unavailable = Vec::new();
            for (name, binary) in &adapters {
                let found = tokio::process::Command::new("which")
                    .arg(binary)
                    .output()
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if found {
                    available.push(*name);
                } else {
                    unavailable.push(*name);
                }
            }

            let msg = if available.is_empty() {
                "No debug adapters found on PATH. Install one of: lldb-dap (via LLVM), \
                 gdb, debugpy (pip install debugpy), dlv (go install github.com/go-delve/delve/cmd/dlv@latest)."
                    .to_string()
            } else {
                format!(
                    "Available debug adapters: {}.\nUnavailable: {}.\n\n\
                     Use `action: \"launch\"` with `adapter` to start a session, or \
                     `action: \"attach\"` to connect to a running process.",
                    available.join(", "),
                    if unavailable.is_empty() {
                        "none".to_string()
                    } else {
                        unavailable.join(", ")
                    }
                )
            };

            return Ok(AgentToolResult::success(msg).with_metadata(json!({
                "available_adapters": available,
            })));
        }

        // ── Other actions: guidance ─────────────────────────────────
        let adapter = params
            .get("adapter")
            .and_then(|v| v.as_str())
            .unwrap_or("(inferred)");
        let program = params
            .get("program")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)");

        let guidance = Self::guidance(action);

        let message = format!(
            "Debug action `{action}` (adapter: {adapter}, program: {program}).\n\n\
             {guidance}\n\n\
             DAP routing via `xd://debug` is not proxied yet. Issue the request above through the\
             host harness directly to drive the debugger.",
            action = action,
            adapter = adapter,
            program = program,
            guidance = guidance,
        );

        Ok(AgentToolResult::success(message).with_metadata(json!({
            "action": action,
            "adapter": params.get("adapter").cloned().unwrap_or(Value::Null),
            "program": params.get("program").cloned().unwrap_or(Value::Null),
            "guidance": guidance,
        })))
    }
}

/// Validate action-specific required parameters against the supplied JSON.
///
/// Returns `Err` with a human-readable message when a required field is
/// missing or has the wrong JSON type.
fn validate_action_params(action: &str, params: &Value) -> Result<(), ToolError> {
    let require_str = |field: &str| -> Result<String, ToolError> {
        params
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Action `{action}` requires string parameter `{field}`"))
    };

    let require_u64 = |field: &str| -> Result<u64, ToolError> {
        params
            .get(field)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("Action `{action}` requires integer parameter `{field}`"))
    };

    match action {
        "launch" => {
            require_str("program")?;
            // `args` is optional but must be a string array if present.
            if let Some(args) = params.get("args") && !args.is_array() {
                return Err("Parameter `args` must be an array of strings".to_string());
            }
        }
        "attach" => {
            // Adapter is required for attach; program is not (attach is to a running process).
            require_str("adapter")?;
        }
        "set_breakpoint" | "remove_breakpoint" => {
            require_str("file")?;
            require_u64("line")?;
        }
        "stack_trace" => {
            require_u64("thread_id")?;
        }
        "scopes" | "variables" => {
            require_u64("frame_id")?;
        }
        "evaluate" => {
            require_str("expression")?;
            require_u64("frame_id")?;
        }
        "continue" | "pause" | "step_in" | "step_over" | "step_out" => {
            require_u64("thread_id")?;
        }
        "terminate" | "threads" | "sessions" => {
            // No required params beyond `action`.
        }
        _ => unreachable!("action was already validated against the supported set"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> ToolContext {
        ToolContext::default()
    }

    #[tokio::test]
    async fn rejects_missing_action() {
        let result = DebugTool.execute("c1", json!({}), None, &ctx()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("action"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn rejects_empty_action() {
        let result = DebugTool
            .execute("c2", json!({"action": "   "}), None, &ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_action() {
        let result = DebugTool
            .execute("c3", json!({"action": "rerun"}), None, &ctx())
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("rerun"), "unexpected error: {err}");
        assert!(err.contains("Supported actions"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn launch_requires_program() {
        let result = DebugTool
            .execute("c4", json!({"action": "launch"}), None, &ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("program"));
    }

    #[tokio::test]
    async fn launch_rejects_non_array_args() {
        let result = DebugTool
            .execute(
                "c5",
                json!({"action": "launch", "program": "./bin/app", "args": "not-an-array"}),
                None,
                &ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("args"));
    }

    #[tokio::test]
    async fn attach_requires_adapter() {
        let result = DebugTool
            .execute("c6", json!({"action": "attach"}), None, &ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("adapter"));
    }

    #[tokio::test]
    async fn set_breakpoint_requires_file_and_line() {
        let result = DebugTool
            .execute(
                "c7",
                json!({"action": "set_breakpoint", "file": "src/main.rs"}),
                None,
                &ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("line"));

        let result = DebugTool
            .execute(
                "c8",
                json!({"action": "set_breakpoint", "line": 42}),
                None,
                &ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("file"));
    }

    #[tokio::test]
    async fn stack_trace_requires_thread_id() {
        let result = DebugTool
            .execute("c9", json!({"action": "stack_trace"}), None, &ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("thread_id"));
    }

    #[tokio::test]
    async fn evaluate_requires_expression_and_frame() {
        let result = DebugTool
            .execute(
                "c10",
                json!({"action": "evaluate", "frame_id": 7}),
                None,
                &ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expression"));

        let result = DebugTool
            .execute(
                "c11",
                json!({"action": "evaluate", "expression": "x + 1"}),
                None,
                &ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("frame_id"));
    }

    #[tokio::test]
    async fn launch_with_program_succeeds() {
        let result = DebugTool
            .execute(
                "c12",
                json!({
                    "action": "launch",
                    "program": "./target/debug/app",
                    "args": ["--flag", "value"],
                    "adapter": "lldb-dap"
                }),
                None,
                &ctx(),
            )
            .await
            .expect("launch should succeed");
        assert!(result.success);
        assert!(result.output.contains("`launch`"));
        assert!(result.output.contains("lldb-dap"));
        assert!(result.output.contains("./target/debug/app"));
        // Scaffold does NOT execute — it just acknowledges the request.
        assert!(result.output.contains("xd://debug"));
    }

    #[tokio::test]
    async fn set_breakpoint_with_condition_succeeds() {
        let result = DebugTool
            .execute(
                "c13",
                json!({
                    "action": "set_breakpoint",
                    "file": "src/main.rs",
                    "line": 42,
                    "condition": "i == 10"
                }),
                None,
                &ctx(),
            )
            .await
            .expect("set_breakpoint should succeed");
        assert!(result.success);
        assert!(result.output.contains("set_breakpoint"));
    }

    #[tokio::test]
    async fn sessions_action_succeeds_without_extras() {
        let result = DebugTool
            .execute("c14", json!({"action": "sessions"}), None, &ctx())
            .await
            .expect("sessions should succeed");
        assert!(result.success);
        // Sessions probes available adapters — output mentions adapters.
        assert!(
            result.output.contains("adapter")
                || result.output.contains("debug"),
            "unexpected output: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn guidance_text_matches_per_action() {
        let result = DebugTool
            .execute(
                "c15",
                json!({"action": "evaluate", "expression": "x + 1", "frame_id": 7}),
                None,
                &ctx(),
            )
            .await
            .expect("evaluate should succeed");
        assert!(result.output.contains("xd://debug"));
        assert!(result.output.contains("evaluate"));
    }

    #[tokio::test]
    async fn metadata_carries_scaffold_flag_and_action() {
        let result = DebugTool
            .execute(
                "c16",
                json!({
                    "action": "set_breakpoint",
                    "file": "main.rs",
                    "line": 10
                }),
                None,
                &ctx(),
            )
            .await
            .expect("set_breakpoint should succeed");
        let meta = result.metadata.expect("metadata should be set");
        assert_eq!(meta["action"], json!("set_breakpoint"));
        assert!(meta["guidance"].as_str().unwrap().contains("xd://debug"));
    }

    #[test]
    fn schema_lists_required_action_and_enum() {
        let schema = DebugTool.parameters_schema();
        let required = schema.get("required").and_then(|v| v.as_array());
        assert_eq!(
            required.and_then(|r| r.first()).and_then(|v| v.as_str()),
            Some("action"),
            "schema must mark `action` as required"
        );

        let actions: Vec<&str> = schema
            .pointer("/properties/action/enum")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        for expected in [
            "attach",
            "continue",
            "launch",
            "pause",
            "stack_trace",
            "step_in",
            "step_over",
            "step_out",
            "terminate",
            "threads",
            "variables",
            "evaluate",
            "scopes",
            "set_breakpoint",
            "remove_breakpoint",
            "sessions",
        ] {
            assert!(
                actions.contains(&expected),
                "schema enum is missing `{expected}`"
            );
        }
    }

    #[test]
    fn schema_adapters_match_supported_set() {
        let schema = DebugTool.parameters_schema();
        let adapters: Vec<&str> = schema
            .pointer("/properties/adapter/enum")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        for expected in ["gdb", "lldb-dap", "debugpy", "dlv"] {
            assert!(
                adapters.contains(&expected),
                "adapter enum missing `{expected}`"
            );
        }
    }

    #[test]
    fn schema_declares_dap_integer_properties() {
        // The validator requires `thread_id` and `frame_id`, and the
        // guidance text references `variable_ref` — all three must be
        // declared in the schema so the model can send them.
        let schema = DebugTool.parameters_schema();
        for field in ["thread_id", "frame_id", "variable_ref"] {
            let ty = schema
                .pointer(&format!("/properties/{field}/type"))
                .and_then(|v| v.as_str());
            assert_eq!(
                ty,
                Some("number"),
                "schema must declare `{field}` as a number"
            );
        }
    }

    #[test]
    fn is_supported_action_matches_constant() {
        for action in DebugTool::ACTIONS {
            assert!(DebugTool::is_supported_action(action));
        }
        assert!(!DebugTool::is_supported_action("fly"));
        assert!(!DebugTool::is_supported_action(""));
    }

    #[test]
    fn intent_and_mode_are_pinned() {
        assert_eq!(DebugTool.intent(), Some("Drive a debugger via DAP"));
        assert!(matches!(
            DebugTool.execution_mode(),
            ToolExecutionMode::SequentialOnly
        ));
    }
}
