<div align="center">

# oxicode

**CLI binary** — the terminal-based AI coding assistant that ties the oxicode workspace together.

[![CI](https://img.shields.io/github/actions/workflow/status/project-oxi/oxicode/ci.yml?style=flat-square&label=CI)](https://github.com/project-oxi/oxicode/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)

</div>

---

## Overview

`oxicode` is a terminal-based AI coding assistant. It provides an interactive REPL for chatting with LLMs, a session system for persisting and branching conversations, built-in tools (read, write, edit, bash), a skill/template system, and dynamic extension loading.

## Architecture

```
oxicode (CLI harness)
├── oxicode-ai      — Unified LLM API (streaming, providers, context, tools)
├── oxicode-agent   — Agent runtime (event loop, tool execution, compaction)
└── oxicode-tui     — Terminal UI framework (components, rendering, themes)
```

The `oxicode` crate itself handles:

- **CLI argument parsing** via `clap`
- **Session management** (JSONL persistence, forking, tree navigation)
- **Settings** (`~/.oxicode/settings.toml`)
- **Skill loading** from `~/.oxicode/skills/<name>/SKILL.md`
- **Prompt templates** from `~/.oxicode/templates/`
- **Package management** (install/uninstall extensions and skills)
- **Extension loading** (dynamic `.so`/`.dylib`/`.dll` shared libraries)

## Installation

```bash
# Build from source
cargo build --release

# The binary is at target/release/oxicode
cp target/release/oxicode /usr/local/bin/
```

### Requirements

- Rust 1.80+ (edition 2021)
- An API key for at least one LLM provider (see Provider Setup below)

## Quick Start

```bash
# Interactive mode (default)
oxicode

# Single prompt (non-interactive)
oxicode "Explain Rust ownership in one paragraph"

# Specify provider and model
oxicode -p openai -m gpt-4o

# With thinking level
oxicode --thinking thorough "Design a REST API for a todo app"

# Load an extension
oxicode -e ./my_extension.so
```

### Interactive Commands

Inside the REPL, type `/` followed by a command:

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/model` | Show current model |
| `/model <provider/model>` | Switch model (e.g., `openai/gpt-4o`) |
| `/scoped-models` | Set/get models for Ctrl+P cycling |
| `/router` | Configure model router |
| `/router pin <tier>` | Pin router tier (low/medium/high/off) |
| `/router disable` | Switch away from router |
| `/router enable` | Switch to router/auto |
| `/skill` | List skills with active status |
| `/skill <name>` | Activate a skill |
| `/skill off <name>` | Deactivate a skill |
| `/new` | Start a new session |
| `/clone` | Duplicate current session |
| `/resume` | Resume a different session |
| `/fork [id]` | Fork a new session from a previous message |
| `/tree` | Show session tree |
| `/session` | Show session info and stats |
| `/compact` | Compact context |
| `/tools` | List/toggle tools |
| `/extensions` | List extensions & WASM tools |
| `/export [path]` | Export session to HTML |
| `/import <path>` | Import session from JSONL |
| `/share` | Share session as GitHub Gist |
| `/copy` | Copy last response to clipboard |
| `/name <name>` | Set session name |
| `/provider` | Configure API key |
| `/logout` | Remove provider authentication |
| `/settings` | Show current settings |
| `/reload` | Reload settings, theme, and extensions |
| `/changelog` | Show changelog |
| `/hotkeys` | Show key shortcuts |
| `/quit` | Quit oxicode |

## Provider Setup

oxicode reads API keys from environment variables:

| Provider | Environment Variable |
|----------|---------------------|
| OpenAI | `OPENAI_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| Google | `GOOGLE_API_KEY` |
| DeepSeek | `DEEPSEEK_API_KEY` |
| Mistral | `MISTRAL_API_KEY` |
| Groq | `GROQ_API_KEY` |
| Cerebras | `CEREBRAS_API_KEY` |
| xAI | `XAI_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |
| Azure OpenAI | `AZURE_OPENAI_API_KEY` |

Add them to your shell profile:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
```

### Default Model

The default model is `anthropic/claude-sonnet-4-20250514`. Override it in settings:

```toml
# ~/.oxicode/settings.toml
default_model = "openai/gpt-4o"
default_provider = "openai"
thinking_level = "standard"
```

Or on the command line:

```bash
oxicode -m gpt-4o -p openai
```

## CLI Reference

```
oxicode [OPTIONS] [PROMPT]

Arguments:
  [PROMPT]  Initial prompt (non-interactive mode)

Options:
  -p, --provider <PROVIDER>    Provider (e.g., anthropic, openai, google)
  -m, --model <MODEL>          Model (e.g., claude-sonnet-4-20250514, gpt-4o)
  -i, --interactive            Force interactive mode
      --thinking <LEVEL>       Thinking level: none, minimal, standard, thorough
  -e, --extension <PATH>       Load extension from shared library (repeatable)
  -h, --help                   Print help
  -V, --version                Print version

Subcommands:
  sessions                     List all sessions
  tree [SESSION_ID]            Show session tree structure
  fork <PARENT_ID> <ENTRY_ID>  Fork a session from a specific entry
  delete <SESSION_ID>          Delete a session
  export [SESSION_ID]          Export session to HTML
  import <PATH>                Import session from JSONL
  share [SESSION_ID]           Share session as GitHub Gist
  pkg install <SOURCE>         Install a package
  pkg list                     List installed packages
  pkg uninstall <NAME>         Uninstall a package
  config show                  Show current settings
  config set <KEY> <VALUE>     Set a config value
  config enable <TYPE> <NAME>  Enable a resource
  config disable <TYPE> <NAME> Disable a resource
  models [--provider]          List available models
  setup [--reset]              Run setup wizard
  reset [--yes]                Reset to factory defaults
  ext install <SOURCE>         Install WASM extension
  ext list                     List extensions
  ext remove <NAME>            Remove extension
```

## Settings

Settings are stored at `~/.oxicode/settings.toml`:

```toml
thinking_level = "standard"       # none, minimal, standard, thorough
theme = "default"                 # TUI color theme
default_model = "anthropic/claude-sonnet-4-20250514"
default_provider = "anthropic"
max_tokens = 4096
temperature = 0.7
session_history_size = 100
```

## Skills

Skills are markdown files that inject context into the system prompt. Place them in `~/.oxicode/skills/<name>/SKILL.md`:

```
~/.oxicode/skills/
├── rust-expert/
│   └── SKILL.md     # Activated with /skill rust-expert
└── code-review/
    └── SKILL.md     # Activated with /skill code-review
```

## Templates

Prompt templates support variable substitution. Place `.md` files in `~/.oxicode/templates/`:

```markdown
<!-- ~/.oxicode/templates/review.md -->
Review the following {{language}} code for bugs and improvements:

{{code}}
```

Use with `/template review language=Rust code="fn main() {}"`.

## Sessions

Sessions are persisted as JSONL files in `~/.oxicode/sessions/`. Each session is a tree of entries that can be listed, inspected, and forked:

```bash
# List sessions
oxicode sessions

# View session tree
oxicode tree

# Fork from a specific entry
oxicode fork <parent-session-id> <entry-id>

# Delete a session
oxicode delete <session-id>
```

## Extensions

Extensions are dynamically loaded shared libraries that register custom tools with the agent:

```bash
oxicode -e ./my_custom_tool.so
```

Extensions implement the `AgentTool` trait from `oxicode-agent` and are registered at startup.

## License

[MIT](../LICENSE.md)
