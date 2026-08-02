# oxi-sdk Stabilization Roadmap

> Defines the criteria and timeline for promoting `#[unstable]` features to `#[stable]`.

## Principles

- A feature graduates when it has: (1) ≥1 consumer using it in production, (2) frozen API surface (no planned breaking changes), (3) test coverage of the public contract.
- Until graduation, features stay behind cargo feature gates + `#[oxi_unstable]` annotations.
- The `unstable` umbrella feature enables all of them at once for development.
- A consumer counts only when it exercises the `oxi-sdk` public surface in a released or production-deployed integration. Using the underlying `oxi-ai` or `oxi-agent` type directly is useful evidence, but does not by itself satisfy the consumer criterion for the SDK re-export.
- Target versions are planning estimates, not promises. A feature that misses any graduation criterion remains unstable even if its target release arrives.

### Status notation

- **oxi-cli ✅** — the SDK feature is enabled and used by oxi-cli today.
- **oxios 🔜** — oxios is the intended production consumer, but its SDK integration is not yet sufficient evidence for graduation.
- **none ❌** — no qualifying production consumer is recorded.
- Test coverage distinguishes **integration** coverage of the exported contract from **unit** coverage in the defining crate. Unit tests are necessary but do not satisfy the integration-test criterion by themselves.

## Current State (v0.63.0)

| Feature | Tier | Consumer | API Stability | Test Coverage | Target Release |
|---|---|---|---|---|---|
| `circuit-breaker` | Unstable | oxios 🔜 | Medium — trait is small; state/error semantics need field validation | Unit coverage; no SDK integration contract test | v0.64.0 |
| `mcp-spawn-validator` | Unstable | oxios 🔜 | Medium — policy boundary is defined; spawn contract may evolve | Unit coverage; no cross-transport SDK integration test | v0.64.0 |
| `mcp-transport` | Unstable | oxios 🔜 | Medium — trait and HTTP lifecycle/error behavior need freezing | Transport tests exist; no SDK re-export integration test | v0.64.0 |
| `delegation` | Unstable | oxios 🔜 | Low — runner construction and result/usage contract may change | Implementation tests only; no production-style SDK integration test | v0.65.0 |
| `url-resolver` | Unstable | oxi-cli ✅ | High — adapter is narrow and already integrated | Unit coverage; SDK end-to-end contract coverage incomplete | v0.64.0 |
| `workflow-dsl` | Unstable | none ❌ | Low — execution/result and DSL semantics remain broad | Extensive engine unit tests; no consumer integration test | v0.66.0 |
| `role-routing` | Unstable | oxi-cli ✅ | Medium — fallback behavior is established; registry boundary may evolve | Unit coverage; no SDK-level routing integration test | v0.65.0 |
| `role-switching` | Unstable | oxi-cli ✅ | High — pure decision functions have a compact surface | Strong unit coverage; SDK re-export contract test missing | v0.64.0 |
| `router` | Unstable | oxi-cli ✅ | Medium — configuration, snapshots, and global controls may evolve | Broad module unit coverage; no SDK-level integration test | v0.65.0 |
| `advisor` | Unstable | none ❌ | Low — runtime/host/delivery surface is still evolving | Runtime unit tests; no SDK consumer integration test | v0.66.0 |
| `memory` | Unstable | oxios 🔜 | Medium — three-layer design is settled; adapter error/search semantics need freezing | SDK integration coverage for port bridge plus unit tests | v0.64.0 |
| `subagent` | Unstable | oxios 🔜 | Low — execution arguments, nesting, and result semantics may change | Tool/runner unit tests; no SDK consumer integration test | v0.65.0 |
| `agent-hub` | Unstable | oxios 🔜 | Low — status vocabulary and pool snapshot contract may expand | Unit coverage only; no SDK integration test | v0.65.0 |
| `lsp` | Unstable | oxios 🔜 | Low — provider lifecycle and action/result types may change | Provider/tool tests exist; no SDK trait integration test | v0.65.0 |
| `browser` | Unstable | oxios 🔜 | Low — engine/tab/config surface is large and still expanding | Mock-backed tool tests; no SDK backend conformance suite | v0.66.0 |

## Feature Details

### circuit-breaker

