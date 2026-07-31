# oxi-lsp

Thin LSP protocol adapter for the [oxi](https://github.com/project-oxi/oxicode) workspace.

Provides JSON-RPC framing, an `lsp-types` wrapper, and an `async-process` +
`async-lsp` runtime. Multi-server lifecycle (spawn, register, route) lives in
the `oxi-cli` composition root — this crate stays a focused, dependency-light
protocol layer so other products can reuse it without pulling in the CLI.
