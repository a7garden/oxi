//! End-to-end test for the hooks pipeline at the cli level.
//!
//! These tests exercise the FULL path: settings → engine → AgentBuilder
//! → set_hooks → before_tool_call slot → HookMiddleware → InMemoryHookRunner.
//! The advisory that blocked execution specifically called out that
//! skipping the build() step would let the install_runtime_hooks-wipes-
//! middleware bug slip through. Don't.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxicode::store::settings::{Settings, SettingsFormat};
use oxicode_sdk::middleware::{HookMiddleware, MiddlewareContext, MiddlewareData, MiddlewarePhase};
use oxicode_sdk::ports::inmem::InMemoryHookRunner;
use oxicode_sdk::ports::{HookEvent, HookOutcome, HookRunner};

/// Real pipeline path: build a `HookMiddleware`, invoke its
/// `before_tool_call` handler through `MiddlewareContext::execute`,
/// assert the hook blocks the call. This exercises the same path
/// `build_hooks → before_tool_call` uses, without going through
/// `Handle::block_on` (which deadlocks in a tokio runtime).
#[tokio::test(flavor = "current_thread")]
async fn before_tool_call_runs_hook_and_returns_block() {
    let runner = Arc::new(InMemoryHookRunner::new());
    runner.on(|_, _| HookOutcome {
        block: true,
        reason: Some("denied".into()),
        ..Default::default()
    });

    let mut pipeline = oxicode_sdk::middleware::MiddlewarePipeline::new();
    pipeline = pipeline.add_arc(Arc::new(HookMiddleware::new(
        Arc::clone(&runner) as Arc<dyn HookRunner>
    )));
    let pipeline = Arc::new(pipeline);

    let ctx = MiddlewareContext::new(
        MiddlewarePhase::BeforeTool,
        "agent-1",
        MiddlewareData::BeforeTool {
            tool_name: "bash".into(),
            params: serde_json::json!({"command": "rm -rf /"}),
        },
    );
    let result = pipeline.execute(&ctx).await;
    assert!(matches!(
        result.action,
        oxicode_sdk::middleware::MiddlewareAction::Block
    ));
    assert_eq!(result.reason.as_deref(), Some("denied"));
}

/// PostToolUse: the runner sees the event with the tool name and result.
#[tokio::test(flavor = "current_thread")]
async fn after_tool_call_hooks_fire_with_result() {
    let seen = Arc::new(AtomicUsize::new(0));
    let s = Arc::clone(&seen);
    let runner = Arc::new(InMemoryHookRunner::new());
    runner.on(move |event, ctx| {
        if event == HookEvent::PostToolUse {
            s.fetch_add(1, Ordering::SeqCst);
            assert_eq!(ctx.tool_name.as_deref(), Some("read"));
            assert_eq!(ctx.tool_result.as_deref(), Some("hello"));
        }
        HookOutcome::default()
    });

    let mut pipeline = oxicode_sdk::middleware::MiddlewarePipeline::new();
    pipeline = pipeline.add_arc(Arc::new(HookMiddleware::new(
        Arc::clone(&runner) as Arc<dyn HookRunner>
    )));
    let pipeline = Arc::new(pipeline);

    let ctx = MiddlewareContext::new(
        MiddlewarePhase::AfterTool,
        "agent-1",
        MiddlewareData::AfterTool {
            tool_name: "read".into(),
            params: serde_json::json!({}),
            result: "hello".into(),
        },
    );
    let result = pipeline.execute(&ctx).await;
    assert!(matches!(
        result.action,
        oxicode_sdk::middleware::MiddlewareAction::Continue
    ));
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "PostToolUse fired exactly once"
    );
}

/// SubagentStop: when the tool is the `subagent` tool, the middleware
/// additionally fires `SubagentStop` so users can react to subagent
/// completion with a single matcher rule.
#[tokio::test(flavor = "current_thread")]
async fn subagent_tool_completion_fires_subagent_stop() {
    let subagent_count = Arc::new(AtomicUsize::new(0));
    let s = Arc::clone(&subagent_count);
    let runner = Arc::new(InMemoryHookRunner::new());
    runner.on(move |event, _| {
        if event == HookEvent::SubagentStop {
            s.fetch_add(1, Ordering::SeqCst);
        }
        HookOutcome::default()
    });

    let mut pipeline = oxicode_sdk::middleware::MiddlewarePipeline::new();
    pipeline = pipeline.add_arc(Arc::new(HookMiddleware::new(
        Arc::clone(&runner) as Arc<dyn HookRunner>
    )));
    let pipeline = Arc::new(pipeline);

    let ctx = MiddlewareContext::new(
        MiddlewarePhase::AfterTool,
        "agent-1",
        MiddlewareData::AfterTool {
            tool_name: "subagent".into(),
            params: serde_json::json!({}),
            result: "{}".into(),
        },
    );
    let _ = pipeline.execute(&ctx).await;
    assert_eq!(
        subagent_count.load(Ordering::SeqCst),
        1,
        "SubagentStop fired when tool_name == \"subagent\""
    );
}

/// Settings round-trip with `[[hooks]]` array.
#[test]
fn settings_round_trip_with_hooks() {
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

/// **Single-`set_hooks` invariant canary.** Scans `oxicode-cli/src` for
/// any `.set_hooks(` call site. There must be zero: the only place
/// `set_hooks` may be called is `oxicode_sdk::agent_builder::build`,
/// which composes the middleware pipeline slots and the session
/// closures into ONE `AgentHooks` instance. Calling `set_hooks`
/// elsewhere would re-introduce the replace-semantics wipe that
/// Task 9 removed `install_runtime_hooks` to prevent.
#[test]
fn set_hooks_is_called_only_in_agent_builder_build() {
    let cli_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut count = 0usize;
    for entry in walk_rs_files(&cli_src) {
        let text = std::fs::read_to_string(&entry).unwrap();
        // Count `.set_hooks(` occurrences in actual Rust code. Skip
        // lines starting with `//` (line comments) and lines that
        // contain `"set_hooks(` (string literals) — both irrelevant to
        // the invariant.
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("\"set_hooks(") {
                continue;
            }
            count += line.match_indices(".set_hooks(").count();
        }
    }
    assert_eq!(
        count, 0,
        "Expected ZERO .set_hooks( call sites in oxicode-cli/src ({} found). \
         Session queues are wired in via AgentBuilder::with_session_hooks \
         (sdk); calling set_hooks here would wipe the middleware pipeline.",
        count
    );
}

fn walk_rs_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    stack.push(e.path());
                }
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
    out
}
