# Attribution

oxicode is a Rust port of [pi](https://github.com/earendil-works/pi) by
[Mario Zechner](https://github.com/badlogicgames), licensed under the MIT License.

The original pi project is a TypeScript-based AI agent toolkit providing:
- A unified multi-provider LLM API
- An agent runtime with tool calling and state management
- A terminal UI library with differential rendering
- An interactive coding agent CLI

oxicode re-implements these concepts in Rust with the following key differences:
- **Language**: Rust (edition 2021) instead of TypeScript
- **Async runtime**: Tokio instead of Node.js
- **Rendering**: Ratatui instead of Ink
- **Serialization**: Serde + JSONL instead of JSON
- **Extension system**: Native shared libraries + WASM instead of npm packages

The architectural design, tool system, provider abstraction, streaming event model,
session tree structure, and overall API shape are derived from the original pi project.
All original pi code is Copyright (c) 2025 Mario Zechner, used under the MIT License.

## OpenAI Codex TUI rendering primitives

`oxicode-vtui/src/presentation/renderable.rs` is derived from
[`openai/codex`](https://github.com/openai/codex),
`codex-rs/tui/src/render/renderable.rs`, commit
`9ded177ce7c1c0bd2047f902936c177612ab3434` (retrieved 2026-08-16).
Copyright 2025 OpenAI. Licensed under the Apache License, Version 2.0.
