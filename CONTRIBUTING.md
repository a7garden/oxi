# Contributing to oxi

Thank you for your interest in contributing to oxi! This document provides
guidelines and instructions for contributing.

## Table of Contents

- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Code Style](#code-style)
- [Testing](#testing)
- [Commit Convention](#commit-convention)
- [Pull Request Process](#pull-request-process)
- [Issue Reporting](#issue-reporting)
- [Architecture](#architecture)

## Getting Started

1. **Fork** the repository on GitHub.
2. **Clone** your fork locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/oxi.git
   cd oxi
   ```
3. **Create a branch** for your change:
   ```bash
   git checkout -b feat/my-feature
   ```
4. **Install the pre-commit hooks** (mirrors the CI gate locally):
   ```bash
   pip install pre-commit        # or: brew install pre-commit
   pre-commit install
   ```
   On every `git commit`, this runs `cargo fmt --check`, `cargo clippy
   --all-targets -- -D warnings`, YAML/TOML lint, large-file scan, and
   blocks accidental private-key commits. Run on demand with
   `pre-commit run --all-files`.

## Development Setup

### Prerequisites

- **Rust** 1.82.0+ (see `rust-toolchain.toml`)
- A C compiler (for native dependencies)
- API keys for at least one LLM provider (for integration testing)

### Build

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Build with all warnings as errors
RUSTFLAGS="-D warnings" cargo build
```

### Run

```bash
# Interactive mode
cargo run

# Single prompt
cargo run -- "Explain Rust ownership"

# With specific provider
cargo run -- -p openai -m gpt-4o "Hello"
```

## Code Style

### Formatting

```bash
# Format check
cargo fmt --all -- --check

# Auto-format
cargo fmt --all
```

### Linting

```bash
# Run clippy (must pass clean)
cargo clippy --workspace -- -D warnings
```

### Guidelines

- Follow standard Rust conventions (see [Rust API Guidelines][rust-api]).
- Use `anyhow::Result` for application code, `thiserror` for library error types.
- Prefer `parking_lot::RwLock` over `std::sync::RwLock`.
- Use `async_trait` for trait objects requiring async methods.
- Document all public API items with `///` doc comments.
- Add `// SAFETY:` comments for every `unsafe` block explaining why it is sound.
- Use atomic file writes (`write to temp → rename`) for persistent data.
- Keep functions focused — avoid functions longer than ~80 lines.

### Module Structure

- `mod.rs` re-exports the public API.
- Implementation lives in sibling files.
- One module per concern (e.g., `providers/`, `tools/`, `widgets/`).

## Testing

### Run Tests

```bash
# All tests
cargo test --workspace

# Single crate
cargo test -p oxi-ai

# Specific test
cargo test -p oxi-agent test_reject_traversal

# Doc tests
cargo test --workspace --doc
```

### Test Guidelines

- Unit tests go in `#[cfg(test)] mod tests` within each module.
- Integration tests go in `<crate>/tests/*.rs`.
- Use `MockProvider` for provider-dependent tests (see `oxi-ai` test utilities).
- Every new public function or significant change should include tests.
- Tool implementations must be idempotent (the agent loop retries on failure).

## Commit Convention

Use clear, descriptive commit messages:

```
feat(agent): add streaming progress callback for tool execution
fix(ai): handle partial UTF-8 in SSE line parsing
docs(readme): add installation instructions for Homebrew
refactor(store): consolidate session tree traversal logic
test(agent): add edge case tests for context compaction
chore(deps): bump reqwest to 0.12.28
```

**Prefixes:**

| Prefix | Usage |
|--------|-------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation |
| `refactor` | Code refactoring |
| `test` | Test additions/changes |
| `chore` | Maintenance, deps, CI |

## Pull Request Process

1. **Ensure all checks pass:**
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```
   (or just `pre-commit run --all-files` once the hooks are installed).

2. **Update documentation** if your change affects public API or behavior.

3. **Add a CHANGELOG entry** under `[Unreleased]` in `CHANGELOG.md`.

4. **Apply labels** — when opening a PR, apply at least one `area:*`
   label and one `type:*` label. The label set is defined in
   `.github/labels.yml` and is auto-synced by the `labels.yml` workflow.
   If you'd like a label that doesn't exist yet, open a small PR
   adding it to that file.

5. **Link an issue** — reference `Fixes #123` or `Closes #123` in the
   PR body. The `pr-gate.yml` workflow warns (not blocks) when no
   issue is linked, but traceability makes triage much smoother.

6. **Keep PRs focused** — one concern per PR is preferred. The
   `pr-gate.yml` workflow **blocks** PRs over 4000 lines changed and
   warns above 2000.

7. **PR title must follow Conventional Commits** — `pr-gate.yml`
   enforces one of: `feat`, `fix`, `docs`, `refactor`, `perf`,
   `test`, `chore`, `ci`, `build`, `style`, `revert`, optionally
   followed by `(scope)`, optionally a `!` for breaking changes, then
   `: <subject>`. Example: `feat(agent): add parallel tool execution`.

8. **Respond to reviews** promptly and be open to feedback.

9. **Squash commits** if requested during review.

### PR Template

When opening a PR, please include:

- **Summary**: What does this change do?
- **Motivation**: Why is this change needed?
- **Testing**: How was it tested?
- **Breaking Changes**: Does this change break any existing API?

## Issue Reporting

When filing an issue, please include:

1. **oxi version** (`oxi --version`)
2. **Rust version** (`rustc --version`)
3. **OS and architecture**
4. **Steps to reproduce**
5. **Expected vs actual behavior**
6. **Relevant logs** (with sensitive data redacted)

### Security Vulnerabilities

**Do NOT file public issues for security vulnerabilities.**

Instead, email [security@a7garden.dev](mailto:security@a7garden.dev) with:

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

See [SECURITY.md](SECURITY.md) for the full security policy.

## Architecture

```
oxi-ai      ← oxi-agent ← oxi-sdk ← oxi-cli
oxi-ai      ← oxi-store           ← oxi-cli
oxi-tui     (independent)         ← oxi-cli
```

### Adding a New Provider

1. Create `oxi-ai/src/providers/<name>.rs`.
2. Implement the `Provider` trait.
3. Register in `oxi-ai/src/providers/register_builtins.rs`.
4. Add model data to `oxi-ai/src/model_db.rs`.

### Adding a New Tool

1. Create `oxi-agent/src/tools/<name>.rs`.
2. Implement the `AgentTool` trait.
3. Register in `oxi-agent/src/tools.rs`.
4. Mark `essential()` as `true` if it cannot be disabled.

### Adding a New Extension Type

1. Define types in `oxi-cli/src/extensions/types.rs`.
2. Implement loading in `oxi-cli/src/extensions/loading.rs` or `wasm.rs`.
3. Register hooks in `oxi-cli/src/extensions/registry.rs`.

## License

By contributing to oxi, you agree that your contributions will be licensed
under the [MIT License](LICENSE.md).

Note: oxi is a Rust port of [pi](https://github.com/earendil-works/pi) (MIT License, Copyright © 2025 Mario Zechner).
All contributions are made under the same MIT License that covers both the original
pi codebase and the oxi port.

[rust-api]: https://rust-lang.github.io/api-guidelines/
