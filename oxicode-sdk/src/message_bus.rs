//! Inter-agent message bus for multi-agent communication.
//!
//! Provides a broadcast-based message bus that agents can use to
//! communicate with each other in an oxicode environment.
//!
//! # Lag Handling
//!
//! The underlying `tokio::sync::broadcast` channel has a fixed capacity.
//! Slow consumers will have old messages automatically dropped. Use
//! [`MessageBus::subscribe_lag_aware`] to receive a [`LagAwareReceiver`]
//! that logs a warning when messages are skipped due to lagging.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

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

/// Result of a publish operation on the message bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    /// Message was delivered to `n` active subscribers.
    Delivered {
        /// Number of subscribers that received the message.
        n: usize,
    },
    /// Message was dropped because there were no active subscribers.
    NoSubscribers,
}

impl PublishResult {
    /// Returns the number of subscribers that received the message, or 0 if
    /// there were no subscribers.
    pub fn delivered_count(&self) -> usize {
        match self {
            PublishResult::Delivered { n } => *n,
            PublishResult::NoSubscribers => 0,
        }
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
    /// Returns a [`PublishResult`] indicating how many receivers received the
    /// message, or whether the message was dropped due to no subscribers.
    /// A warning is logged when messages are dropped.
    pub fn publish(&self, msg: InterAgentMessage) -> PublishResult {
        match self.sender.send(msg) {
            Ok(n) => PublishResult::Delivered { n },
            Err(broadcast::error::SendError(msg)) => {
                tracing::warn!(
                    from = %msg.from,
                    message_type = %msg.message_type,
                    "MessageBus publish dropped message: no active subscribers"
                );
                PublishResult::NoSubscribers
            }
        }
    }

    /// Subscribe to all messages on the bus.
    ///
    /// **Warning**: The raw broadcast receiver will silently drop messages if
    /// the receiver lags behind. Consider using [`subscribe_lag_aware`] instead.
    ///
    /// [`subscribe_lag_aware`]: MessageBus::subscribe_lag_aware
    pub fn subscribe(&self) -> broadcast::Receiver<InterAgentMessage> {
        self.sender.subscribe()
    }

    /// Subscribe with automatic lag handling.
    ///
    /// Returns a [`LagAwareReceiver`] that logs a warning when messages are
    /// skipped due to the receiver falling behind.
    pub fn subscribe_lag_aware(&self) -> LagAwareReceiver {
        LagAwareReceiver {
            inner: self.sender.subscribe(),
            total_skipped: std::sync::atomic::AtomicU64::new(0),
        }
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

/// A broadcast receiver that logs warnings when messages are dropped due to
/// lagging instead of silently losing them.
///
/// Obtained via [`MessageBus::subscribe_lag_aware`].
pub struct LagAwareReceiver {
    inner: broadcast::Receiver<InterAgentMessage>,
    total_skipped: std::sync::atomic::AtomicU64,
}

impl LagAwareReceiver {
    /// Receive the next message.
    ///
    /// If the receiver has fallen behind and messages were skipped, a warning
    /// is logged and the next available message is returned.
    ///
    /// Returns `None` if all senders have been dropped (channel closed).
    pub async fn recv(&mut self) -> Option<InterAgentMessage> {
        loop {
            match self.inner.recv().await {
                Ok(msg) => return Some(msg),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let prev = self
                        .total_skipped
                        .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        skipped_now = n,
                        total_skipped = prev + n,
                        "MessageBus receiver lagged — messages were dropped"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Try to receive a message without waiting.
    ///
    /// Returns the message if available, or an indication of why no message
    /// is available.
    pub fn try_recv(&mut self) -> Result<InterAgentMessage, broadcast::error::TryRecvError> {
        loop {
            match self.inner.try_recv() {
                Ok(msg) => return Ok(msg),
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    let prev = self
                        .total_skipped
                        .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        skipped_now = n,
                        total_skipped = prev + n,
                        "MessageBus receiver lagged — messages were dropped"
                    );
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Returns the total number of messages skipped due to lagging since this
    /// receiver was created.
    pub fn total_skipped(&self) -> u64 {
        self.total_skipped
            .load(std::sync::atomic::Ordering::Relaxed)
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
        let msg =
            InterAgentMessage::broadcast("agent-1", "status_update", json!({"status": "idle"}));
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
        let result = bus.publish(msg.clone());
        assert_eq!(result.delivered_count(), 1);

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
        let result = bus.publish(msg);
        assert_eq!(result, PublishResult::Delivered { n: 2 });

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn test_message_bus_no_subscribers() {
        let bus = MessageBus::new(16);
        // No subscribers — publish should return NoSubscribers.
        let msg = InterAgentMessage::broadcast("agent-1", "ping", json!("pong"));
        let result = bus.publish(msg);
        assert_eq!(result, PublishResult::NoSubscribers);
    }

    #[test]
    fn test_message_serialization() {
        let msg = InterAgentMessage::direct("a", "b", "test", json!({"key": "value"}));
        let json_str = serde_json::to_string(&msg).unwrap();
        let deserialized: InterAgentMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.from, "a");
        assert_eq!(deserialized.to, Some("b".to_string()));
    }

    #[tokio::test]
    async fn test_lag_aware_receiver() {
        let bus = MessageBus::new(2);
        let mut rx = bus.subscribe_lag_aware();

        // Publish 5 messages to a capacity-2 channel.
        for i in 0..5 {
            bus.publish(InterAgentMessage::broadcast("sender", "test", json!(i)));
        }

        // LagAwareReceiver should still return available messages after logging lag.
        let mut received = Vec::new();
        for _ in 0..3 {
            match rx.try_recv() {
                Ok(msg) => received.push(msg),
                Err(_) => break,
            }
        }

        // We should get at least some messages (the most recent ones).
        assert!(!received.is_empty());
        // Some messages were skipped due to lagging.
        assert!(rx.total_skipped() > 0);
    }

    #[tokio::test]
    async fn test_lag_aware_receiver_recv() {
        let bus = MessageBus::new(4);
        let mut rx = bus.subscribe_lag_aware();

        bus.publish(InterAgentMessage::broadcast("a", "ping", json!(1)));
        bus.publish(InterAgentMessage::broadcast("a", "pong", json!(2)));

        let msg = rx.recv().await.expect("should receive");
        assert_eq!(msg.message_type, "ping");
        let msg = rx.recv().await.expect("should receive");
        assert_eq!(msg.message_type, "pong");
    }

    #[test]
    fn test_publish_result_delivered_count() {
        assert_eq!(PublishResult::Delivered { n: 3 }.delivered_count(), 3);
        assert_eq!(PublishResult::NoSubscribers.delivered_count(), 0);
    }
}
