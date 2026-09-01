//! Persistent-session bash tool — the `coding-omp-v1` routed `bash`
//! implementation backed by [`ShellSession`](crate::runtime::ShellSession).
//!
//! Schema-compatible with the legacy
//! [`BashTool`](super::bash::BashTool) (`command`/`timeout`/`cwd`/`env`),
//! but every call executes in one long-lived bash session: the working
//! directory and exported environment persist across calls, matching the
//! OMP compatibility target. The session init merges stderr into stdout
//! and traps SIGINT so a cancel aborts only the foreground command
//! (exit code 130) while the session survives.
//!
//! The pack picks this variant when the host provides a `ShellSession`
//! service and falls back to the legacy per-invocation tool otherwise.

use super::bash::{BashTool, is_dangerous_command, validate_cwd};
use super::truncate;
use super::{
    AgentTool, AgentToolResult, ProgressCallback, ToolContext, ToolError, ToolExecutionMode,
};
use crate::runtime::ShellSession;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

/// Quote an arbitrary string as a single-quoted POSIX shell word.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `bash` agent tool routed through a persistent [`ShellSession`].
pub struct SessionBashTool {
    session: Arc<dyn ShellSession>,
    progress_callback: Arc<std::sync::Mutex<Option<ProgressCallback>>>,
}

impl SessionBashTool {
    /// Route executions through `session`.
    pub fn new(session: Arc<dyn ShellSession>) -> Self {
        Self {
            session,
            progress_callback: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn report(&self, message: String) {
        // SAFETY: a poisoned lock means the previous holder panicked while
        // holding it — a real bug that must surface, not be swallowed.
        #[allow(clippy::expect_used)]
        if let Some(cb) = self
            .progress_callback
            .lock()
            .expect("progress callback lock poisoned")
            .as_ref()
        {
            cb(message);
        }
    }
}

#[async_trait]
impl AgentTool for SessionBashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn label(&self) -> &str {
        "Bash (persistent session)"
    }

    fn essential(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        "Execute a bash command in a persistent shell session. The working \
         directory and exported environment persist across calls (OMP-style \
         session semantics). Returns combined stdout/stderr. Output is \
         truncated to 2000 lines or 50KB. Set timeout to limit execution \
         time; cancellation aborts only the running command (exit code 130) \
         and the session stays alive."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)",
                    "default": 120
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command; the change persists for subsequent calls (optional)"
                },
                "env": {
                    "type": "object",
                    "description": "Environment variables exported for this and subsequent calls (optional)",
                    "additionalProperties": {
                        "type": "string"
                    }
                }
            },
            "required": ["command"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Execute bash in a persistent session")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        // One shell session = one mutable terminal; parallel calls would
        // interleave inside the same interpreter.
        ToolExecutionMode::SequentialOnly
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?
            .to_string();
        if command.trim().is_empty() {
            return Err("Parameter `command` must be a non-empty string".to_string());
        }

        if std::env::var_os("OXICODE_STRICT_BASH").as_deref() == Some(std::ffi::OsStr::new("1"))
            && let Some(reason) = is_dangerous_command(&command)
        {
            return Err(format!(
                "OXICODE_STRICT_BASH=1 blocked dangerous command: {reason}"
            ));
        }

        let timeout_secs = params
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);
        let cwd = params.get("cwd").and_then(|v| v.as_str());
        let env = params.get("env").and_then(|v| v.as_object());

        // Compose the session command: persistent exports first, then a
        // persistent `cd`, then the raw command. Prefixes are shell-quoted.
        let mut prefixes = String::new();
        if let Some(env) = env {
            for (key, value) in env {
                let Some(value) = value.as_str() else {
                    return Err(format!("Parameter `env.{key}` must be a string"));
                };
                prefixes.push_str(&format!("export {}={}\n", key, sh_quote(value)));
            }
        }
        if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
            // Same workspace containment rule as the legacy tool.
            let _ = validate_cwd(cwd, Some(ctx.root()))?;
            prefixes.push_str(&format!("cd {}\n", sh_quote(cwd)));
        }
        let session_command = format!("{prefixes}{command}");

        // Bridge the abort signal to session cancellation: SIGINT aborts
        // only the foreground command; the session survives. The receiver
        // resolving (send OR sender drop) both count as an abort request.
        let cancel_session = self.session.clone();
        let cancel_task = signal.map(|rx| {
            tokio::spawn(async move {
                let resolved = rx.await.is_ok();
                eprintln!("PROBE cancel task resolved ok={resolved}");
                cancel_session.cancel();
                eprintln!("PROBE cancel() called");
            })
        });
        self.report(format!("Executing (persistent session): {command}"));
        let start = Instant::now();
        let outcome = self
            .session
            .execute(
                &session_command,
                std::time::Duration::from_secs(timeout_secs),
            )
            .await;
        let elapsed = start.elapsed();
        if let Some(task) = cancel_task {
            task.abort();
        }
        self.report(format!(
            "Session command completed in {}",
            BashTool::format_duration(elapsed)
        ));

        let out = outcome.map_err(|e| -> ToolError { format!("persistent bash session: {e}") })?;

        let combined = if out.stdout.is_empty() {
            "(no output)".to_string()
        } else {
            out.stdout.clone()
        };
        let truncation = truncate::truncate_head(&combined, &Default::default());
        let mut text = BashTool::build_output(&truncation, elapsed, Some(out.exit_code));
        if out.truncated {
            // The session applied its own byte bound before the shared
            // head-truncation ran.
            text.push_str("\n[Session output bound applied]");
        }
        if let Some(reason) = is_dangerous_command(&command) {
            text.push_str(&format!("\n{reason}"));
        }
        if out.exit_code == 0 {
            Ok(AgentToolResult::success(text))
        } else {
            Ok(AgentToolResult::error(text))
        }
    }

    fn on_progress(&self, callback: ProgressCallback) {
        // SAFETY: a poisoned lock means the previous holder panicked while
        // holding it — a real bug that must surface, not be swallowed.
        #[allow(clippy::expect_used)]
        let mut guard = self
            .progress_callback
            .lock()
            .expect("progress callback lock poisoned");
        *guard = Some(callback);
    }
}
