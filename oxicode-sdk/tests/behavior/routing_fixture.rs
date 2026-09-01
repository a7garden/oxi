//! Routed-tool fixtures: the pack's `bash`/`eval`/`debug` tools installed
//! WITH their runtime services execute through the persistent runtimes
//! (`behavior::routed_bash_session_persistence`,
//! `behavior::routed_eval_kernel_persistence`,
//! `behavior::routed_debug_dap_lifecycle`), and fall back to the legacy
//! per-invocation implementations when the host wires nothing
//! (`behavior::routed_tools_fall_back_to_legacy`).
//!
//! Persistence across two separate tool calls is impossible with the
//! legacy per-invocation implementation, so these scenarios distinguish
//! routed from legacy behavior while exercising the exact
//! `AgentTool::execute` surface the model sees.
use crate::common::*;
use oxicode_agent::runtime::{
    DapDebugService, EvalKernel, JavaScriptEvalKernel, PersistentShellSession, PythonEvalKernel,
    ShellSession,
};
use oxicode_agent::{AgentTool, ToolContext};
use std::time::Duration;

/// Fetch the host-wrapped tool installed under `name`.
fn installed_tool<'a>(installer: &'a RecordingInstaller, name: &str) -> &'a Arc<dyn AgentTool> {
    installer
        .wrapped
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, t)| t)
        .unwrap_or_else(|| panic!("tool {name} not installed"))
}

