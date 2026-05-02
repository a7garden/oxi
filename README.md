# oxi

A terminal-based AI coding assistant built in Rust. Multi-provider, streaming-first, extensible.

```
┌─────────────────────────────────────────────────────┐
│  oxi                                                │
│  CLI coding harness                                 │
├─────────────────────────────────────────────────────┤
│                                                     │
│   oxi-ai        Unified LLM API                     │
│   oxi-agent     Agent runtime (tool loop)            │
│   oxi-tui       Terminal UI framework                │
│   oxi           CLI binary & skills                  │
│                                                     │
│   77 source files · 43,800 LOC · 656 tests          │
└─────────────────────────────────────────────────────┘
```

## Features

- **Multi-provider** — OpenAI, Anthropic, Google, DeepSeek, Mistral, Groq, Cerebras, xAI, OpenRouter, Azure OpenAI
- **Streaming** — real-time token streaming with thinking/reasoning support
- **Tool calling** — built-in read, write, edit, bash tools with extensible trait system
- **Interactive TUI** — component-based terminal UI with differential rendering, themes, markdown
- **Session system** — persistent conversations with branching and forking
- **Skill system** — pluggable prompt skills (brainstorming, deep research, code review, etc.)
- **Extensions** — dynamically load `.so`/`.dylib`/`.dll` plugins at runtime
- **Package manager** — install, update, and manage skill/extension packages

## Quick Start

```bash
# Build
cargo build --release

# Interactive mode (default)
./target/release/oxi

# Single prompt
./target/release/oxi "Explain Rust ownership"

# Specify provider and model
./target/release/oxi -p openai -m gpt-4o

# Install to PATH
cp target/release/oxi /usr/local/bin/
```

### Provider Setup

Set API keys in your shell profile:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GOOGLE_API_KEY="..."
```

### CLI Reference

```
oxi [OPTIONS] [PROMPT]...

Options:
  -p, --provider <PROVIDER>    Provider (anthropic, openai, google, deepseek, ...)
  -m, --model <MODEL>          Model (claude-sonnet-4-20250514, gpt-4o, ...)
  -i, --interactive            Force interactive mode
      --thinking <LEVEL>       none, minimal, standard, thorough
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
  pkg uninstall <NAME>         Uninstall a package
  pkg update [NAME]            Update package(s)
  config show                  Show configuration
  config set <KEY> <VALUE>     Set a configuration value
  config get <KEY>             Get a configuration value
  config enable <TYPE> <NAME>  Enable a resource
  config disable <TYPE> <NAME> Disable a resource
```

## Architecture

```
oxi (CLI harness)
├── oxi-ai         Unified LLM API
│   ├── providers/     OpenAI, Anthropic, Google, DeepSeek, ...
│   ├── messages.rs    Provider-agnostic message types
│   ├── context.rs     Conversation context management
│   ├── compaction.rs  Automatic context compaction
│   ├── tools.rs       Tool/function calling
│   ├── model_registry.rs  Model definitions & discovery
│   └── types.rs       Shared types (tokens, costs, modalities)
│
├── oxi-agent      Agent runtime
│   ├── agent.rs       Event loop with tool-calling cycle
│   ├── tools/         Built-in tools (read, write, edit, bash, ...)
│   ├── state.rs       Shared agent state
│   ├── events.rs      AgentEvent streaming types
│   ├── compaction.rs  Compaction integration
│   └── config.rs      Agent configuration
│
├── oxi-tui        Terminal UI framework
│   ├── tui.rs         Main TUI event loop
│   ├── terminal.rs    Terminal setup/teardown
│   ├── renderer.rs    Differential renderer
│   ├── surface.rs     Cell grid buffer
│   ├── theme.rs       Color themes & hot-reload
│   ├── components/    Text, Input, Editor, Markdown, Chat, Image, ...
│   ├── autocomplete.rs Path/command completion
│   └── event.rs       Unified event types
│
└── oxi            CLI binary & skills
    ├── main.rs        CLI entry point (clap)
    ├── session.rs     Session persistence (JSONL)
    ├── settings.rs    Layered settings (defaults → global → project → env → CLI)
    ├── skills/        13 built-in skills
    ├── templates.rs   Prompt template system
    ├── extensions.rs  Dynamic library loading
    └── packages.rs    Package management
