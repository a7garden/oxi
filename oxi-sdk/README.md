<div align="center">

# oxi-sdk

**Multi-agent SDK for oxi** — build isolated, secure, observable AI agent systems in Rust.

[![Version](https://img.shields.io/badge/Version-0.23.0-blue?style=flat-square)](https://crates.io/search?q=oxi)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)

</div>

---

## Overview

`oxi-sdk` provides a fluent builder API for constructing single and multi-agent AI systems
on top of `oxi-ai` and `oxi-agent`.

### Key Concepts

| Component | Purpose |
|-----------|---------|
| `OxiBuilder` | Configure the engine: providers, models, API keys, built-ins |
| `AgentBuilder` | Build individual agents with model, prompt, tools, and security |
| `AgentGroup` | Orchestrate multiple agents (pipeline, parallel, orchestrated) |
| `MessageBus` | Pub/sub inter-agent communication |
| `AgentSupervisor` | Lifecycle management: spawn, suspend, resume, restart |
| `Security` | Capability-based access control with role hierarchy |
| `Observability` | Tracing, audit logging, cost tracking, event sourcing |
| `Coordination` | Work queue, shared memory, consensus voting |

## Quick Start

```rust
use oxi_sdk::prelude::*;

let oxi = OxiBuilder::new()
    .with_builtins()
    .api_key("anthropic", "sk-ant-...")
    .build();

let agent = oxi.agent(AgentConfig {
    model_id: "anthropic/claude-sonnet-4-20250514".into(),
    max_iterations: 20,
    ..Default::default()
})
.workspace("/my/project")
.coding_tools()
.system_prompt("You are a senior Rust developer.")
.build()?;

let (response, events) = agent.run("Refactor main.rs".into()).await?;
println!("{}", response.content);
```

## Architecture

```
┌─────────────────────────────────────────────┐
│  OxiBuilder → Oxi                           │  Engine + Registry
├─────────────────────────────────────────────┤
│  AgentBuilder → Agent                       │  Per-agent config
├─────────────────────────────────────────────┤
│  AgentGroup │ MessageBus │ Supervisor       │  Orchestration
├─────────────────────────────────────────────┤
│  Security │ Middleware │ Observability      │  Cross-cutting
├─────────────────────────────────────────────┤
│  Coordination (Queue + Memory + Consensus)  │  Inter-agent
└─────────────────────────────────────────────┘
```

### Design Principles

- **No global state** — each `Oxi` instance has its own isolated provider/model registries
- **Streaming-first** — every provider streams tokens
- **Capability-based security** — deny-by-default with fine-grained permissions
- **Composable** — agents combine into groups with different orchestration strategies
- **Observable** — tracing, audit, cost tracking, and event sourcing built in

## Ownership Contract

See [`docs/oxi-sdk-ownership.md`](../docs/oxi-sdk-ownership.md) for the
canonical "who owns what" table. The SDK owns behavior (interfaces + reference
impls); consumers own policy (domain thresholds, validation, tiering).

## Feature Flags
## Features

### Security

```rust
use oxi_sdk::prelude::*;

let audit = Arc::new(AuditLog::new(64));
let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

// Define roles
authorizer.define_role("coder", CapabilitySet::coding("/workspace"));
authorizer.define_role("reader", CapabilitySet::read_only("/workspace"));

// Bind to agents
authorizer.bind_role("dev-agent", "coder");
```

### Observability

```rust
use oxi_sdk::prelude::*;

// Distributed tracing
let tracer = Arc::new(Tracer::new());
{
    let mut span = tracer.start("agent-run", SpanKind::Agent);
    span.set_attribute("model", json!("claude-sonnet-4"));
}

// Cost tracking
let registry = Arc::new(ModelRegistry::new());
let cost = Arc::new(CostTracker::new(registry, CostTrackerConfig {
    per_agent_budget: Some(10.0),
    global_budget: Some(100.0),
    ..Default::default()
}));
```

### Multi-Agent Orchestration

```rust
use oxi_sdk::prelude::*;

let group = AgentGroup::new(
    GroupStrategy::Parallel { max_concurrency: 4 }
)
.agent(Arc::new(reviewer))
.agent(Arc::new(tester));

let result = group.run("Review this codebase.".into()).await?;
println!("Success: {}/{}", result.success_count(), result.results.len());
```

### Coordination

```rust
use oxi_sdk::prelude::*;

// Work queue with priority-based claiming
let queue = WorkQueue::new(WorkQueueConfig::default());
queue.enqueue("review", json!({"file": "main.rs"}), 5);
let item = queue.claim("agent-1", None);

// Shared memory with optimistic locking
let memory = SharedMemory::new();
let key = MemoryKey::new("reviews", "main-rs");
let v1 = memory.write(&key, json!("approved"), "agent-1", None)?;
let v2 = memory.write(&key, json!("merged"), "agent-2", Some(v1))?; // succeeds
memory.write(&key, json!("conflict"), "agent-3", Some(v1)); // fails: VersionConflict

// Consensus voting
let consensus = Consensus::new();
consensus.start("deploy-vote", vec!["agent-1".into(), "agent-2".into()], 0.5);
```

## Feature Flags

| Flag | Description |
|------|-------------|
| `default` | Core SDK |
| `native-browser` | Built-in headless browser tools |

## Module Index

| Module | Purpose |
|--------|---------|
| `builder` | `OxiBuilder` and `Oxi` engine |
| `agent_builder` | `AgentBuilder` for agent configuration |
| `agent_group` | `AgentGroup` for multi-agent orchestration |
| `security` | `Authorizer`, `Capability`, `CapabilitySet`, `SecurityMiddleware` |
| `middleware` | `Middleware` trait, pipeline, built-in middleware |
| `observability` | `Tracer`, `AuditLog`, `CostTracker`, `EventStore` |
| `lifecycle` | `AgentSupervisor`, `AgentHandle`, snapshots |
| `coordination` | `WorkQueue`, `SharedMemory`, `Consensus` |
| `message_bus` | `MessageBus` for inter-agent pub/sub |
| `routing` | `RoutingControl` for dynamic model routing |
| `metrics` | `AgentMetrics` for execution statistics |
| `error` | `SdkError` and `SdkResult<T>` |

## Crate Dependencies

```
oxi-ai  ←  oxi-agent  ←  oxi-sdk
```

| Crate | Description |
|-------|-------------|
| `oxi-ai` | LLM API — multi-provider streaming |
| `oxi-agent` | Agent runtime with tool-calling loop |
| `oxi-sdk` | SDK entry point (this crate) |

## License

[MIT](../LICENSE.md)
