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
use std::sync::Arc;
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
            "sessions" => {
                "List active debug sessions. Write `{\"action\":\"sessions\"}` to `xd://debug`."
            }
            "launch" => {
                "Start a new DAP session. Send `{\"action\":\"launch\",\"program\":\"<bin>\",\"args\":[…],\"adapter\":\"<gdb|lldb-dap|debugpy|dlv>\"}` to `xd://debug`."
            }
            "attach" => {
                "Attach to a running process. Send `{\"action\":\"attach\",\"adapter\":\"<gdb|lldb-dap|debugpy|dlv>\"}` plus the adapter's attach parameters to `xd://debug`."
            }
            "set_breakpoint" => {
                "Set a source breakpoint. Send `{\"action\":\"set_breakpoint\",\"file\":\"<path>\",\"line\":<n>,\"condition\":\"<expr>\"?}` to `xd://debug`."
            }
            "remove_breakpoint" => {
                "Remove a previously set breakpoint. Send `{\"action\":\"remove_breakpoint\",\"file\":\"<path>\",\"line\":<n>}` to `xd://debug`."
            }
            "continue" => {
                "Resume execution on a thread. Send `{\"action\":\"continue\",\"thread_id\":<n>}` to `xd://debug`."
            }
            "pause" => {
                "Suspend a running thread. Send `{\"action\":\"pause\",\"thread_id\":<n>}` to `xd://debug`."
            }
            "step_in" => {
                "Step into the current call. Send `{\"action\":\"step_in\",\"thread_id\":<n>}` to `xd://debug`."
            }
            "step_over" => {
                "Step over the current call. Send `{\"action\":\"step_over\",\"thread_id\":<n>}` to `xd://debug`."
            }
            "step_out" => {
                "Step out of the current frame. Send `{\"action\":\"step_out\",\"thread_id\":<n>}` to `xd://debug`."
            }
            "threads" => {
                "List threads in the current session. Send `{\"action\":\"threads\"}` to `xd://debug`."
            }
            "stack_trace" => {
                "Fetch the stack frames for a thread. Send `{\"action\":\"stack_trace\",\"thread_id\":<n>}` to `xd://debug`."
            }
            "scopes" => {
                "Fetch the lexical scopes for a frame. Send `{\"action\":\"scopes\",\"frame_id\":<n>}` to `xd://debug`."
            }
            "variables" => {
                "Fetch variables for a scope or variable reference. Send `{\"action\":\"variables\",\"frame_id\":<n>,\"variable_ref\":<n>?}` to `xd://debug`."
            }
            "evaluate" => {
                "Evaluate an expression in a frame. Send `{\"action\":\"evaluate\",\"expression\":\"<expr>\",\"frame_id\":<n>}` to `xd://debug`."
            }
            "terminate" => {
                "End the debug session. Send `{\"action\":\"terminate\"}` to `xd://debug`."
            }
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
            if let Some(args) = params.get("args")
                && !args.is_array()
            {
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
        // Unreachable by construction: `action` is validated against the
        // supported set before this match runs. No-op instead of panicking
        // so a future supported action added to the validator but forgotten
        // here degrades to a no-op rather than crashing the agent.
        _ => {}
    }

    Ok(())
}

/// `debug` agent tool routed through a real [`DebugService`](crate::runtime::DebugService).
///
/// Same action surface as the [`DebugTool`] scaffold, but every operation
/// drives an actual DAP session: `launch`/`attach` start adapter processes,
/// breakpoint/stepping/inspection actions issue DAP requests, and
/// `terminate` tears the session down. The pack picks this variant when the
/// host provides a `DebugService` and falls back to the scaffold otherwise.
pub struct DapDebugTool {
    service: Arc<dyn crate::runtime::DebugService>,
    sessions: std::sync::Mutex<Vec<String>>,
}

/// Adapter enum value → DAP adapter command line.
fn adapter_command(adapter: &str) -> Vec<String> {
    match adapter {
        "gdb" => vec!["gdb".into(), "--interpreter=dap".into(), "-q".into()],
        "lldb-dap" => vec!["lldb-dap".into()],
        "debugpy" => vec!["python3".into(), "-m".into(), "debugpy.adapter".into()],
        "dlv" => vec!["dlv".into(), "dap".into()],
        other => vec![other.to_string()],
    }
}

