<div align="center">

# oxi

**A terminal-based AI coding assistant built in Rust.**

A Rust port of [pi](https://github.com/earendil-works/pi) by Mario Zechner.
Multi-provider · Streaming-first · Extensible · Session persistence

[![CI](https://img.shields.io/github/actions/workflow/status/a7garden/oxi/ci.yml?style=flat-square&label=CI)](https://github.com/a7garden/oxi/actions)
[![License](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](LICENSE.md)
[![Rust](https://img.shields.io/badge/Rust-1.82%2B-orange.svg?style=flat-square)](https://www.rust-lang.org/)

[Getting Started](#getting-started) · [Architecture](#architecture) · [Configuration](#configuration) · [Contributing](CONTRIBUTING.md) · [Changelog](CHANGELOG.md)

</div>

---

## Why oxi?

oxi is a Rust port of [pi](https://github.com/earendil-works/pi), re-implementing its core architecture — unified LLM API, agent runtime, tool calling, terminal UI, and session persistence — in idiomatic Rust with Tokio, Ratatui, and Serde.

It brings the power of LLM-based coding assistants directly to your terminal — fast, private, and fully under your control.

| Feature | Description |
|---------|-------------|
| 🌐 **Multi-provider** | OpenAI, Anthropic, Google, DeepSeek, Mistral, Groq, Cerebras, xAI, OpenRouter, Azure |
| ⚡ **Streaming-first** | Real-time token streaming with thinking/reasoning support |
| 🔧 **Tool calling** | Built-in read, write, edit, bash, grep, find — plus extensible trait system |
| 🖥️ **Interactive TUI** | Component-based terminal UI with themes, markdown rendering, and image support |
| 🌳 **Session system** | Persistent conversations with branching, forking, and JSONL storage |
| 🧩 **Skill system** | Pluggable prompt skills (brainstorming, deep research, code review, etc.) |
| 🔌 **Extensions** | Dynamically load native `.so`/`.dylib`/`.dll` or WASM plugins |
| 📦 **Package manager** | Install, update, and manage skill/extension packages |
| 🤖 **Multi-agent SDK** | Build multi-agent pipelines with the oxi-sdk builder pattern |

## Getting Started

### Install

```bash
# Build from source (requires Rust 1.82+)
git clone https://github.com/a7garden/oxi.git
cd oxi && cargo build --release

# The binary is at target/release/oxi
cp target/release/oxi /usr/local/bin/
```

### Configure

```bash
# Set your API key (Anthropic, OpenAI, Google, etc.)
export ANTHROPIC_API_KEY="sk-ant-..."
```

### Run

```bash
# Interactive mode (default)
oxi

# Single prompt
oxi "Explain Rust ownership"

# Specify provider and model
oxi -p openai -m gpt-4o "Design a REST API"
```

That's it. No config files required — oxi works out of the box.

## Architecture

oxi is a multi-crate Rust workspace designed for modularity:

```mermaid
graph LR
    A[oxi-cli] --> B[oxi-sdk]
    A --> C[oxi-tui]
    B --> E[oxi-agent]
    E --> F[oxi-ai]
```

| Crate | Purpose |
|-------|---------|
| [**oxi-ai**](oxi-ai/) | Unified LLM API — 10 providers, streaming, tool calling, compaction |
| [**oxi-agent**](oxi-agent/) | Agent runtime — tool-calling loop, event system, MCP client |
| [**oxi-tui**](oxi-tui/) | Terminal UI — differential rendering, themes, markdown, chat widgets |
| [**oxi-sdk**](oxi-sdk/) | Multi-agent SDK — agent groups, message bus, port-based adapters, builder pattern |
| [**oxi-cli**](oxi-cli/) | CLI binary — ties everything together (TUI + RPC + port composition root) |

## Configuration

Settings are layered — later layers override earlier ones:

```
defaults → ~/.oxi/settings.toml → .oxi/settings.toml → env vars → CLI flags
```

Example `~/.oxi/settings.toml`:

```toml
default_model = "anthropic/claude-sonnet-4-20250514"
default_provider = "anthropic"
thinking_level = "medium"
theme = "default"
temperature = 0.7
max_tokens = 4096
auto_compaction = true
```

Environment variable overrides:

| Variable | Setting |
|----------|---------|
| `OXI_MODEL` | `default_model` |
| `OXI_PROVIDER` | `default_provider` |
| `OXI_THEME` | `theme` |
| `OXI_TEMPERATURE` | `default_temperature` |
| `OXI_MAX_TOKENS` | `max_response_tokens` |

## CLI Reference

```
oxi [OPTIONS] [PROMPT]

Options:
  -p, --provider <PROVIDER>    Provider (anthropic, openai, google, ...)
  -m, --model <MODEL>          Model (claude-sonnet-4-20250514, gpt-4o, ...)
  -i, --interactive            Force interactive mode
      --thinking <LEVEL>       off, minimal, low, medium, high, xhigh
  -e, --extension <PATH>       Load extension (repeatable)
  -h, --help                   Print help
  -V, --version                Print version

Commands:
  sessions                     List all sessions
  tree [SESSION_ID]            Show session tree
  fork <PARENT> <ENTRY>        Fork a session
  delete <SESSION>             Delete a session
  pkg install <SOURCE>         Install a package
  pkg list                     List packages
  config show                  Show configuration
  config set <KEY> <VALUE>     Set a configuration value
```

## Built-in Skills

| Skill | Purpose |
|-------|---------|
| **Autonomous Loop** | Self-correcting design→implement→verify cycle |
| **Brainstorming** | Collaborative ideation with multi-approach comparison |
| **Deep Research** | Investigation, analysis, and design documentation |
| **Reviewer** | Multi-axis code review |
| **Super Review** | Deep system-level review |
| **Planner** | Implementation planning with dependency batching |
| **Scout** | Fast codebase reconnaissance and structure analysis |
| **Design Farmer** | Design system construction (colors, tokens, a11y) |
| **Playwright CLI** | Browser automation via Playwright |
| **Worktree** | Git worktree management |

## Development

```bash
cargo build                          # Debug build
cargo build --release                # Release binary
cargo test --workspace               # Run all tests (2,000+)
cargo clippy --workspace -- -D warnings   # Lint
cargo fmt --all -- --check           # Format check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development guide.

## FAQ

<details>
<summary><strong>Which providers are supported?</strong></summary>

OpenAI, Anthropic, Google, DeepSeek, Mistral, Groq, Cerebras, xAI, OpenRouter, and Azure OpenAI. Set the relevant `API_KEY` environment variable and use `-p <provider>` to select one.
</details>

<details>
<summary><strong>Can I use oxi as a library?</strong></summary>

Yes. Each crate is published independently: `oxi-ai` (LLM API), `oxi-agent` (agent runtime), `oxi-tui` (terminal UI), `oxi-sdk` (multi-agent SDK). See individual crate READMEs for details.
</details>

<details>
<summary><strong>How do sessions work?</strong></summary>

Sessions are stored as append-only JSONL files in `~/.oxi/sessions/`. Each session is a tree — you can fork from any point to explore alternative paths without losing history. Use `oxi sessions` to list, `oxi tree` to inspect, and `oxi fork` to branch.
</details>

<details>
<summary><strong>Does oxi send my code to third parties?</strong></summary>

oxi only communicates with the LLM provider you select. No telemetry, no analytics, no third-party data collection. Your code stays between you and your chosen provider.
</details>

## Attribution

oxi is a Rust port of [pi](https://github.com/earendil-works/pi) by [Mario Zechner](https://github.com/badlogicgames).
The architectural design, provider abstraction, tool system, streaming events, and session tree
are derived from the original pi project (MIT License). See [NOTICE.md](NOTICE.md) for details.

## License

[MIT](LICENSE.md) © 2025 Mario Zechner, 2025–2026 oxi contributors
