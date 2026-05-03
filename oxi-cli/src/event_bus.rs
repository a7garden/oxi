//! Async event bus for pub/sub communication
//!
//! Provides a type-safe event system for agent session events.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Event types for the agent session
#[derive(Debug, Clone)]
pub enum AgentSessionEvent {
    /// A new message was received or sent
    Message {
        role: String,
        content: String,
        timestamp: u64,
    },
    /// A tool started executing
    ToolStart {
        tool_name: String,
        input: serde_json::Value,
    },
    /// A tool finished executing
    ToolEnd {
        tool_name: String,
        output: Result<serde_json::Value, String>,
        duration_ms: u64,
    },
    /// An error occurred
    Error { message: String, recoverable: bool },
    /// Model started generating a response
    ModelStart { model_id: String },
    /// Model finished generating a response
    ModelEnd {
        model_id: String,
        duration_ms: u64,
        tokens_used: Option<u32>,
    },
    /// Token usage update
    TokenUsage {
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: Option<u32>,
    },
    /// Session started
    SessionStart { session_id: String },
    /// Session ended
    SessionEnd {
        session_id: String,
        total_messages: u32,
    },
    /// Thinking block started
    ThinkingStart,
    /// Thinking block ended
    ThinkingEnd { thoughts: String },
    /// Stream chunk received
    StreamChunk { content: String },
    /// Tool call requested
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Tool result received
    ToolResult {
        tool_name: String,
        result: serde_json::Value,
    },
    /// Custom event from extensions
    Custom {
        name: String,
        data: serde_json::Value,
    },
}

/// Async event handler type - returns a pinned boxed future
pub type EventHandler = Arc<
    dyn Fn(
            AgentSessionEvent,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>
        + Send
        + Sync,
>;

/// Sync event handler type (for simpler handlers)
pub type SyncEventHandler = Arc<dyn Fn(AgentSessionEvent) + Send + Sync>;

/// A subscriber handle
pub struct Subscriber {
    pub channel: String,
    pub id: u64,
}

impl Subscriber {
    /// Unsubscribe from the event channel
    pub fn unsubscribe(self) {
        // Subscriber is dropped
    }
}

/// Internal subscriber storage
struct BusInner {
    subscribers: RwLock<HashMap<String, HashMap<u64, EventHandler>>>,
    sync_subscribers: RwLock<HashMap<String, HashMap<u64, SyncEventHandler>>>,
    next_id: RwLock<u64>,
}

/// Thread-safe async event bus for publish/subscribe pattern
pub struct EventBus {
    inner: Arc<BusInner>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// Create a new event bus
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BusInner {
                subscribers: RwLock::new(HashMap::new()),
                sync_subscribers: RwLock::new(HashMap::new()),
                next_id: RwLock::new(0),
            }),
        }
    }

    /// Create a new Arc-wrapped event bus
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Subscribe to an event channel with an async handler
    pub async fn subscribe_async<F, Fut>(&self, channel: &str, handler: F) -> Subscriber
    where
        F: Fn(AgentSessionEvent) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut next_id = self.inner.next_id.write().await;
        let id = *next_id;
        *next_id = id + 1;
        drop(next_id);

        let handler: EventHandler = Arc::new(move |event| {
            let fut = handler(event);
            Box::pin(fut)
        });

        self.inner
            .subscribers
            .write()
            .await
            .entry(channel.to_string())
            .or_insert_with(HashMap::new)
            .insert(id, handler);

        Subscriber {
            channel: channel.to_string(),
            id,
        }
    }

    /// Subscribe to an event channel with a sync handler
    pub async fn subscribe_sync(&self, channel: &str, handler: SyncEventHandler) -> Subscriber {
        let mut next_id = self.inner.next_id.write().await;
        let id = *next_id;
        *next_id = id + 1;
        drop(next_id);

        self.inner
            .sync_subscribers
            .write()
            .await
            .entry(channel.to_string())
            .or_insert_with(HashMap::new)
            .insert(id, handler);

        Subscriber {
            channel: channel.to_string(),
            id,
        }
    }

    /// Subscribe to an event channel (sync version for convenience)
    pub fn subscribe(&self, channel: &str, handler: SyncEventHandler) -> Subscriber {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async { self.subscribe_sync(channel, handler).await })
    }

    /// Publish an event to a channel
    pub async fn publish(&self, channel: &str, event: AgentSessionEvent) {
        // Notify sync handlers first
        {
            let sync_handlers = self.inner.sync_subscribers.read().await;
            if let Some(handlers) = sync_handlers.get(channel) {
                for handler in handlers.values() {
                    handler(event.clone());
                }
            }
        }

        // Then notify async handlers
        let handlers: Vec<EventHandler> = {
            let async_handlers = self.inner.subscribers.read().await;
            async_handlers
                .get(channel)
                .map(|h| h.values().cloned().collect())
                .unwrap_or_default()
        };

        for handler in handlers {
            let event_clone = event.clone();
            tokio::spawn(async move {
                handler(event_clone).await;
            });
        }
    }

    /// Unsubscribe a specific handler
    pub async fn unsubscribe(&self, channel: &str, id: u64) {
        if let Some(handlers) = self.inner.subscribers.write().await.get_mut(channel) {
            handlers.remove(&id);
        }
        if let Some(handlers) = self.inner.sync_subscribers.write().await.get_mut(channel) {
            handlers.remove(&id);
        }
    }

    /// Unsubscribe all handlers for a channel
    pub async fn unsubscribe_all(&self, channel: &str) {
        self.inner.subscribers.write().await.remove(channel);
        self.inner.sync_subscribers.write().await.remove(channel);
    }

    /// Clear all subscriptions
    pub async fn clear(&self) {
        self.inner.subscribers.write().await.clear();
        self.inner.sync_subscribers.write().await.clear();
    }

    /// Get the number of active subscriptions
    pub async fn subscription_count(&self) -> usize {
        let async_count: usize = self
            .inner
            .subscribers
            .read()
            .await
            .values()
            .map(|h| h.len())
            .sum();
        let sync_count: usize = self
            .inner
            .sync_subscribers
            .read()
            .await
            .values()
            .map(|h| h.len())
            .sum();
        async_count + sync_count
    }
}

