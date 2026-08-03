//! Integration tests for oxicode-mnemopi Phase 1 (SQLite + FTS5 recall).

use oxicode_mnemopi::{Mnemopi, MnemopiConfig, RecallOptions, RememberOptions, Veracity};

// ── Schema ──────────────────────────────────────────────────────────────

#[test]
fn schema_creates_all_tables() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    assert!(mnemo.db_path().is_none(), "in-memory DB has no path");

    // Verify by inserting and recalling — if schema is broken, these fail.
    let id = mnemo.blocking_remember("test entry", Default::default());
    assert!(!id.is_empty());
}

#[test]
fn fts_triggers_fire_on_insert() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let _id = mnemo.blocking_remember(
        "The quick brown fox jumps over the lazy dog",
        Default::default(),
    );

    // FTS search should find it
    let results = mnemo.recall_blocking("quick fox", RecallOptions::default());
    assert!(!results.is_empty(), "FTS should find the inserted memory");
    assert!(results[0].content.contains("fox"));
}

#[test]
fn fts_triggers_fire_on_content_update() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("Original content about apples", Default::default());

    // Update content — FTS should reflect the change
    mnemo.blocking_update(&id, Some("Updated content about zebras"), None);

    // Old content should not match
    let old_results = mnemo.recall_blocking("apples", RecallOptions::default());
    assert!(
        old_results.is_empty(),
        "old content should not be in FTS index"
    );

    // New content should match
    let new_results = mnemo.recall_blocking("zebras", RecallOptions::default());
    assert!(
        !new_results.is_empty(),
        "new content should be in FTS index"
    );
}

// ── Remember / Get ──────────────────────────────────────────────────────

#[test]
fn remember_and_get_roundtrip() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember(
        "The user prefers dark mode",
        RememberOptions {
            source: Some("test".to_string()),
            importance: Some(0.9),
            veracity: Some(Veracity::True),
            ..Default::default()
        },
    );

    let row = mnemo.blocking_get(&id).expect("memory should exist");
    assert_eq!(row.content, "The user prefers dark mode");
    assert_eq!(row.source.as_deref(), Some("test"));
    assert!((row.importance - 0.9).abs() < 1e-6);
    assert_eq!(row.veracity, Veracity::True);
}

#[test]
fn remember_assigns_unique_ids() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id1 = mnemo.blocking_remember("first memory", Default::default());
    let id2 = mnemo.blocking_remember("second memory", Default::default());
    assert_ne!(id1, id2);
}

// ── Forget ──────────────────────────────────────────────────────────────

#[test]
fn forget_deletes_memory() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("ephemeral memory", Default::default());

    let deleted = mnemo.blocking_forget(&id);
    assert!(deleted);

    assert!(mnemo.blocking_get(&id).is_none());
}

#[test]
fn forget_returns_false_for_missing() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let deleted = mnemo.blocking_forget("nonexistent-id");
    assert!(!deleted);
}

// ── Update ──────────────────────────────────────────────────────────────

#[test]
fn update_changes_content() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("old content", Default::default());

    let updated = mnemo.blocking_update(&id, Some("new content"), None);
    assert!(updated);

    let row = mnemo.blocking_get(&id).expect("row should exist");
    assert_eq!(row.content, "new content");
}

#[test]
fn update_changes_importance() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("important stuff", Default::default());

    mnemo.blocking_update(&id, None, Some(0.95));

    let row = mnemo.blocking_get(&id).expect("row should exist");
    assert!((row.importance - 0.95).abs() < 1e-6);
}

#[test]
fn update_noop_when_nothing_provided() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("unchanged", Default::default());

    let result = mnemo.blocking_update(&id, None, None);
    assert!(result);
}

// ── Recall ──────────────────────────────────────────────────────────────

#[test]
fn recall_returns_relevant_results() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    mnemo.blocking_remember("Rust is a systems programming language", Default::default());
    mnemo.blocking_remember("Python is great for scripting", Default::default());
    mnemo.blocking_remember("The weather is nice today", Default::default());

    let results = mnemo.recall_blocking("Rust programming", RecallOptions::default());
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.content.contains("Rust")));
}

#[test]
fn recall_respects_limit() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    for i in 0..20 {
        mnemo.blocking_remember(&format!("memory entry number {i}"), Default::default());
    }

    let results = mnemo.recall_blocking(
        "memory entry",
        RecallOptions {
            limit: Some(5),
            ..Default::default()
        },
    );
    assert!(results.len() <= 5);
}

