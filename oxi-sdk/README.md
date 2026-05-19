<div align="center">

# oxi-sdk

**Multi-agent SDK for oxi** — build isolated, multi-agent AI systems in Rust.

[![Version](https://img.shields.io/badge/Version-0.20.0-blue?style=flat-square)](https://crates.io/search?q=oxi)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)
[![Docs](https://img.shields.io/docsrs/oxi-sdk/latest?style=flat-square)](https://docs.rs/oxi-sdk)

</div>

---

## Overview

`oxi-sdk` provides a fluent builder API for constructing single and multi-agent AI systems
on top of `oxi-ai` and `oxi-agent`.

### Key Concepts

| Component | Purpose |
|-----------|---------|
| `OxiBuilder` | Configure the engine: providers, builtins, custom resolvers |
| `AgentBuilder` | Build individual agents with model, prompt, and tools |
| `AgentGroup` | Orchestrate multiple agents (pipeline, parallel, fan-out) |
| `MessageBus` | Pub/sub inter-agent communication |
| `AgentMetrics` | Track run count, tokens, and durations |
| `ClosureTool` | Create tools from closures without implementing the full trait |

## Quick Start

```rust
use oxi_sdk::OxiBuilder;

// Build the engine
let engine = OxiBuilder::new()
    .include_builtins(true)
    .build();

// Create a provider
let provider = engine.create_provider("anthropic")?;

// Build an agent
let agent = engine.agent("my-agent")
    .model_id("claude-sonnet-4-20250514")
    .system_prompt("You are a helpful coding assistant.")
    .coding_tools()
    .workspace("/path/to/project")
    .build()?;

// Run
let (response, events) = agent.run("Implement a REST API".into()).await?;
println!("{}", response.content);
```

## Design Principles

- **No global state** — each `Oxi` instance has its own provider/model registries
- **Runtime workspace injection** — `ToolContext` passed at execution time for tool reuse
- **ProviderResolver trait** — custom provider/model resolution for embedded use cases
- **Composable** — agents can be combined into groups with different orchestration strategies

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `oxi-sdk` | SDK entry point (this crate) |
| `oxi-ai` | LLM API — multi-provider streaming |
| `oxi-agent` | Agent runtime with tool-calling loop |

## License

[MIT](../LICENSE.md)