/// Builder for creating event buses with predefined channels
pub struct EventBusBuilder {
    channels: Vec<String>,
}

impl EventBusBuilder {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channels.push(channel.into());
        self
    }

    pub fn build(self) -> Arc<EventBus> {
        let bus = EventBus::arc();
        let _ = self.channels; // Channels are created on-demand
        bus
    }
}

impl Default for EventBusBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Common channel names
pub mod channels {
    pub const SESSION: &str = "session:*";
    pub const MESSAGE: &str = "session:message";
    pub const TOOL: &str = "session:tool";
    pub const ERROR: &str = "session:error";
    pub const TOKEN_USAGE: &str = "session:token_usage";
    pub const MODEL: &str = "session:model";
    pub const THINKING: &str = "session:thinking";
    pub const STREAM: &str = "session:stream";
    pub const CUSTOM: &str = "session:custom";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscribe_and_publish() {
        let bus = EventBus::arc();
        let received = Arc::new(RwLock::new(Vec::new()));
        let received_clone = received.clone();

        bus.subscribe_async("test", move |event| {
            let received = received_clone.clone();
            async move {
                received.write().await.push(event);
            }
        })
        .await;

        let event = AgentSessionEvent::Error {
            message: "test error".to_string(),
            recoverable: true,
        };

        bus.publish("test", event.clone()).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let captured = received.read().await;
        assert_eq!(captured.len(), 1);
        if let AgentSessionEvent::Error { message, .. } = &captured[0] {
            assert_eq!(message, "test error");
        }
    }

    #[tokio::test]
    async fn test_sync_handler() {
        let bus = EventBus::arc();
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = received.clone();

        bus.subscribe_sync(
            "test",
            Arc::new(move |event| {
                received_clone.lock().unwrap().push(event);
            }),
        )
        .await;

        let event = AgentSessionEvent::SessionStart {
            session_id: "123".to_string(),
        };

        bus.publish("test", event.clone()).await;

        let captured = received.lock().unwrap();
        assert_eq!(captured.len(), 1);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::arc();
        let count1 = Arc::new(std::sync::Mutex::new(0));
        let count2 = Arc::new(std::sync::Mutex::new(0));
        let count1_clone = count1.clone();
        let count2_clone = count2.clone();

        bus.subscribe_sync(
            "test",
            Arc::new(move |_| {
                *count1_clone.lock().unwrap() += 1;
            }),
        )
        .await;
        bus.subscribe_sync(
            "test",
            Arc::new(move |_| {
                *count2_clone.lock().unwrap() += 1;
            }),
        )
        .await;

        bus.publish("test", AgentSessionEvent::ThinkingStart).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(*count1.lock().unwrap(), 1);
        assert_eq!(*count2.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let bus = EventBus::arc();
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = received.clone();

        let subscriber = bus
            .subscribe_sync(
                "test",
                Arc::new(move |_| {
                    received_clone.lock().unwrap().push(1);
                }),
            )
            .await;

        bus.publish("test", AgentSessionEvent::ThinkingStart).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert_eq!(received.lock().unwrap().len(), 1);

        bus.unsubscribe("test", subscriber.id).await;

        bus.publish("test", AgentSessionEvent::ThinkingStart).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let bus = EventBus::arc();
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = received.clone();

        bus.subscribe_sync(
            "test",
            Arc::new(move |_| {
                received_clone.lock().unwrap().push(1);
            }),
        )
        .await;

        bus.publish("test", AgentSessionEvent::ThinkingStart).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        bus.clear().await;

        bus.publish("test", AgentSessionEvent::ThinkingStart).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_subscription_count() {
        let bus = EventBus::arc();

        assert_eq!(bus.subscription_count().await, 0);

        let _sub1 = bus.subscribe_sync("test", Arc::new(|_| {})).await;
        let _sub2 = bus.subscribe_sync("test", Arc::new(|_| {})).await;

        assert_eq!(bus.subscription_count().await, 2);
    }
}
