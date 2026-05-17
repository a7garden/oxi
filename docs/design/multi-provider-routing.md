# Multi-Provider Routing & Fallback — Design Document

## 1. Overview

**Goal**: Provide intelligent multi-provider routing that:
1. Routes requests to the best-fit model based on task complexity
2. Falls back to alternative models on failure (with circuit breakers)
3. Is fully accessible via `oxi-sdk` (mandatory requirement)

**Non-goal**: Modifying existing single-provider code paths. All routing is opt-in via new types.

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Code (SDK)                         │
│                                                                 │
│  MultiProvider::builder()                                       │
│    .with_fallbacks([...])                                       │
│    .with_routing(ComplexityRouter::default())                   │
│    .build()                                                     │
└───────────────────────────┬─────────────────────────────────────┘
                            │ resolves to one concrete provider
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                     MultiProvider (oxi-ai)                       │  ← new type
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ ComplexityRouter │ │ CircuitBreaker │ │  FallbackChain        │ │
│  │ (analyzes task) │ │ (tracks health) │ │  (ordered fallback)   │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘   │
│                                                                 │
│  Implements Provider trait — transparently wraps any provider   │
└───────────────────────────┬─────────────────────────────────────┘
                            │ streams to
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│              Concrete Provider (8 built-in + custom)            │
│                                                                 │
│  Anthropic │ OpenAI │ Google │ Azure │ Bedrock │ Mistral │ ... │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Where to Implement

| Component | Crate | Reason |
|---|---|---|
| `MultiProvider` (core) | `oxi-ai` | Provider trait lives here; routing is provider-selection logic |
| `ComplexityRouter` | `oxi-ai` | Uses `model_db` data (cost, reasoning, context window) |
| `CircuitBreaker` | `oxi-ai` (extend) | Already in `oxi-agent/recovery.rs` — extend it into `oxi-ai` for reusability |
| `FallbackChain` | `oxi-ai` | Already in `oxi-agent/recovery.rs` — promote to `oxi-ai` |
| **SDK re-export & ergonomic API** | **`oxi-sdk`** | **Mandatory requirement — must be accessible via SDK** |

**Key design decision**: Routing logic lives in `oxi-ai` so it can be used standalone. `oxi-sdk` re-exports and wraps it with a fluent builder API.

**Why not `oxi-agent`?**: Routing is about provider selection, not agent behavior. `oxi-agent` already uses `Provider` from `oxi-ai`. Putting routing in `oxi-ai` avoids circular dependency and keeps concerns separated.

---

## 4. Core Types

### 4.1 `Complexity` — Task Complexity Level

```rust
/// Task complexity level for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Complexity {
    /// Simple, single-step tasks (e.g., "translate this text")
    Trivial,
    /// Routine tasks that need moderate reasoning (e.g., "write a function")
    Simple,
    /// Tasks requiring multi-step reasoning (e.g., "architect a service")
    Moderate,
    /// Complex tasks needing deep analysis (e.g., "write a full codebase")
    Complex,
    /// Research-grade tasks needing the best models
    Research,
}
```

### 4.2 `ComplexityRouter` — Routes to Best-Fit Model

```rust
/// Routes tasks to models based on estimated complexity.
///
/// Uses static `model_db` data to score models:
/// - reasoning flag → scores up for Moderate+
/// - cost_input + cost_output → preference for cheaper models when capable
/// - context_window → preference for models with headroom
pub trait ComplexityRouter: Send + Sync {
    /// Classify the complexity of the given context.
    ///
    /// Takes `&Context` to access messages, system prompt, and tool count.
    /// Tool count is derived from `context.tools.len()`.
    fn classify(&self, context: &Context) -> Complexity;


    /// Pick the best model for a given complexity, optionally preferring cost efficiency.
    fn route(&self, complexity: Complexity, prefer_cost_efficient: bool) -> Vec<&'static ModelEntry>;
}
```

**Default implementation** (`DefaultRouter`):
- Counts tokens as rough proxy for message length
- Analyzes **last user message** content for keywords
- Analyzes **system prompt** for complexity hints (e.g., "You are a senior architect")
- Tool count (`context.tools.len() > 0`) bumps complexity by one level
- Preference order per complexity:

| Complexity | Preferred Models | Cost-Efficient Alternative |
|---|---|---|
| Trivial | haiku-3.5, gpt-4o-mini | haiku-3.5, gpt-4o-mini |
| Simple | sonnet-4, gpt-4o | haiku-3.5, gpt-4o-mini |
| Moderate | opus-4, gpt-4.1 | sonnet-4, gpt-4o |
| Complex | opus-4.5+, claude-4-6 | opus-4, gpt-4.1 |
| Research | opus-4.5, gemini-3-pro | opus-4, gemini-2.5-pro |

