//! Port 16 — HookRunner: user-configurable event→shell-command hooks.
//!
//! See spec at `docs/superpowers/specs/2026-08-04-hooks-system-design.md`.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Event kinds a hook can subscribe to. Serialised PascalCase to match
/// Claude Code's `settings.json` schema (and our own `[[hooks]]` config).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    /// Fires before a tool is executed. Exit 2 (block) prevents the call.
    #[default]
    PreToolUse,
    /// Fires after a tool executes. Can override the result.
    PostToolUse,
    /// Fires when the agent is about to stop after a turn. Exit 2 keeps it going.
    Stop,
    /// Fires when a subagent (the `subagent` tool) completes.
    SubagentStop,
    /// Fires when a session starts.
    SessionStart,
    /// Fires when a session ends.
    SessionEnd,
    /// Fires on notifications (e.g. permission requests).
    Notification,
}

/// Payload passed to a hook. Serialised to JSON on the script's stdin.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookContext {
    /// Event being fired.
    pub event: HookEvent,
    /// Tool name (PreToolUse/PostToolUse/SubagentStop).
    pub tool_name: Option<String>,
    /// Tool arguments (PreToolUse). For PostToolUse the input is omitted to
    /// keep the payload small; consumers that need it can match by `tool_name`.
    pub tool_args: Option<serde_json::Value>,
    /// Tool result content (PostToolUse).
    pub tool_result: Option<String>,
    /// Whether the result was an error (PostToolUse).
    pub is_error: Option<bool>,
    /// Identifier of the owning session.
    pub session_id: Option<String>,
    /// CWD of the owning session.
    pub session_cwd: Option<PathBuf>,
    /// Escape hatch for future fields without breaking the contract.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extra: Option<serde_json::Value>,
}

/// Outcome of a hook invocation.
///
/// `block` corresponds to exit code 2. The semantic of "block" depends on
/// the event:
/// - PreToolUse → block the tool call (`BeforeToolCallResult { block: true }`)
/// - Stop → block the stop (agent continues running)
/// - Other events → block has no effect (notification only)
#[derive(Debug, Clone, Default)]
pub struct HookOutcome {
    /// Exit code 2 from a script → `true`. See struct doc for semantics.
    pub block: bool,
    /// Human-readable reason (maps to `reason` in `BeforeToolCallResult`).
    pub reason: Option<String>,
    /// PostToolUse only: override the tool's result content.
    pub override_content: Option<String>,
}

/// A user-configured hook spec. Mirrors the `[[hooks]]` config schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    /// Event the hook subscribes to.
    pub event: HookEvent,
    /// Tool-name glob matcher (e.g. `"bash|write"`). `None` matches all.
    #[serde(default)]
    pub matcher: Option<String>,
    /// Shell command to execute. The runner uses `sh -c "<command>"`.
    pub command: String,
    /// Per-invocation timeout in seconds. `None` → runner default (60s).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// The hook runner contract. SDK defines the trait + a noop fallback;
/// products (cli, oxios) register concrete implementations.
pub trait HookRunner: Send + Sync + 'static {
    /// Run every spec that matches `(event, tool_name)` and merge results.
    ///
    /// Implementations are expected to be fail-open: a script that errors,
    /// times out, or returns a non-zero exit code other than 2 must NOT
    /// propagate the error as `SdkError` — log and return the merged
    /// outcome with `block = false` for that script's contribution.
    fn run<'a>(
        &'a self,
        event: HookEvent,
        ctx: &'a HookContext,
    ) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>>;
}

/// Noop runner: never blocks, never overrides. The default for products
/// that don't opt into hooks.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHookRunner;

impl HookRunner for NoopHookRunner {
    fn run<'a>(
        &'a self,
        _event: HookEvent,
        _ctx: &'a HookContext,
    ) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>> {
        Box::pin(async { HookOutcome::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_runner_returns_default_outcome() {
        let runner = NoopHookRunner;
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("bash".into()),
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(!outcome.block);
        assert!(outcome.reason.is_none());
        assert!(outcome.override_content.is_none());
    }

    #[test]
    fn hook_event_serialises_pascalcase() {
        let json = serde_json::to_string(&HookEvent::PreToolUse).unwrap();
        assert_eq!(json, "\"PreToolUse\"");
        let json = serde_json::to_string(&HookEvent::SessionStart).unwrap();
        assert_eq!(json, "\"SessionStart\"");
        // Round-trip
        let parsed: HookEvent = serde_json::from_str("\"SubagentStop\"").unwrap();
        assert_eq!(parsed, HookEvent::SubagentStop);
    }

    #[test]
    fn hook_context_serialises_with_extras() {
        let ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some("bash".into()),
            tool_args: Some(serde_json::json!({"command": "ls"})),
            ..Default::default()
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["event"], "PreToolUse");
        assert_eq!(json["tool_name"], "bash");
        assert_eq!(json["tool_args"]["command"], "ls");
        // `extra` is None so should be absent
        assert!(json.get("extra").is_none());
    }

    #[test]
    fn hook_spec_minimal_parses() {
        let toml = r#"
            event = "PreToolUse"
            command = "echo hi"
        "#;
        let spec: HookSpec = toml::from_str(toml).unwrap();
        assert_eq!(spec.event, HookEvent::PreToolUse);
        assert_eq!(spec.command, "echo hi");
        assert!(spec.matcher.is_none());
        assert!(spec.timeout_secs.is_none());
    }
}
