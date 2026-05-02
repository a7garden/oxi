# oxi

CLI coding agent harness — the top-level binary that ties together the oxi workspace.

## Overview

`oxi` is a terminal-based AI coding assistant. It provides an interactive REPL for chatting with LLMs, a session system for persisting and branching conversations, built-in tools (read, write, edit, bash), a skill/template system, and dynamic extension loading.

## Architecture

```
oxi (CLI harness)
├── oxi-ai      — Unified LLM API (streaming, providers, context, tools)
├── oxi-agent   — Agent runtime (event loop, tool execution, compaction)
└── oxi-tui     — Terminal UI framework (components, rendering, themes)
```

The `oxi` crate itself handles:

- **CLI argument parsing** via `clap`
- **Session management** (JSONL persistence, forking, tree navigation)
- **Settings** (`~/.oxi/settings.toml`)
- **Skill loading** from `~/.oxi/skills/<name>/SKILL.md`
- **Prompt templates** from `~/.oxi/templates/`
- **Package management** (install/uninstall extensions and skills)
- **Extension loading** (dynamic `.so`/`.dylib`/`.dll` shared libraries)

## Installation

```bash
# Build from source
cargo build --release

# The binary is at target/release/oxi
cp target/release/oxi /usr/local/bin/
```

### Requirements

- Rust 1.80+ (edition 2021)
- An API key for at least one LLM provider (see Provider Setup below)

## Quick Start

```bash
# Interactive mode (default)
oxi

# Single prompt (non-interactive)
oxi "Explain Rust ownership in one paragraph"

# Specify provider and model
oxi -p openai -m gpt-4o

# With thinking level
oxi --thinking thorough "Design a REST API for a todo app"

# Load an extension
oxi -e ./my_extension.so
```

### Interactive Commands

Inside the REPL, type `/` followed by a command:

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/model` | Show current model |
| `/model <provider/model>` | Switch model (e.g., `openai/gpt-4o`) |
| `/models` | List available models |
| `/sessions` | List all sessions |
| `/tree` | Show current session tree |
| `/fork <entry_id>` | Fork a new session from an entry |
| `/skill` | List available skills |
| `/skill <name>` | Activate a skill |
| `/skill off <name>` | Deactivate a skill |
| `/template` | List prompt templates |
| `/template <name> [key=val ...]` | Expand and send a template |
| `/history` | Show conversation history |
| `exit` / `quit` | Exit the REPL |

## Provider Setup

oxi reads API keys from environment variables:

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
# ~/.oxi/settings.toml
default_model = "openai/gpt-4o"
default_provider = "openai"
thinking_level = "standard"
```

Or on the command line:

```bash
oxi -m gpt-4o -p openai
```

## CLI Reference

```
oxi [OPTIONS] [PROMPT]

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
  pkg install <SOURCE>         Install a package
  pkg list                     List installed packages
  pkg uninstall <NAME>         Uninstall a package
```

## Settings

Settings are stored at `~/.oxi/settings.toml`:

```toml
thinking_level = "standard"       # none, minimal, standard, thorough
theme = "default"                 # TUI color theme
default_model = "anthropic/claude-sonnet-4-20250514"
default_provider = "anthropic"
max_tokens = 4096
temperature = 0.7
session_history_size = 100
stream_responses = true
```

## Skills

Skills are markdown files that inject context into the system prompt. Place them in `~/.oxi/skills/<name>/SKILL.md`:

```
~/.oxi/skills/
├── rust-expert/
│   └── SKILL.md     # Activated with /skill rust-expert
└── code-review/
    └── SKILL.md     # Activated with /skill code-review
```

## Templates

Prompt templates support variable substitution. Place `.md` files in `~/.oxi/templates/`:

```markdown
<!-- ~/.oxi/templates/review.md -->
Review the following {{language}} code for bugs and improvements:

{{code}}
```

Use with `/template review language=Rust code="fn main() {}"`.

## Sessions

Sessions are persisted as JSONL files in `~/.oxi/sessions/`. Each session is a tree of entries that can be listed, inspected, and forked:

```bash
# List sessions
oxi sessions

# View session tree
oxi tree

# Fork from a specific entry
oxi fork <parent-session-id> <entry-id>

# Delete a session
oxi delete <session-id>
```

## Extensions

Extensions are dynamically loaded shared libraries that register custom tools with the agent:

```bash
oxi -e ./my_custom_tool.so
```

Extensions implement the `AgentTool` trait from `oxi-agent` and are registered at startup.

## License

MIT
