//! Honest-degradation fixture: with minimal services the manifest must
//! report shell/eval/DAP as Unavailable and LSP/TTSR/delegation as
//! unsatisfied optional extensions.
use crate::common::*;

#[test]
fn degradation_report_is_honest() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&minimal_services(&ws), &mut installer);

    // All 16 canonical tools installed (legacy implementations for
    // bash/eval/debug still ship; the *extensions* degrade).
    let mut names: Vec<&str> = installer.installed.iter().map(String::as_str).collect();
    names.sort_unstable();
    let mut expected: Vec<&str> = [
        "read",
        "write",
        "edit",
        "bash",
        "grep",
        "find",
        "ls",
        "ast_grep",
        "ast_edit",
        "web_search",
        "get_search_results",
        "todo",
        "subagent",
        "lsp",
        "eval",
        "debug",
    ]
    .into();
    expected.sort_unstable();
    assert_eq!(names, expected);

    // Degradations: the six unsatisfied optional extensions.
    let mut degraded: Vec<&str> = manifest
        .degraded
        .iter()
        .map(|d| d.feature.as_str())
        .collect();
    degraded.sort_unstable();
    let mut expected_degraded = vec![
        "debug-service",
        "delegation",
        "eval-kernel",
        "lsp-host",
        "shell-session",
        "ttsr-engine",
    ];
    expected_degraded.sort_unstable();
    assert_eq!(degraded, expected_degraded);

    // Ledger: 9 entries, worst status Unavailable — never advertised as
    // fully OMP-equivalent while Unavailable rows remain.
    assert_eq!(manifest.compatibility.entries.len(), 9);
    assert_eq!(
        manifest.compatibility_level(),
        oxicode_sdk::behavior::FeatureStatus::Unavailable
    );
    for entry in manifest
        .compatibility
        .entries
        .iter()
        .filter(|e| e.status == oxicode_sdk::behavior::FeatureStatus::Unavailable)
    {
        assert!(
            entry.evidence.is_empty(),
            "{} claims no evidence while Unavailable",
            entry.feature
        );
    }
    // Extension degradations name their affected tools.
    let shell = manifest
        .degraded
        .iter()
        .find(|d| d.feature == "shell-session")
        .unwrap();
    assert_eq!(shell.affected_tools, vec!["bash"]);
}