### 4.3 `CircuitBreaker` — Promoted to `oxi-ai`

Move from `oxi-agent/src/recovery.rs` to `oxi-ai/src/circuit_breaker.rs` (new file).

**Important**: `oxi-agent` will re-export from `oxi-ai` after promotion (see §11 Phase 8 for migration plan).

**Enhancement**: Track per-provider circuit state, not global.

```rust
/// Per-provider circuit breaker state.
/// Thread-safe via atomics + parking_lot mutex.
pub struct ProviderCircuitBreaker {
    provider_name: String,
    state: AtomicU8,
    consecutive_failures: AtomicU64,
    opened_at: parking_lot::Mutex<Option<Instant>>,
    config: CircuitBreakerConfig,
}

impl ProviderCircuitBreaker {
    /// Record a failure for this provider.
    pub fn record_failure(&self) { ... }
    
    /// Record a success for this provider.
    pub fn record_success(&self) { ... }
    
    /// Check if requests are allowed.
    pub fn allow_request(&self) -> Result<(), CircuitOpenError> { ... }
}
```

### 4.4 `FallbackChain` — Promoted to `oxi-ai`

Move from `oxi-agent/src/recovery.rs` to `oxi-ai/src/fallback_chain.rs` (new file).

```rust
/// Ordered list of fallback models.
/// When `MultiProvider` fails with a retryable error, it tries the next model.
#[derive(Debug, Clone)]
pub struct FallbackChain {
    models: Vec<ModelEntry>,
    names: Vec<String>,  // "provider/model" strings for quick lookup
}

impl FallbackChain {
    /// Create from model ID strings (e.g., `["anthropic/claude-haiku-3.5", "openai/gpt-4o-mini"]`).
    pub fn from_ids(ids: &[&str]) -> Self { ... }
    
    /// Get the next fallback in the chain.
    pub fn next(&self, current: &str) -> Option<&ModelEntry> { ... }
    
    /// Return the index of a model, or None.
    pub fn index_of(&self, model_id: &str) -> Option<usize> { ... }
}
```

### 4.5 `MultiProvider` — The Main Router

```rust
/// A Provider that routes requests based on complexity and falls back on failure.
///
/// Wraps multiple concrete providers. Implements the `Provider` trait,
/// so it can be used anywhere a `Provider` is accepted.
pub struct MultiProvider {
    /// The primary router for complexity-based routing.
    router: Arc<dyn ComplexityRouter>,
    /// Registered concrete providers.
    providers: HashMap<String, Arc<dyn Provider>>,
    /// Fallback chain (ordered by preference).
    fallback: FallbackChain,
    /// Per-provider circuit breakers.
    breakers: HashMap<String, Arc<ProviderCircuitBreaker>>,
    /// Config.
    config: MultiProviderConfig,
}
```

**Behavior**:

1. **On `stream()` call**:
   - Determine candidate model priority (see §8.3 Priority Order)
   - Try each candidate:
     a. Check circuit breaker → if open, skip
     b. Get provider for that model
     c. Call `provider.stream()`
     d. If retryable error → record failure, try next
     e. If success → record success, yield events
   - If all candidates fail → return error

2. **Circuit breaker integration**:
   - On retryable error → `breaker.record_failure()`
   - On success → `breaker.record_success()`
   - Circuit opens after `failure_threshold` consecutive failures
   - After `open_duration`, transitions to half-open (allows one test request)


3. **Complexity-based auto-routing** (when enabled):
   - The incoming `Context` is analyzed via `router.classify(context)`
   - `model` parameter in `stream()` is treated as a hint, not a mandate
   - If model doesn't match complexity tier, a better model is selected

---

## 5. SDK API (`oxi-sdk`)

### 5.1 Re-export from `oxi-ai`

```rust
// oxi-sdk/src/lib.rs
pub use oxi_ai::{
    // Existing
    Provider, ProviderEvent, ProviderError, StreamOptions,
    // New multi-provider types
    Complexity, ComplexityRouter, DefaultRouter,
    MultiProvider, MultiProviderConfig,
    FallbackChain,
    ProviderCircuitBreaker, CircuitBreakerConfig,
};
```

### 5.2 Fluent Builder API (Advanced)

