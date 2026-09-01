//! Persistent eval kernel fixture (`behavior::persistent_eval_kernel_contract`):
//! with `BehaviorSessionServices.eval_kernels` wired, the pack manifest
//! stops degrading the `eval-kernel` extension and both bundled kernels
//! honor the contract — state continuity across cells, error capture, and
//! explicit reset.
use crate::common::*;
use oxicode_agent::runtime::{EvalKernel, EvalLanguage, JavaScriptEvalKernel, PythonEvalKernel};
use std::time::Duration;

#[tokio::test]
async fn persistent_eval_kernel_contract() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let python: Arc<dyn EvalKernel> = Arc::new(PythonEvalKernel::new());
    let js: Arc<dyn EvalKernel> = Arc::new(JavaScriptEvalKernel::new());
    let mut services = minimal_services(&ws);
    services.eval_kernels = vec![python.clone(), js.clone()];

    // Manifest: eval-kernel satisfied, shell/debug still degrade honestly.
    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    assert!(
        !manifest.degraded.iter().any(|d| d.feature == "eval-kernel"),
        "eval-kernel must be satisfied: {:?}",
        manifest.degraded
    );
    assert!(
        manifest
            .degraded
            .iter()
            .any(|d| d.feature == "shell-session"),
        "shell-session must still degrade"
    );

    // Python: state continuity — a value defined in one cell is visible in
    // the next.
    let out = python
        .execute("fixture_x = 41", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.error.is_none(), "python cell 1 failed: {out:?}");
    let out = python
        .execute("print(fixture_x + 1)", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.error.is_none(), "python cell 2 failed: {out:?}");
    assert!(
        out.stdout.contains("42"),
        "python state must persist across cells: {out:?}"
    );

    // Python: errors are captured, not surfaced as transport failures.
    let out = python
        .execute("1 / 0", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(
        out.error
            .as_deref()
            .is_some_and(|e| e.contains("ZeroDivisionError")),
        "python must capture ZeroDivisionError: {out:?}"
    );

    // Python: reset drops kernel state.
    python.reset().await.unwrap();
    let out = python
        .execute("print(fixture_x)", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(
        out.error
            .as_deref()
            .is_some_and(|e| e.contains("NameError")),
        "reset must drop python state: {out:?}"
    );

    // JavaScript: same contract — continuity, error capture, reset.
    assert_eq!(js.language(), EvalLanguage::JavaScript);
    let out = js
        .execute("globalThis.fixtureJ = 5;", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.error.is_none(), "js cell 1 failed: {out:?}");
    let out = js
        .execute(
            "console.log(globalThis.fixtureJ * 2)",
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "js cell 2 failed: {out:?}");
    assert!(
        out.stdout.contains("10"),
        "js state must persist across cells: {out:?}"
    );
    js.reset().await.unwrap();
    let out = js
        .execute("console.log(fixtureJ)", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.error.is_some(), "reset must drop js state: {out:?}");
}
