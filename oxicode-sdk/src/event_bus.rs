//! Generic event bus for pub/sub communication.
//!
//! Provides a type-safe, broadcast-based event bus that works with any
//! event type implementing `Clone + Send + 'static`. This is the
//! generalized version of the kernel's `EventBus<KernelEvent>`.
//!
//! # Example
//!
//! ```ignore
//! use oxicode_sdk::EventBus;
//!
//! #[derive(Debug, Clone)]
//! struct MyEvent { name: String }
//!
//! let bus: EventBus<MyEvent> = EventBus::new(256);
//! let mut rx = bus.subscribe();
//! bus.publish(MyEvent { name: "hello".into() }).unwrap();
//! let event = rx.try_recv().unwrap();
//! assert_eq!(event.name, "hello");
//! ```

use std::fmt;

/// Generic broadcast-based event bus.
///
/// Wraps `tokio::sync::broadcast` for type-safe pub/sub event distribution.
/// Any number of subscribers can consume events concurrently.
pub struct EventBus<E: Clone + Send + 'static> {
    tx: tokio::sync::broadcast::Sender<E>,
}

impl<E: Clone + Send + 'static> EventBus<E> {
    /// Create a new event bus with the given broadcast channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(capacity);
        // Drop _rx — the channel stays open as long as tx exists.
        drop(_rx);
        Self { tx }
    }

    /// Subscribe to events. Returns a receiver that will receive all
    /// events published after this call.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<E> {
        self.tx.subscribe()
    }

    /// Publish an event to all subscribers.
    ///
    /// Returns `Ok(())` even if there are zero subscribers.
    /// Returns an error only if all receivers have been dropped.
    pub fn publish(&self, event: E) -> anyhow::Result<()> {
        let _ = self.tx.send(event);
        Ok(())
    }

    /// Returns the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl<E: Clone + Send + 'static> Clone for EventBus<E> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<E: Clone + Send + 'static> fmt::Debug for EventBus<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventBus")
            .field("subscribers", &self.tx.receiver_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestEvent {
        name: String,
        value: i32,
    }

    #[test]
    fn test_new_bus() {
        let bus: EventBus<TestEvent> = EventBus::new(16);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn test_publish_with_no_subscribers() {
        let bus: EventBus<TestEvent> = EventBus::new(16);
        assert!(
            bus.publish(TestEvent {
                name: "test".into(),
                value: 1
            })
            .is_ok()
        );
    }

    #[tokio::test]
    async fn test_single_subscriber() {
        let bus: EventBus<TestEvent> = EventBus::new(16);
        let mut rx = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        bus.publish(TestEvent {
            name: "hello".into(),
            value: 42,
        })
        .unwrap();

        let event = rx.recv().await.unwrap();
        assert_eq!(event.name, "hello");
        assert_eq!(event.value, 42);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus: EventBus<TestEvent> = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(TestEvent {
            name: "broadcast".into(),
            value: 99,
        })
        .unwrap();

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1, e2);
    }

    #[tokio::test]
    async fn test_late_subscriber_misses_events() {
        let bus: EventBus<TestEvent> = EventBus::new(16);

        bus.publish(TestEvent {
            name: "early".into(),
            value: 1,
        })
        .unwrap();

        // Subscribe after publish — should NOT receive the event
        let mut rx = bus.subscribe();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_clone() {
        let bus: EventBus<TestEvent> = EventBus::new(16);
        let bus2 = bus.clone();
        // Both share the same underlying channel
        assert_eq!(bus.subscriber_count(), 0);
        assert_eq!(bus2.subscriber_count(), 0);

        let _rx = bus.subscribe();
        // Visible from clone
        assert_eq!(bus2.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn test_generic_with_string() {
        let bus: EventBus<String> = EventBus::new(64);
        let mut rx = bus.subscribe();
        bus.publish("hello world".into()).unwrap();
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg, "hello world");
    }
}