#[test]
fn recall_scores_by_importance() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    mnemo.blocking_remember(
        "The cat sat on the mat",
        RememberOptions {
            importance: Some(0.1),
            ..Default::default()
        },
    );
    mnemo.blocking_remember(
        "The cat climbed the tall tree",
        RememberOptions {
            importance: Some(0.95),
            ..Default::default()
        },
    );

    let results = mnemo.recall_blocking("cat", RecallOptions::default());
    assert!(results.len() >= 2);
    // The high-importance memory should score higher
    let high = results
        .iter()
        .find(|r| r.content.contains("tree"))
        .expect("tree result");
    let low = results
        .iter()
        .find(|r| r.content.contains("mat"))
        .expect("mat result");
    assert!(
        high.score > low.score,
        "high importance should score higher (high={}, low={})",
        high.score,
        low.score
    );
}

#[test]
fn recall_includes_signal_breakdown() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    mnemo.blocking_remember("unique searchable phrase", Default::default());

    let results = mnemo.recall_blocking("unique phrase", RecallOptions::default());
    assert!(!results.is_empty());
    let signals = results[0]
        .signals
        .as_ref()
        .expect("signals should be present");
    assert!(signals.fts_matched);
    assert!(signals.fts > 0.0);
}

#[test]
fn recall_empty_for_no_match() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    mnemo.blocking_remember("cats and dogs", Default::default());

    let results = mnemo.recall_blocking("quantum physics relativity", RecallOptions::default());
    assert!(results.is_empty());
}

#[test]
fn recall_updates_recall_count() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("memorable event today", Default::default());

    let _ = mnemo.recall_blocking("memorable", RecallOptions::default());

    let row = mnemo.blocking_get(&id).expect("row should exist");
    let count = row.recall_count.unwrap_or(0);
    assert!(count >= 1, "recall_count should be incremented");
}

// ── Invalidate ──────────────────────────────────────────────────────────

#[test]
fn invalidate_marks_superseded() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("old fact that is now wrong", Default::default());

    let invalidated = mnemo.blocking_invalidate(&id, None);
    assert!(invalidated);

    let row = mnemo.blocking_get(&id).expect("row should exist");
    assert!(row.superseded_by.is_some(), "superseded_by should be set");
}

#[test]
fn invalidate_is_idempotent() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo.blocking_remember("some fact", Default::default());

    assert!(mnemo.blocking_invalidate(&id, None));
    // Second invalidate should return false (already invalidated)
    assert!(!mnemo.blocking_invalidate(&id, None));
}

// ── Stats ───────────────────────────────────────────────────────────────

#[test]
fn stats_reflect_insertions() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    mnemo.blocking_remember(
        "first",
        RememberOptions {
            source: Some("chat".to_string()),
            ..Default::default()
        },
    );
    mnemo.blocking_remember(
        "second",
        RememberOptions {
            source: Some("chat".to_string()),
            ..Default::default()
        },
    );
    mnemo.blocking_remember(
        "third",
        RememberOptions {
            source: Some("tool".to_string()),
            ..Default::default()
        },
    );

    let stats = mnemo.blocking_get_stats();
    assert_eq!(stats.working_count, 3);
    assert_eq!(stats.by_source.get("chat"), Some(&2));
    assert_eq!(stats.by_source.get("tool"), Some(&1));
}

// ── Async facade ────────────────────────────────────────────────────────

#[tokio::test]
async fn async_remember_and_recall() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let _id = mnemo
        .remember(
            "async test memory about databases",
            RememberOptions {
                importance: Some(0.8),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let results = mnemo
        .recall("databases", RecallOptions::default())
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert!(results[0].content.contains("databases"));
}

#[tokio::test]
async fn async_forget_and_verify() {
    let mnemo = Mnemopi::open_in_memory().unwrap();
    let id = mnemo
        .remember("to be deleted", Default::default())
        .await
        .unwrap();

    let deleted = mnemo.forget(&id).await.unwrap();
    assert!(deleted);

    let row = mnemo.get(&id).await.unwrap();
    assert!(row.is_none());
}

// ── Config ──────────────────────────────────────────────────────────────

#[test]
fn custom_session_id_isolates_memories() {
    let cfg1 = MnemopiConfig {
        session_id: "session-a".to_string(),
        ..Default::default()
    };
    let cfg2 = MnemopiConfig {
        session_id: "session-b".to_string(),
        ..Default::default()
    };

    let mnemo_a = Mnemopi::open_in_memory_with_config(cfg1).unwrap();
    let mnemo_b = Mnemopi::open_in_memory_with_config(cfg2).unwrap();

    mnemo_a.blocking_remember("shared keyword test", Default::default());

    // Session B should NOT find session A's memory (different SQLite DBs)
    let results = mnemo_b.recall_blocking("shared keyword", RecallOptions::default());
    assert!(results.is_empty(), "different in-memory DBs are isolated");
}
