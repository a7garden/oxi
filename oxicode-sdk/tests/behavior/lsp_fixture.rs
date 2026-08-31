//! Family 2 (Partial scope) — LSP actions route through the host-provided
//! `LspProvider` via the pack-installed `lsp` tool.
use crate::common::*;
use std::sync::Arc;

#[tokio::test]
async fn lsp_mock_actions() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let lsp = Arc::new(MockLspProvider::default());
    let services = BehaviorSessionServices::new(ws.clone())
        .with_snapshot_store(Arc::new(oxicode_hashline::InMemorySnapshotStore::new()))
        .with_lsp(lsp.clone() as Arc<dyn oxicode_agent::tools::LspProvider>);

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, patch) = install_pack_with_services(&services, &mut installer);
    assert!(!manifest.degraded.iter().any(|d| d.feature == "lsp-host"));
    assert!(patch.lsp.is_some(), "patch must carry the LSP provider");

    let trace = installer.trace.clone();
    let registry = Arc::new(installer.into_registry());
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedReply::ToolCalls(vec![(
            "lsp".into(),
            serde_json::json!({"action": "status"}),
        )]),
        ScriptedReply::Text("done".into()),
    ]));

    let config = loop_config(&ws, None, None, Some(lsp.clone()));
    let agent_loop = oxicode_agent::AgentLoop::new(
        provider,
        config,
        registry,
        oxicode_agent::SharedState::new(),
    );
    agent_loop
        .run("check lsp".to_string(), |_| {})
        .await
        .unwrap();

    // The action reached the host provider and the result came back through
    // the pack-installed tool.
    assert_eq!(lsp.actions.lock().len(), 1);
    assert!(
        trace
            .lock()
            .iter()
            .any(|e| e.starts_with("lsp:call_0:ok:mock-lsp:"))
    );
}
