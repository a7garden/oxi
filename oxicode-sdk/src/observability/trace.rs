//! Distributed tracing — spans, trace IDs, and RAII guards.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

// ── TraceId ──────────────────────────────────────────────────────────────────

/// Unique trace identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceId(u64);

impl TraceId {
    /// Generate a new random trace ID.
    pub fn new() -> Self {
        Self(fastrand::u64(1..))
    }
    /// Zero trace ID (invalid).
    pub fn zero() -> Self {
        Self(0)
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

// ── SpanId ───────────────────────────────────────────────────────────────────

/// Unique span identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpanId(u64);

impl SpanId {
    /// Generate a new random span ID.
    pub fn new() -> Self {
        Self(fastrand::u64(1..))
    }
    /// Zero span ID (invalid).
    pub fn zero() -> Self {
        Self(0)
    }
}

impl Default for SpanId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SpanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

// ── SpanKind ────────────────────────────────────────────────────────────────

/// Role or classification of a span within a trace.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum SpanKind {
    /// Span covering an agent's execution.
    Agent,
    /// Span covering a tool invocation.
    #[default]
    Tool,
    /// Span covering an LLM request/response.
    Llm,
    /// Span covering internal SDK work.
    Internal,
}

// ── SpanStatus ───────────────────────────────────────────────────────────────

/// Outcome status of a span.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum SpanStatus {
    /// The span completed successfully.
    #[default]
    Ok,
    /// The span completed with an error.
    Error {
        /// Message describing the error.
        message: String,
    },
}

// ── SpanContext ──────────────────────────────────────────────────────────────

/// Identifies a span within a trace and links it to its parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanContext {
    /// Trace this span belongs to.
    pub trace_id: TraceId,
    /// Unique id of this span.
    pub span_id: SpanId,
    /// Id of the parent span, if any.
    pub parent_span_id: Option<SpanId>,
}

// ── SpanEvent ─────────────────────────────────────────────────────────────────

/// A timestamped event recorded on a span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanEvent {
    /// Human-readable name of the event.
    pub name: String,
    /// Wall-clock time the event occurred, in milliseconds.
    pub timestamp_ms: u64,
    /// Arbitrary key/value attributes for the event.
    #[serde(default)]
    pub attributes: Vec<(String, serde_json::Value)>,
}

// ── Span ─────────────────────────────────────────────────────────────────────

/// A single unit of work within a trace, with timing, status, and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    /// Identifier and parent link for this span.
    pub context: SpanContext,
    /// Human-readable name of the operation.
    pub name: String,
    /// Classification of the span.
    pub kind: SpanKind,
    /// Wall-clock start time in milliseconds.
    pub start_ms: u64,
    /// Wall-clock end time in milliseconds, once the span is closed.
    pub end_ms: Option<u64>,
    /// Outcome of the span.
    pub status: SpanStatus,
    /// Arbitrary key/value metadata attached to the span.
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
    /// Timestamped events recorded during the span.
    #[serde(default)]
    pub events: Vec<SpanEvent>,
    /// Links to related span contexts (e.g. cross-trace references).
    #[serde(default)]
    pub links: Vec<SpanContext>,
}

impl Span {
    /// Duration in milliseconds.
    pub fn duration_ms(&self) -> Option<u64> {
        self.end_ms.map(|end| end.saturating_sub(self.start_ms))
    }
    /// True if the span has an end time.
    pub fn is_complete(&self) -> bool {
        self.end_ms.is_some()
    }
}

// ── Tracer ────────────────────────────────────────────────────────────────────

/// Collects completed spans and broadcasts them to subscribers.
#[derive(Debug)]
pub struct Tracer {
    spans: Arc<RwLock<Vec<Span>>>,
    completed_tx: broadcast::Sender<Span>,
}

