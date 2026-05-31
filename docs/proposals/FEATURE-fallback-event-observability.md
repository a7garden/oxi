# Feature Request: Fallback Event Observability for oxi-sdk Routing

**Date**: 2026-05-30
**Requested by**: Oxios (won@oxios.ai)
**Crate**: `oxi-sdk` / `oxi-ai` (MultiProvider)
**Status**: ✅ **Implemented** (oxi-sdk 0.25.0 / oxi-ai 0.25.0, 2026-05-31)
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
// In oxi-ai/src/providers/event.rs

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
    RateLimit,
    ContextOverflow,
    AuthError,
    NetworkError,
    ModelError,
    Unknown,
}

impl FallbackReason {
    pub fn as_str(&self) -> &'static str {
        match self { ... }
    }
}
```

### 2.2 Option B: Callback Hook on MultiProvider

Alternatively, add a callback hook to `MultiProviderBuilder`:

```rust
pub struct FallbackCallback {
    pub on_fallback: Box<dyn Fn(FallbackInfo) + Send + Sync>,
}

impl MultiProviderBuilder {
    pub fn with_fallback_callback(mut self, cb: FallbackCallback) -> Self { ... }
}
```

**Recommendation**: Option A (more composable, works with existing event system).

---

## 3. Scope

### In Scope
- `ProviderEvent` enum extension with `FallbackStart` and `FallbackExhausted` variants
- `FallbackReason` enum with common cases
- `MultiProvider` emitting events when fallback occurs

### Out of Scope
- Metrics / alerting infrastructure
- Tracing integration (separate RFC)
- Fallback chain visualization

---

## 4. Acceptance Criteria

- [x] `ProviderEvent::FallbackStart` is emitted when `MultiProvider` switches from one model to another in a fallback chain
- [x] `ProviderEvent::FallbackExhausted` is emitted when all models in the fallback chain fail
- [x] `FallbackReason` covers at minimum: `RateLimit`, `ContextOverflow`, `AuthError`, `NetworkError`, `ModelError`, `Unknown`
- [x] Events are emitted through the same channel as `ProviderEvent::Usage` (so existing consumers get them for free)
- [x] Oxios can record fallback events in `RoutingStats::fallbacks` circular buffer
- [x] No breaking changes to existing `ProviderEvent` consumers

---

## 5. Implementation Summary (2026-05-31)

### What Shipped in oxi-ai 0.25.0

**`ProviderEvent` extension** (`oxi-ai/src/providers/event.rs`):

```rust
// ── Routing / Fallback events ─────────────────────────────────────────
FallbackStart {
    from_model: String,
    to_model: String,
    reason: FallbackReason,
},
FallbackExhausted {
    models_tried: Vec<String>,
    final_error: String,
},
```

**`FallbackReason`** enum with **8 variants**: `RateLimit`, `ContextOverflow`, `AuthError`, `NetworkError`, `ServerError`, `ModelError`, `CircuitBreaker`, `Unknown`. More comprehensive than what was proposed.

**`FallbackStream` wrapper** (`oxi-ai/src/multi_provider.rs`): A stream wrapper that emits `FallbackStart` first, then delegates to the inner stream. Used when `MultiProvider` switches from one model to another in the candidate chain.

**`FallbackExhaustedStream` wrapper**: Emits `FallbackExhausted` and terminates. Used when all fallback candidates have been exhausted without success.

**`AgentEvent::Fallback`** (`oxi-agent/src/events.rs`): Wraps `ProviderEvent::FallbackStart` for consumer convenience:

```rust
Fallback {
    from_model: String,
    to_model: String,
},
```

### Oxios Integration

`agent_runtime.rs` captures `AgentEvent::Fallback`:

```rust
AgentEvent::Fallback { from_model, to_model } => {
    if let Some(stats) = &routing_stats_for_cb {
        stats.record_fallback(FallbackEvent {
            timestamp: Utc::now(),
            from_model: from_model.clone(),
            to_model: to_model.clone(),
            reason: "fallback".to_string(),
            success: true,
        });
    }
}
```

The circular buffer (`RoutingStats::fallbacks`, max 200 entries) stores fallback history accessible via `GET /api/engine/routing/fallbacks`.

### Changes Made to Oxios

| File | Change |
|------|--------|
| `Cargo.toml` | `oxi-sdk = "0.24.0"` → `"0.25.0"` |
| `agent_runtime.rs` | Added `AgentEvent::Fallback` handler in `run_agent()` callback |

---

## 6. Related Context

- Oxios RFC-011: `https://github.com/oxios-org/oxios/blob/main/docs/rfc-011-oxi-sdk-0.24-migration.md`
- oxi-sdk `multi_provider.rs`: FallbackChain + MultiProvider + FallbackStream
- oxi-sdk `routing.rs`: RoutingControl provides runtime control
- oxi-ai `ProviderEvent`: Existing event channel for usage tracking, extended with fallback variants
- oxi-agent `events.rs`: `AgentEvent::Fallback` wraps `ProviderEvent::FallbackStart`