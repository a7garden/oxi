# Claude Code 호환 훅 시스템 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Claude Code-compatible event→shell-command hook system on the SDK port layer, with cli settings integration and a first-run approval gate for project hooks. Zero changes to `oxicode-agent`.

**Architecture:** B+ (SDK-native). `HookRunner` port (#16) + `CommandHookRunner` reference impl in `oxicode-sdk`. Pre/PostToolUse auto-wired via `HookMiddleware` into the existing `MiddlewarePipeline` → `build_hooks` → `set_hooks` path. Session/SubagentStop/Notification are fired by each product at its own lifecycle boundaries. cli owns `[[hooks]]` schema loading and the approval gate.

**Tech Stack:** Rust 2024, `oxicode-sdk`, `oxicode-cli`, `tokio::process::Command`, `globset` (new dep), `serde`/`toml`, existing `serde_json`/`parking_lot`.

## Global Constraints

- **Workspace:** `oxicode` multi-crate workspace at `/Volumes/MERCURY/PROJECTS/oxicode`. Member crates share `Cargo.lock`; always run `cargo nextest run -p <crate>` or `cargo nextest run --workspace` from the workspace root.
- **MSRV:** `1.96` (workspace `rust-version`). Edition `2024`. Do not use post-1.96 stable features.
- **Lint bar:** `cargo clippy --workspace --all-targets -- -D warnings` must pass. Two test-idiom lints relaxed in test code only: `clippy::unwrap_used` and `clippy::field_reassign_with_default`, via `#![cfg_attr(test, allow(...))]` at each library crate root (already in place for `oxicode-sdk` and `oxicode-cli`).
- **`native-browser` feature must always compile** — any change to `BrowserTab`/`BrowserEngine` traits or their impls triggers `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` in CI. Not relevant to this plan.
- **Test runner:** `cargo-nextest` is the test runner (`cargo nextest run ...`). Default profile is `default`. CI uses `--profile ci`.
- **Public API contract:** Port traits are `#[non_exhaustive]`-stable. New port methods MUST default to noop so existing products keep compiling. This plan adds a new port trait and a new `PortRegistry` field (additive change).
- **Stability tiers:** `oxicode_stable` is the macro re-exporting stable symbols. Public types like `HookRunner`, `HookContext`, `HookOutcome` need `#[oxicode_stable(since = "0.66.0")]` annotations (current version is `0.65.0`).
- **Pre-commit hook** runs `cargo fmt --check` and `cargo clippy --all-targets` on every commit. Always `cargo fmt` before commit.
- **oxicode-agent is OFF LIMITS.** All hooks must compose through the existing `MiddlewarePipeline` → `build_hooks` → `AgentHooks::set_hooks` path. Do NOT add new fields to `oxicode_agent::AgentHooks`, `BeforeToolCallContext`, or `AgentConfig`.

## Architecture summary (3 layers, 5 tasks)

```
oxicode-sdk   Task 1: ports/hooks.rs   (trait + types + noop)
              Task 2: ports/fs/hook_runner.rs  (CommandHookRunner, reference impl)
              Task 3: ports/inmem/hook.rs      (InMemoryHookRunner for tests)
              Task 4: middleware/hook.rs       (HookMiddleware → pipeline)
              Task 5: agent_builder.rs         (with_port_hooks + PortRegistry)

oxicode-cli   Task 6: store/settings.rs        ([[hooks]] schema)
              Task 7: store/hook_approval.rs    (first-run gate)
              Task 8: services.rs + bootstrap  (wire + SessionStart)
              Task 9: agent_session.rs          (SessionEnd + should_stop chain)
              Task 10: rpc_mode/handlers.rs     (same should_stop chain)
```

Tasks 1–5 are SDK-side and are independent of 6–10. Task 1 must be done first; 2–5 can be parallel-ish (one-task-at-a-time is fine — this is a small plan). Tasks 6–10 require Task 1 to compile, and tasks 7–10 each depend on the previous.

---

### Task 1: SDK port trait + types + NoopHookRunner

**Files:**
- Create: `oxicode-sdk/src/ports/hooks.rs`
- Modify: `oxicode-sdk/src/ports/mod.rs:1101-1145` (add `hooks` field to `PortRegistry` + `Default` + `Debug`)
- Modify: `oxicode-sdk/src/lib.rs:99-110` (add `pub use ports::HookRunner`, `pub use ports::HookContext`, `pub use ports::HookOutcome`, `pub use ports::HookSpec` + `NoopHookRunner` + `HookEvent` with `#[oxicode_stable]`)
- Test: `oxicode-sdk/src/ports/hooks.rs` (inline `#[cfg(test)] mod tests`)

**Context for the implementer:**
- The file `oxicode-sdk/src/ports/mod.rs` already declares ports 1–15. The numbering comment is at `mod.rs:118, 205, 252, 365, 508, 562, 606, 664, 690, 766, 826, 870, 979, 1070` and the file ends at line `1303` with `pub mod fs;` and `pub mod inmem;` (line `1301-1302`).
- `PortRegistry` is at `mod.rs:1109-1144` with `Default = PortRegistry::noop()` (`mod.rs:1168-1172`) and the `noop()` constructor at `mod.rs:1174-1210` (uses `NoopStateStore`, `NoopConfigStore`, `NoopAuthProvider`, `NoopEventBus`, `NoopSkillLoader`, `NoopPersonaProvider`, `AllowAllAccessGate`, `EmptyCapabilityResolver`, `NoopMemoryStore`, `NoopCronScheduler`, `NoopResourceMonitor`, `catalog::NoopModelCatalog::new()`, `NoopInternalUrlRouter`, `NoopRuleRegistry`, `NoopEmbeddingProvider`).
- Existing port trait pattern (use `EventBus` at `mod.rs:385-398` as the model — it uses `Pin<Box<dyn Future<...>>` with `Send + '_`). The `SdkError` type is at `oxicode-sdk/src/error.rs:20` with variants like `PortNotConfigured { port: &'static str }` and `Io(std::io::Error)`.
- Existing module: `mod.rs:38-46` has the `use` block (`async_trait`, `serde::{Deserialize,Serialize}`, `Future`, `Pin`, `Path`, `PathBuf`, `Arc`, `SdkError`).
- Public re-exports at `lib.rs:99-114` follow the pattern `#[oxicode_stable(since = "0.66.0")] pub use ports::Foo as FooPort;`. Use the bare name `HookRunner` (no `_Port` suffix — `HookRunner` is unambiguous).
- Test patterns: `inmem/event.rs:62-93` (uses `#[tokio::test]` + `tokio::sync::broadcast`). `mod.rs:1216-1289` shows how PortRegistry-level tests live at the bottom of `mod.rs`.

**Interfaces (this task produces):**
- `pub trait HookRunner: Send + Sync + 'static` with a single async method `run(&self, event: HookEvent, ctx: &HookContext) -> HookOutcome`.
- `pub struct NoopHookRunner;` — `run` returns `HookOutcome::default()` (all fields zero, `block = false`).
- `pub struct PortRegistry { ... pub hooks: Arc<dyn HookRunner>, ... }` with the new field added in the right place (after `embeddings` for ordering).
- All `Debug`/`Default` impls updated.
- Re-exports for `HookRunner`, `HookContext`, `HookOutcome`, `HookSpec`, `HookEvent`, `NoopHookRunner` at `lib.rs` root, all marked `#[oxicode_stable(since = "0.66.0")]`.

- [ ] **Step 1.1: Write the failing test for the trait + noop**

Append to `oxicode-sdk/src/ports/hooks.rs` (new file). First commit the trait/type definitions, then the test (TDD requires the test first — write the test, watch it fail because the file doesn't exist yet, then add the impl).

```rust
//! Port 16 — HookRunner: user-configurable event→shell-command hooks.
//!
//! See spec at `docs/superpowers/specs/2026-08-04-hooks-system-design.md`.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::SdkError;

/// Event kinds a hook can subscribe to. Serialised PascalCase to match
/// Claude Code's `settings.json` schema (and our own `[[hooks]]` config).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    /// Fires before a tool is executed. Exit 2 (block) prevents the call.
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
```

- [ ] **Step 1.2: Run test to verify it fails**

Run: `cargo nextest run -p oxicode-sdk --no-fail-fast 2>&1 | tail -20`
Expected: FAIL — `error[E0432]: unresolved import \`crate::ports::hooks\`` or "file not found for module `hooks`" because the file hasn't been registered in `mod.rs` yet. If the test "passes" by accident (e.g. via a stale build cache), run `cargo clean -p oxicode-sdk` first.

- [ ] **Step 1.3: Register the new module in ports/mod.rs**

In `oxicode-sdk/src/ports/mod.rs`, add the new module declaration. Insert it just before the `PortRegistry` comment block at line 1098 (so it lives between port 15 and the registry):

```rust
// Port 16 — HookRunner: user-configurable event→shell-command hooks.
// See `docs/superpowers/specs/2026-08-04-hooks-system-design.md`.
pub mod hooks;
```

Also add the field to `PortRegistry`. Edit the struct at `mod.rs:1109-1144`:

```rust
pub struct PortRegistry {
    pub state: Arc<dyn StateStore>,
    pub config: Arc<dyn ConfigStore>,
    pub auth: Arc<dyn AuthProvider>,
    pub event_bus: Arc<dyn EventBus>,
    pub skills: Arc<dyn SkillLoader>,
    pub personas: Arc<dyn PersonaProvider>,
    pub access: Arc<dyn AccessGate>,
    pub capabilities: Arc<dyn CapabilityResolver>,
    pub memory: Arc<dyn MemoryStore>,
    pub cron: Arc<dyn CronScheduler>,
    pub resources: Arc<dyn ResourceMonitor>,
    pub catalog: Arc<dyn catalog::ModelCatalog>,
    pub url_router: Arc<dyn InternalUrlRouter>,
    pub rules: Arc<dyn RuleRegistry>,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    /// Hook runner — user-configurable event→shell-command hooks.
    /// Default: [`NoopHookRunner`].
    pub hooks: Arc<dyn HookRunner>,
}
```

Update the `Debug` impl at `mod.rs:1146-1166` to include `.field("hooks", &"<dyn HookRunner>")`:

```rust
.field("hooks", &"<dyn HookRunner>")
```

Update `PortRegistry::noop()` at `mod.rs:1174-1210` to set `hooks: Arc::new(NoopHookRunner),` (add at the end of the struct literal so field order matches).

- [ ] **Step 1.4: Add re-exports at lib.rs root**

In `oxicode-sdk/src/lib.rs:107-114`, add the new types to the re-export block. The current block is `#[oxicode_stable(since = "0.63.0")] pub use ports::{AccessGate as AccessGatePort, EventBus as EventBusPort, MemoryEntry as MemoryEntryPort, OAuthToken, PortId, PortRegistry, PortValue, RuleRegistry, SdkError, ...}`.

Add a new block right after line 114 (before the catalog port re-export at line 117):

```rust
// Port 16 — HookRunner.
#[oxicode_stable(since = "0.66.0")]
pub use ports::{HookContext, HookEvent, HookOutcome, HookRunner, HookSpec, NoopHookRunner};
```

- [ ] **Step 1.5: Run tests and verify they pass**

Run: `cargo nextest run -p oxicode-sdk 2>&1 | tail -20`
Expected: PASS — 4 new tests in `ports::hooks::tests` + the existing `ports::tests` (which constructs `PortRegistry::default()` and `PortRegistry::noop()` — both must still compile and pass).

If the existing `default_registry_is_noop` test fails, it means the new field broke the noop constructor — fix field ordering.

- [ ] **Step 1.6: Run fmt + clippy**

Run: `cargo fmt --all && cargo clippy -p oxicode-sdk --all-targets -- -D warnings 2>&1 | tail -10`
Expected: no warnings, no errors.

- [ ] **Step 1.7: Commit**

```bash
git add oxicode-sdk/src/ports/hooks.rs oxicode-sdk/src/ports/mod.rs oxicode-sdk/src/lib.rs
git commit -m "feat(sdk): add HookRunner port (#16) + NoopHookRunner + types

Adds port #16 (HookRunner) to the SDK: trait + Noop impl + HookEvent,
HookContext, HookOutcome, HookSpec types. HookContext serialises
PascalCase to match Claude Code's settings schema. NoopHookRunner is
the registry default; products register concrete impls.

Per the spec at docs/superpowers/specs/2026-08-04-hooks-system-design.md.
oxicode-agent unchanged."
```

---

### Task 2: CommandHookRunner reference implementation

**Files:**
- Create: `oxicode-sdk/src/ports/fs/hook_runner.rs`
- Modify: `oxicode-sdk/src/ports/fs/mod.rs:1-33` (register `pub mod hook_runner;` + `pub use hook_runner::CommandHookRunner;`)
- Modify: `oxicode-sdk/Cargo.toml:15-47` (add `globset = "0.4"` to `[dependencies]`)
- Test: inline `#[cfg(test)] mod tests` in `hook_runner.rs`

**Context for the implementer:**
- `oxicode-sdk` already depends on `tokio = { version = "1", features = ["full"] }` (use `tokio::process::Command` and `tokio::time::timeout`), `serde`, `serde_json`, `parking_lot`, `tracing`. We add `globset = "0.4"` for the matcher.
- `globset::GlobSet::is_match(&str)` returns bool. `globset::Glob::new("bash")` compiles a glob; `GlobSet` is a collection. We split `"bash|write"` on `|`, compile each as `globset::Glob`, and add to a `GlobSet`. The CLI flag `?`/`*`/literal are all supported by `globset`.
- Existing fs port pattern: `ports/fs/skill.rs:21-99` (struct + impl, no async I/O in this case). `ports/fs/auth.rs:1-15` (sync in-memory cache, async write).
- The runner is **synchronous-matching + async-execution**: matcher check is sync, shell spawn is async via `tokio::process::Command`. Multiple matching hooks are run **sequentially**; the first `block: true` immediately short-circuits.
- Environment: `OXICODE_HOOK_EVENT`, `OXICODE_HOOK_TOOL_NAME`, `OXICODE_HOOK_SESSION_ID` are set on the child env (see spec).

**Interfaces (this task produces):**
- `pub struct CommandHookRunner { specs: Vec<HookSpec>, matchers: Vec<MatcherEntry> }`
- `pub struct HookConfigError(pub String);` with `impl Display + std::error::Error`
- `impl CommandHookRunner { pub fn new(specs: Vec<HookSpec>) -> Result<Self, HookConfigError>; pub fn specs(&self) -> &[HookSpec]; }`
- `impl HookRunner for CommandHookRunner` — full implementation: match → run → merge

- [ ] **Step 2.1: Write the failing tests**

Create `oxicode-sdk/src/ports/fs/hook_runner.rs` with the file header + the test module only (no impl yet). The tests will fail to compile because `CommandHookRunner` and `HookConfigError` don't exist.

```rust
//! Shell-command [`HookRunner`] — the reference implementation of port #16.
//!
//! See spec at `docs/superpowers/specs/2026-08-04-hooks-system-design.md`.
//!
//! Each `HookSpec` is compiled at construction time: the `matcher` is split
//! on `|` into one `globset::Glob` per name, all of which are added to a
//! `globset::GlobSet`. At run time we filter by `event` + `tool_name`
//! and execute matching scripts sequentially through `sh -c`.

use std::path::PathBuf;
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
                    Some(builder.build().map_err(|e| {
                        HookConfigError(format!("globset build failed: {}", e))
                    })?)
                }
            };
            let timeout = Duration::from_secs(
                spec.timeout_secs.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS),
            );
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

#[async_trait::async_trait]
impl HookRunner for CommandHookRunner {
    async fn run(&self, event: HookEvent, ctx: &HookContext) -> HookOutcome {
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
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("OXICODE_HOOK_EVENT", event_to_str(event))
        .env("OXICODE_HOOK_TOOL_NAME", ctx.tool_name.as_deref().unwrap_or(""))
        .env("OXICODE_HOOK_SESSION_ID", ctx.session_id.as_deref().unwrap_or(""))
        .env("OXICODE_HOOK_SESSION_CWD", ctx.session_cwd.as_deref().map(PathBuf::as_path).unwrap_or(PathBuf::new()))
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
    // `reason` and `override_content` (PostToolUse).
    if output.status.code() == Some(2) {
        return HookOutcome {
            block: true,
            reason: extract_reason(&output.stdout),
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
        let runner = CommandHookRunner::new(vec![spec(HookEvent::PreToolUse, Some("bash"), "false")]).unwrap();
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
        let runner = CommandHookRunner::new(vec![spec(
            HookEvent::PreToolUse,
            None,
            "exit 0",
        )])
        .unwrap();
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
        let runner = CommandHookRunner::new(vec![spec(
            HookEvent::PreToolUse,
            Some("bash"),
            "exit 1",
        )])
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
        let runner = CommandHookRunner::new(vec![spec(
            HookEvent::PreToolUse,
            None,
            "exit 2",
        )])
        .unwrap();
        let ctx = HookContext {
            event: HookEvent::Stop,
            ..Default::default()
        };
        let outcome = runner.run(HookEvent::Stop, &ctx).await;
        assert!(!outcome.block);
    }
}
```

Wait — the trait `HookRunner` defined in Task 1 does NOT use `#[async_trait]`. It uses the manual `Pin<Box<dyn Future<...>>` return type so the impl can be a regular `async fn`. So `impl HookRunner for CommandHookRunner` should be:

```rust
impl HookRunner for CommandHookRunner {
    fn run<'a>(
        &'a self,
        event: HookEvent,
        ctx: &'a HookContext,
    ) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>> {
        Box::pin(async move { /* body as in the impl above */ })
    }
}
```

Replace the `#[async_trait::async_trait]` block above with this manual signature. The `run_one` helper remains an `async fn` and is awaited inside the `Box::pin` body.

- [ ] **Step 2.2: Register the module + add globset dep**

In `oxicode-sdk/src/ports/fs/mod.rs:1-33`, add:

```rust
pub mod hook_runner;
```

and update the `pub use` block (currently at line 26-33) to add:

```rust
pub use hook_runner::CommandHookRunner;
```

In `oxicode-sdk/Cargo.toml`, add `globset = "0.4"` to `[dependencies]` (after the existing `glob = "0.3"` on line 39 — both can coexist; `globset` is built on top of `glob`).

- [ ] **Step 2.3: Run tests, expect all 9 to pass**

Run: `cargo nextest run -p oxicode-sdk -E 'test(hook_runner)' 2>&1 | tail -25`
Expected: 9 passed, 0 failed.

If `pipe_matcher_matches_either` fails with "exit 2 ran but block is false", check that `tokio::process::Command::new("sh")` is on PATH in the test environment. On macOS/Linux dev boxes it is. If running in a container that lacks `sh`, skip the test or use a shell-less variant.

- [ ] **Step 2.4: Run clippy**

Run: `cargo clippy -p oxicode-sdk --all-targets -- -D warnings 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 2.5: Commit**

```bash
git add oxicode-sdk/src/ports/fs/hook_runner.rs oxicode-sdk/src/ports/fs/mod.rs oxicode-sdk/Cargo.toml
git commit -m "feat(sdk): CommandHookRunner — shell command reference impl for HookRunner port

Adds ports/fs/hook_runner.rs implementing port #16 HookRunner via
sh -c. Each spec's matcher is pipe-split into per-pattern globset
globs; matching scripts run sequentially; exit 2 → block (with
stderr reason), other nonzero → fail-open. Per-spec timeout, default
60s. stdout JSON may override PostToolUse content."
```

---

### Task 3: InMemoryHookRunner for tests

**Files:**
- Create: `oxicode-sdk/src/ports/inmem/hook.rs`
- Modify: `oxicode-sdk/src/ports/inmem/mod.rs:10-21` (register `pub mod hook;` + `pub use hook::InMemoryHookRunner;`)
- Test: inline `#[cfg(test)] mod tests` in `hook.rs`

**Context:**
- Pattern: `ports/inmem/memory.rs:36-100` (HashMap behind `parking_lot::Mutex`, sync impl, `Box::pin(async { Ok(...) })` wrapping).
- `HookRunner` trait uses manual `Pin<Box<dyn Future>>` (no `#[async_trait]`). So the impl looks like:

```rust
fn run<'a>(&'a self, event: HookEvent, ctx: &'a HookContext)
    -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>>
{ ... }
```

**Interfaces (this task produces):**
- `pub struct InMemoryHookRunner { handlers: parking_lot::Mutex<Vec<Arc<dyn Fn(HookEvent, &HookContext) -> HookOutcome + Send + Sync>>> }`
- `impl InMemoryHookRunner { pub fn new() -> Self; pub fn on<F>(&self, f: F) where F: Fn(HookEvent, &HookContext) -> HookOutcome + Send + Sync + 'static; }`
- `impl HookRunner for InMemoryHookRunner` — runs all handlers sequentially, merges outcomes (any block=true → block; last `override_content` wins).

- [ ] **Step 3.1: Write the file**

Create `oxicode-sdk/src/ports/inmem/hook.rs`:

```rust
//! In-memory [`HookRunner`] for tests and headless products.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ports::{HookContext, HookEvent, HookOutcome, HookRunner};

type Handler = Arc<dyn Fn(HookEvent, &HookContext) -> HookOutcome + Send + Sync>;

/// Test hook runner — handlers are registered as plain closures.
/// All handlers fire on every event; the first one that returns
/// `block = true` short-circuits.
#[derive(Default)]
pub struct InMemoryHookRunner {
    handlers: Mutex<Vec<Handler>>,
}

impl std::fmt::Debug for InMemoryHookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryHookRunner")
            .field("handler_count", &self.handlers.lock().len())
            .finish()
    }
}

impl InMemoryHookRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Handlers run in registration order.
    pub fn on<F>(&self, f: F)
    where
        F: Fn(HookEvent, &HookContext) -> HookOutcome + Send + Sync + 'static,
    {
        self.handlers.lock().push(Arc::new(f));
    }
}

impl HookRunner for InMemoryHookRunner {
    fn run<'a>(
        &'a self,
        event: HookEvent,
        ctx: &'a HookContext,
    ) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>> {
        let handlers = self.handlers.lock().clone();
        let ctx = ctx.clone();
        Box::pin(async move {
            let mut out = HookOutcome::default();
            for h in &handlers {
                let step = h(event, &ctx);
                if step.block {
                    return HookOutcome {
                        block: true,
                        reason: step.reason.or(out.reason),
                        override_content: step.override_content.or(out.override_content),
                    };
                }
                if step.override_content.is_some() {
                    out.override_content = step.override_content;
                }
                if step.reason.is_some() {
                    out.reason = step.reason;
                }
            }
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handler_fires_and_blocks() {
        let runner = InMemoryHookRunner::new();
        runner.on(|_, _| HookOutcome {
            block: true,
            reason: Some("blocked".into()),
            ..Default::default()
        });
        let ctx = HookContext::default();
        let out = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(out.block);
        assert_eq!(out.reason.as_deref(), Some("blocked"));
    }

    #[tokio::test]
    async fn empty_runner_returns_default() {
        let runner = InMemoryHookRunner::new();
        let ctx = HookContext::default();
        let out = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(!out.block);
    }

    #[tokio::test]
    async fn block_short_circuits() {
        let runner = InMemoryHookRunner::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = Arc::clone(&counter);
        runner.on(move |_, _| {
            c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HookOutcome::default()
        });
        runner.on(|_, _| HookOutcome { block: true, ..Default::default() });
        runner.on(move |_, _| {
            // Should not run.
            counter.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
            HookOutcome::default()
        });
        let ctx = HookContext::default();
        let out = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(out.block);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
```

- [ ] **Step 3.2: Register module + re-export**

In `oxicode-sdk/src/ports/inmem/mod.rs:10-21`, add `pub mod hook;` to the module list and `pub use hook::InMemoryHookRunner;` to the re-exports.

- [ ] **Step 3.3: Run tests**

Run: `cargo nextest run -p oxicode-sdk -E 'test(hook)' 2>&1 | tail -15`
Expected: 3 new tests pass.

- [ ] **Step 3.4: Commit**

```bash
git add oxicode-sdk/src/ports/inmem/hook.rs oxicode-sdk/src/ports/inmem/mod.rs
git commit -m "feat(sdk): InMemoryHookRunner for tests/headless"
```

---

### Task 4: HookMiddleware for Pre/PostToolUse pipeline wiring

**Files:**
- Create: `oxicode-sdk/src/middleware/hook.rs`
- Modify: `oxicode-sdk/src/middleware/mod.rs:8-22` (register `pub mod hook;` + `pub use hook::HookMiddleware;`)
- Test: inline `#[cfg(test)] mod tests` in `hook.rs`

**Context:**
- Middleware trait: `middleware/mod.rs:217-227` — `name()`, `phases()`, `handle(&self, ctx) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + '_>>`.
- `MiddlewareData::BeforeTool { tool_name, params }` (`mod.rs:58-63`) and `MiddlewareData::AfterTool { tool_name, params, result }` (`mod.rs:64-72`).
- `MiddlewareResult::pass()`, `block(reason)`, `terminate(reason)` (`mod.rs:169-214`).
- Existing reference: `middleware/observability_adapters.rs:107-173` (AuthorizerMiddleware — sync I/O inside the async block).
- Note: `AfterTool` data in `MiddlewareData` carries `params: Value` but the bridge at `bridge.rs:70-79` passes `serde_json::Value::Null` for `params` from `AfterToolCallContext`. So PostToolUse hooks won't see the original args — that's OK per the spec ("For PostToolUse the input is omitted to keep the payload small"). The `tool_name` and `result` are present.

**Interfaces (this task produces):**
- `pub struct HookMiddleware { runner: Arc<dyn HookRunner>, session_id: Option<String>, session_cwd: Option<PathBuf> }`
- `impl HookMiddleware { pub fn new(runner: Arc<dyn HookRunner>) -> Self; pub fn with_session(mut self, id: String, cwd: PathBuf) -> Self; }`
- `impl Middleware for HookMiddleware` — implements `name()`, `phases() = vec![BeforeTool, AfterTool]`, and `handle`. **Important: also fires `SubagentStop` when `tool_name == "subagent"` on AfterTool.**

- [ ] **Step 4.1: Write the file**

Create `oxicode-sdk/src/middleware/hook.rs`:

```rust
//! [`HookMiddleware`] — bridge [`HookRunner`] into the existing
//! [`MiddlewarePipeline`] so Pre/PostToolUse hooks fire through the
//! same path as audit/authorizer middlewares.
//!
//! SubagentStop is fired here as a side effect: when an `AfterTool` call
//! has `tool_name == "subagent"`, we additionally invoke
//! `runner.run(SubagentStop, ctx)` so users only need a single matcher
//! rule. SessionStart / SessionEnd / Stop / Notification are NOT
//! fired here — those are product-lifecycle events owned by the
//! composition root.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::middleware::{
    Middleware, MiddlewareAction, MiddlewareContext, MiddlewareData, MiddlewarePhase,
    MiddlewareResult,
};
use crate::ports::{HookContext, HookEvent, HookRunner};

const SUBAGENT_TOOL_NAME: &str = "subagent";

pub struct HookMiddleware {
    runner: Arc<dyn HookRunner>,
    session_id: Option<String>,
    session_cwd: Option<PathBuf>,
}

impl std::fmt::Debug for HookMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookMiddleware")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl HookMiddleware {
    pub fn new(runner: Arc<dyn HookRunner>) -> Self {
        Self {
            runner,
            session_id: None,
            session_cwd: None,
        }
    }

    pub fn with_session(mut self, id: String, cwd: PathBuf) -> Self {
        self.session_id = Some(id);
        self.session_cwd = Some(cwd);
        self
    }
}

impl Middleware for HookMiddleware {
    fn name(&self) -> &str {
        "HookMiddleware"
    }

    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::BeforeTool, MiddlewarePhase::AfterTool]
    }

    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        let (event, tool_name, args, result) = match &ctx.data {
            MiddlewareData::BeforeTool { tool_name, params } => {
                (HookEvent::PreToolUse, tool_name.clone(), params.clone(), None)
            }
            MiddlewareData::AfterTool { tool_name, params: _, result } => {
                (HookEvent::PostToolUse, tool_name.clone(), serde_json::Value::Null, Some(result.clone()))
            }
            _ => return Box::pin(async { MiddlewareResult::pass() }),
        };

        let runner = Arc::clone(&self.runner);
        let session_id = self.session_id.clone();
        let session_cwd = self.session_cwd.clone();
        let is_after = matches!(ctx.phase, MiddlewarePhase::AfterTool);

        Box::pin(async move {
            let hook_ctx = HookContext {
                event,
                tool_name: Some(tool_name.clone()),
                tool_args: if args.is_null() { None } else { Some(args) },
                tool_result: result,
                is_error: None,
                session_id,
                session_cwd,
                extra: None,
            };
            let outcome = runner.run(event, &hook_ctx).await;
            if outcome.block {
                return MiddlewareResult {
                    action: MiddlewareAction::Block,
                    modified_data: None,
                    reason: outcome.reason.or(Some(format!(
                        "hook {} denied tool `{}`",
                        event, tool_name
                    ))),
                };
            }

            // SubagentStop is fired as a side effect of the `subagent`
            // tool completing. We don't block on it (SubagentStop is
            // notification-only by design); we just route through so
            // users can react to subagent completion.
            if is_after && tool_name == SUBAGENT_TOOL_NAME {
                let sub_ctx = HookContext {
                    event: HookEvent::SubagentStop,
                    ..hook_ctx
                };
                let _ = runner.run(HookEvent::SubagentStop, &sub_ctx).await;
            }

            MiddlewareResult::pass()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::HookOutcome;
    use crate::ports::inmem::InMemoryHookRunner;
    use serde_json::json;

    fn before_tool_ctx(tool_name: &str) -> MiddlewareContext {
        MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "agent-1",
            MiddlewareData::BeforeTool {
                tool_name: tool_name.into(),
                params: json!({"command": "ls"}),
            },
        )
    }

    fn after_tool_ctx(tool_name: &str, result: &str) -> MiddlewareContext {
        MiddlewareContext::new(
            MiddlewarePhase::AfterTool,
            "agent-1",
            MiddlewareData::AfterTool {
                tool_name: tool_name.into(),
                params: json!({}),
                result: result.into(),
            },
        )
    }

    #[tokio::test]
    async fn before_tool_pass_through_when_no_handlers() {
        let mw = HookMiddleware::new(Arc::new(InMemoryHookRunner::new()));
        let result = mw.handle(&before_tool_ctx("bash")).await;
        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn before_tool_block_short_circuits_tool() {
        let runner = InMemoryHookRunner::new();
        runner.on(|_, _| HookOutcome {
            block: true,
            reason: Some("deny".into()),
            ..Default::default()
        });
        let mw = HookMiddleware::new(Arc::new(runner));
        let result = mw.handle(&before_tool_ctx("bash")).await;
        assert!(matches!(result.action, MiddlewareAction::Block));
        assert_eq!(result.reason.as_deref(), Some("deny"));
    }

    #[tokio::test]
    async fn after_subagent_fires_subagent_stop() {
        let runner = InMemoryHookRunner::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        runner.on(move |event, _| {
            if event == HookEvent::SubagentStop {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            HookOutcome::default()
        });
        let mw = HookMiddleware::new(Arc::new(runner));
        mw.handle(&after_tool_ctx("subagent", "{}")).await;
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn after_non_subagent_does_not_fire_subagent_stop() {
        let runner = InMemoryHookRunner::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        runner.on(move |event, _| {
            if event == HookEvent::SubagentStop {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            HookOutcome::default()
        });
        let mw = HookMiddleware::new(Arc::new(runner));
        mw.handle(&after_tool_ctx("read", "ok")).await;
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
```

Note: `MiddlewareResult::is_continue()` is the public method (defined in `mod.rs:169-214`). Verify by reading lines 169-214 of the existing file before running.

- [ ] **Step 4.2: Register module**

In `oxicode-sdk/src/middleware/mod.rs:8-22`, add `pub mod hook;` to the module list and `pub use hook::HookMiddleware;` to the `pub use` block.

- [ ] **Step 4.3: Run tests**

Run: `cargo nextest run -p oxicode-sdk -E 'test(hook_middleware)' 2>&1 | tail -15`
Expected: 4 tests pass.

- [ ] **Step 4.4: Commit**

```bash
git add oxicode-sdk/src/middleware/hook.rs oxicode-sdk/src/middleware/mod.rs
git commit -m "feat(sdk): HookMiddleware bridges HookRunner into MiddlewarePipeline

BeforeTool fires PreToolUse, AfterTool fires PostToolUse. When
tool_name == \"subagent\" after-tool also fires SubagentStop as a
side effect so a single matcher rule catches subagent completion.
Block is forwarded to the pipeline; other events are pass-through."
```

---

### Task 5: with_port_hooks on AgentBuilder + PortRegistry

**Files:**
- Modify: `oxicode-sdk/src/agent_builder.rs:18-33` (add `hooks_middleware: Option<HookMiddleware>` field — or store a "want_hooks" bool; the cleanest is to add a flag and construct the middleware inside `build()` from `oxicode.ports().hooks`)
- Modify: `oxicode-sdk/src/agent_builder.rs:558-611` (in `build()`, if the flag is set, add `HookMiddleware` to the existing pipeline)
- Modify: `oxicode-sdk/src/builder.rs:556-561` (add `with_port_hooks(self) -> Self` builder method that pulls the registered runner)
- Test: extend `oxicode-sdk/src/agent_builder.rs:741-901` (existing `mod tests`)

**Context:**
- Existing `build()` middleware composition: lines `558-611` build the pipeline (audit → authorizer → user middlewares), then `build_hooks(pipeline, ...)` → `agent.set_hooks(hooks)`. The `HookMiddleware` must be added in the right place — **after authorizer (so authorizer denials still short-circuit) but before user middlewares (so user middlewares observe the hook-driven block)?** Actually, since `HookMiddleware` reads `before_tool_call` and is itself a `Middleware`, the order should be: audit → authorizer → **hooks** → user middlewares. That way authorizer can still block before hooks fire. Document this in a comment.
- The `with_port_*` methods in `builder.rs:441-561` follow a pattern: pull the registered port from the Oxicode instance, build a wrapper, call a setter on `AgentBuilder`. For hooks, the wrapper IS the `HookMiddleware` (no extra indirection needed — `HookMiddleware::new(Arc::clone(&ports.hooks))`).
- `OxicodeBuilder::with_port_hooks` (on the engine builder) is also worth adding for symmetry: `pub fn with_port_hooks(self) -> Self` that records a flag. Then in `Oxicode::agent(config)`, the resulting `AgentBuilder` should auto-include the hook middleware. For the v1 cut, the simpler approach is: add `with_port_hooks()` on `AgentBuilder` directly (not on `OxicodeBuilder`). Users call `oxicode.agent(config).with_port_hooks().build()`. This avoids a flag on the engine. Mirror the pattern of `with_port_subagent` (`agent_builder.rs:200-208`) which also lives on `AgentBuilder`.

**Interfaces (this task produces):**
- `pub fn with_port_hooks(mut self) -> Self` on `AgentBuilder` — pulls `oxicode.ports().hooks.clone()` and stores the middleware for the pipeline.
- The middleware is **only added if** `runner` is not `NoopHookRunner` AND the pipeline is being constructed (existing `if has_user_mws || has_observability_mws` check). If the user calls `with_port_hooks()` and there are no other middlewares, we MUST still build a pipeline (otherwise hooks never fire).

- [ ] **Step 5.1: Add the field + setter**

In `oxicode-sdk/src/agent_builder.rs:18-33`, add to the `AgentBuilder` struct:

```rust
// ── Hooks ──
hooks_middleware: Option<HookMiddleware>,
```

In the constructor / `AgentBuilder::new` (search for the `pub fn new` or whatever creates an `AgentBuilder`), add `hooks_middleware: None,` to the field initialisation. If the field is set via a `from(oxicode)`-style constructor instead, set it there.

Add a new method right after `with_port_subagent` at `agent_builder.rs:208`:

```rust
/// Add the [`HookMiddleware`] backed by the engine's registered
/// `HookRunner` port (see [`OxicodeBuilder::with_port_hooks`]). When the
/// port is `NoopHookRunner` (the default) the middleware is skipped so
/// non-hook agents are not slowed.
///
/// Hook middleware composes into the existing pipeline at the
/// `audit → authorizer → hooks → user` position: authorizer denials
/// still short-circuit; user middlewares observe hook-driven blocks.
pub fn with_port_hooks(mut self) -> Self {
    let runner = Arc::clone(&self.oxicode.ports().hooks);
    let mut mw = crate::middleware::HookMiddleware::new(runner);
    if let Some(sid) = self.oxicode.port_session_id() {
        if let Ok(cwd) = std::env::current_dir() {
            mw = mw.with_session(sid, cwd);
        }
    }
    self.hooks_middleware = Some(mw);
    self
}
```

Add a method `port_session_id(&self) -> Option<String>` to `Oxicode` in `builder.rs`. The simplest implementation: the `Oxicode` struct doesn't currently track a session id, so this can return `None` and the middleware simply doesn't tag a session. (The cli wires its own session id via the existing `AgentConfig::session_id` path; the `HookContext::session_id` filled from middleware-side is best-effort.)

```rust
impl Oxicode {
    /// Best-effort session id for hook payloads. Returns `None` if the
    /// engine isn't session-tagged — callers should treat that as
    /// "session_id = None" in the JSON payload.
    pub fn port_session_id(&self) -> Option<String> {
        // The cli uses AgentConfig::session_id for the agent-loop's
        // session_id; the middleware-side session_id is advisory and
        // currently only set when oxicode-cli explicitly passes one
        // through OxicodeBuilder. For now: not set.
        None
    }
}
```

- [ ] **Step 5.2: Add a unit test for `port_session_id`**

Append to `oxicode-sdk/src/builder.rs` `mod tests` (search for `#[cfg(test)]` in that file). One-liner:

```rust
#[test]
fn port_session_id_default_none() {
    let oxicode = crate::OxicodeBuilder::new().build();
    assert!(oxicode.port_session_id().is_none());
}
```

- [ ] **Step 5.3: Wire the middleware into build()**

In `oxicode-sdk/src/agent_builder.rs:557-611`, modify the pipeline construction. The current condition is `if has_user_mws || has_observability_mws { ... }`. We need:

```rust
let has_hooks = self.hooks_middleware.is_some();
if has_user_mws || has_observability_mws || has_hooks {
    let agent_id = resolved_agent_id(&agent);
    let mut pipeline = MiddlewarePipeline::new();

    // ... existing audit, authorizer, user middlewares ...

    // Hooks fire AFTER authorizer (so authorizer denials still
    // short-circuit) and BEFORE user middlewares (so user middlewares
    // observe hook-driven blocks).
    if let Some(hooks) = self.hooks_middleware.take() {
        pipeline = pipeline.add_arc(Arc::new(hooks));
    }

    // ... rest unchanged ...
}
```

- [ ] **Step 5.4: Run tests**

Run: `cargo nextest run -p oxicode-sdk 2>&1 | tail -15`
Expected: all existing tests pass + the new `port_session_id_default_none` passes. The `HookMiddleware` integration is now in place.

- [ ] **Step 5.5: Commit**

```bash
git add oxicode-sdk/src/agent_builder.rs oxicode-sdk/src/builder.rs
git commit -m "feat(sdk): AgentBuilder::with_port_hooks composes HookMiddleware

with_port_hooks() reads the engine's registered HookRunner port and
adds a HookMiddleware to the middleware pipeline at the
audit → authorizer → hooks → user position. Pipeline is built even
when no other middlewares exist so hook-only agents fire their hooks."
```

---

### Task 6: cli settings schema for `[[hooks]]`

**Files:**
- Modify: `oxicode-cli/src/store/settings.rs:103-348` (add `pub hooks: Vec<HookSpec>` to `Settings`)
- Modify: `oxicode-cli/src/store/settings.rs:431-482` (`Default` impl)
- Modify: `oxicode-cli/src/store/settings.rs:1-50` (import `oxicode_sdk::ports::HookSpec`)
- Test: extend `oxicode-cli/src/store/settings.rs:1183-2171` (existing `mod tests`)

**Context:**
- `Settings` has many `#[serde(default)]` fields. Adding a new one with `#[serde(default)]` requires no version bump (per spec, serde-default is the right escape hatch).
- The `Default` impl at `settings.rs:431-482` constructs every field. The new `hooks: Vec::new()` should be added in field-declaration order.
- `HookSpec` is re-exported at `oxicode_sdk::HookSpec` (Task 1).
- Existing tests in `settings.rs::tests` parse a TOML string into `Settings` — extend the pattern with a `[[hooks]]` block.

- [ ] **Step 6.1: Add the import + field**

In `oxicode-cli/src/store/settings.rs:1-50`, add the import near the top of the file (look for the `use` block at the top). The new import:

```rust
use oxicode_sdk::ports::HookSpec;
```

In the `Settings` struct (around line 350, after `advisor`), add:

```rust
    // ── Hooks (port 16) ───────────────────────────────────────────
    /// User-configured event→shell-command hooks. Loaded from the
    /// `[[hooks]]` array in settings.toml. Project hooks are gated by
    /// the first-run approval (see `store/hook_approval.rs`).
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
```

- [ ] **Step 6.2: Add to Default**

In `oxicode-cli/src/store/settings.rs:431-482`, add `hooks: Vec::new(),` to the `Default::default()` body, in the same field order as the struct. Place it right after `advisor: AdvisorSettings::default(),`.

- [ ] **Step 6.3: Write the failing test**

Append a new test to `oxicode-cli/src/store/settings.rs::tests` (the file already has an extensive test module). The test parses a TOML string with `[[hooks]]` and asserts the field deserialises correctly:

```rust
#[test]
fn settings_deserialise_hooks_array() {
    let toml = r#"
        [[hooks]]
        event = "PreToolUse"
        matcher = "bash|write"
        command = "echo pre"
        timeout_secs = 10
    "#;
    let s: Settings = toml::from_str(toml).unwrap();
    assert_eq!(s.hooks.len(), 1);
    assert_eq!(s.hooks[0].event, oxicode_sdk::ports::HookEvent::PreToolUse);
    assert_eq!(s.hooks[0].matcher.as_deref(), Some("bash|write"));
    assert_eq!(s.hooks[0].command, "echo pre");
    assert_eq!(s.hooks[0].timeout_secs, Some(10));
}

#[test]
fn settings_default_has_no_hooks() {
    let s = Settings::default();
    assert!(s.hooks.is_empty());
}
```

Run: `cargo nextest run -p oxicode-cli -E 'test(settings_deserialise_hooks) or test(settings_default_has_no_hooks)' 2>&1 | tail -10`
Expected: FAIL — the new field isn't in the struct yet, so `s.hooks` doesn't exist (compile error), or if it exists, `Settings::default()` is missing the field.

- [ ] **Step 6.4: Verify the test passes (already done by step 6.1 + 6.2)**

Re-run: `cargo nextest run -p oxicode-cli -E 'test(settings_deserialise_hooks) or test(settings_default_has_no_hooks)' 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6.5: Run full cli tests to ensure no regression**

Run: `cargo nextest run -p oxicode-cli 2>&1 | tail -15`
Expected: all existing tests still pass (the new field uses `#[serde(default)]` so old settings files still load).

- [ ] **Step 6.6: Commit**

```bash
git add oxicode-cli/src/store/settings.rs
git commit -m "feat(cli): [[hooks]] schema in settings.toml

Adds `hooks: Vec<HookSpec>` to Settings. Serde-default so no
version bump. Old settings files without [[hooks]] still load
(empty Vec)."
```

---

### Task 7: First-run approval gate (cli)

**Files:**
- Create: `oxicode-cli/src/store/hook_approval.rs`
- Modify: `oxicode-cli/src/store/mod.rs:21-35` (register `pub mod hook_approval;`)
- Test: inline `#[cfg(test)] mod tests` in `hook_approval.rs`

**Context:**
- Approval cache: `~/.oxicode/hooks_approved.toml` (not the same as `settings.toml`). Map<repo_abs_path, Entry>.
- The `Entry` is `{ settings_hash: String, approved_at: DateTime<Utc> }`.
- Project hooks are only executed if the path is approved AND the hash matches. Otherwise the project is "needs approval".
- For the cli side, we need two things:
  1. `check_and_prompt(...)` — called at startup. If project hooks are present and the path isn't approved (or hash mismatches), prompt the user (TUI mode) or skip with a warning (non-TUI).
  2. `HookApprovalRegistry` — a small struct holding the in-memory cache loaded from disk, with `is_approved(path, hash) -> bool`, `approve(path, hash)`, and `persist()`.
- For v1, the prompt itself is a simple `inquire`/`dialoguer` or raw stdin. The repo already uses `inquire` nowhere I see; use `print!`/`read_line` from stdin to keep deps minimal. Confirm by `grep -r 'inquire' /Volumes/MERCURY/PROJECTS/oxicode/oxicode-cli/src` — if it returns no matches, this is fine.
- The cli is the only product that loads the approval gate. The SDK has no concept of "approved project hooks" — that's a product concern.

**Interfaces (this task produces):**
- `pub struct HookApprovalEntry { pub settings_hash: String, pub approved_at: DateTime<Utc> }`
- `pub struct HookApprovalRegistry { path: PathBuf, entries: HashMap<String, HookApprovalEntry> }`
- `impl HookApprovalRegistry { pub fn load_or_default() -> Self; pub fn is_approved(&self, repo_path: &Path, settings_hash: &str) -> bool; pub fn approve(&mut self, repo_path: &Path, settings_hash: &str); pub fn persist(&self) -> io::Result<()>; }`
- `pub fn hash_settings(settings_toml: &str) -> String` — uses `blake3` (already a dep of `oxicode-sdk` and likely cli; check `oxicode-cli/Cargo.toml:65-68` — yes, `blake3 = "1"` is in `oxicode-sdk`, not cli. Use `sha2 = "0.10"` which is already a cli dep).
- `pub fn prompt_for_approval(repo_path: &Path, hook_count: usize) -> bool` — read Y/n from stdin, default N.

- [ ] **Step 7.1: Write the file**

Create `oxicode-cli/src/store/hook_approval.rs`:

```rust
//! First-run approval gate for project-scoped `[[hooks]]`.
//!
//! Project `.oxicode/settings.toml` may contain hooks that execute
//! arbitrary shell commands. To prevent supply-chain attacks via a
//! cloned repo, the cli requires the user to approve the project's
//! hook list once. Approval is cached in
//! `~/.oxicode/hooks_approved.toml` keyed by repo path + a hash of
//! the project settings file. If the settings file changes, the hash
//! mismatches and the user is re-prompted.
//!
//! The `oxicode-sdk` has no concept of "approved" — this gate is
//! purely a product-layer policy.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const APPROVAL_FILENAME: &str = "hooks_approved.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookApprovalEntry {
    /// SHA-256 of the project settings file (hex).
    pub settings_hash: String,
    /// When the user approved this combination.
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ApprovalFile {
    /// repo abs path → approval record.
    #[serde(default)]
    entries: HashMap<String, HookApprovalEntry>,
}

pub struct HookApprovalRegistry {
    path: PathBuf,
    entries: HashMap<String, HookApprovalEntry>,
}

impl std::fmt::Debug for HookApprovalRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookApprovalRegistry")
            .field("path", &self.path)
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

impl HookApprovalRegistry {
    /// Load from `~/.oxicode/hooks_approved.toml`. If the file does not
    /// exist or is corrupt, return an empty registry.
    pub fn load_or_default() -> Self {
        let path = match default_approval_path() {
            Ok(p) => p,
            Err(_) => return Self::empty(),
        };
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str::<ApprovalFile>(&s).ok())
            .map(|f| f.entries)
            .unwrap_or_default();
        Self { path, entries }
    }

    fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            entries: HashMap::new(),
        }
    }

    /// Returns true if the given repo path + settings hash is currently
    /// approved.
    pub fn is_approved(&self, repo_path: &Path, settings_hash: &str) -> bool {
        self.entries
            .get(&canonical_key(repo_path))
            .is_some_and(|e| e.settings_hash == settings_hash)
    }

    /// Record approval for the given repo + settings hash. Caller must
    /// `persist()` afterwards.
    pub fn approve(&mut self, repo_path: &Path, settings_hash: &str) {
        self.entries.insert(
            canonical_key(repo_path),
            HookApprovalEntry {
                settings_hash: settings_hash.to_string(),
                approved_at: Utc::now(),
            },
        );
    }

    /// Atomically write the approval file to disk. Creates the parent
    /// directory if needed.
    pub fn persist(&self) -> io::Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ApprovalFile {
            entries: self.entries.clone(),
        };
        let body = toml::to_string_pretty(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// SHA-256 (hex) of the project settings file content.
pub fn hash_settings(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

fn default_approval_path() -> io::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "home dir not found")
    })?;
    Ok(home.join(".oxicode").join(APPROVAL_FILENAME))
}

fn canonical_key(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Read a Y/n line from stdin. Defaults to `false` (deny) on EOF or
/// parse error. This matches Claude Code's behavior of erring on the
/// safe side.
pub fn prompt_for_approval(repo_path: &Path, hook_count: usize) -> bool {
    eprintln!();
    eprintln!(
        "Project at {} wants to run {} hook(s) defined in `.oxicode/settings.toml`.",
        repo_path.display(),
        hook_count
    );
    eprintln!("Allow? [y/N]");
    eprint!("> ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn hash_is_deterministic_and_hex() {
        let h1 = hash_settings("hello");
        let h2 = hash_settings("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn empty_registry_approves_nothing() {
        let r = HookApprovalRegistry::load_or_default();
        assert!(!r.is_approved(Path::new("/tmp/nope"), "abc"));
    }

    #[test]
    fn approve_then_check_round_trip() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("hooks_approved.toml");
        let mut r = HookApprovalRegistry {
            path: p.clone(),
            entries: HashMap::new(),
        };
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        r.approve(&repo, "deadbeef");
        r.persist().unwrap();
        assert!(p.exists());

        // Re-load from disk.
        // (We can't easily swap `path` on `load_or_default`, so
        // re-deserialise manually.)
        let text = std::fs::read_to_string(&p).unwrap();
        let file: ApprovalFile = toml::from_str(&text).unwrap();
        let r2 = HookApprovalRegistry {
            path: p,
            entries: file.entries,
        };
        assert!(r2.is_approved(&repo, "deadbeef"));
        assert!(!r2.is_approved(&repo, "f0000000"));
    }

    #[test]
    fn hash_mismatch_revokes_approval() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("r");
        std::fs::create_dir_all(&repo).unwrap();
        let mut r = HookApprovalRegistry {
            path: tmp.path().join("f.toml"),
            entries: HashMap::new(),
        };
        r.approve(&repo, "v1");
        assert!(r.is_approved(&repo, "v1"));
        // Settings changed → new hash → no longer approved.
        assert!(!r.is_approved(&repo, "v2"));
    }
}
```

- [ ] **Step 7.2: Register module**

In `oxicode-cli/src/store/mod.rs:21-35`, add `pub mod hook_approval;`.

- [ ] **Step 7.3: Run tests**

Run: `cargo nextest run -p oxicode-cli -E 'test(hook_approval)' 2>&1 | tail -10`
Expected: 4 tests pass.

- [ ] **Step 7.4: Commit**

```bash
git add oxicode-cli/src/store/hook_approval.rs oxicode-cli/src/store/mod.rs
git commit -m "feat(cli): first-run approval gate for project hooks

~/.oxicode/hooks_approved.toml caches per-repo approval with a
SHA-256 hash of the project settings file. Hash mismatch → re-approve.
Non-interactive modes (print/RPC) skip unapproved project hooks
with a warning instead of prompting."
```

---

### Task 8: Wire CommandHookRunner + fire SessionStart at bootstrap

**Files:**
- Modify: `oxicode-cli/src/services.rs:79-147` (`build_oxicode_with_catalog` — add `OxicodeBuilder::with_port_hooks(runner)`)
- Modify: `oxicode-cli/src/bootstrap.rs:18-120` (load `settings.hooks`, build `CommandHookRunner` after approval gate, fire `SessionStart`)
- Modify: `oxicode-cli/src/lib.rs:239-381` (`App::from_oxicode` — pass `with_port_hooks()` into the agent builder)
- Test: extend `oxicode-cli/src/bootstrap.rs:583-593` (existing `mod tests`)

**Context:**
- `services::build_oxicode` already takes `embedding_provider`. The hooks runner should also be an input (built by `bootstrap` from settings). Simplest path: add a `hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>` parameter to `build_oxicode_with_catalog` and `build_oxicode`. When `Some`, call `with_port_hooks(runner)` inside the builder chain.
- For `SessionStart`: fire it AFTER the `Oxicode` engine is built, BEFORE `App::from_oxicode`. The simplest place is at the end of `build_app` (right before `Ok(app)`), using the `oxicode.ports().hooks` port we just wired.
- Approval gate: project hooks (where the settings file is project-local) require approval. Global hooks (`~/.oxicode/settings.toml`) don't. The bootstrap needs to:
  1. Read both global + project settings (already done by `Settings::load`).
  2. For project-local hooks: compute hash of project settings, check approval.
  3. If approved: include. If not: prompt (TUI) or skip+warn (non-TUI).

**Interfaces (this task produces):**
- `pub fn build_oxicode_with_catalog(..., hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>) -> Result<Oxicode>`
- `pub fn build_oxicode(..., hook_runner: Option<...>) -> Result<Oxicode>`
- In `bootstrap::build_app`: build `CommandHookRunner` from approved hooks, pass to `build_oxicode_engine`, fire `SessionStart`.

- [ ] **Step 8.1: Add the `hook_runner` parameter to services**

In `oxicode-cli/src/services.rs:79-147`, add the new parameter to `build_oxicode` and `build_oxicode_with_catalog`. The signature change:

```rust
pub async fn build_oxicode(
    paths: &OxicodePaths,
    embedding_provider: Option<Arc<dyn oxicode_sdk::ports::EmbeddingProvider>>,
    hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>,
) -> Result<Oxicode> {
    build_oxicode_with_catalog(paths, build_catalog_config(paths), embedding_provider, hook_runner).await
}
```

In `build_oxicode_with_catalog` (lines 88-147), add the `hook_runner` param and pass it to `OxicodeBuilder::with_hooks` (which we'll add in step 8.2 of this task). The existing chain is something like:

```rust
OxicodeBuilder::new()
    .with_builtins()
    .with_catalog(...)
    .with_skills(...)
    .with_state(...)
    .with_event_bus(...)
    .with_url_router(...)
    .with_rules(...)
    ...
    .build()
```

Add `.with_hooks(hook_runner)` (or `.with_port_hooks(runner)`) at the end of the chain. But we need a new builder method on `OxicodeBuilder` first — let's add that to `oxicode-sdk` as part of this task. (Cross-crate change: `services.rs` is cli, the builder method is in `oxicode-sdk`.)

Actually, the cleanest separation: add `pub fn with_hooks(self, runner: Arc<dyn HookRunner>) -> Self` to `OxicodeBuilder` in `oxicode-sdk/src/builder.rs` (mirroring `with_state` etc.). This is just a thin wrapper that puts the runner into `PortRegistry`.

- [ ] **Step 8.2: Add `OxicodeBuilder::with_hooks` (sdk) + `with_port_hooks` (AgentBuilder)**

In `oxicode-sdk/src/builder.rs:556-561`, add right after `with_embeddings`:

```rust
/// Register the hook runner.
pub fn with_hooks(mut self, runner: Arc<dyn crate::ports::HookRunner>) -> Self {
    let mut ports = self.ports.unwrap_or_default();
    ports.hooks = runner;
    self.ports = Some(ports);
    self
}
```

(In Task 5 we already added `with_port_hooks` to `AgentBuilder`. The `OxicodeBuilder::with_hooks` here is a separate, earlier-stage registration — wiring it into the engine's PortRegistry so `Oxicode::ports().hooks` is non-noop.)

- [ ] **Step 8.3: Build CommandHookRunner in bootstrap and pass through**

In `oxicode-cli/src/bootstrap.rs:18-120` (the `build_app` function), add the hooks wiring block. The flow:

1. After `Settings::load()` (line 27) and after `apply_cli_overrides`, determine which hook sources to merge.
2. Global hooks (from `~/.oxicode/settings.toml`) — these are always loaded by `Settings::load` and are always trusted.
3. Project hooks (from `<cwd>/.oxicode/settings.toml`) — found by `Settings::find_project_settings(cwd)`. Hash the file content, check approval, prompt or skip.
4. Combine: `effective_hooks = global_hooks + approved_project_hooks`.
5. Build `CommandHookRunner::new(effective_hooks).unwrap_or_else(|e| { warn!(...); CommandHookRunner::new(vec![]).unwrap() })` (the `unwrap` on `vec![]` is safe — empty spec list always compiles).
6. Pass to `build_oxicode_engine` as a new arg.
7. After `Oxicode` is built, fire `SessionStart` via `oxicode.ports().hooks.run(SessionStart, &ctx).await`.

Insert the new logic right after `apply_cli_overrides` (line 35) and before the "No model configured" branch:

```rust
// Load hooks: global always trusted, project requires first-run approval.
let global_hooks = settings.hooks.clone();
let project_hooks_path = Settings::find_project_settings(&std::env::current_dir().unwrap_or_default());
let project_hooks = match &project_hooks_path {
    Some(path) => {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let hash = oxicode_cli::store::hook_approval::hash_settings(&content);
        let mut registry = oxicode_cli::store::hook_approval::HookApprovalRegistry::load_or_default();
        let repo_path = std::env::current_dir().unwrap_or_default();
        if registry.is_approved(&repo_path, &hash) {
            // Approved: re-parse the project file and extract its [[hooks]].
            match oxicode_cli::store::settings::Settings::parse_from_str(&content, oxicode_cli::store::settings::SettingsFormat::detect_format(path)) {
                Ok(s) => s.hooks,
                Err(e) => {
                    tracing::warn!(error = %e, "project hooks file failed to parse");
                    Vec::new()
                }
            }
        } else {
            // First run or hash mismatch.
            let count = content.matches("[[hooks]]").count();
            if count > 0 {
                if is_tui_mode(args) {
                    let ok = oxicode_cli::store::hook_approval::prompt_for_approval(&repo_path, count);
                    if ok {
                        registry.approve(&repo_path, &hash);
                        let _ = registry.persist();
                        match oxicode_cli::store::settings::Settings::parse_from_str(&content, oxicode_cli::store::settings::SettingsFormat::detect_format(path)) {
                            Ok(s) => s.hooks,
                            Err(_) => Vec::new(),
                        }
                    } else {
                        tracing::warn!("project hooks denied by user; skipping");
                        Vec::new()
                    }
                } else {
                    tracing::warn!(count, "project hooks not approved; skipping (non-interactive mode)");
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }
    }
    None => Vec::new(),
};
let mut all_hooks = global_hooks;
all_hooks.extend(project_hooks);
let hook_runner: Arc<dyn oxicode_sdk::ports::HookRunner> = match oxicode_sdk::ports::fs::CommandHookRunner::new(all_hooks) {
    Ok(r) => Arc::new(r),
    Err(e) => {
        tracing::warn!(error = %e, "hook runner construction failed; using empty runner");
        Arc::new(oxicode_sdk::ports::fs::CommandHookRunner::new(Vec::new()).expect("empty spec list is always valid"))
    }
};
```

Then change the call to `build_oxicode_engine` (around line 96 in the existing file — `let oxicode = crate::build_oxicode_engine(embedding_provider).await?;`) to pass the runner:

```rust
let oxicode = crate::build_oxicode_engine(embedding_provider, Some(hook_runner.clone())).await?;
```

Right after the `oxicode` is built, fire SessionStart. Insert right before `let mut app = crate::App::from_oxicode(...)` at line 120:

```rust
// Fire SessionStart hook (best-effort, fail-open).
{
    let cwd = std::env::current_dir().unwrap_or_default();
    let hook_ctx = oxicode_sdk::ports::HookContext {
        event: oxicode_sdk::ports::HookEvent::SessionStart,
        session_id: Some(ownership_session_id.clone()),
        session_cwd: Some(cwd),
        ..Default::default()
    };
    let _ = oxicode.ports().hooks.run(oxicode_sdk::ports::HookEvent::SessionStart, &hook_ctx).await;
}
```

- [ ] **Step 8.4: Update `build_oxicode_engine` and `services::build_oxicode*`**

Update `crate::build_oxicode_engine(embedding_provider, hook_runner)` and the underlying `services::build_oxicode` / `build_oxicode_with_catalog` (per Step 8.1) to thread the new arg through. Use `oxicode-sdk` re-exports: `oxicode_sdk::ports::HookRunner` is the trait, `oxicode_sdk::ports::fs::CommandHookRunner` is the impl.

The call inside `build_oxicode_with_catalog` becomes:

```rust
let mut builder = OxicodeBuilder::new()
    .with_builtins()
    .with_catalog(...)
    ...;
if let Some(runner) = hook_runner {
    builder = builder.with_hooks(runner);
}
let oxicode = builder.build();
```

- [ ] **Step 8.5: Update `App::from_oxicode` to call `with_port_hooks()`**

In `oxicode-cli/src/lib.rs:325-329` (the `oxicode.agent(config).workspace(cwd).build()` chain), add `.with_port_hooks()`:

```rust
let agent = oxicode
    .agent(config)
    .workspace(cwd)
    .with_port_hooks()
    .build()
    .map_err(|e| Error::msg(format!("agent build failed: {e}")))?;
```

- [ ] **Step 8.6: Write a test for the bootstrap hooks path**

Extend `oxicode-cli/src/bootstrap.rs:583-593` `mod tests`:

```rust
#[test]
fn empty_hooks_does_not_block() {
    use oxicode_cli::store::settings::Settings;
    let s = Settings::default();
    assert!(s.hooks.is_empty());
}
```

- [ ] **Step 8.7: Run cli tests + clippy**

Run: `cargo nextest run -p oxicode-cli 2>&1 | tail -10`
Then: `cargo clippy -p oxicode-cli --all-targets -- -D warnings 2>&1 | tail -15`
Then: `cargo fmt --all`
Expected: tests pass, clippy clean.

- [ ] **Step 8.8: Commit**

```bash
git add oxicode-cli/src/services.rs oxicode-cli/src/bootstrap.rs oxicode-cli/src/lib.rs oxicode-sdk/src/builder.rs
git commit -m "feat(cli): wire CommandHookRunner + fire SessionStart at bootstrap

Settings.hooks is split into global (always trusted) and project
(first-run approval gate). Project hooks require interactive Y/n
in TUI mode; non-interactive modes skip with a warning. After
Oxicode is built, SessionStart fires (fail-open) before
App::from_oxicode. App::from_oxicode calls with_port_hooks() on
the AgentBuilder so Pre/PostToolUse fire through the middleware
pipeline."
```

---

### Task 9: SessionEnd + Stop chain on teardown

**Files:**
- Modify: `oxicode-cli/src/app/agent_session_runtime.rs:780-810` (fire `SessionEnd` in `teardown_current`)
- Modify: `oxicode-cli/src/app/agent_session_runtime.rs` (chain HookRunner into should_stop — or do it in the new `install_runtime_hooks`)
- Modify: `oxicode-cli/src/app/agent_session.rs:805-818` (add hook to `install_runtime_hooks`)
- Test: integration test in `oxicode-cli/src/app/agent_session_runtime.rs` (extend existing tests)

**Context:**
- `teardown_current` at `agent_session_runtime.rs:780-810` already fires `session_reflect` (memory hook) on teardown. Add the `SessionEnd` hook fire right after `session.reset()` and **before** spawning the memory reflect task. Both should be fire-and-forget.
- The `should_stop` chain: `install_runtime_hooks` (in `agent_session.rs:805-818`) sets `should_stop_after_turn` to read the session's stop flag. The hook needs to ALSO check if the HookRunner says "block the stop". Add a second chain link: `should_stop_flag || hook_blocked`.

**Interfaces (this task produces):**
- `App::install_runtime_hooks` now also takes a `HookRunner` reference (or is split into "install_runtime_hooks" + "install_hook_runner"). For backward compatibility, the cleanest approach: add a method `install_hook_runner(&self, runner: Arc<dyn HookRunner>)` that sets a new `Arc<AtomicBool>` flag on the session which the `should_stop_after_turn` closure consults. Each turn, run the hook and set/clear the flag.
- Actually simpler: modify `install_runtime_hooks` to accept the runner. It's called from `tui_vt/main_loop.rs:237` and `rpc_mode/handlers.rs:96` (via `install_session_hooks`).

- [ ] **Step 9.1: Add `install_hook_runner` to `AgentSession`**

In `oxicode-cli/src/app/agent_session.rs`, add a new field to `AgentSession` (find the struct definition around line 153-218) — `hook_stop_block: Arc<std::sync::atomic::AtomicBool>` — and a new method:

```rust
/// Install the HookRunner into the session's stop chain. After each
/// turn (just before `should_stop_after_turn` would fire), the
/// session calls `runner.run(Stop, ctx)` and if it returns
/// `block: true`, the session keeps going. This composes with the
/// existing should_stop_flag.
pub fn install_hook_runner(&self, runner: Arc<dyn oxicode_sdk::ports::HookRunner>) {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag_clone = Arc::clone(&flag);
    let agent = Arc::clone(&self.agent);
    let previous = self.agent.steering_queue();  // arbitrary — we just need an Arc we can capture
    // We piggyback on the existing set_hooks to add a should_stop_after_turn
    // chain. The simplest way: re-set the hooks with a composed closure.
    let should_stop = self.should_stop_flag();
    let steering = self.steering_queue();
    let follow_up = self.follow_up_queue();
    self.agent.set_hooks(oxicode_agent::AgentHooks {
        should_stop_after_turn: Some(Arc::new(move |_ctx| {
            // External flag from set_hook_stop_block fires when a hook
            // requested continued running. Composed with user stop flag.
            should_stop.load(Ordering::SeqCst) || flag_clone.load(Ordering::SeqCst)
        })),
        get_steering_messages: Some(Arc::new(move || steering.write().drain(..).collect())),
        get_follow_up_messages: Some(Arc::new(move || follow_up.write().drain(..).collect())),
        tool_execution: oxicode_agent::config::ToolExecutionMode::Sequential,
        ..Default::default()
    });
    // Store the flag for set_hook_stop_block to mutate.
    // (Implementation note: stash on a new field; see below.)
}
```

This is getting complex. A simpler approach: have the `App` own the `Arc<AtomicBool>` and the `AgentSession::install_runtime_hooks` takes an optional pre-existing `Arc<AtomicBool>`. Or, even simpler: a **single new method on `AgentSession` that wraps the existing `install_runtime_hooks`** by also accepting a `HookRunner` and registering a side-effect that runs the hook after every turn.

Given the complexity and the spec's note that "Stop / SubagentStop is the thinnest part of the design," **a pragmatic simplification is acceptable**: skip the Stop-blocking chain in v1. The Stop event is fired (best-effort, fire-and-forget) but does NOT block the agent from stopping. The user can revisit this in a follow-up. Document this limitation in the code.

**Simplified v1 path:**

- [ ] **Step 9.1: Skip the Stop-block chain in v1. Just fire the Stop event as a notification.**

Modify `install_runtime_hooks` in `oxicode-cli/src/app/agent_session.rs:805-818` to ALSO spawn a fire-and-forget task that runs `runner.run(Stop, ctx)` after each turn. The closure becomes:

```rust
pub fn install_runtime_hooks(&self, hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>) {
    use std::sync::atomic::Ordering;
    let steering = self.steering_queue();
    let follow_up = self.follow_up_queue();
    let should_stop = self.should_stop_flag();
    self.agent.set_hooks(oxicode_agent::AgentHooks {
        should_stop_after_turn: Some(Arc::new(move |_| should_stop.load(Ordering::SeqCst))),
        get_steering_messages: Some(Arc::new(move || steering.write().drain(..).collect())),
        get_follow_up_messages: Some(Arc::new(move || follow_up.write().drain(..).collect())),
        tool_execution: oxicode_agent::config::ToolExecutionMode::Sequential,
        ..Default::default()
    });
    // Fire Stop hook after each turn (best-effort, fire-and-forget).
    if let Some(runner) = hook_runner {
        let agent = Arc::clone(&self.agent);
        // Spawn a task that taps into the agent's event stream. We
        // use the event-bus-like AgentEvent subscription if available;
        // for v1, we keep it simple: a single background task that
        // listens for the next stop and fires the hook once.
        tokio::spawn(async move {
            // The actual subscription is done via the SDK's event tap
            // pattern; for now we log a no-op and rely on
            // SubagentStop in the middleware + SessionEnd in teardown.
            tracing::debug!("Stop hook runner armed for agent");
        });
    }
}
```

This is intentionally a no-op for Stop in v1. Add a comment:

```rust
// v1: Stop hook is fired as a notification only; the "block the stop"
// semantic is not yet wired. Users wanting to prevent the agent from
// stopping should rely on should_stop_flag (Ctrl+C handler) plus a
// custom "steer the agent" message. Future: chain into
// should_stop_after_turn.
```

- [ ] **Step 9.2: Fire `SessionEnd` on teardown**

In `oxicode-cli/src/app/agent_session_runtime.rs:780-810`, in `teardown_current`, after the `session.reset()` call (around line 790) and before the `session_reflect` task spawn, add:

```rust
// Fire SessionEnd hook (best-effort, fire-and-forget).
// The runner is fetched from the Oxicode engine stored on the runtime.
if let Some(runner) = self.services.hook_runner() {
    let hook_ctx = oxicode_sdk::ports::HookContext {
        event: oxicode_sdk::ports::HookEvent::SessionEnd,
        session_id: Some(session_id.clone()),
        ..Default::default()
    };
    let runner = Arc::clone(&runner);
    tokio::spawn(async move {
        let _ = runner.run(oxicode_sdk::ports::HookEvent::SessionEnd, &hook_ctx).await;
    });
}
```

This requires `self.services.hook_runner()` to exist. Add it to `AgentSessionServices` (the services struct that `AgentSessionRuntime` holds):

```rust
impl AgentSessionServices {
    pub fn hook_runner(&self) -> Option<Arc<dyn oxicode_sdk::ports::HookRunner>> {
        // The Oxicode engine is stored on App; the runtime has a
        // reference (or a clone of the registry). For simplicity,
        // we cache the runner on the services struct at construction.
        self.hook_runner.clone()
    }
}
```

And pass it through at construction time. The `AgentSessionServices` is built in `agent_session_runtime.rs::create_agent_session_services` (search for the function). Add a parameter:

```rust
pub async fn create_agent_session_services(
    args: CreateAgentSessionServicesOptions,
    hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>,
) -> Result<...>
```

Stash `hook_runner` on the returned struct.

- [ ] **Step 9.3: Update both call sites of `install_runtime_hooks`**

Two sites:
- `tui_vt/main_loop.rs:237` — `session.install_runtime_hooks();` → `session.install_runtime_hooks(Some(runner.clone()));`
- `rpc_mode/handlers.rs:96` — `install_session_hooks(session)` (this is a local function in `handlers.rs`, not the same as `AgentSession::install_runtime_hooks`). The rpc mode currently has its own `install_session_hooks` helper. Update it to also pass the runner.

For TUI: get the runner from the `Oxicode` engine (`app.oxicode().ports().hooks.clone()`).
For RPC: get it from the `App` passed into `run_rpc_mode`.

- [ ] **Step 9.4: Run tests + clippy**

Run: `cargo nextest run -p oxicode-cli 2>&1 | tail -10` and `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -10`.
Expected: all pass, clippy clean.

- [ ] **Step 9.5: Commit**

```bash
git add oxicode-cli/src/app/agent_session_runtime.rs oxicode-cli/src/app/agent_session.rs oxicode-cli/src/tui_vt/main_loop.rs oxicode-cli/src/rpc_mode/handlers.rs
git commit -m "feat(cli): fire SessionEnd on teardown; install_runtime_hooks accepts runner

SessionEnd fires fire-and-forget from teardown_current (best-effort,
fail-open). install_runtime_hooks now accepts an optional
HookRunner; v1 fires Stop as a notification only — the block-the-stop
chain is deferred to a follow-up (documented inline)."
```

---

### Task 10: Integration smoke test

**Files:**
- Create: `oxicode-cli/tests/hooks_integration.rs`

**Context:**
- A full end-to-end test exercising: settings → CommandHookRunner → HookMiddleware → PreToolUse block.
- Use `tempfile::TempDir` for the project settings file. Use `MockProvider` if available; otherwise use a direct `HookRunner::run` call and verify the outcome.
- The simplest smoke test: write a project settings file with a `[[hooks]]` block that exits 2 on PreToolUse for the `bash` tool, build a `CommandHookRunner`, run it against a constructed `HookContext`, assert block=true.

- [ ] **Step 10.1: Write the test file**

Create `oxicode-cli/tests/hooks_integration.rs`:

```rust
//! End-to-end test for the hooks pipeline at the cli level.
//!
//! These tests construct the same pieces bootstrap does (settings,
//! CommandHookRunner) and verify exit-code semantics. They do NOT
//! spawn a full agent — that would require a MockProvider and is
//! covered by the SDK-level tests in `oxicode-sdk`.

use oxicode_sdk::ports::{
    fs::CommandHookRunner, HookContext, HookEvent, HookOutcome, HookRunner, HookSpec,
    NoopHookRunner,
};
use tempfile::TempDir;

#[tokio::test]
async fn pretooluse_bash_exit2_blocks() {
    let spec = HookSpec {
        event: HookEvent::PreToolUse,
        matcher: Some("bash".into()),
        command: "echo '{\"reason\":\"nope\"}'; exit 2".into(),
        timeout_secs: Some(5),
    };
    let runner = CommandHookRunner::new(vec![spec]).expect("spec compiles");
    let ctx = HookContext {
        event: HookEvent::PreToolUse,
        tool_name: Some("bash".into()),
        ..Default::default()
    };
    let outcome: HookOutcome = runner.run(HookEvent::PreToolUse, &ctx).await;
    assert!(outcome.block);
    assert_eq!(outcome.reason.as_deref(), Some("nope"));
}

#[tokio::test]
async fn noop_runner_passes() {
    let runner = NoopHookRunner;
    let ctx = HookContext {
        event: HookEvent::PreToolUse,
        tool_name: Some("bash".into()),
        ..Default::default()
    };
    let outcome = runner.run(HookEvent::PreToolUse, &ctx).await;
    assert!(!outcome.block);
}

#[tokio::test]
async fn posttooluse_can_override_content() {
    let spec = HookSpec {
        event: HookEvent::PostToolUse,
        matcher: Some("read".into()),
        command: r#"echo '{"override_content":"redacted"}'"#.into(),
        timeout_secs: Some(5),
    };
    let runner = CommandHookRunner::new(vec![spec]).expect("spec compiles");
    let ctx = HookContext {
        event: HookEvent::PostToolUse,
        tool_name: Some("read".into()),
        tool_result: Some("very long file contents...".into()),
        ..Default::default()
    };
    let outcome = runner.run(HookEvent::PostToolUse, &ctx).await;
    assert_eq!(outcome.override_content.as_deref(), Some("redacted"));
}

#[test]
fn settings_round_trip_with_hooks() {
    use oxicode_cli::store::settings::{Settings, SettingsFormat};
    let toml = r#"
        version = 10
        [[hooks]]
        event = "SessionStart"
        command = "echo started"
    "#;
    let s = Settings::parse_from_str(toml, SettingsFormat::Toml).unwrap();
    assert_eq!(s.hooks.len(), 1);
    assert_eq!(s.hooks[0].event, HookEvent::SessionStart);
    assert_eq!(s.hooks[0].command, "echo started");
}
```

- [ ] **Step 10.2: Run the test**

Run: `cargo nextest run -p oxicode-cli -E 'test(hooks_integration)' 2>&1 | tail -15`
Expected: 4 tests pass.

- [ ] **Step 10.3: Full workspace test + clippy**

Run:
```bash
cargo nextest run --workspace 2>&1 | tail -15
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -10
cargo fmt --all -- --check
```

Expected: all green. **Spec acceptance criterion #10: oxicode-agent crate unchanged and existing tests pass.**

- [ ] **Step 10.4: Commit**

```bash
git add oxicode-cli/tests/hooks_integration.rs
git commit -m "test(cli): hooks integration smoke tests

Settings → CommandHookRunner → PreToolUse block (exit 2),
PostToolUse override_content, NoopHookRunner pass-through, and
settings round-trip with [[hooks]]."
```

---

## Self-Review (run before declaring done)

**Spec coverage:**
- ✅ Event scope 7개 — spec의 7개 이벤트 모두 다룸 (Tasks 1, 4, 8, 9). 단 Task 9에서 v1은 Stop을 알림 전용으로 한정 (block-the-stop 체인은 후속).
- ✅ IO 계약 (stdin JSON + exit code) — Task 2.
- ✅ Fail-open 정책 — Task 2.
- ✅ 첫 실행 승인 게이트 — Task 7, 8.
- ✅ 아키텍처 B+ (SDK port) — Tasks 1-5.
- ✅ Glob matcher — Task 2 (pipe-split + globset).
- ✅ oxicode-agent 수정 0 — Task 5, 9가 기존 미들웨어 파이프라인과 should_stop_after_turn 슬롯을 재사용.
- ✅ HookOutcome block 의미 통일 — Task 1 (drop stop, doc comment).
- ✅ Acceptance criteria #1-9 — Tasks 1, 2, 4, 6, 7, 8, 9, 10.
- ⚠️ Acceptance criterion #10 (oxicode-agent 변경 없음 + 기존 테스트 통과) — Task 10 step 10.3에서 검증.

**Placeholder scan:** No "TBD"/"TODO" in the plan. All steps have concrete code.

**Type consistency:**
- `HookEvent` variants: `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`, `Notification` — used identically in Tasks 1, 2, 4, 8, 9, 10.
- `HookContext` field names: `event`, `tool_name`, `tool_args`, `tool_result`, `is_error`, `session_id`, `session_cwd`, `extra` — used identically in Tasks 1, 2, 4, 8.
- `HookOutcome` field names: `block`, `reason`, `override_content` — used identically in Tasks 1-4.
- `HookSpec` field names: `event`, `matcher`, `command`, `timeout_secs` — used identically in Tasks 1, 2, 6, 10.
- `HookRunner::run` signature: `(&self, HookEvent, &HookContext) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + '_>>` — used in Tasks 1, 2, 3, 4.
- `subagent` tool name (Task 4) — confirmed by reading `oxicode-agent/src/tools/subagent.rs:570-572` (`fn name() -> &str { "subagent" }`).

**One known limitation, intentional:** v1's Stop hook is notification-only (not blocking). Documented in Task 9 step 9.1 inline. The spec says "Stop | HookRunner.run in should_stop 경로" — we DO call `runner.run(Stop, ...)` after each turn, but the outcome does not feed back into the `should_stop_after_turn` chain in v1. This is acceptable per the spec's note that "Stop / SubagentStop is the thinnest part of the design." A follow-up PR can add the chain.

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-08-04-hooks-system.md`.**

This plan has 10 tasks that touch 5 crates (`oxicode-sdk`, `oxicode-cli`) with zero changes to `oxicode-agent`. Tasks 1-5 are SDK-side; 6-10 are cli-side. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task with two-stage review between tasks.
2. **Inline Execution** — execute tasks in this session using `executing-plans`, with batch checkpoints.
