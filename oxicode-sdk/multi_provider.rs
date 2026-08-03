

### Completed

Created `oxicode-sdk/src/multi_provider.rs` — fluent builder API for MultiProvider with:
- **RoutingConfig**: Configuration for complexity-based routing (auto_routing, prefer_cost_efficient, custom router)
- **MultiProviderBuilder**: Fluent builder with methods for providers, fallbacks, routing, circuit breakers

Also updated:
- `oxicode-sdk/src/builder.rs` — added `enable_routing()` method to `OxicodeBuilder`
- `oxicode-sdk/src/lib.rs` — added module declaration and re-exports
- `oxicode-sdk/src/prelude.rs` — added MultiProviderBuilder and RoutingConfig exports

### Files Changed

- **`oxicode-sdk/src/multi_provider.rs`** (new file) — 300+ lines with RoutingConfig, MultiProviderBuilder, and tests
- **`oxicode-sdk/src/builder.rs`** — Added `enable_routing()` method with routing config support
- **`oxicode-sdk/src/lib.rs`** — Added `pub mod multi_provider;`, re-exports for MultiProviderBuilder, RoutingConfig, CircuitBreakerConfig, MultiProviderConfig
- **`oxicode-sdk/src/prelude.rs`** — Added MultiProviderBuilder, RoutingConfig, CircuitBreakerConfig to prelude

### Notes

- Used `oxicode_ai::ComplexityRouter` instead of direct module import due to private module restrictions
- Boxed router workaround uses a local `BoxedRouter` struct wrapper to satisfy the `impl ComplexityRouter + 'static` bound
- The `enable_routing()` method in `OxicodeBuilder` collects existing providers before building the MultiProvider, as OxicodeBuilder consumes itself during build