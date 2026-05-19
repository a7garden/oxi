<div align="center">

# oxi-store

**Shared persistent state for oxi** — sessions, settings, auth, model registry.

[![Version](https://img.shields.io/badge/Version-0.20.0-blue?style=flat-square)](https://crates.io/search?q=oxi)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)
[![Docs](https://img.shields.io/docsrs/oxi-store/latest?style=flat-square)](https://docs.rs/oxi-store)

</div>

---

## Overview

`oxi-store` provides the persistence layer for the oxi coding assistant:

- **Session management** — append-only JSONL storage with tree branching and forking
- **Settings store** — layered configuration (defaults → global → project → env → CLI)
- **Auth storage** — secure API key and OAuth token storage
- **Model registry** — model definitions with pricing, context windows, and capabilities
- **Settings validation** — startup validation to prevent runtime panics

## Quick Start

```rust
use oxi_store::settings::Settings;

// Load settings (layered: defaults → global → project → env)
let settings = Settings::load()?;

// Validate
let report = settings.validate();
assert!(report.is_valid());

// Access
println!("Model: {:?}", settings.default_model);
println!("Theme: {:?}", settings.theme);
```

## Core Types

| Type | Purpose |
|------|---------|
| `SessionManager` | Create, persist, and navigate conversation sessions |
| `SessionEntry` | Single entry in a session tree (message + metadata) |
| `Settings` | Layered configuration with validation |
| `ValidationReport` | Settings validation results (errors + warnings) |
| `AuthStorage` | Secure API key and credential storage |
| `CliModelRegistry` | Model definitions with auth integration |

## Session Tree

Sessions are stored as trees — each entry has a `parent_id`, enabling branching:

```
Session Root
├── Entry 1 (user message)
├── Entry 2 (assistant response)
│   ├── Entry 3 (fork A: user follow-up)
│   └── Entry 4 (fork B: alternative follow-up)
```

## Settings Layers

Settings are resolved in order (later overrides earlier):

1. Built-in defaults
2. Global config: `~/.oxi/settings.toml`
3. Project config: `.oxi/settings.toml`
4. Environment variables (`OXI_*`)
5. CLI arguments (`-m`, `-p`, `--thinking`)

## License

[MIT](../LICENSE.md)
