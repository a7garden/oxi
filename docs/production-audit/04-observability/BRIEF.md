# Brief 04: Observability Gaps

**Area:** Structured logging, metrics export, and runtime health monitoring
**Severity:** 🟢 Normal
**Estimated scope:** 244 tracing instrumented call sites, 0 exporter configuration, metrics module exists but is internal-only

---

## Context

oxicode uses `tracing` throughout the codebase (244 call sites across all crates) for structured logging. However, there is no configured `tracing_subscriber` or exporter — trace events go to the default subscriber (if any). For a CLI application, this is partially acceptable (output goes to stderr), but for SDK consumers and multi-agent deployments, lack of structured export is a gap.

The project has a `oxicode-sdk/src/metrics.rs` module with `AtomicAgentMetrics` for in-process counters, but there is no export mechanism (Prometheus, OTel, or even JSON logging of metrics).

Current state:

| Component | Status | Detail |
|-----------|--------|--------|
| `tracing` instrumentation | ✅ Present | 244 call sites across all crates |
| Subscriber configuration | ❌ Missing | No `tracing_subscriber::fmt()` or structured exporter setup |
| Metrics collection | ⚠️ Partial | `AtomicAgentMetrics` in `oxicode-sdk/src/metrics.rs` — internal counters only |
| Metrics export | ❌ Missing | No Prometheus, OTel, or JSON metrics output |
| Health endpoints | ❌ N/A | CLI/TUI application — no HTTP server |
| Circuit breaker health | ✅ Present | `oxicode-ai/src/circuit_breaker.rs` — per-provider health tracking with TUI display |
| Session health | ⚠️ Implicit | Session errors surface via TUI/RPC but no structured health signal |

---

## Objective

Ensure that the tracing infrastructure is properly initialized and that trace events are accessible to users and operators.

This does NOT mean:
- ❌ Adding OpenTelemetry or Prometheus dependencies (overkill for a CLI tool)
- ❌ Creating an HTTP health endpoint (oxicode is a CLI, not a server)
- ❌ Adding `metrics` crate dependency
- ❌ Rewriting existing tracing calls

It DOES mean:
- ✅ oxicode CLI initializes a `tracing_subscriber` with appropriate formatting
- ✅ `RUST_LOG` environment variable is respected for log level control
- ✅ SDK users can bring their own subscriber without conflicts
- ✅ Log output is structured (JSON option available for machine parsing)

---

## Approach

### Phase 1: Audit (read-only)

1. Check how tracing is currently initialized (or not) in `oxicode-cli/src/main.rs`.
2. Check if `tracing_subscriber` is already a dependency.
3. Check if `RUST_LOG` is handled anywhere.
4. Read `oxicode-sdk/src/metrics.rs` and understand the metrics model.

```bash
rg "tracing_subscriber" --type rust | grep -v target/
rg "RUST_LOG" --type rust | grep -v target/
rg "init" oxicode-cli/src/main.rs | head -10
```

### Phase 2: Implement tracing initialization

1. In `oxicode-cli/src/main.rs`, add a `tracing_subscriber::fmt()` initialization:
   ```rust
   tracing_subscriber::fmt()
       .with_env_filter(
           tracing_subscriber::EnvFilter::from_default_env()
               .add_directive("oxicode=info".parse().unwrap())
       )
       .with_writer(std::io::stderr)
       .init();
   ```
2. Ensure this doesn't conflict with TUI mode (tracing output should go to stderr, not interfere with the alternate screen).

3. Add a `--log-format json` CLI option for machine-parseable output.

### Phase 3: SDK user opt-out

1. In `oxicode-sdk`, document that SDK consumers should initialize their own `tracing_subscriber` before creating agents.
2. Ensure oxicode-sdk never calls `tracing::subscriber::set_global_default()` — leave that to the application layer.

### Phase 4: Verify

1. `RUST_LOG=debug cargo run -- help 2>log.txt` — verify log output appears in stderr.
2. `RUST_LOG=off cargo run -- help` — verify no log output.
3. `cargo nextest run --workspace` — all tests pass.

---

## Constraints

- **Do not** add OTel/Prometheus/metrics-exporter dependencies.
- **Do not** change existing `tracing::info!` / `tracing::debug!` call sites.
- **Preserve** TUI display — tracing must not interfere with the alternate screen buffer.
- **Do not** call `set_global_default()` in library crates (oxicode-ai, oxicode-agent, oxicode-sdk, oxicode-store, oxicode-tui).

## Verification

1. `RUST_LOG=info cargo run -- help 2>&1 | grep -c "INFO"` — tracing output appears
2. `cargo nextest run --workspace` — 2131 tests pass
3. `cargo clippy --workspace -- -D warnings` — clean