```

## Crates

### oxi-ai — Unified LLM API

Provider-agnostic streaming interface for LLM interactions.

- **10 providers** with a single `Provider` trait
- Streaming via `ProviderEvent` (TextDelta, ThinkingDelta, ToolCall, Done, ...)
- Typed `Message`/`ContentBlock` system
- `Tool` definitions with JSON Schema validation
- `ModelRegistry` with 50+ model definitions
- Token estimation and context compaction
- Cross-provider message transformation (e.g., Anthropic → OpenAI format)

[→ Full documentation](oxi-ai/README.md)

### oxi-agent — Agent Runtime

Manages the LLM tool-calling loop, event emission, and state.

- Streaming `AgentEvent` system (thinking, text, tool calls, completion)
- `AgentTool` trait for defining LLM-callable tools
- `ToolRegistry` with 4 built-in tools (read, write, edit, bash)
- Automatic context compaction for long conversations
- Progress callbacks for long-running tool operations
- Mid-conversation model switching with format transformation

[→ Full documentation](oxi-agent/README.md)

### oxi-tui — Terminal UI Framework

Component-based terminal UI with differential rendering.

- `Component` trait for composable UI building blocks
- Double-buffered differential rendering (only redraws changed lines)
- Theme system with TOML/JSON hot-reload
- Built-in components: Text, Input, Editor, Markdown, Completion, ChatView, Image
- Image rendering (Kitty and iTerm2 protocols)
- Overlay system for modals
- Unified keyboard/mouse/resize events

[→ Full documentation](oxi-tui/README.md)

### oxi — CLI Binary

The top-level binary that ties everything together.

- Interactive REPL with streaming display
- Session system with JSONL persistence and tree branching
- Layered settings (defaults → global → project → env → CLI)
- 13 built-in skills
- Prompt templates with variable substitution
- Package management for extensions and skills
- Dynamic extension loading

## Skills

oxi ships with 13 built-in skills that provide structured workflows:

| Skill | Purpose |
|-------|---------|
| **Autonomous Loop** | Self-correcting design→implement→verify cycle |
| **Brainstorming** | Collaborative ideation with multi-approach comparison |
| **Context Builder** | Requirements gathering and validation |
| **Deep Research** | Investigation, analysis, and design documentation |
| **Design Farmer** | Design system construction (colors, tokens, a11y) |
| **Obsidian** | Obsidian vault operations (search, tags, backlinks) |
| **Oracle** | High-context decision-making with scoring |
| **Planner** | Implementation planning with dependency batching |
| **Playwright CLI** | Browser automation via Playwright |
| **Reviewer** | Multi-axis code review |
| **Scout** | Fast codebase reconnaissance and structure analysis |
| **Super Review** | Deep system-level review |
| **Worktree** | Git worktree management |

## Settings

Settings are layered (later layers override earlier):

```
1. Built-in defaults
2. Global config:   ~/.oxi/settings.toml
3. Project config:  .oxi/settings.toml
4. Environment:     OXI_* variables
5. CLI arguments:   -m, -p, --thinking
```

Example `~/.oxi/settings.toml`:

```toml
default_model = "anthropic/claude-sonnet-4-20250514"
default_provider = "anthropic"
thinking_level = "standard"
theme = "default"
temperature = 0.7
max_tokens = 4096
stream_responses = true
tool_timeout_seconds = 120
session_history_size = 100
extensions_enabled = true
auto_compaction = true
```

Environment variable overrides:

| Variable | Setting |
|----------|---------|
| `OXI_MODEL` | `default_model` |
| `OXI_PROVIDER` | `default_provider` |
| `OXI_THEME` | `theme` |
| `OXI_TOOL_TIMEOUT` | `tool_timeout_seconds` |
| `OXI_TEMPERATURE` | `default_temperature` |
| `OXI_MAX_TOKENS` | `max_response_tokens` |
| `OXI_SESSION_DIR` | `session_dir` |
| `OXI_STREAM` | `stream_responses` |
| `OXI_EXTENSIONS_ENABLED` | `extensions_enabled` |

## Project Stats

```
Language:     Rust (edition 2021)
Crates:       4 (oxi, oxi-ai, oxi-agent, oxi-tui)
Source files: 77
Lines of code: 43,806
Tests:        656 (all passing)
Warnings:     0
License:      MIT
```

## Development

```bash
# Build
cargo build

# Run all tests
cargo test

# Build release binary
cargo build --release

# Run with warnings as errors
RUSTFLAGS="-D warnings" cargo build

# Run specific crate tests
cargo test -p oxi-agent
```

## License

MIT
