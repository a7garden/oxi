//! Bridges `AgentSession` to the Agent Hub display registry.

use std::path::{Path, PathBuf};

use oxi_sdk::{HubKind, HubStatus};

use super::agent_hub_registry::{HubEntry, HubRegistry, now_ms};

/// Register persisted JSONL transcripts as parked subagents.
pub fn register_persisted_subagents(hub: &HubRegistry, session_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(session_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem == "__advisor" || is_main_session_stem(stem) {
            continue;
        }

        let last_activity_ms = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_else(now_ms);
        hub.register(
            stem.to_owned(),
            HubEntry {
                kind: HubKind::Subagent,
                status: HubStatus::Parked,
                display_name: stem.to_owned(),
                current_task: None,
                last_activity_ms,
                session_file: Some(path),
            },
        );
    }
}

/// Register the optional advisor runtime and its transcript.
pub fn register_advisor(hub: &HubRegistry, transcript_path: Option<PathBuf>) {
    let last_activity_ms = transcript_path
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_else(now_ms);
    hub.register(
        "advisor".into(),
        HubEntry {
            kind: HubKind::Advisor,
            status: HubStatus::Idle,
            display_name: "Advisor".into(),
            current_task: None,
            last_activity_ms,
            session_file: transcript_path,
        },
    );
}

fn is_main_session_stem(_stem: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_sdk::{HubKind, HubStatus};

    #[test]
    fn registers_subagent_jsonl_excluding_main_and_advisor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("__advisor.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("sub-a.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("sub-b.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored\n").unwrap();

        let hub = HubRegistry::new();
        register_persisted_subagents(&hub, dir.path());

        let snap = hub.snapshot();
        let mut ids: Vec<&str> = snap.iter().map(|(id, _)| id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["sub-a", "sub-b"]);
        for (id, entry) in &snap {
            assert_eq!(entry.kind, HubKind::Subagent);
            assert_eq!(entry.status, HubStatus::Parked);
            assert_eq!(entry.display_name, *id);
            assert_eq!(
                entry.session_file,
                Some(dir.path().join(format!("{id}.jsonl")))
            );
            assert!(entry.last_activity_ms > 0);
        }
    }

    #[test]
    fn empty_dir_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let hub = HubRegistry::new();
        register_persisted_subagents(&hub, dir.path());
        assert!(hub.snapshot().is_empty());
    }

    #[test]
    fn missing_dir_is_noop() {
        let hub = HubRegistry::new();
        register_persisted_subagents(&hub, Path::new("/nonexistent/dir/12345"));
        assert!(hub.snapshot().is_empty());
    }
}