- **What it provides:** Re-exports the `CircuitBreaker` behavior trait, `DefaultCircuitBreaker` reference implementation, breaker states, shared wrapper, and typed `BreakerError`. Consumers can supply domain-specific failure thresholds while retaining the SDK's retry integration.
- **Consumer:** oxios 🔜. The ownership contract identifies oxios's A2A breaker as the intended policy implementation, but a released oxios integration against this SDK feature must be recorded before graduation.
- **API stability:** **Medium.** The three-method trait is intentionally compact, but adding a required method would break every consumer implementation. The meaning of `check`, half-open transitions, concurrency behavior, and named error variants must be frozen.
- **Test coverage:** The defining crate has state-machine and retry-path unit coverage; there is no SDK integration test that implements the public trait and verifies fail-fast/recovery behavior through the exported agent-loop path.
- **Graduation criteria:** Ship one oxios production implementation; freeze the three trait methods and `BreakerError` semantics; document thread-safety and half-open behavior; add an SDK integration test covering open rejection, successful recovery, and failure recording through retry integration; confirm `DefaultCircuitBreaker` remains a reference policy rather than a consumer contract.
- **Priority:** **High** — oxios needs the behavior/policy boundary now.
- **Target version:** **v0.64.0**.

### mcp-spawn-validator

- **What it provides:** Re-exports `SpawnValidator` and `NoopSpawnValidator`, allowing consumers to enforce command and environment policy before an MCP stdio server is spawned. SDK-owned loader-injection filtering remains mandatory independently of consumer policy.
- **Consumer:** oxios 🔜. Oxios is expected to provide its sandbox-specific spawn policy.
- **API stability:** **Medium.** The policy boundary is established, but command representation, environment sanitization ownership, async needs, and validation error semantics must not still be under design.
- **Test coverage:** Validator and stdio spawn paths have lower-level tests; there is no SDK integration test proving a custom validator is invoked for the public spawn API while mandatory SDK filtering cannot be bypassed.
- **Graduation criteria:** Deploy an oxios validator in production; freeze validator method signatures, invocation order, and rejection semantics; document which checks belong to the SDK versus the consumer; add integration tests for allow, deny, environment sanitization, and mandatory blocked-variable enforcement; verify all spawn entry points apply the validator consistently.
- **Priority:** **High** — oxios needs MCP spawn policy composition now.
- **Target version:** **v0.64.0**.

### mcp-transport

- **What it provides:** Re-exports the `McpTransport` abstraction and the SDK's `StdioTransport` and `StreamableHttpTransport` implementations. It gives consumers a common lifecycle and message path for local-process and HTTP MCP servers.
- **Consumer:** oxios 🔜. Oxios intends to reuse SDK transport behavior instead of maintaining a parallel MCP client.
- **API stability:** **Medium.** Core responsibilities are clear, but connection lifecycle, cancellation, capability negotiation, reconnection, and typed error behavior must be frozen across both implementations.
- **Test coverage:** The MCP module has transport-level tests, but the SDK re-export has no integration contract suite applied identically to stdio and streamable HTTP.
- **Graduation criteria:** Run at least one transport in oxios production; freeze trait methods and lifecycle/error semantics; document cancellation and shutdown guarantees; add a shared conformance suite covering connect, request/response, notification, malformed input, cancellation, and graceful close for both transports; verify the SDK public re-exports compile independently of internal paths.
- **Priority:** **High** — oxios needs the common transport layer now.
- **Target version:** **v0.64.0**.

### delegation

- **What it provides:** Exposes `SdkSubagentRunner`, the SDK adapter that creates fresh in-process agents and implements the subagent execution contract. It isolates child context and returns only the child's final text and usage.
- **Consumer:** oxios 🔜. Oxios is the intended in-process consumer; oxi-cli's existing process-oriented delegation does not qualify as use of this SDK adapter.
- **API stability:** **Low.** Agent selection, cancellation, inherited configuration, resource limits, usage reporting, and error typing may still change as production integrations mature.
- **Test coverage:** The implementation and related agent tool have unit tests, but no SDK integration test exercises a complete delegated run through `SdkSubagentRunner` with a deterministic provider.
- **Graduation criteria:** Deploy the adapter in one production consumer; freeze isolation guarantees, cancellation behavior, configuration inheritance, result fields, and failure semantics; add deterministic integration tests for successful execution, child isolation, propagated usage, provider failure, and cancellation; document resource and nesting limits.
- **Priority:** **High** — oxios needs in-process agent execution, but the contract requires production hardening.
- **Target version:** **v0.65.0**.

### url-resolver

