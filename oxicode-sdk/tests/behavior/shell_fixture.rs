//! Persistent shell session fixture (`behavior::persistent_shell_session_contract`):
//! with `BehaviorSessionServices.shell_session` wired, the pack manifest
//! stops degrading the `shell-session` extension and the session honors the
//! contract — working directory + environment continuity, prompt cancel
//! (exit 130), reset to the workspace root, bounded output.
use crate::common::*;
use std::time::Duration;

use oxicode_agent::runtime::{PersistentShellSession, ShellSession};
#[tokio::test]
async fn persistent_shell_session_contract() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    let session: Arc<dyn ShellSession> =
        Arc::new(PersistentShellSession::new(ws.clone()).with_max_output(4_096));
    let mut services = minimal_services(&ws);
    services.shell_session = Some(session.clone());

    // Manifest: shell-session satisfied, the other five optional
    // extensions still degrade honestly.
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
    assert!(
        manifest.degraded.iter().any(|d| d.feature == "eval-kernel"),
        "eval-kernel must still degrade"
    );

    // Continuity: cwd and exported env persist across executes.
    let out = session
        .execute("cd sub && export OXI_FIXTURE=1", Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    let out = session
        .execute("echo \"$PWD $OXI_FIXTURE\"", Duration::from_secs(5))
        .await
        .unwrap();
    assert!(out.stdout.contains("sub"), "cwd must persist: {out:?}");
    assert!(
        out.stdout.trim_end().ends_with(" 1"),
        "env must persist: {out:?}"
    );

    // Cancel: group SIGINT aborts the foreground command (trap : INT keeps
    // bash alive) and surfaces as exit code 130, promptly.
    let worker_session = session.clone();
    let worker = tokio::spawn(async move {
        worker_session
            .execute("sleep 30", Duration::from_secs(60))
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.cancel();
    let started = std::time::Instant::now();
    let out = tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .expect("execute must return after cancel")
        .expect("join")
        .expect("execute ok");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "cancel must be prompt"
    );
    assert_eq!(out.exit_code, 130, "SIGINT must surface as 130: {out:?}");

    // Reset: fresh environment rooted at the workspace root.
    session
        .execute("cd /", Duration::from_secs(5))
        .await
        .unwrap();
    session.reset().await.unwrap();
    let out = session
        .execute(
            "pwd && echo \"${OXI_FIXTURE:-unset}\"",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(
        out.stdout.contains(&dir_path_marker(&ws)),
        "reset must return to the workspace root: {out:?}"
    );
    assert!(
        out.stdout.contains("unset"),
        "reset must drop exported state: {out:?}"
    );

    // Bounds: output past the host bound is elided and reported.
    let out = session
        .execute("seq 1 200000", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.truncated, "bound must be reported");
    assert!(
        out.stdout.len() <= 4_096 + 8,
        "output must be bounded: {}",
        out.stdout.len()
    );
}

/// Marker text distinguishing the workspace root from paths a stray `cd`
/// could land on (the tempdir's unique component).
fn dir_path_marker(ws: &std::path::Path) -> String {
    ws.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
