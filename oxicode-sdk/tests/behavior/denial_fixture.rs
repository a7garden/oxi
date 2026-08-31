//! Family 8 — host denial cannot be bypassed: the interceptor substitutes a
//! structured-deny wrapper for `bash`; the model receives a normal,
//! recoverable tool result and no process ever runs.
use crate::common::*;

#[tokio::test]
async fn host_denial_cannot_be_bypassed() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let marker = ws.join("pwned-marker");

    let mut installer = RecordingInstaller::new(WrapMode::Deny {
        tool: "bash".to_string(),
    });
    install_builtin_pack(&ws, &mut installer);
    let trace = installer.trace.clone();
    let registry = Arc::new(installer.into_registry());

    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedReply::ToolCalls(vec![(
            "bash".into(),
            serde_json::json!({"command": format!("touch {}", marker.display())}),
        )]),
        ScriptedReply::Text("acknowledged the denial".into()),
    ]));

    let config = loop_config(&ws, None, None, None);
    let events = run_scripted_turns(provider, registry, config, "run a command").await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, oxicode_agent::AgentEvent::AgentEnd { .. })),
        "loop must complete after the denial"
    );
    let trace = trace.lock();
    // The denial was surfaced as a normal tool result...
    assert!(
        trace.iter().any(|e| e.starts_with("bash:call_0:denied")),
        "trace: {trace:?}"
    );
    // ...and the real bash never executed.
    assert!(
        !trace.iter().any(|e| e.starts_with("bash:call_0:ok")),
        "denied tool must not execute, trace: {trace:?}"
    );
    assert!(!marker.exists(), "denied command must have no side effects");
}
