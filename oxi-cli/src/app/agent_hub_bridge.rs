//! Bridges `AgentSession` to the Agent Hub display registry.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use oxi_sdk::{HubKind, HubStatus};
use regex::Regex;

use super::agent_hub_registry::{HubEntry, HubRegistry, now_ms};

/// Recognises the main session file stem produced by `SessionManager`.
///
/// Real naming pattern (from `oxi-cli/src/store/session.rs:2166-2170`):
///   file_timestamp = RFC3339 with `:` `.` `T` `-` `+` replaced by `-`
///   short_id       = first 8 chars of the session UUID
///   stem           = `"{file_timestamp}_{short_id}"`
///
/// Examples that MUST match (from production data):
///   `2026-07-29-14-30-00-00-00_a1b2c3d4`
///   `2026-07-29-14-30-00-06-00_deadbeef`
///
/// Examples that MUST NOT match:
///   `__advisor`, `sub-a`, `01HXY` (UUID-style alone is not a main file),
///   `2026-07-29.jsonl` (no underscore + 8hex suffix).
///
/// The pattern is: 4-digit year, then any sequence of digits/dashes, then
/// `_` + 8 lowercase hex chars. Stored as a `LazyLock` to avoid paying
/// the compile cost on every call.
static MAIN_SESSION_STEM_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}.*_[0-9a-f]{8}$")
        .expect("MAIN_SESSION_STEM_REGEX pattern is a literal; must compile")
});

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

fn is_main_session_stem(stem: &str) -> bool {
    MAIN_SESSION_STEM_REGEX.is_match(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_sdk::{HubKind, HubStatus};

    #[test]
    fn registers_subagent_jsonl_excluding_main_and_advisor() {
        let dir = tempfile::tempdir().unwrap();
        // Real main session file pattern: `{rfc3339-derived}_{8hex}`.
        // See `oxi-cli/src/store/session.rs:2166-2170`.
        std::fs::write(
            dir.path().join("2026-07-29-14-30-00-00-00_a1b2c3d4.jsonl"),
            "{}\n",
        )
        .unwrap();
        // Advisor transcript — reserved stem.
        std::fs::write(dir.path().join("__advisor.jsonl"), "{}\n").unwrap();
        // Subagent transcripts (2).
        std::fs::write(dir.path().join("sub-a.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("sub-b.jsonl"), "{}\n").unwrap();
        // Non-JSONL fixture must be ignored.
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
    fn main_session_stem_heuristic_matches_real_pattern() {
        // Real production names.
        assert!(is_main_session_stem("2026-07-29-14-30-00-00-00_a1b2c3d4"));
        assert!(is_main_session_stem("2026-07-29-14-30-00-06-00_deadbeef"));
        // Subagent slugs MUST NOT match.
        assert!(!is_main_session_stem("sub-a"));
        assert!(!is_main_session_stem("sub-b"));
        // Reserved advisor stem MUST NOT match (handled separately by the
        // hard-coded `__advisor` check).
        assert!(!is_main_session_stem("__advisor"));
        // Bare UUID-style / ULID MUST NOT match — only the timestamped
        // `{rfc3339}_{8hex}` pattern is recognised.
        assert!(!is_main_session_stem("01HXY"));
        assert!(!is_main_session_stem("01HXYABCDEFG"));
        // No underscore + 8hex suffix → not a main session file.
        assert!(!is_main_session_stem("2026-07-29"));
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