/// Map a tool action to the DAP request command + arguments built from
/// the tool parameters. Returns `None` for actions that are not plain
/// DAP requests (lifecycle actions are handled by the service).
fn dap_request(action: &str, params: &Value) -> Option<(String, Value)> {
    let thread_id = || params.get("thread_id").cloned().unwrap_or(json!(0));
    let frame_id = || params.get("frame_id").cloned().unwrap_or(json!(0));
    match action {
        "set_breakpoint" => {
            let mut bp = json!({ "line": params.get("line").cloned().unwrap_or(json!(0)) });
            if let Some(condition) = params.get("condition").filter(|v| !v.is_null()) {
                bp["condition"] = condition.clone();
            }
            Some((
                "setBreakpoints".to_string(),
                json!({
                    "source": { "path": params.get("file").cloned().unwrap_or(json!("")) },
                    "breakpoints": [bp],
                }),
            ))
        }
        // DAP removes breakpoints by re-setting the file's breakpoint list.
        "remove_breakpoint" => Some((
            "setBreakpoints".to_string(),
            json!({
                "source": { "path": params.get("file").cloned().unwrap_or(json!("")) },
                "breakpoints": [],
            }),
        )),
        "continue" => Some(("continue".to_string(), json!({ "threadId": thread_id() }))),
        "pause" => Some(("pause".to_string(), json!({ "threadId": thread_id() }))),
        "step_in" => Some(("stepIn".to_string(), json!({ "threadId": thread_id() }))),
        "step_over" => Some(("next".to_string(), json!({ "threadId": thread_id() }))),
        "step_out" => Some(("stepOut".to_string(), json!({ "threadId": thread_id() }))),
        "threads" => Some(("threads".to_string(), json!({}))),
        "stack_trace" => Some(("stackTrace".to_string(), json!({ "threadId": thread_id() }))),
        "scopes" => Some(("scopes".to_string(), json!({ "frameId": frame_id() }))),
        "variables" => {
            // DAP walks variables by reference handle; the frame id is the
            // entry handle when the model has not obtained one from scopes.
            let reference = params
                .get("variable_ref")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(frame_id);
            Some((
                "variables".to_string(),
                json!({ "variablesReference": reference }),
            ))
        }
        "evaluate" => Some((
            "evaluate".to_string(),
            json!({
                "expression": params.get("expression").cloned().unwrap_or(json!("")),
                "frameId": frame_id(),
            }),
        )),
        _ => None,
    }
}

