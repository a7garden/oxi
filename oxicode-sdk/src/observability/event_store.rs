//! Event sourcing — append-only event store with streams and replay.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;

/// Configuration for the event store.
#[derive(Debug, Clone)]
pub struct EventStoreConfig {
    /// Maximum entries per stream before eviction.
    pub max_entries_per_stream: usize,
}

impl Default for EventStoreConfig {
    fn default() -> Self {
        Self {
            max_entries_per_stream: 10_000,
        }
    }
}

/// A stored domain event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Monotonically increasing sequence number.
    pub sequence: u64,
    /// Logical stream this event belongs to.
    pub stream_id: String,
    /// Event type name.
    pub event_type: String,
    /// Arbitrary payload.
    pub payload: serde_json::Value,
    /// Wall-clock time (ms since epoch).
    pub timestamp_ms: u64,
}

/// Filter for querying events.
#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    /// Filter by stream.
    pub stream_id: Option<String>,
    /// Filter by event type.
    pub event_type: Option<String>,
    /// Filter by minimum sequence.
    pub min_sequence: Option<u64>,
    /// Filter by maximum sequence.
    pub max_sequence: Option<u64>,
}

/// Append-only event store with per-stream indexing.
///
/// Internally uses a flat sequence (no nested streams), filtered at query time.
pub struct EventStore {
    /// Monotonically increasing global sequence counter.
    sequence: AtomicU64,
    /// Append-only flat event log.
    events: parking_lot::RwLock<Vec<StoredEvent>>,
    /// Per-stream sequence cursor.
    stream_cursors: parking_lot::RwLock<HashMap<String, u64>>,
    /// Broadcast channel for subscribers.
    tx: broadcast::Sender<StoredEvent>,
    config: EventStoreConfig,
}

impl Default for EventStore {
    fn default() -> Self {
        Self::new(EventStoreConfig::default())
    }
}

impl EventStore {
    /// Create a new event store with the given configuration.
    pub fn new(config: EventStoreConfig) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            sequence: AtomicU64::new(1),
            events: parking_lot::RwLock::new(Vec::new()),
            stream_cursors: parking_lot::RwLock::new(HashMap::new()),
            tx,
            config,
        }
    }

    /// Append an event to a stream. Returns the assigned sequence number.
    pub fn append(
        &self,
        stream_id: impl Into<String>,
        event_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> u64 {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        let stream_id = stream_id.into();

        let event = StoredEvent {
            sequence,
            stream_id: stream_id.clone(),
            event_type: event_type.into(),
            payload,
            timestamp_ms: now_ms(),
        };

        {
            let mut events = self.events.write();
            events.push(event.clone());

            // Simple eviction: keep at most max_entries_per_stream overall
            if events.len() > self.config.max_entries_per_stream {
                let drain_count = events.len() - self.config.max_entries_per_stream;
                events.drain(0..drain_count);
            }
        }

        {
            let mut cursors = self.stream_cursors.write();
            let _current = cursors.get(&stream_id).copied().unwrap_or(0);
            cursors.insert(stream_id, sequence);
        }

        let _ = self.tx.send(event);
        sequence
    }

    /// Query events matching the filter.
    pub fn query(&self, query: EventQuery) -> Vec<StoredEvent> {
        self.events
            .read()
            .iter()
            .filter(|e| {
                if let Some(ref sid) = query.stream_id {
                    &e.stream_id == sid
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(ref et) = query.event_type {
                    &e.event_type == et
                } else {
                    true
                }
            })
            .filter(|e| query.min_sequence.map(|m| e.sequence >= m).unwrap_or(true))
            .filter(|e| query.max_sequence.map(|m| e.sequence <= m).unwrap_or(true))
            .cloned()
            .collect()
    }

    /// Replay all events for a particular stream, in order.
    pub fn replay(&self, stream_id: &str) -> Vec<StoredEvent> {
        self.events
            .read()
            .iter()
            .filter(|e| e.stream_id == stream_id)
            .cloned()
            .collect()
    }

    /// Subscribe to new events.
    pub fn subscribe(&self) -> broadcast::Receiver<StoredEvent> {
        self.tx.subscribe()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_store_append_returns_sequence() {
        let store = EventStore::default();
        let seq = store.append("s1", "Created", serde_json::json!({}));
        assert_eq!(seq, 1);

        let seq2 = store.append("s1", "Updated", serde_json::json!({}));
        assert_eq!(seq2, 2);
    }

    #[test]
    fn event_store_query_by_stream() {
        let store = EventStore::default();
        store.append("s1", "A", serde_json::json!({"n": 1}));
        store.append("s2", "B", serde_json::json!({"n": 2}));
        store.append("s1", "C", serde_json::json!({"n": 3}));

        let results = store
            .query(EventQuery {
                stream_id: Some("s1".into()),
                ..Default::default()
            })
            .into_iter()
            .map(|e| e.event_type)
            .collect::<Vec<_>>();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&"A".into()));
        assert!(results.contains(&"C".into()));
    }

    #[test]
    fn event_store_query_by_event_type() {
        let store = EventStore::default();
        store.append("s1", "Click", serde_json::json!({}));
        store.append("s1", "Hover", serde_json::json!({}));
        store.append("s1", "Click", serde_json::json!({}));

        let results = store
            .query(EventQuery {
                event_type: Some("Click".into()),
                ..Default::default()
            })
            .len();

        assert_eq!(results, 2);
    }

    #[test]
    fn event_store_replay() {
        let store = EventStore::default();
        store.append("order-1", "Created", serde_json::json!({"id": 1}));
        store.append("order-1", "Paid", serde_json::json!({"amount": 100}));
        store.append("order-2", "Created", serde_json::json!({"id": 2}));

        let events = store.replay("order-1");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "Created");
        assert_eq!(events[1].event_type, "Paid");
    }
}
