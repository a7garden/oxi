//! Coordination demo — work queue, shared memory, consensus.
//!
//! Run: `cargo run -p oxicode-sdk --example coordination`
//!
//! No API key required — this example demonstrates the coordination module only.

use oxicode_sdk::WorkQueueConfig;
use oxicode_sdk::prelude::*;

fn main() {
    // ── Work Queue ──
    let queue = WorkQueue::new(WorkQueueConfig::default());
    let id1 = queue.enqueue("review", serde_json::json!({"file": "main.rs"}), 5);
    let _id2 = queue.enqueue("build", serde_json::json!({"target": "release"}), 3);

    println!("=== Work Queue ===");
    let item = queue.claim("agent-1", None).unwrap();
    println!("Claimed: {} (priority={})", item.work_type, item.priority);
    queue.start(&id1).unwrap();
    queue
        .complete(
            &id1,
            WorkResult {
                success: true,
                content: "LGTM".into(),
                error: None,
                duration_ms: 100,
                tokens_used: None,
            },
        )
        .unwrap();
    println!("Stats: {:?}", queue.stats());

    // ── Shared Memory ──
    println!("\n=== Shared Memory ===");
    let mem = SharedMemory::new();
    let key = MemoryKey::new("reviews", "main-rs");
    let v1 = mem
        .write(&key, serde_json::json!("approved"), "agent-1", None)
        .unwrap();
    println!("Written v{}", v1);
    let v2 = mem
        .write(&key, serde_json::json!("merged"), "agent-2", Some(v1))
        .unwrap();
    println!("Updated v{}", v2);
    let conflict = mem.write(&key, serde_json::json!("conflict"), "agent-3", Some(1));
    println!("Conflict: {:?}", conflict.unwrap_err());
    let val = mem.increment(&MemoryKey::new("counters", "reviews"), 1, "agent-1");
    println!("Counter: {}", val);

    // ── Consensus ──
    println!("\n=== Consensus ===");
    let c = Consensus::new();
    c.start(
        "deploy-vote",
        vec!["agent-1".into(), "agent-2".into(), "agent-3".into()],
        0.5,
    );
    c.vote("deploy-vote", "agent-1", "deploy".into()).unwrap();
    let r = c.vote("deploy-vote", "agent-2", "deploy".into()).unwrap();
    println!(
        "Decided: {} ({}/{})",
        r.decided, r.votes_received, r.total_voters
    );
    println!("Decision: {:?}", r.decision);
}
