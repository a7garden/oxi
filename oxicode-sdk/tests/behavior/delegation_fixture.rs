//! Family 7 — delegation contract: a host-provided `SubagentRunner` receives
//! the child task, and the manifest no longer degrades the delegation
//! extension.
use crate::common::*;
use std::sync::Arc;

#[tokio::test]
async fn child_agent_runner_contract() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let runner = Arc::new(MockSubagentRunner::default());
    let services = BehaviorSessionServices::new(ws.clone())
        .with_snapshot_store(Arc::new(oxicode_hashline::InMemorySnapshotStore::new()))
        .with_subagent_runner(runner.clone() as Arc<dyn oxicode_agent::tools::SubagentRunner>);

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    // Delegation satisfied → no degradation for it.
    assert!(
        !manifest.degraded.iter().any(|d| d.feature == "delegation"),
        "trace: {:?}",
        manifest.degraded
    );

    let trace = installer.trace.clone();
    let registry = Arc::new(installer.into_registry());
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedReply::ToolCalls(vec![(
            "subagent".into(),
            serde_json::json!({"agent": "scout", "task": "find TODOs"}),
        )]),
        ScriptedReply::Text("done".into()),
    ]));

    let config = loop_config(&ws, None, Some(runner.clone()), None);
    let agent_loop = oxicode_agent::AgentLoop::new(
        provider,
        config,
        registry,
        oxicode_agent::SharedState::new(),
    );
    agent_loop
        .run("delegate the search".to_string(), |_| {})
        .await
        .expect("agent loop run");

    // The typed child-task context reached the host runner.
    let prompts = runner.prompts.lock();
    assert_eq!(prompts.len(), 1, "runner must see exactly one child task");
    assert_eq!(prompts[0].0, "scout");
    assert_eq!(prompts[0].1, "find TODOs");

    // And the tool call itself executed through the interceptor.
    assert!(
        trace
            .lock()
            .iter()
            .any(|e| e.starts_with("subagent:call_0:ok"))
    );
}
