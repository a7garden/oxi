# oxi-sdk

**oxi AI agent SDK** — build isolated, multi-agent AI systems.

## Features

- **Isolated instances** — No global state. Each `Oxi` instance has its own provider/model registries.
- **Runtime workspace injection** — `ToolContext` passed at execution time, so tools can be reused across workspaces.
- **ProviderResolver trait** — Custom provider/model resolution for embedded use cases (oxios).
- **Fluent builder API** — `OxiBuilder` → `AgentBuilder` → `Agent`.

## Example

```rust
use oxi_sdk::{OxiBuilder, AgentConfig};

let oxi = OxiBuilder::new().with_builtins().build();

let agent = oxi.agent(AgentConfig {
    model_id: "anthropic/claude-sonnet-4-20250514".into(),
    max_iterations: 20,
    ..Default::default()
})
.workspace("/workspace/agent-1")
.system_prompt("You are an autonomous coding agent.")
.coding_tools()
.build()?;

let (response, events) = agent.run("Build a REST API".into()).await?;
println!("Result: {}", response.content);
```

## Packages

| Crate | Description |
|-------|-------------|
| `oxi-sdk` | SDK entry point (this crate) |
| `oxi-ai` | LLM API — multi-provider streaming |
| `oxi-agent` | Agent runtime with tool-calling loop |

## License

MIT