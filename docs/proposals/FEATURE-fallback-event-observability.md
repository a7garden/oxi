# Feature Request: Fallback Event Observability for oxi-sdk Routing

**Date**: 2026-05-30
**Requested by**: Oxios (won@oxios.ai)
**Crate**: `oxi-sdk` / `oxi-ai` (MultiProvider)
**Related**: RFC-011 (oxi-sdk 0.24 migration + Model Routing UI)

---

## 1. Problem

### 1.1 Current State

oxi-sdk 0.24.0 provides `MultiProvider` with fallback chain support and `ComplexityRouter` for automatic model routing. The routing configuration (`RoutingConfig`, `RoutingControl`) works well for **control** (toggle on/off, add/remove fallback models, exclude models).

However, there is **no way to observe fallback events** from the SDK consumer side.

When `MultiProvider` falls back from model A to model B due to rate limiting, context overflow, or error, the consumer (oxios) has no hook to record this event. Consequently:
- Dashboard cannot display "model X fell back to Y" history
- Cost analysis cannot account for fallback overhead
- Observability cannot detect unstable model chains
- UX cannot surface fallback occurrences to users

### 1.2 Why This Matters for Oxios

Oxios uses `MultiProvider` via `OxiosEngine::builder().build_with_routing()` and needs to expose **per-model usage statistics** and **fallback history** in the Web UI dashboard.

Currently oxios has the data structures in place:
```rust
// oxios-kernel/src/kernel_handle/engine_api.rs
pub struct FallbackEvent {
    pub timestamp: DateTime<Utc>,
    pub from_model: String,
    pub to_model: String,
    pub reason: String,
    pub success: bool,
}
```

But `record_fallback()` is never called — because the SDK does not emit fallback events.

---

## 2. Proposed Solution

### 2.1 Option A: `ProviderEvent` Extension (Recommended)

Extend the existing `ProviderEvent` enum (already used for usage tracking) with fallback variants:

```rust
// In oxi-ai/src/provider_event.rs (or wherever ProviderEvent lives)

/// Event emitted by providers during inference.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    /// Token usage for a request.
    Usage { input_tokens: u64, output_tokens: u64, cache_read: u64, cache_write: u64 },
    /// Inference started.
    InferenceStart { model_id: String },
    /// Inference completed (success).
    InferenceEnd { model_id: String, duration_ms: u64 },
    /// Provider error (non-fatal, e.g., rate limit, timeout).
    Error { model_id: String, code: String, message: String },
    // ── NEW ──────────────────────────────────────────────────────────────────
    /// Model fallback occurred — primary model replaced by fallback.
    FallbackStart {
        /// Model that was attempted first.
        from_model: String,
        /// Model that was used instead.
        to_model: String,
        /// Reason for fallback.
        reason: FallbackReason,
    },
    /// Fallback chain exhausted — all models failed.
    FallbackExhausted {
        /// All models that were tried, in order.
        models_tried: Vec<String>,
        /// Final error from the last model.
        final_error: String,
    },
}

/// Reason for a model fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// Rate limit exceeded.
    RateLimit,
    /// Context window exceeded.
    ContextOverflow,
    /// Auth / quota error.
    AuthError,
    /// Network error.
    NetworkError,
    /// Model returned an error response.
    ModelError,
    /// Unknown or custom reason.
    Unknown,
}

impl FallbackReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            FallbackReason::RateLimit => "rate_limit",
            FallbackReason::ContextOverflow => "context_overflow",
            FallbackReason::AuthError => "auth_error",
            FallbackReason::NetworkError => "network_error",
            FallbackReason::ModelError => "model_error",
            FallbackReason::Unknown => "unknown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "rate_limit" => FallbackReason::RateLimit,
            "context_overflow" => FallbackReason::ContextOverflow,
            "auth_error" => FallbackReason::AuthError,
            "network_error" => FallbackReason::NetworkError,
            "model_error" => FallbackReason::ModelError,
            _ => FallbackReason::Unknown,
        }
    }
}
```

**Integration point**: `MultiProvider::call()` (or equivalent) emits `ProviderEvent::FallbackStart` when it switches from the primary model to a fallback. Oxios captures this in the existing `AgentEvent::Provider` handler:

```rust
// In oxios-kernel/src/agent_runtime.rs
AgentEvent::Provider(event) => {
    match event {
        ProviderEvent::FallbackStart { from_model, to_model, reason } => {
            routing_stats.record_fallback(FallbackEvent {
                timestamp: Utc::now(),
                from_model,
                to_model,
                reason: reason.as_str().to_string(),
                success: true, // fallback succeeded, we're continuing
            });
        }
        _ => {}
    }
}
```

### 2.2 Option B: Callback Hook on MultiProvider

Alternatively, add a callback hook to `MultiProviderBuilder`:

```rust
pub struct FallbackCallback {
    pub on_fallback: Box<dyn Fn(FallbackInfo) + Send + Sync>,
}

pub struct FallbackInfo {
    pub from_model: String,
    pub to_model: String,
    pub reason: FallbackReason,
}

impl MultiProviderBuilder {
    /// Register a callback for fallback events.
    pub fn with_fallback_callback(mut self, cb: FallbackCallback) -> Self {
        self.fallback_callback = Some(cb);
        self
    }
}
```

**Trade-off**: Option A is more composable (works with existing `ProviderEvent` system used by AgentRuntime) and doesn't require per-consumer builder changes. Option B is more explicit but adds builder complexity.

**Recommendation**: Option A.

---

## 3. Scope

### In Scope
- `ProviderEvent` enum extension with `FallbackStart` and `FallbackExhausted` variants
- `FallbackReason` enum with common cases
- `MultiProvider` emitting events when fallback occurs
- Documentation of the new event types

### Out of Scope (for this request)
- Metrics / alerting infrastructure
- Tracing integration (separate RFC)
- Custom fallback reason registration
- Fallback chain visualization

---

## 4. Example Consumer Code (Oxios)

After this feature ships, oxios will integrate as follows:

```rust
// oxios-kernel/src/agent_runtime.rs — after ProviderEvent extension lands

AgentEvent::Provider(event) => {
    match event {
        ProviderEvent::FallbackStart { from_model, to_model, reason } => {
            stats.record_fallback(FallbackEvent {
                timestamp: Utc::now(),
                from_model,
                to_model,
                reason: reason.as_str().to_string(),
                success: true,
            });
        }
        ProviderEvent::FallbackExhausted { models_tried, final_error } => {
            // Log total fallback chain failure
            tracing::warn!(
                models_tried = ?models_tried,
                error = %final_error,
                "All fallback models exhausted"
            );
            stats.record_fallback(FallbackEvent {
                timestamp: Utc::now(),
                from_model: models_tried.last().cloned().unwrap_or_default(),
                to_model: "none".to_string(),
                reason: "exhausted".to_string(),
                success: false,
            });
        }
        _ => {}
    }
}
```

---

## 5. Acceptance Criteria

- [ ] `ProviderEvent::FallbackStart` is emitted when `MultiProvider` switches from one model to another in a fallback chain
- [ ] `ProviderEvent::FallbackExhausted` is emitted when all models in the fallback chain fail
- [ ] `FallbackReason` covers at minimum: `RateLimit`, `ContextOverflow`, `AuthError`, `NetworkError`, `ModelError`, `Unknown`
- [ ] Events are emitted through the same channel as `ProviderEvent::Usage` (so existing consumers get them for free)
- [ ] Oxios can record fallback events in `RoutingStats::fallbacks` circular buffer
- [ ] No breaking changes to existing `ProviderEvent` consumers

---

## 6. Priority

**P2** — Not blocking current oxios release, but needed for full routing observability UX. Target for SDK 0.25 or 0.26.

---

## 7. Related Context

- Oxios RFC-011: `https://github.com/oxios-org/oxios/blob/main/docs/rfc-011-oxi-sdk-0.24-migration.md`
- oxi-sdk `multi_provider.rs`: FallbackChain + MultiProvider already exist, just needs event emission
- oxi-sdk `routing.rs`: RoutingControl provides runtime control, but no observability
- oxi-ai `ProviderEvent`: Already exists as the event channel for usage tracking