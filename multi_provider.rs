

### Completed

Created `oxi-ai/src/multi_provider.rs` — the core routing provider that ties together `ComplexityRouter`, `CircuitBreaker`, and `FallbackChain`.

### Files Changed

- `oxi-ai/src/multi_provider.rs` — New file (~1200 lines)
- `oxi-ai/src/lib.rs` — Added module export for multi_provider

### Key Features Implemented

1. **`MultiProviderConfig`** — Configuration with auto_routing, prefer_cost_efficient, max_retries_per_model, per_model_timeout, and circuit_breaker settings

2. **`MultiProviderError`** — Error enum with:
   - `AllProvidersExhausted` — all candidates failed
   - `NoProviderForModel` — no provider for requested model
   - `CircuitBreakerOpen` — circuit breaker blocking requests
   - `NoFallback` — no fallback available
   - `NoProviderRegistered` — no providers registered

3. **`MultiProvider` struct** — Implements `Provider` trait with:
   - Router integration for complexity-based routing
   - Provider registry with circuit breakers
   - Fallback chain support
   - Full `stream()` implementation with priority order logic

4. **Priority order (from design §8.3)**:
   - auto_routing=true → router's best model → incoming model → fallback chain
   - auto_routing=false → incoming model → fallback chain

5. **Error handling**:
   - Retryable errors (429, 5xx, network, timeout): record failure, retry or try next
   - Non-retryable errors (400, 401, 403): return immediately without recording failure

6. **Test suite** — 12 tests covering:
   - Configuration defaults and builder methods
   - Provider registration/unregistration
   - Circuit breaker diagnostics
   - Router classification
   - Error display and helper methods

### Notes

- The implementation uses `model_registry::get_model` for model lookups (returns `Option<&'static Model>`), with fallback to constructing models from `ModelEntry` when not in registry
- Uses `tracing` for debug logging of routing decisions
- All 12 unit tests pass, and the full test suite completes successfully