- **What it provides:** Exposes `SdkUrlResolver`, which adapts the SDK `InternalUrlRouter` port to the agent tool layer's `UrlResolver`. Registered schemes such as `issue://` or `pr://` can then be dispatched by read/search tools without product-specific tool forks.
- **Consumer:** oxi-cli ✅. oxi-cli enables `url-resolver` and installs `SdkUrlResolver` in its agent configuration.
- **API stability:** **High.** The adapter is narrow; remaining decisions are error mapping, URI parsing normalization, and the exact metadata carried in resolved content.
- **Test coverage:** Router and adapter behavior have unit coverage, but the public SDK path lacks a complete integration test from registered protocol handler through an agent read/search tool.
- **Graduation criteria:** Freeze URI parsing, scheme matching, error conversion, and resolved-content semantics; add an SDK integration test that registers a handler and verifies successful dispatch, unknown scheme, malformed URI, and handler failure through a tool; confirm oxi-cli production use across one release without breaking changes.
- **Priority:** **Medium** — already consumed, so stabilization is primarily contract verification.
- **Target version:** **v0.64.0**.

### workflow-dsl

- **What it provides:** Exposes `WorkflowEngine`, `StepOutput`, and `WorkflowResult` for executing parsed workflow definitions across named agents, shared memory, and consensus steps. The surface defines execution ordering and externally visible step outcomes.
- **Consumer:** none ❌.
- **API stability:** **Low.** Result shape, partial-failure behavior, cancellation, retry semantics, step variants, expression/data model, and persistence hooks may evolve after real use.
- **Test coverage:** The engine has extensive unit tests for step execution and failures; there is no integration test with a qualifying consumer or compatibility fixture for serialized workflow/result contracts.
- **Graduation criteria:** Gain a production consumer; publish and freeze the supported DSL grammar and versioning policy; freeze `StepOutput`/`WorkflowResult` serialization and partial-failure semantics; add integration fixtures for sequencing, branching, foreach, consensus, unknown agents, malformed definitions, cancellation, and backward-compatible deserialization; demonstrate one release without a breaking DSL change.
- **Priority:** **Low** — no production consumer is currently recorded.
- **Target version:** **v0.66.0**.

### role-routing

- **What it provides:** Exposes `RoleRoutingProvider` and role registry types that select a role-specific model for each request while falling back to the default provider when routing is unavailable or fails. It is the provider-layer composition point for role-based model selection.
- **Consumer:** oxi-cli ✅. oxi-cli enables `role-routing` and wraps providers with `RoleRoutingProvider` in session and runtime construction.
- **API stability:** **Medium.** Transparent fallback is established, but global registry access, provider resolution, observability, and routing-failure policy may still change.
- **Test coverage:** Signal extraction and fallback assumptions have unit coverage; no SDK-level integration test uses deterministic providers to verify selected-model dispatch and fallback through the re-exported provider.
- **Graduation criteria:** Freeze constructor/registry ownership and fallback semantics; document provider-resolution and concurrency guarantees; add integration tests for configured routing, empty registry pass-through, missing provider, routed-provider failure, and no recursive routing; validate oxi-cli production behavior for one release.
- **Priority:** **Medium** — already in production, but provider-level behavior needs contract tests.
- **Target version:** **v0.65.0**.

### role-switching

- **What it provides:** Exposes `role_for_tool`, `decide_role`, `resolve_role_to_model`, and `RoleSignals`, the pure decision layer that maps observable turn signals to model roles and concrete models. Its precedence rules cover explicit overrides, tool bindings, thinking, long context, and trivial turns.
- **Consumer:** oxi-cli ✅. oxi-cli enables the feature and uses role resolution in session construction.
- **API stability:** **High.** The functions are compact and deterministic; the principal stability commitment is signal precedence and the role-to-model pattern format.
- **Test coverage:** The defining module has strong unit coverage of precedence and tool bindings; the SDK re-export lacks a small public-contract integration test.
- **Graduation criteria:** Publish and freeze the precedence table, tool-role bindings policy, long-context threshold policy, and model-pattern parsing; add SDK integration tests for every precedence branch and unresolved models; confirm existing oxi-cli mappings require no planned signature or semantic change.
- **Priority:** **Medium** — used today and comparatively ready to stabilize.
- **Target version:** **v0.64.0**.

### router

- **What it provides:** Re-exports `oxi_ai::router`, including complexity signals, routing profiles, `RouterProvider`, configuration/state types, snapshots, and pin controls. It selects configured model tiers based on observable request complexity.
- **Consumer:** oxi-cli ✅. oxi-cli enables `router` for its opt-in `router/*` model profiles and controls.
- **API stability:** **Medium.** Classification behavior is mature enough for use, but configuration schema, global mutable controls, snapshot fields, profile selection, and fallback behavior may evolve.
- **Test coverage:** Classifier, signals, types, and provider behavior have broad module unit tests; there is no SDK integration test that treats the re-exported module as a consumer would.
- **Graduation criteria:** Freeze the serialized `RouterConfig`/`RouterState` schema and migration policy; document decision precedence, pin scope, fallback, and global-state concurrency; add deterministic SDK integration tests for profile registration, tier selection, pin override, unresolved providers, and state snapshots; validate backward-compatible settings from the prior release.
- **Priority:** **Medium** — production use exists, but the public surface is broad.
- **Target version:** **v0.65.0**.

