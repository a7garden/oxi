//! Shell-command [`HookRunner`] — the reference implementation of port #16.
//!
//! See spec at `docs/superpowers/specs/2026-08-04-hooks-system-design.md`.
//!
//! Each `HookSpec` is compiled at construction time: the `matcher` is split
//! on `|` into one `globset::Glob` per name, all of which are added to a
//! `globset::GlobSet`. At run time we filter by `event` + `tool_name`
//! and execute matching scripts sequentially through `sh -c`.

use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

use crate::ports::{HookContext, HookEvent, HookOutcome, HookRunner, HookSpec};

/// Default per-invocation timeout when `HookSpec::timeout_secs` is `None`.
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 60;

/// Error constructing a [`CommandHookRunner`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfigError(pub String);

impl std::fmt::Display for HookConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HookConfigError {}

struct MatcherEntry {
    event: HookEvent,
    set: Option<GlobSet>, // None = match all
    timeout: Duration,
    command: String,
}

/// Reference [`HookRunner`] backed by a list of [`HookSpec`]s.
pub struct CommandHookRunner {
    specs: Vec<HookSpec>,
    matchers: Vec<MatcherEntry>,
}

impl std::fmt::Debug for CommandHookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandHookRunner")
            .field("spec_count", &self.specs.len())
            .finish()
    }
}

impl CommandHookRunner {
    /// Compile the given specs. Returns an error if any `matcher` is an
    /// invalid glob.
    pub fn new(specs: Vec<HookSpec>) -> Result<Self, HookConfigError> {
        let mut matchers = Vec::with_capacity(specs.len());
        for spec in &specs {
            let set = match &spec.matcher {
                None => None,
                Some(pat) => {
                    let mut builder = GlobSetBuilder::new();
                    for piece in pat.split('|') {
                        let piece = piece.trim();
                        if piece.is_empty() {
                            return Err(HookConfigError(format!(
                                "empty matcher segment in `{}`",
                                pat
                            )));
                        }
                        let glob = Glob::new(piece).map_err(|e| {
                            HookConfigError(format!("invalid glob `{}`: {}", piece, e))
                        })?;
                        builder.add(glob);
                    }
                    Some(
                        builder
                            .build()
                            .map_err(|e| HookConfigError(format!("globset build failed: {}", e)))?,
                    )
                }
            };
            let timeout =
                Duration::from_secs(spec.timeout_secs.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS));
            matchers.push(MatcherEntry {
                event: spec.event,
                set,
                timeout,
                command: spec.command.clone(),
            });
        }
        Ok(Self { specs, matchers })
    }

    /// Borrow the original specs (read-only).
    pub fn specs(&self) -> &[HookSpec] {
        &self.specs
    }
}

impl HookRunner for CommandHookRunner {
    fn run<'a>(
        &'a self,
        event: HookEvent,
        ctx: &'a HookContext,
    ) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>> {
        Box::pin(async move {
            let mut outcome = HookOutcome::default();

            for entry in &self.matchers {
                if entry.event != event {
                    continue;
                }
                // Matcher: None = all; Some(set) = tool_name must be is_match.
                let tool_name = ctx.tool_name.as_deref().unwrap_or("");
                if let Some(set) = &entry.set
                    && !set.is_match(tool_name)
                {
                    continue;
                }

                // Run this script. Fail-open: any error → log + continue.
                let script_outcome = run_one(&entry.command, entry.timeout, event, ctx).await;
                if script_outcome.block {
                    outcome.block = true;
                    outcome.reason = script_outcome.reason.or(outcome.reason);
                    // block short-circuits: stop processing further scripts.
                    return outcome;
                }
                if script_outcome.override_content.is_some() {
                    outcome.override_content = script_outcome.override_content;
                }
            }

            outcome
        })
    }
}

/// Execute one hook script. Never panics; never returns `Err`. The result
/// is translated to the script's effect on the agent loop (block / override).
async fn run_one(
    command: &str,
    timeout_dur: Duration,
    event: HookEvent,
    ctx: &HookContext,
) -> HookOutcome {
    let session_cwd = ctx
        .session_cwd
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("OXICODE_HOOK_EVENT", event_to_str(event))
        .env(
            "OXICODE_HOOK_TOOL_NAME",
            ctx.tool_name.as_deref().unwrap_or(""),
        )
        .env(
            "OXICODE_HOOK_SESSION_ID",
            ctx.session_id.as_deref().unwrap_or(""),
        )
        .env("OXICODE_HOOK_SESSION_CWD", &session_cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(command, error = %e, "hook script failed to spawn (fail-open)");
            return HookOutcome::default();
        }
    };

    // Write the JSON context to stdin.
    let stdin_payload = match serde_json::to_string(ctx) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "failed to serialise hook context (fail-open)");
            return HookOutcome::default();
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(stdin_payload.as_bytes()).await {
            warn!(error = %e, "failed to write hook stdin (fail-open)");
        }
        drop(stdin);
    }

    // Wait with timeout. On timeout, kill the child (kill_on_drop handles
    // the case where the future is dropped).
    let result = timeout(timeout_dur, child.wait_with_output()).await;
    let output = match result {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            warn!(command, error = %e, "hook wait failed (fail-open)");
            return HookOutcome::default();
        }
        Err(_) => {
            warn!(command, ?timeout_dur, "hook timed out (fail-open)");
            return HookOutcome::default();
        }
    };

    // Exit code 2 → block. The optional JSON on stdout can override
    if output.status.code() == Some(2) {
        return HookOutcome {
            block: true,
            reason: extract_reason(&output.stderr),
            override_content: None,
        };
    }

    // Non-2, non-0 → log + pass.
    if !output.status.success()
        && let Some(code) = output.status.code()
    {
        warn!(
            command,
            code,
            stderr = %String::from_utf8_lossy(&output.stderr),
            "hook script exited non-zero (fail-open)"
        );
    }

    // Best-effort: parse stdout JSON for override / reason. Unknown shape
    // is ignored silently (Claude Code permits a wide variety).
    if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
        let override_content = parsed
            .get("override_content")
            .or_else(|| parsed.get("continue"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let reason = parsed
            .get("reason")
            .or_else(|| parsed.get("message"))
            .and_then(|v| v.as_str())
            .map(String::from);
        return HookOutcome {
            block: false,
            reason,
            override_content,
        };
    }

    HookOutcome::default()
}

