//! In-process `EventBus` impls.
//!
//! - `InProcessEventBus`: tokio broadcast channel (no broker, no network)
//! - `NullEventBus`: accepts publish, drops everything (for tests)

use async_trait::async_trait;
use std::sync::Arc;

use crate::SdkError;
use crate::ports::{EventBus, EventPayload, EventTopic, SubscriptionHandle};

/// In-process event bus using `tokio::sync::broadcast`.
///
/// Capacity is the broadcast buffer. Slow consumers may lag.
pub struct InProcessEventBus {
    tx: tokio::sync::broadcast::Sender<(EventTopic, EventPayload)>,
}

impl std::fmt::Debug for InProcessEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessEventBus").finish()
    }
}

impl InProcessEventBus {
    /// Create a new bus with the given broadcast capacity.
    pub fn new(capacity: usize) -> Arc<Self> {
        let (tx, _) = tokio::sync::broadcast::channel(capacity);
        Arc::new(Self { tx })
    }
}

#[async_trait]
impl EventBus for InProcessEventBus {
    async fn publish(&self, topic: &EventTopic, payload: EventPayload) -> Result<(), SdkError> {
        // Best-effort: no active subscribers is not an error.
        let _ = self.tx.send((topic.clone(), payload));
        Ok(())
    }

    async fn subscribe(&self, _topic: &EventTopic) -> Result<SubscriptionHandle, SdkError> {
        let mut rx = self.tx.subscribe();
        let (tx, rx2) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(SubscriptionHandle::from_receiver(rx2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn publish_then_receive() {
        let bus = InProcessEventBus::new(8);
        bus.publish(&"kernel.agent.started".to_string(), json!({"id": "a1"}))
            .await
            .unwrap();
        let mut sub = bus.subscribe(&"kernel".to_string()).await.unwrap();
        bus.publish(&"kernel.tool.completed".to_string(), json!({"k": 1}))
            .await
            .unwrap();
        let (topic, payload) = sub.recv().await.unwrap();
        assert_eq!(topic, "kernel.tool.completed");
        assert_eq!(payload, json!({"k": 1}));
    }

    #[tokio::test]
    async fn multiple_subscribers() {
        let bus = InProcessEventBus::new(8);
        let mut s1 = bus.subscribe(&"x".to_string()).await.unwrap();
        let mut s2 = bus.subscribe(&"x".to_string()).await.unwrap();
        bus.publish(&"x".to_string(), json!(1)).await.unwrap();
        let (_, p1) = s1.recv().await.unwrap();
        let (_, p2) = s2.recv().await.unwrap();
        assert_eq!(p1, json!(1));
        assert_eq!(p2, json!(1));
    }
}
