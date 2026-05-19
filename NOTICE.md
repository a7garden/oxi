# Attribution

oxi is a Rust port of [pi](https://github.com/earendil-works/pi) by
[Mario Zechner](https://github.com/badlogicgames), licensed under the MIT License.

The original pi project is a TypeScript-based AI agent toolkit providing:
- A unified multi-provider LLM API
- An agent runtime with tool calling and state management
- A terminal UI library with differential rendering
- An interactive coding agent CLI

oxi re-implements these concepts in Rust with the following key differences:
- **Language**: Rust (edition 2021) instead of TypeScript
- **Async runtime**: Tokio instead of Node.js
- **Rendering**: Ratatui instead of Ink
- **Serialization**: Serde + JSONL instead of JSON
- **Extension system**: Native shared libraries + WASM instead of npm packages

The architectural design, tool system, provider abstraction, streaming event model,
session tree structure, and overall API shape are derived from the original pi project.
All original pi code is Copyright (c) 2025 Mario Zechner, used under the MIT License.
