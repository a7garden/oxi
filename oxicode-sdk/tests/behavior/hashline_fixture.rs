//! Family 1 — hashline read/edit with stale-anchor recovery after a
//! concurrent file change (ledger: read-write-search + hashline-anchors).
use crate::common::*;

/// 16-hex DefaultHasher content hash — same algorithm as
/// `oxicode_agent::tools::edit` conflict detection (edit.rs:678-681).
fn content_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[tokio::test]
async fn hashline_read_edit_stale_anchor_recovery() {
    let (dir, file) = workspace_with_lib_rs();
    let ws = dir.path().to_path_buf();

    // Content states, all precomputed (deterministic).
    let c0 = "fn main() {}\n";
    let c1 = "fn main() { changed(); }\n";
    let c2 = "fn main() { changed(); fixed(); }\n";
    let h0 = content_hash(c0);
    let h1 = content_hash(c1);

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    install_builtin_pack(&ws, &mut installer);
    let trace = installer.trace.clone();
    let registry = Arc::new(installer.into_registry());

    let file_str = file.to_string_lossy().to_string();
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedReply::ToolCalls(vec![("read".into(), serde_json::json!({"path": file_str}))]),
        ScriptedReply::ToolCalls(vec![(
            "edit".into(),
            serde_json::json!({
                "path": file_str,
                "old_text": "fn main() {}",
                "new_text": "fn main() { changed(); }",
                "expected_hash": h0,
            }),
        )]),
        // Stale anchor: same hash, but content moved on during turn 2.
        ScriptedReply::ToolCalls(vec![(
            "edit".into(),
            serde_json::json!({
                "path": file_str,
                "old_text": "fn main() {}",
                "new_text": "fn main() { changed(); }",
                "expected_hash": h0,
            }),
        )]),
        // Recovery: re-read to refresh the anchor.
        ScriptedReply::ToolCalls(vec![("read".into(), serde_json::json!({"path": file_str}))]),
        ScriptedReply::ToolCalls(vec![(
            "edit".into(),
            serde_json::json!({
                "path": file_str,
                "old_text": "fn main() { changed(); }",
                "new_text": "fn main() { changed(); fixed(); }",
                "expected_hash": h1,
            }),
        )]),
        ScriptedReply::Text("done".into()),
    ]));

    let config = loop_config(
        &ws,
        Some(Arc::new(oxicode_hashline::InMemorySnapshotStore::new())
            as Arc<dyn oxicode_hashline::SnapshotStore>),
        None,
        None,
    );
    let events = run_scripted_turns(provider, registry, config, "fix lib.rs").await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, oxicode_agent::AgentEvent::AgentEnd { .. })),
        "loop must run to completion"
    );

    let trace = trace.lock();
    // Call order: read, edit(ok), edit(conflict), read, edit(ok).
    let tools: Vec<&str> = trace.iter().filter_map(|e| e.split(':').next()).collect();
    assert_eq!(
        tools,
        vec!["read", "edit", "edit", "read", "edit"],
        "trace: {trace:?}"
    );

    // Edit #2 hit the stale anchor: structured conflict, not a hard error.
    assert!(
        trace[2].contains("File has been modified since last read"),
        "stale anchor must surface the re-read guidance, got: {}",
        trace[2]
    );
    // Reads emitted hashline anchors ([<path>#TAG] — path may be absolute).
    assert!(
        trace[0].contains("lib.rs#"),
        "read must emit [path#TAG]: {}",
        trace[0]
    );
    assert!(
        trace[1].contains("applied") || !trace[1].contains("err"),
        "trace: {trace:?}"
    );

    // Final content is exactly the third state.
    let final_content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(final_content, c2);
}