### advisor

- **What it provides:** Re-exports the shadow-reviewer subsystem: `AdvisorRuntime`, `AdviseTool`, advisor agent/host traits, delivery policy types, emission guards, note formatting, and guidance constants. It lets a read-only secondary agent observe transcript deltas and enqueue advice to the primary session.
- **Consumer:** none ❌ for the `oxi-sdk` feature. oxi-cli currently uses the underlying `oxi-agent` advisor API directly, which is valuable operational evidence but does not yet exercise this SDK-gated surface.
- **API stability:** **Low.** Runtime installation, host callbacks, scheduling/retry behavior, delivery channels, note schema, severity policy, and exported constants form a large evolving contract.
- **Test coverage:** Runtime and emission behavior have lower-level tests; there is no SDK integration test or consumer using the SDK re-export end to end.
- **Graduation criteria:** Migrate one production consumer to the `oxi-sdk` surface; narrow and freeze the minimum supported export set; freeze host callbacks, note/delivery schema, retry and deduplication semantics; add deterministic integration tests for transcript deltas, accepted/suppressed advice, delivery routing, retry, cancellation, and shutdown; document read-only tool and prompt compatibility guarantees.
- **Priority:** **Low** — direct lower-layer usage should be migrated before an SDK stability promise.
- **Target version:** **v0.66.0**.

### memory

- **What it provides:** Exposes `PortMemoryBackend`, `MemoryBackend`, `MemoryItem`, and the memory retain/recall/reflect/edit tools. `PortMemoryBackend` bridges the stable SDK `MemoryStore` and optional embedding ports to the agent tool-facing backend without collapsing the intentionally separate storage and tool contracts.
- **Consumer:** oxios 🔜. Oxios is expected to implement the tool-facing backend or adopt the port adapter; oxi-cli's current bespoke lower-layer backend is not use of this SDK feature.
- **API stability:** **Medium.** The three-layer ownership design is settled, but search behavior, embedding requirements, item metadata, pagination/listing, delete semantics, and error mapping must be frozen.
- **Test coverage:** `PortMemoryBackend` has unit tests and the SDK integration suite verifies the port-to-tool bridge. Production consumer coverage and a reusable backend conformance suite are still missing.
- **Graduation criteria:** Deploy either `MemoryBackend` or `PortMemoryBackend` through `oxi-sdk` in production; freeze trait methods, `MemoryItem` fields, text/JSON conversion, search-without-embeddings behavior, and tool error semantics; add a conformance suite covering put/search/list/delete plus all four tools, empty results, foreign JSON, embedding failure, and storage failure; keep the documented three-layer ownership intact.
- **Priority:** **High** — oxios needs a supported memory composition boundary now.
- **Target version:** **v0.64.0**.

### subagent

- **What it provides:** Exposes the `SubagentRunner` trait and `SubagentTool`, allowing a product to provide isolated child-agent execution while reusing the SDK's tool protocol. The trait carries task, context, depth, execution mode, and usage/result data between the tool and runner.
- **Consumer:** oxios 🔜. Oxios is the intended in-process implementation; oxi-cli's fallback that shells out does not qualify as use of the SDK trait.
- **API stability:** **Low.** The argument list is large, and nesting, cancellation, permissions, model selection, streaming, and structured result semantics may change after production adoption.
- **Test coverage:** The tool has mock-runner tests for single and parallel execution; no SDK integration test implements the public trait and drives it through an agent tool call.
- **Graduation criteria:** Land and operate a production trait implementation; reduce or deliberately freeze the runner input contract; document isolation, depth, concurrency, cancellation, permission, and usage guarantees; add SDK integration tests for single/parallel execution, depth limits, partial failure, cancellation, and runner absence; ensure `SdkSubagentRunner` conforms to the same suite.
- **Priority:** **High** — oxios needs the extension point, though stabilization follows production validation.
- **Target version:** **v0.65.0**.

### agent-hub

