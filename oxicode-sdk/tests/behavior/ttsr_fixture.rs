//! Family 6 — TTSR contract: the patch carries the host engine and the
//! engine's rule evaluation matches scripted stream deltas.
use crate::common::*;
use oxicode_agent::agent_loop::ttsr::{
    InterruptMode, MatchSource, Rule, RuleRegistry, RuleSource, ScopeToken, TtsrEngine,
    TtsrMatchContext, TtsrSettings,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug, Default)]
struct StaticRules;

impl RuleRegistry for StaticRules {
    fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>> {
        let rule = Rule {
            name: "no-leak-marker".to_string(),
            content: "Do not mention the leak marker.".to_string(),
            description: None,
            condition: vec![regex::Regex::new("LEAK_MARKER").unwrap()],
            scope: vec![ScopeToken::Text],
            interrupt_mode: InterruptMode::Never,
            globs: Vec::new(),
            always_apply: false,
            source: RuleSource::BuiltinDefaults,
            ast_condition: None,
        };
        Box::pin(std::future::ready(vec![rule]))
    }
}

#[test]
fn ttsr_patch_and_rule_retry() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let engine = Arc::new(TtsrEngine::new(
        Arc::new(StaticRules),
        TtsrSettings {
            enabled: true,
            // Rule falls back to the settings mode when its own is
            // `Never`; ProseOnly allows matching assistant text.
            interrupt_mode: InterruptMode::ProseOnly,
            ..Default::default()
        },
    ));
    let services = BehaviorSessionServices::new(ws)
        .with_snapshot_store(Arc::new(oxicode_hashline::InMemorySnapshotStore::new()))
        .with_ttsr_engine(engine.clone() as Arc<TtsrEngine>);

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (_manifest, patch) = install_pack_with_services(&services, &mut installer);

    // The pack's config patch requests the host-provided engine.
    assert!(
        patch.ttsr_engine.is_some(),
        "patch must carry the TTSR engine"
    );

    // Rule evaluation matches the scripted stream delta.
    let ctx = TtsrMatchContext {
        source: MatchSource::Text,
        file_paths: Vec::new(),
        tool_name: None,
        file_contents: Vec::new(),
    };
    let matched = engine.check_delta("mentions LEAK_MARKER mid-stream", &ctx);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].name, "no-leak-marker");
    // Benign deltas do not match — with a fresh buffer (deltas accumulate
    // per source, so the benign turn must not see the earlier match text).
    engine.reset_buffers();
    let benign = engine.check_delta("nothing to see here", &ctx);
    assert!(benign.is_empty());
}
