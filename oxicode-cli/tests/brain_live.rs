//! Live round-trip against a running oxibrain daemon.
//!
//! Ignored by default — requires the daemon at `~/.oxi/brain/oxibrain.sock`
//! (or `OXIBRAIN_SOCKET`). Run explicitly:
//!
//! ```text
//! cargo nextest run -p oxicode-cli --run-ignored only -E test(brain_live)
//! ```
//!
//! Exercises the real `MemoryBackend` mapping end-to-end:
//! `put` → `ingest`, `search` → `recall`, `delete` → `redact`.

use oxicode::foundation::brain::{BrainMemoryBackend, default_socket_path};
use oxicode_agent::tools::MemoryBackend;

const SPACE: &str = "oxicode-live-selftest";

#[tokio::test]
#[ignore = "requires a running oxibrain daemon"]
async fn put_search_delete_round_trip() {
    let backend = BrainMemoryBackend::new(default_socket_path()).with_scope(SPACE);

    backend.ping().await.expect("daemon reachable");

    // put → ingest(extract:false) → episode id
    let id = backend
        .put(
            "live round-trip marker: oxicode selftest zqx-42",
            "selftest",
            "brain_live",
        )
        .await
        .expect("put succeeds");
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit()),
        "put must return the episode id, got {id:?}"
    );

    // search → recall: the note must surface via the recent-episodes layer
    let hits = backend.search("zqx-42", 5).await.expect("search succeeds");
    let found = hits
        .iter()
        .find(|m| m.content.contains("zqx-42"))
        .expect("recall must surface the freshly ingested note");
    assert_eq!(found.kind, "recent_episodes");

    // delete → redact the exact episode
    backend.delete(&id).await.expect("delete succeeds");

    // and it is gone from recall
    let after = backend
        .search("zqx-42", 5)
        .await
        .expect("search after delete");
    assert!(
        !after.iter().any(|m| m.content.contains("zqx-42")),
        "redacted episode must not surface again"
    );
}