impl Tracer {
    /// Create a new tracer.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            spans: Arc::new(RwLock::new(Vec::new())),
            completed_tx: tx,
        }
    }

    /// Start a root span.
    pub fn start(self: &Arc<Self>, name: &str, kind: SpanKind) -> SpanGuard {
        self.start_with_parent(name, kind, None)
    }

    /// Start a child span with optional parent context.
    pub fn start_with_parent(
        self: &Arc<Self>,
        name: &str,
        kind: SpanKind,
        parent: Option<&SpanContext>,
    ) -> SpanGuard {
        let trace_id = parent.map(|c| c.trace_id).unwrap_or_default();
        let span_id = SpanId::new();
        let context = SpanContext {
            trace_id,
            span_id,
            parent_span_id: parent.map(|c| c.span_id),
        };
        let span = Span {
            context,
            name: name.to_string(),
            kind,
            start_ms: now_ms(),
            end_ms: None,
            status: SpanStatus::Ok,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
        };
        SpanGuard {
            tracer: Arc::clone(self),
            span,
        }
    }

    fn record(&self, span: Span) {
        self.spans.write().push(span.clone());
        let _ = self.completed_tx.send(span);
    }

    /// Retrieve all spans for a given trace ID.
    pub fn trace(&self, trace_id: TraceId) -> Vec<Span> {
        self.spans
            .read()
            .iter()
            .filter(|s| s.context.trace_id == trace_id)
            .cloned()
            .collect()
    }

    /// Subscribe to completed span events.
    pub fn subscribe(&self) -> broadcast::Receiver<Span> {
        self.completed_tx.subscribe()
    }
}

impl Clone for Tracer {
    fn clone(&self) -> Self {
        Self {
            spans: Arc::clone(&self.spans),
            completed_tx: self.completed_tx.clone(),
        }
    }
}

impl Default for Tracer {
    fn default() -> Self {
        Self::new()
    }
}

// ── SpanGuard ────────────────────────────────────────────────────────────────

/// RAII guard for an in-flight span; finalizes and records the span on drop.
pub struct SpanGuard {
    tracer: Arc<Tracer>,
    span: Span,
}

impl SpanGuard {
    /// Borrowed span context (trace, span, and parent ids).
    pub fn context(&self) -> &SpanContext {
        &self.span.context
    }

    /// Trace id this span belongs to.
    pub fn trace_id(&self) -> TraceId {
        self.span.context.trace_id
    }

    /// Unique id of this span.
    pub fn span_id(&self) -> SpanId {
        self.span.context.span_id
    }

    /// Attach a key/value attribute to the span.
    pub fn set_attribute(&mut self, key: &str, value: serde_json::Value) {
        self.span.attributes.insert(key.to_string(), value);
    }

    /// Record a named, timestamped event on the span.
    pub fn add_event(&mut self, name: &str) {
        self.span.events.push(SpanEvent {
            name: name.to_string(),
            timestamp_ms: now_ms(),
            attributes: vec![],
        });
    }

    /// Mark the span as failed with the given error message.
    pub fn set_error(&mut self, message: &str) {
        self.span.status = SpanStatus::Error {
            message: message.to_string(),
        };
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        let mut span = self.span.clone();
        span.end_ms = Some(now_ms());
        self.tracer.record(span);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_static<T: Send + 'static>() {}

    #[test]
    fn span_guard_is_send_and_static() {
        assert_send_static::<SpanGuard>();
    }

    #[tokio::test]
    async fn smoke() {
        let tracer = Arc::new(Tracer::new());
        let guard = tracer.start("s", SpanKind::Agent);
        let tid = guard.trace_id();
        drop(guard);
        let spans = tracer.trace(tid);
        assert!(!spans.is_empty());
        assert_eq!(spans[0].name, "s");
        assert!(spans[0].is_complete());
    }

    #[tokio::test]
    async fn child_span() {
        let tracer = Arc::new(Tracer::new());
        let parent = tracer.start("parent", SpanKind::Agent);
        let parent_ctx = parent.context().clone();
        drop(parent);
        let child = tracer.start_with_parent("child", SpanKind::Tool, Some(&parent_ctx));
        let tid = child.trace_id();
        drop(child);
        let spans = tracer.trace(tid);
        assert_eq!(spans.len(), 2);
    }
}