- **What it provides:** Exposes `AgentHubStatus`, `AgentInfo`, `AgentKind`, and `AgentPoolProvider` for reporting active agents and matching available subagents to work. UIs and coordination tools can consume a product-supplied pool snapshot without owning the pool implementation.
- **Consumer:** oxios 🔜. Oxios needs the hub/pool boundary for multi-agent status and scheduling; no qualifying SDK integration is recorded yet.
- **API stability:** **Low.** Status variants, identity, timestamps, task metadata, capability matching, snapshot consistency, and refresh/streaming behavior may expand.
- **Test coverage:** Lower-level status and tool behavior have unit coverage; there is no SDK integration test for a custom pool provider, snapshot semantics, or matching lifecycle.
- **Graduation criteria:** Deploy an oxios `AgentPoolProvider`; freeze identity and status semantics and make extensible enums non-exhaustive where appropriate; document snapshot consistency, ordering, and stale-agent handling; add integration tests for empty/running/idle/completed pools, task metadata, matching, concurrent updates, and provider failure; demonstrate UI compatibility across one release.
- **Priority:** **High** — oxios needs the agent-hub contract now.
- **Target version:** **v0.65.0**.

### lsp

- **What it provides:** Exposes `LspProvider` and `LspAction`, the capability boundary used by the agent's LSP tool for startup, readiness, diagnostics, navigation, and edits. Products retain ownership of language-server processes and routing.
- **Consumer:** oxios 🔜 for the SDK feature. oxi-cli implements the underlying `oxi-agent` trait directly, proving the concept but not yet qualifying the `oxi-sdk` re-export as consumed.
- **API stability:** **Low.** The lifecycle methods and action/result vocabulary are broad; cancellation, timeouts, multi-server routing, diagnostic ownership, and edit application may evolve.
- **Test coverage:** oxi-cli and the tool layer have provider tests, but there is no SDK-level mock-provider conformance suite covering the re-exported public contract.
- **Graduation criteria:** Adopt the SDK trait path in a production consumer; freeze startup/readiness/shutdown and action/result semantics; document cancellation, timeout, diagnostics-drain, and edit-application ownership; add a conformance suite for disabled providers, lazy startup, diagnostics, definition/references/rename, server failure, malformed responses, and concurrent requests; keep trait evolution implementor-safe.
- **Priority:** **High** — oxios needs a stable capability port, and existing lower-layer use supplies useful validation.
- **Target version:** **v0.65.0**.

### browser

- **What it provides:** Exposes `BrowserEngine`, `BrowserTab`, `BrowseConfig`, browser errors and page/element/link types, tab guards, and browse tools. Consumers can supply a custom backend without depending directly on `oxi-agent`; the native backend remains separately gated by `native-browser`.
- **Consumer:** oxios 🔜 for the SDK feature. oxi-cli uses native browser functionality, but the generic `browser` SDK gate is not yet its declared public integration boundary.
- **API stability:** **Low.** The engine/tab traits and configuration surface cover navigation, extraction, observation, interaction, waits, sessions, errors, and resource limits; additions to required trait methods would be breaking.
- **Test coverage:** Browse tools have mock-backed tests and the native backend has lower-level coverage; there is no public SDK backend conformance suite or production custom backend using the gated re-exports.
- **Graduation criteria:** Deploy a consumer through the SDK `BrowserEngine`/`BrowserTab` path; freeze the minimum required trait methods and provide defaults for additive capabilities; freeze config defaults, error taxonomy, page/element identity, timeout, cleanup, and tab-lifetime semantics; add a backend conformance suite covering navigation, extraction, actions, waits/timeouts, tab cleanup, malformed pages, and backend failures; verify generic `browser` and `native-browser` feature composition independently.
- **Priority:** **High** — oxios needs a backend-neutral browser boundary, but the large contract requires the longest validation period.
- **Target version:** **v0.66.0**.

## Graduation Process

1. Confirm the feature meets every roadmap criterion: at least one production consumer, a frozen public API with no planned breaking change, and integration coverage of the public contract.
2. Review the consumer evidence and public-contract tests in the stabilization pull request; update this roadmap's consumer, stability, and coverage fields.
3. Remove the `#[oxi_unstable(feature = "...")]` annotation and add `#[oxi_stable(since = "X.Y.0")]` to every public item in the feature's surface.
4. Remove the cargo feature gate. If ecosystem compatibility requires retaining the feature name, keep it temporarily as a documented no-op and set a release for its removal.
5. Remove the feature from the `unstable` umbrella list once no unstable item depends on it.
6. Add a CHANGELOG entry under `## Stabilized` naming the feature, the stable surface, the first stable version, and the validated production consumer.
7. Run a `cargo-public-api` diff against the previous release and confirm that stabilization introduces no unintended removal, signature change, or semantic break.
8. Run the feature's targeted integration/conformance tests and compile a minimal consumer with default features to prove that the graduated API no longer requires an unstable opt-in.

A target release alone never authorizes graduation. If evidence is incomplete, maintainers update the target version and leave both the cargo gate and `#[oxi_unstable]` annotation in place.