```rust
// oxi-sdk/src/multi_provider.rs

/// Fluent builder for MultiProvider.
pub struct MultiProviderBuilder {
    router: Box<dyn ComplexityRouter>,
    providers: HashMap<String, Arc<dyn Provider>>,
    fallback: Vec<String>,
    circuit_config: CircuitBreakerConfig,
    prefer_cost_efficient: bool,
    enable_auto_routing: bool,
}

impl MultiProviderBuilder {
    pub fn new() -> Self { ... }

    /// Add a provider with a name.
    pub fn provider(self, name: &str, provider: Arc<dyn Provider>) -> Self { ... }

    /// Add fallback models by ID.
    pub fn with_fallbacks(self, ids: &[&str]) -> Self { ... }

    /// Set custom complexity router.
    pub fn with_router(self, router: impl ComplexityRouter + 'static) -> Self { ... }

    /// Prefer cheaper models when capable.
    pub fn prefer_cost_efficient(self) -> Self { ... }

    /// Enable automatic routing based on task complexity.
    pub fn enable_auto_routing(self) -> Self { ... }

    /// Build the MultiProvider.
    pub fn build(self) -> Result<Arc<dyn Provider>> { ... }
}
```

### 5.3 `OxiBuilder` Integration (Convenient)

For the common case, add routing config directly to `OxiBuilder`:

