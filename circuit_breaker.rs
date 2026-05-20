

### Completed

Created `/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/src/circuit_breaker.rs` implementing a per-provider circuit breaker, enhanced from the original `oxi-agent/recovery.rs` implementation.

### Files Changed

- **`oxi-ai/src/circuit_breaker.rs`** (31,445 bytes) — New file with complete circuit breaker implementation
- **`oxi-ai/src/lib.rs`** — Added `pub mod circuit_breaker;` declaration

### Implementation Summary

| Component | Description |
|-----------|-------------|
| `CircuitState` | Enum with `Closed=0`, `Open=1`, `HalfOpen=2` (repr u8) |
| `CircuitBreakerConfig` | Config with `failure_threshold`, `open_duration`, `half_open_successes` + Default + builder methods |
| `CircuitOpenError` | Error type with `#[derive(thiserror::Error)]` displaying remaining duration |
| `ProviderCircuitBreaker` | Full circuit breaker with atomic state, consecutive counters, mutex-protected timestamp |
| `CircuitBreakerDiagnostics` | Diagnostic struct for monitoring |

### Key Behaviors

- **Closed**: Allow all requests; reset failures on success
- **Open**: Block after threshold failures for `open_duration`; auto-transition to half-open when elapsed
- **Half-Open**: Allow limited probes; require `half_open_successes` successes before closing; any failure reopens

### Test Results

All **20 tests pass**, covering:
- State transitions (Closed → Open → HalfOpen → Closed)
- Failure threshold triggering
- Duration-based transitions
- Half-open success/failure handling
- Multiple half-open successes requirement
- Reset functionality
- Error display and cloning
- Configuration validation

### Notes

- The original `oxi-agent/recovery.rs` had a generic `CircuitBreaker` struct; this new implementation uses `ProviderCircuitBreaker` to emphasize the per-provider scope
- Added `CircuitBreakerDiagnostics` for monitoring/metrics integration
- Added builder-style methods on `CircuitBreakerConfig` for ergonomic configuration
- All atomic operations use `Ordering::SeqCst` for simplicity and correctness