fn event_to_str(e: HookEvent) -> &'static str {
    match e {
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::PostToolUse => "PostToolUse",
        HookEvent::Stop => "Stop",
        HookEvent::SubagentStop => "SubagentStop",
        HookEvent::SessionStart => "SessionStart",
        HookEvent::SessionEnd => "SessionEnd",
        HookEvent::Notification => "Notification",
    }
}

fn extract_reason(stdout: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| {
            v.get("reason")
                .or_else(|| v.get("message"))
                .and_then(|r| r.as_str())
                .map(String::from)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(event: HookEvent, matcher: Option<&str>, command: &str) -> HookSpec {
        HookSpec {
            event,
            matcher: matcher.map(String::from),
            command: command.into(),
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn no_match_runs_nothing() {
        let runner =
            CommandHookRunner::new(vec![spec(HookEvent::PreToolUse, Some("bash"), "false")])
                .unwrap();
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("read".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(!outcome.block);
    }

    #[tokio::test]
    async fn no_matcher_runs_for_any_tool() {
        let runner =
            CommandHookRunner::new(vec![spec(HookEvent::PreToolUse, None, "exit 0")]).unwrap();
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("anything".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(!outcome.block);
    }

    #[tokio::test]
    async fn exit_2_blocks() {
        let runner = CommandHookRunner::new(vec![spec(
            HookEvent::PreToolUse,
            Some("bash"),
            "echo '{\"reason\":\"nope\"}' >&2; exit 2",
        )])
        .unwrap();
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("bash".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(outcome.block);
        assert_eq!(outcome.reason.as_deref(), Some("nope"));
    }

    #[tokio::test]
    async fn nonzero_nonzero_2_fails_open() {
        let runner =
            CommandHookRunner::new(vec![spec(HookEvent::PreToolUse, Some("bash"), "exit 1")])
                .unwrap();
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("bash".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        // Exit 1 is NOT a block. Tool should proceed.
        assert!(!outcome.block);
    }

    #[tokio::test]
    async fn pipe_matcher_matches_either() {
        let runner = CommandHookRunner::new(vec![spec(
            HookEvent::PreToolUse,
            Some("bash|write"),
            "exit 2",
        )])
        .unwrap();
        for tool in ["bash", "write"] {
            let ctx = HookContext {
                event: HookEvent::PreToolUse,
                tool_name: Some(tool.into()),
                ..Default::default()
            };
            let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
            assert!(outcome.block, "expected block for tool={tool}");
        }
        // And a non-matching tool passes.
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("read".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(!outcome.block);
    }

    #[tokio::test]
    async fn stdout_json_overrides_content() {
        let runner = CommandHookRunner::new(vec![spec(
            HookEvent::PostToolUse,
            Some("read"),
            r#"echo '{"override_content":"replaced"}'"#,
        )])
        .unwrap();
        let ctx = HookContext {
            event: HookEvent::PostToolUse,
            tool_name: Some("read".into()),
            tool_result: Some("original".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PostToolUse, &ctx).await;
        assert_eq!(outcome.override_content.as_deref(), Some("replaced"));
    }

    #[tokio::test]
    async fn multiple_matching_scripts_run_sequentially() {
        let runner = CommandHookRunner::new(vec![
            spec(HookEvent::PreToolUse, Some("bash"), "exit 0"),
            spec(HookEvent::PreToolUse, Some("bash"), "exit 2"),
        ])
        .unwrap();
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("bash".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        // Second script blocks; first passes. We should see block=true.
        assert!(outcome.block);
    }

    #[tokio::test]
    async fn empty_matcher_segment_errors_at_construction() {
        let bad = vec![spec(HookEvent::PreToolUse, Some("bash||write"), "true")];
        let err = CommandHookRunner::new(bad).unwrap_err();
        assert!(err.0.contains("empty matcher"));
    }

    #[tokio::test]
    async fn invalid_glob_errors_at_construction() {
        let bad = vec![spec(HookEvent::PreToolUse, Some("["), "true")];
        assert!(CommandHookRunner::new(bad).is_err());
    }

    #[tokio::test]
    async fn event_must_match() {
        // A spec for PreToolUse should NOT fire for Stop.
        let runner =
            CommandHookRunner::new(vec![spec(HookEvent::PreToolUse, None, "exit 2")]).unwrap();
        let ctx = HookContext {
            event: HookEvent::Stop,
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::Stop, &ctx).await;
        assert!(!outcome.block);
    }
}
