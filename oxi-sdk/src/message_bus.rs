//! Inter-agent message bus for multi-agent communication.
//!
//! Provides a broadcast-based message bus that agents can use to
//! communicate with each other in an oxios environment.

use tokio::sync::broadcast;
use serde::{Deserialize, Serialize};

/// A message sent between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterAgentMessage {
    /// Sender agent ID.
    pub from: String,
    /// Recipient agent ID. `None` means broadcast to all subscribers.
    pub to: Option<String>,
    /// Message type (e.g. "task_complete", "delegation", "status").
    pub message_type: String,
    /// Message payload (arbitrary JSON).
    pub payload: serde_json::Value,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl InterAgentMessage {
    /// Create a new directed message.
    pub fn direct(
        from: impl Into<String>,
        to: impl Into<String>,
        message_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            from: from.into(),
            to: Some(to.into()),
            message_type: message_type.into(),
            payload,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Create a broadcast message.
    pub fn broadcast(
        from: impl Into<String>,
        message_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            from: from.into(),
            to: None,
            message_type: message_type.into(),
            payload,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Check if this message is intended for the given agent.
    pub fn is_for(&self, agent_id: &str) -> bool {
        self.to.as_deref() == Some(agent_id) || self.to.is_none()
    }
}

/// Broadcast-based message bus for inter-agent communication.
///
/// Agents subscribe to the bus and receive messages addressed to them
/// or broadcast messages. Thread-safe and async-compatible.
#[derive(Clone)]
pub struct MessageBus {
    sender: broadcast::Sender<InterAgentMessage>,
    capacity: usize,
}

impl MessageBus {
    /// Create a new message bus with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self {
            sender: tx,
            capacity,
        }
    }

    /// Publish a message to the bus.
    ///
    /// Returns the number of receivers that received the message.
    pub fn publish(&self, msg: InterAgentMessage) -> usize {
        self.sender.send(msg).unwrap_or(0)
    }

    /// Subscribe to all messages on the bus.
    pub fn subscribe(&self) -> broadcast::Receiver<InterAgentMessage> {
        self.sender.subscribe()
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Get the configured capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_direct_message() {
        let msg = InterAgentMessage::direct(
            "agent-1",
            "agent-2",
            "task_complete",
            json!({"result": "ok"}),
        );
        assert_eq!(msg.from, "agent-1");
        assert_eq!(msg.to, Some("agent-2".to_string()));
        assert!(msg.is_for("agent-2"));
        assert!(!msg.is_for("agent-1"));
        assert!(!msg.is_for("agent-3"));
    }

    #[test]
    fn test_broadcast_message() {
        let msg = InterAgentMessage::broadcast(
            "agent-1",
            "status_update",
            json!({"status": "idle"}),
        );
        assert_eq!(msg.from, "agent-1");
        assert!(msg.to.is_none());
        assert!(msg.is_for("agent-2"));
        assert!(msg.is_for("agent-3"));
    }

    #[tokio::test]
    async fn test_message_bus_pub_sub() {
        let bus = MessageBus::new(16);
        let mut rx = bus.subscribe();

        let msg = InterAgentMessage::broadcast("agent-1", "ping", json!("pong"));
        bus.publish(msg.clone());

        let received = rx.try_recv().expect("should receive message");
        assert_eq!(received.from, "agent-1");
        assert_eq!(received.message_type, "ping");
    }

    #[tokio::test]
    async fn test_message_bus_multiple_subscribers() {
        let bus = MessageBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        assert_eq!(bus.subscriber_count(), 2);

        let msg = InterAgentMessage::broadcast("coordinator", "start", json!({}));
        let count = bus.publish(msg);
        assert_eq!(count, 2);

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn test_message_serialization() {
        let msg = InterAgentMessage::direct("a", "b", "test", json!({"key": "value"}));
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: InterAgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.from, "a");
        assert_eq!(deserialized.to, Some("b".to_string()));
    }
}