impl DapDebugTool {
    /// Route operations through `service`.
    pub fn new(service: Arc<dyn crate::runtime::DebugService>) -> Self {
        Self {
            service,
            sessions: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn track_session(&self, id: String) {
        // SAFETY: a poisoned lock means the previous holder panicked while
        // holding it — a real bug that must surface, not be swallowed.
        #[allow(clippy::expect_used)]
        self.sessions
            .lock()
            .expect("session list poisoned")
            .push(id);
    }

    fn forget_session(&self, id: &str) {
        #[allow(clippy::expect_used)]
        self.sessions
            .lock()
            .expect("session list poisoned")
            .retain(|s| s != id);
    }

    fn listed_sessions(&self) -> Vec<String> {
        #[allow(clippy::expect_used)]
        self.sessions.lock().expect("session list poisoned").clone()
    }

    /// The session a non-lifecycle action targets: explicit `session`
    /// param, else the most recently started session.
    fn target_session(&self, explicit: Option<&str>) -> Result<String, ToolError> {
        if let Some(id) = explicit {
            return Ok(id.to_string());
        }
        #[allow(clippy::expect_used)]
        self.sessions
            .lock()
            .expect("session list poisoned")
            .last()
            .cloned()
            .ok_or_else(|| "No active debug session — call `launch` or `attach` first".to_string())
    }
}

#[async_trait]
impl AgentTool for DapDebugTool {
    fn name(&self) -> &str {
        "debug"
    }

    fn label(&self) -> &str {
        "Debug (DAP, routed)"
    }

    fn essential(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Drive a real debugger through the Debug Adapter Protocol (DAP). \
         `launch`/`attach` start adapter sessions; breakpoint, stepping, \
         inspection, and evaluation actions issue live DAP requests and \
         return the adapter's JSON responses; `terminate` ends the session. \
         Pass the `session` id returned by `launch`/`attach` to target a \
         specific session; the most recent one is used by default."
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
                "session": {
                    "type": "string",
                    "description": "Session id returned by `launch`/`attach`. Defaults to the most recent session."
                },
                "program": {
                    "type": "string",
                    "description": "Path to the debug target binary/script. Required for `launch`."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments forwarded to the program under debug. Honoured by `launch`."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the debug target. Honoured by `launch`/`attach` when the adapter supports it."
                },
                "adapter": {
                    "type": "string",
                    "enum": ["gdb", "lldb-dap", "debugpy", "dlv"],
                    "description": "DAP adapter to use. `gdb`/`lldb-dap` for native binaries, `debugpy` for Python, `dlv` for Go. Defaults to an adapter inferred from the program extension when unset."
                },
                "adapter_command": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Explicit adapter command line (argv) overriding the `adapter` preset — for custom or bundled adapters."
                },
                "expression": {
                    "type": "string",
                    "description": "Expression to evaluate. Used by `evaluate`."
                },
                "file": {
                    "type": "string",
                    "description": "Source file path. Required for `set_breakpoint` / `remove_breakpoint`."
                },
                "line": {
                    "type": "number",
                    "description": "Source line (1-based). Required for `set_breakpoint` / `remove_breakpoint`."
                },
                "condition": {
                    "type": "string",
                    "description": "Breakpoint condition expression. Used with `set_breakpoint`."
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
        Some("Drive a real debugger via DAP")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        // A debug session is a single mutable resource shared across the
        // model: two parallel step / set_breakpoint calls would race on the
        // same DAP client.
        ToolExecutionMode::SequentialOnly
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?
            .trim();
        if action.is_empty() {
            return Err("Parameter `action` must be a non-empty string".to_string());
        }
        if !DebugTool::is_supported_action(action) {
            return Err(format!(
                "Unsupported debug action: `{}`. Supported actions: {}",
                action,
                DebugTool::ACTIONS.join(", ")
            ));
        }
        validate_action_params(action, &params)?;
        let explicit_session = params.get("session").and_then(|v| v.as_str());

        // ── Session lifecycle ──────────────────────────────────────
        match action {
            "sessions" => {
                let sessions = self.listed_sessions();
                let text = if sessions.is_empty() {
                    "No active debug sessions. Use `launch` or `attach` to start one.".to_string()
                } else {
                    format!(
                        "Active debug sessions (most recent last):\n{}",
                        sessions
                            .iter()
                            .rev()
                            .map(|s| format!("- {s}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                return Ok(AgentToolResult::success(text).with_metadata(json!({
                    "sessions": sessions,
                })));
            }
            "launch" | "attach" => {
                let adapter_argv: Vec<String> = params
                    .get("adapter_command")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        adapter_command(
                            params
                                .get("adapter")
                                .and_then(|v| v.as_str())
                                .unwrap_or("lldb-dap"),
                        )
                    });
                let adapter_label = params
                    .get("adapter")
                    .and_then(|v| v.as_str())
                    .unwrap_or("custom");
                let mut config = json!({
                    "type": adapter_label,
                    "request": action,
                    "adapter": adapter_argv,
                });
                if let Some(obj) = config.as_object_mut() {
                    for (key, value) in params.as_object().into_iter().flatten() {
                        if key != "action"
                            && key != "adapter"
                            && key != "adapter_command"
                            && key != "session"
                        {
                            obj.insert(key.clone(), value.clone());
                        }
                    }
                }
                let session = self
                    .service
                    .start(&config)
                    .await
                    .map_err(|e| -> ToolError { format!("DAP start failed: {e}") })?;
                self.track_session(session.clone());
                let text = format!(
                    "Debug session started ({action}, adapter: {adapter_label}).\nsession: {session}\n\
                     The adapter reports `stopped` once the target is ready; use `threads`, \
                     `set_breakpoint`, `continue`, `stack_trace`, `variables`, `evaluate`, \
                     stepping actions, and `terminate` against this session."
                );
                return Ok(AgentToolResult::success(text).with_metadata(json!({
                    "session": session,
                    "request": action,
                })));
            }
            "terminate" => {
                let id = self.target_session(explicit_session)?;
                self.service
                    .terminate(&id)
                    .await
                    .map_err(|e| -> ToolError { format!("DAP terminate failed: {e}") })?;
                self.forget_session(&id);
                return Ok(AgentToolResult::success(format!(
                    "Debug session terminated: {id}"
                )));
            }
            _ => {}
        }

        // ── Plain DAP requests ─────────────────────────────────────
        let Some((command, args)) = dap_request(action, &params) else {
            return Err(format!("Action `{action}` is not mapped to a DAP request"));
        };
        let target = self.target_session(explicit_session)?;
        let body = self
            .service
            .request(&target, &command, &args)
            .await
            .map_err(|e| -> ToolError { format!("DAP {command} failed: {e}") })?;
        let pretty = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
        Ok(
            AgentToolResult::success(format!("{action} → {command}\n{pretty}"))
                .with_metadata(json!({ "session": target, "command": command, "body": body })),
        )
    }
}