```rust
// oxi-sdk/src/builder.rs

/// Routing configuration for `OxiBuilder::enable_routing()`.
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    /// Enable automatic complexity-based routing.
    pub auto_routing: bool,

    /// Prefer cost-efficient models.
    pub prefer_cost_efficient: bool,
    /// Fallback model IDs in priority order.
    pub fallback_chain: Vec<String>,
    /// Custom complexity router (optional).
    pub router: Option<Box<dyn ComplexityRouter>>,
}

impl OxiBuilder {
    /// Enable intelligent routing with the given configuration.
    ///
    /// # Example
    /// ```ignore
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .enable_routing(RoutingConfig {
    ///         auto_routing: true,
    ///         prefer_cost_efficient: true,
    ///         fallback_chain: vec!["anthropic/claude-haiku-3.5", "openai/gpt-4o-mini"],
    ///         router: None,
    ///     })
    ///     .build();
    /// ```
    pub fn enable_routing(mut self, config: RoutingConfig) -> Self { ... }
}
```

**Two API levels**:
- `OxiBuilder::enable_routing()` — convenient, for most users
- `MultiProviderBuilder` — advanced, for fine-grained control

### 5.4 Usage Examples

**Advanced (MultiProviderBuilder)**:
```rust
use oxi_sdk::{MultiProviderBuilder, ComplexityRouter};

let multi = MultiProviderBuilder::new()
    .provider("anthropic", oxi_ai::create_builtin_provider("anthropic").unwrap())
    .provider("openai", oxi_ai::create_builtin_provider("openai").unwrap())
    .with_fallbacks(&["anthropic/claude-haiku-3.5", "openai/gpt-4o-mini"])
    .prefer_cost_efficient()
    .enable_auto_routing()
    .build()
    .unwrap();

let oxi = OxiBuilder::new()
    .with_builtins()
    .provider("multi", multi)
    .build();
```

**Convenient (OxiBuilder integration)**:
```rust
use oxi_sdk::{OxiBuilder, RoutingConfig};

let oxi = OxiBuilder::new()
    .with_builtins()
    .enable_routing(RoutingConfig {
        auto_routing: true,
        prefer_cost_efficient: true,
        fallback_chain: vec!["anthropic/claude-haiku-3.5", "openai/gpt-4o-mini"],
        router: None,
    })
    .build();
```

---

## 6. File Layout

```
oxi-ai/src/
  ├── circuit_breaker.rs     ← NEW (moved from oxi-agent/recovery.rs + enhanced)
  ├── fallback_chain.rs     ← NEW (moved from oxi-agent/recovery.rs + enhanced)
  ├── complexity.rs          ← NEW (Complexity enum)
  ├── complexity_router.rs  ← NEW (ComplexityRouter trait + DefaultRouter)
  ├── multi_provider.rs      ← NEW (MultiProvider struct + impl Provider)
  └── lib.rs                 ← re-export new types

oxi-sdk/src/
  ├── multi_provider.rs      ← NEW (MultiProviderBuilder fluent API)
  ├── lib.rs                 ← re-export all new types + add to prelude
```

---

## 7. Key Design Decisions

### 7.1 Why `MultiProvider` implements `Provider`, not wrap it?

**Option A (chosen)**: `MultiProvider` *is* a `Provider`.  
**Option B**: Create a `ProviderRouter` wrapper.

Chosen A because:
- Backward compatible: any code accepting `Arc<dyn Provider>` works with routing
- No new traits needed in consumer code
- Can be registered in `ProviderRegistry` directly

### 7.2 Why promote `CircuitBreaker` and `FallbackChain` to `oxi-ai`?

Currently in `oxi-agent/recovery.rs`. Problems:
- `oxi-ai` is lower-level and should not depend on `oxi-agent`
- Routing logic in `oxi-agent` would create circular deps (routing → provider → ??)
- These are general concepts (not agent-specific) — belong in `oxi-ai`

**Migration**: `oxi-agent` will import from `oxi-ai` after promotion (no code duplication, just re-export).

### 7.3 Complexity Classification — Static vs Dynamic

**Static** (chosen for v1):
- Keyword matching + token count
- No LLM calls for classification
- Deterministic, zero-latency

**Future** (v2):
- Lightweight LLM classifier (e.g., `haiku` calling itself)
- Cached decisions per task type

### 7.4 SDK Re-export Requirement

Per requirement: *"sdk를 통해서도 무조건 제공되어야해"*

Solution: Re-export all types in `oxi-sdk/src/lib.rs` + provide `MultiProviderBuilder` for ergonomic access.

```rust
// oxi-sdk/src/lib.rs
pub use oxi_ai::{
    multi_provider::{MultiProvider, MultiProviderConfig},
    complexity::{Complexity, ComplexityRouter, DefaultRouter},
    fallback_chain::FallbackChain,
    circuit_breaker::{ProviderCircuitBreaker, CircuitBreakerConfig},
};
```

---

## 8. Error Handling

### 8.1 `MultiProviderError`

```rust
#[derive(Error, Debug)]
pub enum MultiProviderError {
    #[error("All providers exhausted")]
    AllProvidersExhausted {
        errors: Vec<(String, ProviderError)>,  // provider → error
    },
    
    #[error("No provider available for model: {0}")]
    NoProviderForModel(String),
    
    #[error("Circuit breaker open for provider: {0}")]
    CircuitBreakerOpen {
        provider: String,
        retry_after: Duration,
    },
    
    #[error("No fallback models configured")]
    NoFallbackConfigured,
}
```

### 8.2 Fallback Behavior

```
Request comes in with model_id = "anthropic/claude-sonnet-4"
    │
    ├─► Circuit breaker: CLOSED → proceed
    │   └─► Provider.stream() succeeds → return events
    │
    ├─► Circuit breaker: CLOSED → proceed
    │   └─► Provider.stream() fails (retryable: 429, 500, network)
    │       ├─► record_failure() → circuit breaker state updated
    │       └─► Try next model in fallback chain
    │           └─► "openai/gpt-4o" → same process
    │
    └─► Circuit breaker: OPEN → skip this provider
        └─► Try next model (circuit prevents wasted calls)
```

### 8.3 Priority Order

When a request arrives via `stream(model, context, options)`, the candidate order is:

| Setting | Priority 1 | Priority 2 | Priority 3 |
|---|---|---|---|
| `auto_routing: true` | Complexity router's best model | Incoming `model` (as fallback) | `fallback` chain |
| `auto_routing: false` | Incoming `model` | `fallback` chain | — |

**Rationale**:
- When auto-routing is enabled, the router's intelligence wins. The incoming model is still tried as a fallback before the chain.
- When auto-routing is disabled, the user's choice is respected. Fallback chain provides resilience.
- All candidates are filtered through circuit breakers — a failed provider is skipped regardless of priority.

### 8.4 Fallback Scope: Pre-Stream Only (v1)

**v1 limitation**: Fallback triggers **before** any content is yielded. If a stream starts yielding `ProviderEvent`s and then fails mid-stream, `MultiProvider` does **not** attempt fallback.

**Rationale**: Mid-stream fallback is complex because:
1. The model has already processed partial context
2. Resending partial context to a new model may produce inconsistent output
3. The caller (agent loop) handles partial response recovery via `PartialResponse`

**Future (v2)**: Mid-stream fallback with context carry-over could be considered if there is demonstrated need.

### 8.5 ProviderPool Interaction

When `MultiProvider` wraps a `ProviderPool` (which wraps a `Provider` with rate limiting):

- `MultiProvider` handles **model-level** retries (model A fails → try model B)
- `ProviderPool` handles **request-level** rate limiting (RPM, concurrency)
- These layers are orthogonal and do not interfere
- On retryable errors (429, 5xx), both layers record failure state independently


**Note**: When registering a provider with `MultiProviderBuilder::provider()`, wrapping with `ProviderPool` is the caller's responsibility if rate limiting is needed.

---

## 9. Configuration Options

```rust
#[derive(Debug, Clone)]
pub struct MultiProviderConfig {
    /// Enable automatic complexity-based routing.
    pub auto_routing: bool,

    /// Prefer cost-efficient models even if slightly less capable.
    pub prefer_cost_efficient: bool,

    /// Fallback chain configuration.
    pub fallback: FallbackChain,

    /// Circuit breaker per provider.
    pub circuit_breaker: CircuitBreakerConfig,

    /// Max retries per model before moving to next.
    pub max_retries_per_model: usize,

    /// Timeout per model attempt before trying next.
    pub per_model_timeout: Option<Duration>,
}

impl Default for MultiProviderConfig {
    fn default() -> Self {
        Self {
            auto_routing: false,  // Opt-in for backward compatibility
            prefer_cost_efficient: false,
            fallback: FallbackChain::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
            max_retries_per_model: 1,
            per_model_timeout: None,
        }
    }
}

impl FallbackChain {
    /// Default fallback chain: cheapest capable models per tier.
    pub fn default() -> Self {
        Self {
            models: vec![
                // These will be looked up from model_db at runtime
                // for maximum flexibility across API versions
            ],
            names: vec![
                "anthropic/claude-haiku-3.5-20241022-v1:0".to_string(),
                "openai/gpt-4o-mini".to_string(),
            ],
        }
    }
}
```

**Default behavior**: When no fallback is configured (`fallback` is empty), a default chain of cost-efficient models is used. This can be disabled by setting `fallback: FallbackChain::empty()`.

---

## 10. Open Questions for Discussion

1. **Provider health tracking**: Should `MultiProvider` expose health metrics (failure rate, latency p50/p99) for observability?
2. **Weighted routing**: Should routing prefer certain providers based on historical success rates? (e.g., 70% Anthropic, 30% OpenAI)
3. **Persistence**: Should circuit breaker state survive across `MultiProvider` instances? (e.g., store in a config file)
4. **Context carry-over**: When falling back, should the failed partial response be resent? (Already handled by `PartialResponse` in `recovery.rs`)
5. **Async routing decision**: Is synchronous classification acceptable, or should classification itself be async (e.g., for LLM-based classification)?

---

## 11. Implementation Priority

| Phase | Work | Crate | Notes |
|---|---|---|---|
| **1** | Promote `CircuitBreaker` to `oxi-ai` + add per-provider tracking | `oxi-ai` | New file: `src/circuit_breaker.rs`. Remove `recovery` module from `oxi-ai` before promotion to avoid conflicts. |
| **2** | Promote `FallbackChain` to `oxi-ai` | `oxi-ai` | New file: `src/fallback_chain.rs` |
| **3** | Implement `Complexity` + `ComplexityRouter` | `oxi-ai` | New files: `src/complexity.rs`, `src/complexity_router.rs` |
| **4** | Implement `MultiProvider` (core routing logic) | `oxi-ai` | New file: `src/multi_provider.rs` |
| **5** | Re-export in `oxi-ai/src/lib.rs` | `oxi-ai` | Add new types to `pub mod` and `pub use` exports |
| **6** | Build `MultiProviderBuilder` in SDK | `oxi-sdk` | New file: `src/multi_provider.rs` |
| **7** | Add `OxiBuilder::enable_routing()` + `RoutingConfig` | `oxi-sdk` | Extend `src/builder.rs` |
| **8** | Add to SDK prelude + docs | `oxi-sdk` | Update `src/prelude.rs` and add documentation |
| **9** | Migrate `oxi-agent` to import from `oxi-ai` | `oxi-agent` | See §11.1 below |

### 11.1 Phase 9: `oxi-agent` Migration Plan

After `CircuitBreaker` and `FallbackChain` are promoted to `oxi-ai`, update `oxi-agent`:

**1. Update `oxi-agent/src/lib.rs`**:
```rust
// Before (duplicates):
pub use recovery::{CircuitBreaker, FallbackChain, ...};

// After (re-export from oxi-ai):
pub use oxi_ai::{CircuitBreaker, FallbackChain, ...};
```

**2. Delete `oxi-agent/src/recovery.rs`**:
The entire file can be deleted after re-exports are in place.

**3. Delete `oxi-agent/src/stream_retry.rs`** (if it overlaps):
Check for duplicate types; consolidate in `oxi-ai`.

**4. Verify no breaking changes**:
- All public re-exports maintain the same API
- Internal types (`PartialResponse`, `CircuitState`) remain internal to `oxi-ai` or `oxi-agent` as appropriate
- Run `cargo test --workspace` to verify