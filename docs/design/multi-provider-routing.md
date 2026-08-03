# Multi-Provider Routing & Fallback — Design Document

## 1. Overview

**Goal**: Provide intelligent multi-provider routing that:
1. Routes requests to the best-fit model based on task complexity
2. Falls back to alternative models on failure (with circuit breakers)
3. Is fully accessible via `oxicode-sdk` (mandatory requirement)

**Non-goal**: Modifying existing single-provider code paths. All routing is opt-in via new types.

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Code (SDK)                         │
│                                                                 │
│  MultiProvider::builder()                                       │
│    .with_fallbacks([...])                                      │
│    .with_routing(ComplexityRouter::default())                  │
│    .build()                                                    │
└───────────────────────────┬─────────────────────────────────────┘
                            │ resolves to one concrete provider
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                     MultiProvider (oxicode-ai)                      │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ComplexityRouter│ │CircuitBreaker │ │  FallbackChain        │ │
│  │ (analyzes task)│ │ (tracks health)│ │  (ordered fallback)   │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
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
| `MultiProvider` (core) | `oxicode-ai` | Provider trait lives here; routing is provider-selection logic |
| `ComplexityRouter` | `oxicode-ai` | Uses `model_db` data (cost, reasoning, context window) |
| `CircuitBreaker` | `oxicode-ai` | Extended into `oxicode-ai` for reusability |
| `FallbackChain` | `oxicode-ai` | Promoted to `oxicode-ai` |
| **SDK re-export & ergonomic API** | **`oxicode-sdk`** | **Mandatory requirement — must be accessible via SDK** |

---

## 4. Core Types

### 4.1 `Complexity` — Task Complexity Level

```rust
/// Task complexity level for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Complexity {
    Trivial,    // Simple, single-step tasks
    Simple,     // Routine tasks needing moderate reasoning
    Moderate,   // Tasks requiring multi-step reasoning
    Complex,    // Complex tasks needing deep analysis
    #[default]
    Research,   // Research-grade tasks needing the best models
}
```

### 4.2 `ComplexityRouter` — Routes to Best-Fit Model

```rust
pub trait ComplexityRouter: Send + Sync {
    fn classify(&self, context: &Context) -> Complexity;
    fn route(&self, complexity: Complexity, prefer_cost_efficient: bool) -> Vec<&'static ModelEntry>;
}
```

**Default implementation** (`DefaultRouter`):
- Analyzes last user message text for keywords
- Analyzes system prompt for complexity hints
- Tool count (`context.tools.len() > 0`) bumps complexity by one level
- Token count as length proxy
- Uses `model_db` for model lookups (not hardcoded)

### 4.3 `CircuitBreaker` — Per-Provider Health Tracking

- `ProviderCircuitBreaker` in `oxicode-ai` — tracks health per provider
- `CircuitBreaker` in `oxicode-agent` — backward-compatible wrapper (no provider name)
- States: Closed → Open → HalfOpen → Closed
- Configurable: `failure_threshold`, `open_duration`, `half_open_successes`

### 4.4 `FallbackChain` — Ordered Model Failover

```rust
pub struct FallbackChain {
    models: Vec<&'static ModelEntry>,
    names: Vec<String>,
}

impl FallbackChain {
    pub fn from_ids(ids: &[&str]) -> Result<Self, FallbackChainError>;
    pub fn next(&self, current: &str) -> Option<&'static ModelEntry>;
    pub fn index_of(&self, model_id: &str) -> Option<usize>;
    pub fn iter(&self) -> impl Iterator<Item = &'static ModelEntry>;
}
```

Uses `model_db::get_model_entry()` for lookup — not hardcoded.

### 4.5 `MultiProvider` — The Main Router

Implements `Provider` trait. On `stream()`:
1. Build candidate list based on priority order
2. For each candidate: check circuit breaker → stream → handle errors
3. Retryable errors (429, 5xx, network, timeout) → record failure → try next
4. Non-retryable errors (400, 401, 403) → return immediately

---

## 5. SDK API (`oxicode-sdk`)

### 5.1 Re-export from `oxicode-ai`

```rust
pub use oxicode_ai::{
    Complexity, ComplexityRouter, DefaultRouter,
    MultiProvider, MultiProviderConfig,
    FallbackChain, FallbackChainError,
    CircuitBreakerConfig, CircuitOpenError, ProviderCircuitBreaker,
    PartialResponse,
};
```

### 5.2 Fluent Builder API (Advanced)

```rust
pub struct MultiProviderBuilder { ... }

impl MultiProviderBuilder {
    pub fn new() -> Self;
    pub fn provider(self, name: &str, provider: Arc<dyn Provider>) -> Self;
    pub fn with_fallbacks(self, ids: &[&str]) -> Self;
    pub fn with_router(self, router: impl ComplexityRouter + 'static) -> Self;
    pub fn prefer_cost_efficient(self) -> Self;
    pub fn enable_auto_routing(self) -> Self;
    pub fn build(self) -> anyhow::Result<Arc<dyn Provider>>;
}
```

### 5.3 `OxicodeBuilder` Integration (Convenient)

```rust
impl OxicodeBuilder {
    pub fn enable_routing(mut self, config: RoutingConfig) -> Self { ... }
}
```

---

## 6. Priority Order

When a request arrives via `stream(model, context, options)`:

| Setting | Priority 1 | Priority 2 | Priority 3 |
|---|---|---|---|
| `auto_routing: true` | Complexity router's best model | Incoming `model` | `fallback` chain |
| `auto_routing: false` | Incoming `model` | `fallback` chain | — |

All candidates are filtered through circuit breakers.

---

## 7. Fallback Scope: Pre-Stream Only

**v1 limitation**: Fallback triggers **before** any content is yielded. Mid-stream fallback is not implemented.

Rationale:
- The model has already processed partial context
- Resending partial context to a new model may produce inconsistent output
- The caller (agent loop) handles partial response recovery via `PartialResponse`

---

## 8. Configuration

### 8.1 CLI Flags

```bash
oxicode --enable-routing                      # Enable automatic routing
oxicode --prefer-cost-efficient               # Prefer cheaper models
oxicode --fallback-chain openai/gpt-4o,anthropic/claude-haiku-3.5
oxicode --disable-fallback                     # Fail fast on errors
```

### 8.2 Settings (oxicode-store)

```toml
enable_routing = true
prefer_cost_efficient = true
fallback_chain = ["openai/gpt-4o", "anthropic/claude-haiku-3.5"]
enable_fallback = true
circuit_breaker_failure_threshold = 5
circuit_breaker_open_duration_secs = 30
```

### 8.3 TUI Widget

Press **Ctrl+R** to toggle routing status panel showing:
- Auto-routing enabled/disabled
- Fallback enabled/disabled
- Current fallback chain
- Provider health indicators (● healthy, ● degraded, ○ unavailable)

---

## 9. Implementation Priority

| Phase | Work | Crate | Status |
|---|---|---|---|
| **1** | Promote `CircuitBreaker` to `oxicode-ai` + add per-provider tracking | `oxicode-ai` | ✅ Complete |
| **2** | Promote `FallbackChain` to `oxicode-ai` | `oxicode-ai` | ✅ Complete |
| **3** | Implement `Complexity` + `ComplexityRouter` | `oxicode-ai` | ✅ Complete |
| **4** | Implement `MultiProvider` (core routing logic) | `oxicode-ai` | ✅ Complete |
| **5** | Re-export in `oxicode-ai/src/lib.rs` | `oxicode-ai` | ✅ Complete |
| **6** | Build `MultiProviderBuilder` in SDK | `oxicode-sdk` | ✅ Complete |
| **7** | Add `OxicodeBuilder::enable_routing()` + `RoutingConfig` | `oxicode-sdk` | ✅ Complete |
| **8** | Add CLI flags + TUI widget (Ctrl+R) | `oxicode-cli`, `oxicode-tui` | ✅ Complete |
| **9** | Migrate `oxicode-agent` to import from `oxicode-ai` | `oxicode-agent` | ✅ Complete |
| **10** | Add routing config to Settings | `oxicode-store` | ✅ Complete |

### 9.1 Phase 9: `oxicode-agent` Migration — COMPLETED

`oxicode-agent/src/recovery.rs` replaced with re-exports from `oxicode-ai`. Backward compatibility preserved.

### 9.2 Phase 10: Settings Integration — COMPLETED

`oxicode-store/src/settings.rs` includes: `enable_routing`, `prefer_cost_efficient`, `fallback_chain`, `enable_fallback`, `circuit_breaker_failure_threshold`, `circuit_breaker_open_duration_secs`.

---

## 10. File Layout

```
oxicode-ai/src/
  ├── circuit_breaker.rs      ← Per-provider circuit breaker + ProviderCircuitBreaker
  ├── fallback_chain.rs       ← Ordered fallback chain (model_db-backed)
  ├── complexity_router.rs    ← ComplexityRouter trait + DefaultRouter
  ├── multi_provider.rs       ← MultiProvider (implements Provider)
  ├── partial_response.rs      ← Partial response accumulator
  └── lib.rs                  ← Re-exports all types

oxicode-sdk/src/
  ├── multi_provider.rs       ← MultiProviderBuilder + RoutingConfig
  ├── builder.rs              ← OxicodeBuilder::enable_routing()
  ├── lib.rs                  ← Re-exports
  └── prelude.rs              ← Added to prelude

oxicode-cli/src/
  ├── cli.rs                  ← --enable-routing, --fallback-chain, etc.
  └── tui/
      ├── overlay/factories.rs ← routing_status() factory
      ├── handlers.rs          ← Ctrl+R handler
      ├── app.rs              ← RoutingStatus overlay variant
      └── render.rs           ← Routing panel rendering

oxicode-store/src/
  └── settings.rs             ← enable_routing, fallback_chain, etc.

oxicode-tui/src/widgets/
  └── routing.rs             ← RoutingStatus widget
```

---

## 11. Design Decisions

### 11.1 Why `MultiProvider` implements `Provider`, not wrap it?

`MultiProvider` *is* a `Provider`. Backward compatible — any code accepting `Arc<dyn Provider>` works with routing. Can be registered in `ProviderRegistry` directly.

### 11.2 Why promote to `oxicode-ai`?

- `Provider` trait lives in `oxicode-ai`
- Routing logic is provider-selection, not agent behavior
- Putting routing in `oxicode-ai` avoids circular dependencies
- General concepts (circuit breaker, fallback) don't belong in `oxicode-agent`

### 11.3 Why not hardcode models?

All model lookups use `model_db::get_model_entry()`. This ensures:
- Up-to-date pricing and capabilities
- Provider-agnostic fallback chains
- No stale model IDs