#[tokio::test]
async fn routed_bash_session_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    let session: Arc<dyn ShellSession> =
        Arc::new(PersistentShellSession::new(ws.clone()).with_max_output(4_096));
    let mut services = minimal_services(&ws);
    services.shell_session = Some(session.clone());

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    assert!(
        !manifest
            .degraded
            .iter()
            .any(|d| d.feature == "shell-session"),
        "shell-session must be satisfied: {:?}",
        manifest.degraded
    );
    let bash = installed_tool(&installer, "bash");
    assert!(
        bash.label().contains("persistent"),
        "pack must install the routed bash variant, got label: {}",
        bash.label()
    );
    let ctx = ToolContext {
        root_dir: Some(ws.clone()),
        ..ToolContext::default()
    };

    // Persistence across two separate tool calls: cwd via the session, then
    // env via the `env` parameter — a legacy per-invocation shell cannot do
    // either across calls.
    let out = bash
        .execute(
            "call-1",
            serde_json::json!({ "command": "cd sub" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "cd failed: {}", out.output);

    let out = bash
        .execute(
            "call-2",
            serde_json::json!({ "command": "echo $PWD $OXI_ROUTED", "env": { "OXI_ROUTED": "42" } }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        out.output.contains("sub") && out.output.contains("42"),
        "cwd+env must persist through the routed tool: {}",
        out.output
    );

    // Raw `cd ..` in the command body returns to the root (the cwd
    // PARAMETER rejects traversal by design).
    let out = bash
        .execute(
            "call-3",
            serde_json::json!({ "command": "cd .. && pwd" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "cd back failed: {}", out.output);

    // The `cwd` parameter routes to a persistent `cd` (workspace-contained,
    // absolute paths only — same policy as the legacy tool).
    let out = bash
        .execute(
            "call-4",
            serde_json::json!({ "command": "pwd", "cwd": dir.path().join("sub") }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        out.output.contains("/sub"),
        "cwd parameter must move the persistent session: {}",
        out.output
    );
    // Abort: the host cancels the running command through the session API
    // (the same surface the agent loop's cancellation path uses). SIGINT
    // aborts only the foreground command; the session survives and the
    // tool surfaces exit code 130.
    let worker_bash = bash.clone();
    let worker = tokio::spawn(async move {
        worker_bash
            .execute(
                "call-5",
                serde_json::json!({ "command": "sleep 30" }),
                None,
                &ToolContext::default(),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.cancel();
    let out = tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("execute must return after abort")
        .expect("join")
        .expect("execute ok");
    assert!(
        out.output.contains("exited with code 130"),
        "abort must surface 130: {}",
        out.output
    );

    // The session is still usable after the abort.
    let out = bash
        .execute(
            "call-5",
            serde_json::json!({ "command": "echo alive" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        out.success && out.output.contains("alive"),
        "{}",
        out.output
    );

    // Bounded output still reported through the tool.
    let out = bash
        .execute(
            "call-6",
            serde_json::json!({ "command": "seq 1 200000" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        out.output.contains("Truncated") || out.output.contains("bound"),
        "bound must be reported: {}",
        out.output
    );
}

#[tokio::test]
async fn routed_eval_kernel_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let python: Arc<dyn EvalKernel> = Arc::new(PythonEvalKernel::new());
    let js: Arc<dyn EvalKernel> = Arc::new(JavaScriptEvalKernel::new());
    let mut services = minimal_services(&ws);
    services.eval_kernels = vec![python, js];

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    assert!(
        !manifest.degraded.iter().any(|d| d.feature == "eval-kernel"),
        "eval-kernel must be satisfied: {:?}",
        manifest.degraded
    );
    let eval = installed_tool(&installer, "eval");
    assert!(
        eval.label().contains("persistent"),
        "pack must install the routed eval variant, got label: {}",
        eval.label()
    );
    let ctx = ToolContext::default();

    // State continuity across two tool calls.
    let out = eval
        .execute(
            "cell-1",
            serde_json::json!({ "language": "py", "code": "fixture_x = 41" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "{}", out.output);
    let out = eval
        .execute(
            "cell-2",
            serde_json::json!({ "language": "py", "code": "print(fixture_x + 1)" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        out.success && out.output.contains("42"),
        "state must persist: {}",
        out.output
    );
    // Errors are captured as text, not transport failures.
    let out = eval
        .execute(
            "cell-3",
            serde_json::json!({ "language": "py", "code": "1 / 0" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        !out.success && out.output.contains("ZeroDivisionError"),
        "error must be captured: {}",
        out.output
    );

    // `reset` drops kernel state.
    let out = eval
        .execute(
            "cell-4",
            serde_json::json!({ "language": "py", "code": "print(fixture_x)", "reset": true }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        !out.success && out.output.contains("NameError"),
        "reset must drop state: {}",
        out.output
    );

    // JS path through the same routed tool.
    let out = eval
        .execute(
            "cell-5",
            serde_json::json!({ "language": "js", "code": "globalThis.j = 21; console.log(j * 2)" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        out.success && out.output.contains("42"),
        "js kernel must work through the tool: {}",
        out.output
    );
}

const FAKE_DAP_ADAPTER: &str = r#"
import json, sys

seq = 0

def send(obj):
    global seq
    seq += 1
    obj["seq"] = seq
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

def read_msg():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            raise EOFError
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            length = int(value.strip())
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def reply(req, body):
    send({
        "type": "response",
        "request_seq": req["seq"],
        "success": True,
        "command": req["command"],
        "body": body,
    })

init = read_msg()
reply(init, {"supportsConfigurationDoneRequest": False})
send({"type": "event", "event": "output", "body": {"output": "adapter up"}})

launch = read_msg()
reply(launch, {})
send({"type": "event", "event": "process", "body": {"name": "fake"}})
send({"type": "event", "event": "stopped",
      "body": {"reason": "entry", "threadId": 1, "allThreadsStopped": True}})

BODIES = {
    "setBreakpoints": {"breakpoints": [{"id": 1, "verified": True, "line": 3}]},
    "threads": {"threads": [{"id": 1, "name": "main"}]},
    "stackTrace": {"stackFrames": [{"id": 100, "name": "main", "line": 3, "column": 1}],
                    "totalFrames": 1},
    "scopes": {"scopes": [{"name": "Locals", "variablesReference": 7}]},
    "variables": {"variables": [{"name": "x", "value": "42", "variablesReference": 0}]},
    "continue": {"allThreadsContinued": True},
    "next": {},
    "stepIn": {},
    "stepOut": {},
    "pause": {},
    "evaluate": {"result": "42", "type": "int"},
}

while True:
    try:
        req = read_msg()
    except EOFError:
        break
    cmd = req.get("command")
    if cmd in ("terminate", "disconnect"):
        reply(req, {})
        send({"type": "event", "event": "exited", "body": {"exitCode": 0}})
        break
    reply(req, BODIES.get(cmd, {}))
"#;

#[tokio::test]
async fn routed_debug_dap_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let mut services = minimal_services(&ws);
    services.debug_service = Some(Arc::new(DapDebugService::new()));

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    assert!(
        !manifest
            .degraded
            .iter()
            .any(|d| d.feature == "debug-service"),
        "debug-service must be satisfied: {:?}",
        manifest.degraded
    );
    let debug = installed_tool(&installer, "debug");
    assert!(
        debug.label().contains("routed"),
        "pack must install the routed debug variant, got label: {}",
        debug.label()
    );
    let ctx = ToolContext::default();

    // Launch through the tool; the routed variant surfaces a session id.
    // `adapter_command` points at the inline fake adapter (no real
    // lldb-dap binary needed).
    let out = debug
        .execute(
            "d-1",
            serde_json::json!({
                "action": "launch",
                "adapter": "lldb-dap",
                "adapter_command": ["python3", "-c", FAKE_DAP_ADAPTER],
                "program": "/tmp/fake_program.py",
            }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "launch failed: {}", out.output);
    let session = out
        .metadata
        .as_ref()
        .and_then(|m| m.get("session"))
        .and_then(|s| s.as_str())
        .expect("launch must return a session id")
        .to_string();

    // Full protocol surface through the tool: breakpoints, stepping,
    // inspection, evaluation.
    let actions: Vec<serde_json::Value> = vec![
        serde_json::json!({ "action": "set_breakpoint", "file": "/tmp/fake_program.py", "line": 3 }),
        serde_json::json!({ "action": "threads" }),
        serde_json::json!({ "action": "stack_trace", "thread_id": 1 }),
        serde_json::json!({ "action": "scopes", "frame_id": 100 }),
        serde_json::json!({ "action": "variables", "frame_id": 100, "variable_ref": 7 }),
        serde_json::json!({ "action": "continue", "thread_id": 1 }),
        serde_json::json!({ "action": "pause", "thread_id": 1 }),
        serde_json::json!({ "action": "step_over", "thread_id": 1 }),
        serde_json::json!({ "action": "step_in", "thread_id": 1 }),
        serde_json::json!({ "action": "step_out", "thread_id": 1 }),
        serde_json::json!({ "action": "evaluate", "expression": "x", "frame_id": 100 }),
    ];
    for (i, params) in actions.into_iter().enumerate() {
        let out = debug
            .execute(&format!("d-{i}"), params, None, &ctx)
            .await
            .unwrap_or_else(|e| panic!("routed debug action failed: {e}"));
        assert!(out.success, "routed debug action failed: {}", out.output);
    }
    // Session listing reflects the tracked session.
    let out = debug
        .execute(
            "d-sessions",
            serde_json::json!({ "action": "sessions" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.output.contains(&session), "{}", out.output);

    // Terminate via the explicit session param.
    let out = debug
        .execute(
            "d-term",
            serde_json::json!({ "action": "terminate", "session": session }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "{}", out.output);

    // Attach flow works through the same tool.
    let out = debug
        .execute(
            "d-attach",
            serde_json::json!({
                "action": "attach",
                "adapter": "lldb-dap",
                "adapter_command": ["python3", "-c", FAKE_DAP_ADAPTER],
            }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "attach failed: {}", out.output);
    let out = debug
        .execute(
            "d-term2",
            serde_json::json!({ "action": "terminate" }),
            None,
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.success, "{}", out.output);
}

#[tokio::test]
async fn routed_tools_fall_back_to_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    // No runtime services wired: bash/eval/debug must still install (bash is
    // essential) using the legacy implementations, and work.
    let services = minimal_services(&ws);
    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    assert_eq!(manifest.tools.len(), 16);
    for feature in ["shell-session", "eval-kernel", "debug-service"] {
        assert!(
            manifest.degraded.iter().any(|d| d.feature == feature),
            "{feature} must degrade honestly when unwired"
        );
    }

    let bash = installed_tool(&installer, "bash");
    assert!(
        !bash.label().contains("persistent"),
        "unwired host must get the legacy bash variant: {}",
        bash.label()
    );
    let out = bash
        .execute(
            "fb-1",
            serde_json::json!({ "command": "echo legacy-ok" }),
            None,
            &ToolContext::default(),
        )
        .await
        .unwrap();
    assert!(
        out.success && out.output.contains("legacy-ok"),
        "{}",
        out.output
    );

    let eval = installed_tool(&installer, "eval");
    assert!(
        !eval.label().contains("persistent"),
        "unwired host must get the legacy eval variant: {}",
        eval.label()
    );
    let out = eval
        .execute(
            "fb-2",
            serde_json::json!({ "language": "py", "code": "print(6 * 7)" }),
            None,
            &ToolContext::default(),
        )
        .await
        .unwrap();
    assert!(out.success && out.output.contains("42"), "{}", out.output);

    let debug = installed_tool(&installer, "debug");
    let out = debug
        .execute(
            "fb-3",
            serde_json::json!({ "action": "threads" }),
            None,
            &ToolContext::default(),
        )
        .await
        .unwrap();
    assert!(
        out.output.contains("xd://debug") || out.output.contains("guidance") || out.success,
        "scaffold debug must still answer: {}",
        out.output
    );
}
