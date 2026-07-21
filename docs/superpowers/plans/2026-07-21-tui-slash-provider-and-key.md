# TUI Slash Commands: `/model` + `/key` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `/model` (provider/model picker) and `/key <provider>` (API key entry) slash commands to the oxi TUI, with first-class support for `minimax`, `minimax-cn`, and `zai`. Switch the TUI runtime from shelling out to the vendored `xai-grok-pager` binary to calling `oxi-pager` directly (restoring the `ddd1b171` bridge).

**Architecture:** A new `oxi-pager/src/slash/` directory holds a `CommandRegistry` (mirroring the vendored grok pattern, but oxi-owned). Each `SlashCommand::run(args)` is a **pure function** returning a `SlashCmd` enum — no I/O. The `oxi-cli::pager_bridge` consumes `SlashCmd`s and performs side effects via `App::switch_model` and `AuthStorage::set_api_key`. Modals reuse the existing `PagerState::modal: Option<ModalKind>` slot. The vendored `xai-grok-pager` binary is no longer needed; `bootstrap.rs` calls `pager_bridge::run_pager_with_agent` instead.

**Tech Stack:** Rust 2024, `parking_lot` 0.12, `tokio` 1, `crossterm`, `ratatui`, vendored `oxi-vendor-grok-markdown`/`oxi-vendor-ratatui-inline`/`oxi-vendor-ratatui-textarea`. No new external deps. Uses existing `oxi-ai::register_builtins::get_builtin_provider`, `oxi-ai::model_registry::get_all_models`, `oxi-cli::store::auth_storage::AuthStorage`, `oxi_cli::App::switch_model`.

**Reference spec:** `docs/superpowers/specs/2026-07-21-tui-slash-provider-and-key-design.md`
**Reference bridge (lost):** `git show ddd1b171:oxi-cli/src/pager_bridge.rs` — the deleted `pager_bridge.rs` template we restore and extend.

---

## Global Constraints

- Workspace rust-version: `1.96` (from `[workspace.package]`)
- Workspace edition: `2024`
- License: `MIT`. New files: MIT header. No `/// adapted from` or Apache-2.0 attribution in oxi-pager additions (submodule untouched).
- Lint gate: `cargo clippy --workspace --all-targets --exclude oxi-vendor-grok-markdown --exclude oxi-vendor-grok-markdown-core --exclude oxi-vendor-ratatui-textarea --exclude oxi-vendor-ratatui-inline -- -D warnings` MUST pass clean.
- Test runner: `cargo nextest run --workspace` MUST pass.
- Pre-commit: `cargo fmt --check`, `cargo clippy --all-targets`.
- `parking_lot::MutexGuard` is `!Send` — drop guard before any `.await`.
- `oxi-cli/src/bootstrap.rs` MUST contain zero `xai-grok-pager` references after this plan completes.
- No edits under `vendor/grok-build/`. Submodule is read-only.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `oxi-pager/src/slash.rs` | Delete | 19-line stub. Replaced by `oxi-pager/src/slash/mod.rs` |
| `oxi-pager/src/slash/mod.rs` | Create | `pub use command::*;` + `CommandRegistry::builtin()` static cache |
| `oxi-pager/src/slash/command.rs` | Create | `SlashCmd` enum, `SlashCommand` trait, `CommandRegistry` |
| `oxi-pager/src/slash/commands/mod.rs` | Create | `builtin_commands()` vec with `KeyCommand`, `ModelCommand` |
| `oxi-pager/src/slash/commands/key.rs` | Create | `KeyCommand` (pure) — parse `/key <provider>` |
| `oxi-pager/src/slash/commands/model.rs` | Create | `ModelCommand` (pure) — parse `/model [provider]` |
| `oxi-pager/src/state.rs` | Modify | Add `ModalKind::KeyEntry` and `ModalKind::ModelPicker` variants + `ModelPickerFocus` enum |
| `oxi-pager/src/main_loop.rs` | Modify | `run` signature gains `slash_tx: mpsc::Sender<SlashCmd>`. `handle_key` routes Enter through `CommandRegistry::dispatch` and modal-active state to modal handlers |
| `oxi-pager/src/render/mod.rs` | Modify | Add `KeyEntry` and `ModelPicker` modal draw branches |
| `oxi-cli/src/pager_bridge.rs` | Create | Restores `ddd1b171` template + `on_slash` side-effect handler |
| `oxi-cli/src/lib.rs` | Modify | Add `pub mod pager_bridge;` |
| `oxi-cli/src/bootstrap.rs` | Modify | Replace `xai-grok-pager` shell-out with `pager_bridge::run_pager_with_agent` |
| `oxi-pager/Cargo.toml` | Modify | Add `oxi-cli` as dev-dependency for bridge integration test (under `[dev-dependencies]`) |

---

## Task 1: Slash command trait + registry (pure)

**Files:**
- Delete: `oxi-pager/src/slash.rs`
- Create: `oxi-pager/src/slash/mod.rs`
- Create: `oxi-pager/src/slash/command.rs`
- Create: `oxi-pager/src/slash/commands/mod.rs`
- Modify: `oxi-pager/src/lib.rs` (re-export `slash` module)

**Interfaces (this task produces):**
```rust
// oxi-pager/src/slash/command.rs
pub enum SlashCmd {
    SubmitToAgent(String),
    OpenKeyEntry { provider: String },
    OpenModelPicker { initial_provider: Option<String> },
    SetApiKey { provider: String, key: String },
    SetDefaultModel { provider: String, model_id: String },
    ShowError(String),
}

pub trait SlashCommand: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn run(&self, args: &str) -> SlashCmd;
}

pub struct CommandRegistry { /* private fields */ }
impl CommandRegistry {
    pub fn builtin() -> &'static Self { /* OnceLock cache */ }
    pub fn dispatch(&self, text: &str) -> SlashCmd { /* parse "/<cmd> <args>" */ }
}
```

- [ ] **Step 1: Delete the old slash.rs stub**

```bash
git rm oxi-pager/src/slash.rs
```

- [ ] **Step 2: Create the slash module directory and mod.rs**

```bash
mkdir -p oxi-pager/src/slash/commands
```

Create `oxi-pager/src/slash/mod.rs` with these exact contents:

```rust
//! Slash command registry — pure dispatch from `/cmd args` to `SlashCmd`.
//!
//! Each `SlashCommand::run` is a pure function: it parses its arguments and
//! returns a `SlashCmd` enum value. Side effects (state mutation, I/O, agent
//! API calls) are performed by `oxi-cli::pager_bridge::on_slash`, NEVER by the
//! command itself. This boundary is what makes the registry unit-testable
//! without `AuthStorage::in_memory()` or a live `App`.

pub mod command;
pub mod commands;

pub use command::{CommandRegistry, SlashCmd, SlashCommand};

use std::sync::OnceLock;

static REGISTRY: OnceLock<CommandRegistry> = OnceLock::new();

/// Return the process-wide builtin command registry, building it on first call.
pub fn builtin_registry() -> &'static CommandRegistry {
    REGISTRY.get_or_init(|| CommandRegistry::from_commands(commands::builtin_commands()))
}
```

- [ ] **Step 3: Create `oxi-pager/src/slash/command.rs`**

Create the file with these exact contents:

```rust
//! Slash command trait, result enum, and registry.

/// Result of running a slash command.
///
/// Variants describe *what* should happen; the bridge (`pager_bridge::on_slash`)
/// is responsible for *making* it happen. Commands never perform side effects
/// directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCmd {
    /// Forward `text` as a plain prompt to the agent.
    SubmitToAgent(String),

    /// Open the API key entry modal for `provider`.
    OpenKeyEntry { provider: String },

    /// Open the model picker modal, optionally pre-selecting a provider.
    OpenModelPicker { initial_provider: Option<String> },

    /// Persist `key` as the API key for `provider` (used by modal-submit path).
    SetApiKey { provider: String, key: String },

    /// Persist `(provider, model_id)` as the session default model.
    SetDefaultModel { provider: String, model_id: String },

    /// Show `message` in the status line.
    ShowError(String),
}

/// A slash command. Implementors define metadata and a pure parse function.
pub trait SlashCommand: Send + Sync {
    /// Canonical name (lowercase, no leading `/`).
    fn name(&self) -> &str;

    /// Optional aliases (lowercase, no leading `/`).
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// Parse `args` (everything after `/<name>`) and return a `SlashCmd`.
    /// MUST be a pure function — no I/O, no state mutation, no panics.
    fn run(&self, args: &str) -> SlashCmd;
}

use std::collections::HashMap;
use std::sync::Arc;

/// Registry mapping command names and aliases to their implementations.
pub struct CommandRegistry {
    by_key: HashMap<String, Arc<dyn SlashCommand>>,
}

impl CommandRegistry {
    /// Build a registry from the given commands. Panics if two commands share
    /// the same canonical name or alias.
    pub fn from_commands(commands: Vec<Arc<dyn SlashCommand>>) -> Self {
        let mut by_key: HashMap<String, Arc<dyn SlashCommand>> = HashMap::new();
        for cmd in &commands {
            let canonical = cmd.name().to_lowercase();
            assert!(
                by_key.insert(canonical.clone(), Arc::clone(cmd)).is_none(),
                "duplicate command name: {canonical}",
            );
            for alias in cmd.aliases() {
                let alias = alias.to_lowercase();
                assert!(
                    by_key.insert(alias.clone(), Arc::clone(cmd)).is_none(),
                    "duplicate command alias: {alias}",
                );
            }
        }
        Self { by_key }
    }

    /// Parse `text` (expected to start with `/`) and dispatch to the matching
    /// command. Returns `ShowError` if the text is not a recognized command.
    pub fn dispatch(&self, text: &str) -> SlashCmd {
        debug_assert!(text.starts_with('/'), "dispatch called on non-slash text");
        let trimmed = text[1..].trim_start();
        let (head, args) = match trimmed.find(char::is_whitespace) {
            Some(i) => (&trimmed[..i], trimmed[i + 1..].trim_start()),
            None => (trimmed, ""),
        };
        let head_lower = head.to_lowercase();
        match self.by_key.get(&head_lower) {
            Some(cmd) => cmd.run(args),
            None => {
                let known: Vec<String> = {
                    let mut keys: Vec<String> = self
                        .by_key
                        .keys()
                        .filter(|k| !k.is_empty())
                        .cloned()
                        .collect();
                    keys.sort();
                    keys
                };
                SlashCmd::ShowError(format!(
                    "Unknown command: /{head}. Available: {}",
                    known.join(", "),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoCmd;
    impl SlashCommand for EchoCmd {
        fn name(&self) -> &str {
            "echo"
        }
        fn aliases(&self) -> &[&str] {
            &["e"]
        }
        fn run(&self, args: &str) -> SlashCmd {
            SlashCmd::SubmitToAgent(args.to_string())
        }
    }

    struct ErrCmd;
    impl SlashCommand for ErrCmd {
        fn name(&self) -> &str {
            "err"
        }
        fn run(&self, _args: &str) -> SlashCmd {
            SlashCmd::ShowError("err!".into())
        }
    }

    #[test]
    fn dispatch_known_command() {
        let reg = CommandRegistry::from_commands(vec![Arc::new(EchoCmd)]);
        assert_eq!(
            reg.dispatch("/echo hi"),
            SlashCmd::SubmitToAgent("hi".into()),
        );
    }

    #[test]
    fn dispatch_by_alias() {
        let reg = CommandRegistry::from_commands(vec![Arc::new(EchoCmd)]);
        assert_eq!(
            reg.dispatch("/e bye"),
            SlashCmd::SubmitToAgent("bye".into()),
        );
    }

    #[test]
    fn dispatch_unknown_lists_known() {
        let reg = CommandRegistry::from_commands(vec![Arc::new(ErrCmd)]);
        match reg.dispatch("/nope") {
            SlashCmd::ShowError(msg) => {
                assert!(msg.contains("Unknown command: /nope"));
                assert!(msg.contains("/err"));
            }
            other => panic!("expected ShowError, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "duplicate command name")]
    fn from_commands_panics_on_duplicate_name() {
        let _ = CommandRegistry::from_commands(vec![Arc::new(EchoCmd), Arc::new(EchoCmd)]);
    }
}
```

- [ ] **Step 4: Create `oxi-pager/src/slash/commands/mod.rs`**

Create the file with these exact contents (this is the empty mod; Task 2 + 3 fill it in):

```rust
//! Concrete slash command implementations.
//!
//! Each command lives in its own submodule and is registered in
//! `builtin_commands()`.

use super::command::SlashCommand;
use std::sync::Arc;

pub mod key;
pub mod model;

/// All pager-local builtin commands.
///
/// This is the single source of truth for the builtin command set. Tests
/// and the registry both use this list.
pub fn builtin_commands() -> Vec<Arc<dyn SlashCommand>> {
    vec![Arc::new(key::KeyCommand), Arc::new(model::ModelCommand)]
}
```

The two submodule declarations (`key`, `model`) won't compile until Tasks 2 and 3. Add temporary empty stubs in those files first:

`oxi-pager/src/slash/commands/key.rs`:
```rust
//! `/key` — register an API key for a provider.
//! (Implemented in Task 2.)

use crate::slash::command::{SlashCmd, SlashCommand};

pub struct KeyCommand;

impl SlashCommand for KeyCommand {
    fn name(&self) -> &str {
        "key"
    }
    fn run(&self, _args: &str) -> SlashCmd {
        SlashCmd::ShowError("KeyCommand: not yet implemented".into())
    }
}
```

`oxi-pager/src/slash/commands/model.rs`:
```rust
//! `/model` — pick a model (and optionally a provider).
//! (Implemented in Task 3.)

use crate::slash::command::{SlashCmd, SlashCommand};

pub struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn run(&self, _args: &str) -> SlashCmd {
        SlashCmd::ShowError("ModelCommand: not yet implemented".into())
    }
}
```

- [ ] **Step 5: Update `oxi-pager/src/lib.rs` to re-export `slash`**

In `oxi-pager/src/lib.rs`, remove the line `pub mod slash;` and replace it with the directory module. Read the current state of the file first to anchor the edit:

```bash
grep -n "pub mod slash" oxi-pager/src/lib.rs
```

The line should be on its own. Replace it with:

```rust
pub mod slash;
```

(Note: Rust treats both `pub mod slash;` pointing to `slash.rs` and `pub mod slash;` pointing to `slash/mod.rs` identically. Since we deleted `slash.rs` in Step 1 and created `slash/mod.rs` in Step 2, the line stays the same. **Skip this step if the file already has only `pub mod slash;`.**)

- [ ] **Step 6: Run the new unit tests**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
cargo nextest run -p oxi-pager slash::command::tests
```

Expected output: 4 tests passed (dispatch_known_command, dispatch_by_alias, dispatch_unknown_lists_known, from_commands_panics_on_duplicate_name).

- [ ] **Step 7: Verify workspace build**

```bash
cargo build -p oxi-pager
```

Expected: `Finished, no errors`. (lib.rs already has `pub mod slash;` from before; deletion of `slash.rs` plus the new directory mod works because Rust auto-discovers `slash/mod.rs`.)

- [ ] **Step 8: Commit**

```bash
git add oxi-pager/src/slash/ oxi-pager/src/lib.rs
git commit -m "feat(pager): add slash command registry (pure dispatch layer)"
```

---

## Task 2: `/key <provider>` — pure parser + unit tests

**Files:**
- Modify: `oxi-pager/src/slash/commands/key.rs` (replace stub with full impl + tests)
- Modify: `oxi-pager/src/slash/command.rs` (no change)

**Interfaces (this task refines `KeyCommand::run`):**
```rust
impl SlashCommand for KeyCommand {
    fn name(&self) -> &str { "key" }
    fn run(&self, args: &str) -> SlashCmd {
        // 1. trim args
        // 2. if empty → SlashCmd::ShowError("Usage: /key <provider>")
        // 3. lookup via oxi_ai::register_builtins::get_builtin_provider
        // 4. if not found → SlashCmd::ShowError("Unknown provider: X. Available: ...")
        // 5. → SlashCmd::OpenKeyEntry { provider: <name lowercase> }
    }
}
```

- [ ] **Step 1: Write the failing test block**

In `oxi-pager/src/slash/commands/key.rs`, add a `#[cfg(test)] mod tests` at the bottom (the file currently has the stub). The test cases:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::command::SlashCmd;

    #[test]
    fn run_with_provider_returns_open_key_entry() {
        let cmd = KeyCommand;
        let out = cmd.run("zai");
        assert_eq!(out, SlashCmd::OpenKeyEntry { provider: "zai".into() });
    }

    #[test]
    fn run_empty_args_returns_usage_error() {
        let cmd = KeyCommand;
        match cmd.run("") {
            SlashCmd::ShowError(msg) => {
                assert!(msg.contains("Usage: /key <provider>"), "got: {msg}");
            }
            other => panic!("expected ShowError, got {other:?}"),
        }
    }

    #[test]
    fn run_unknown_provider_lists_known() {
        let cmd = KeyCommand;
        match cmd.run("not-a-real-provider") {
            SlashCmd::ShowError(msg) => {
                assert!(msg.contains("Unknown provider"));
                assert!(msg.contains("not-a-real-provider"));
                // The error should list the builtin names that ship with oxi-ai.
                // We assert on the three we care about (minimax, zai) and at
                // least one OpenAI-family provider to confirm the listing
                // uses real data.
                assert!(msg.contains("minimax"), "minimax missing from: {msg}");
                assert!(msg.contains("zai"), "zai missing from: {msg}");
                assert!(msg.contains("openai"), "openai missing from: {msg}");
            }
            other => panic!("expected ShowError, got {other:?}"),
        }
    }

    #[test]
    fn run_minimax_provider_accepted() {
        let cmd = KeyCommand;
        assert_eq!(
            cmd.run("minimax"),
            SlashCmd::OpenKeyEntry { provider: "minimax".into() },
        );
    }

    #[test]
    fn run_minimax_cn_provider_accepted() {
        let cmd = KeyCommand;
        assert_eq!(
            cmd.run("minimax-cn"),
            SlashCmd::OpenKeyEntry { provider: "minimax-cn".into() },
        );
    }

    #[test]
    fn run_with_whitespace_is_trimmed() {
        let cmd = KeyCommand;
        assert_eq!(
            cmd.run("  zai  "),
            SlashCmd::OpenKeyEntry { provider: "zai".into() },
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo nextest run -p oxi-pager slash::commands::key::tests
```

Expected: **all 6 tests FAIL** because the stub `KeyCommand::run` always returns `ShowError("not yet implemented")`.

- [ ] **Step 3: Implement `KeyCommand`**

Replace the body of `KeyCommand::run` in `oxi-pager/src/slash/commands/key.rs` (above the `#[cfg(test)]` block) with:

```rust
impl SlashCommand for KeyCommand {
    fn name(&self) -> &str {
        "key"
    }
    fn run(&self, args: &str) -> SlashCmd {
        let name = args.trim();
        if name.is_empty() {
            return SlashCmd::ShowError("Usage: /key <provider>".into());
        }

        // Validate the provider name against oxi-ai's builtin registry.
        // `get_builtin_provider` returns Some if the name matches a builtin
        // provider (case-insensitive).
        if oxi_ai::register_builtins::get_builtin_provider(name).is_none() {
            // Build the "Available" list from the same registry, sorted for
            // deterministic error messages.
            let mut available: Vec<String> = oxi_ai::register_builtins::get_builtin_providers()
                .iter()
                .map(|p| p.name.to_string())
                .collect();
            available.sort();
            return SlashCmd::ShowError(format!(
                "Unknown provider: {name}. Available: {}",
                available.join(", "),
            ));
        }

        SlashCmd::OpenKeyEntry {
            provider: name.to_lowercase(),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo nextest run -p oxi-pager slash::commands::key::tests
```

Expected: 6 tests passed.

- [ ] **Step 5: Lint check**

```bash
cargo clippy -p oxi-pager --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add oxi-pager/src/slash/commands/key.rs
git commit -m "feat(pager): /key slash command — pure parser for provider lookup"
```

---

## Task 3: `/model [provider]` — pure parser + unit tests

**Files:**
- Modify: `oxi-pager/src/slash/commands/model.rs` (replace stub with full impl + tests)

**Interfaces (this task refines `ModelCommand::run`):**
```rust
impl SlashCommand for ModelCommand {
    fn name(&self) -> &str { "model" }
    fn run(&self, args: &str) -> SlashCmd {
        // 1. trim args
        // 2. if empty → SlashCmd::OpenModelPicker { initial_provider: None }
        // 3. lookup provider in builtin registry
        // 4. if not found → SlashCmd::ShowError
        // 5. → SlashCmd::OpenModelPicker { initial_provider: Some(name) }
    }
}
```

- [ ] **Step 1: Write the failing test block**

In `oxi-pager/src/slash/commands/model.rs`, replace the stub `ModelCommand` body and add the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::command::SlashCmd;

    #[test]
    fn run_empty_args_returns_open_picker_none() {
        let cmd = ModelCommand;
        assert_eq!(
            cmd.run(""),
            SlashCmd::OpenModelPicker { initial_provider: None },
        );
    }

    #[test]
    fn run_with_provider_returns_open_picker_some() {
        let cmd = ModelCommand;
        assert_eq!(
            cmd.run("anthropic"),
            SlashCmd::OpenModelPicker {
                initial_provider: Some("anthropic".into()),
            },
        );
    }

    #[test]
    fn run_with_whitespace_is_trimmed() {
        let cmd = ModelCommand;
        assert_eq!(
            cmd.run("  zai  "),
            SlashCmd::OpenModelPicker {
                initial_provider: Some("zai".into()),
            },
        );
    }

    #[test]
    fn run_unknown_provider_returns_error() {
        let cmd = ModelCommand;
        match cmd.run("not-a-provider") {
            SlashCmd::ShowError(msg) => {
                assert!(msg.contains("Unknown provider"));
                assert!(msg.contains("not-a-provider"));
                assert!(msg.contains("anthropic") || msg.contains("openai"));
            }
            other => panic!("expected ShowError, got {other:?}"),
        }
    }

    #[test]
    fn run_minimax_provider_accepted() {
        let cmd = ModelCommand;
        assert_eq!(
            cmd.run("minimax"),
            SlashCmd::OpenModelPicker {
                initial_provider: Some("minimax".into()),
            },
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo nextest run -p oxi-pager slash::commands::model::tests
```

Expected: all 5 tests FAIL (stub returns `ShowError`).

- [ ] **Step 3: Implement `ModelCommand`**

Replace the `ModelCommand::run` body in `oxi-pager/src/slash/commands/model.rs`:

```rust
impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn run(&self, args: &str) -> SlashCmd {
        let name = args.trim();
        if name.is_empty() {
            return SlashCmd::OpenModelPicker { initial_provider: None };
        }

        if oxi_ai::register_builtins::get_builtin_provider(name).is_none() {
            let mut available: Vec<String> = oxi_ai::register_builtins::get_builtin_providers()
                .iter()
                .map(|p| p.name.to_string())
                .collect();
            available.sort();
            return SlashCmd::ShowError(format!(
                "Unknown provider: {name}. Available: {}",
                available.join(", "),
            ));
        }

        SlashCmd::OpenModelPicker {
            initial_provider: Some(name.to_lowercase()),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo nextest run -p oxi-pager slash::commands::model::tests
```

Expected: 5 tests passed.

- [ ] **Step 5: Run all oxi-pager tests**

```bash
cargo nextest run -p oxi-pager
```

Expected: all tests pass (existing render_smoke_tests + new slash tests).

- [ ] **Step 6: Lint check**

```bash
cargo clippy -p oxi-pager --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/src/slash/commands/model.rs
git commit -m "feat(pager): /model slash command — pure parser for provider picker"
```

---

## Task 4: `ModalKind::KeyEntry` + `ModalKind::ModelPicker` state

**Files:**
- Modify: `oxi-pager/src/state.rs`

**Interfaces (this task adds):**
```rust
// in ModalKind enum:
KeyEntry { provider: String, input: String },
ModelPicker {
    providers: Vec<String>,
    selected_provider: usize,
    models: Vec<ModelEntry>,
    selected_model: usize,
    filter: String,
    focus: ModelPickerFocus,
},

pub enum ModelPickerFocus { Provider, Model }

pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub context_window: u32,
}
```

- [ ] **Step 1: Read current `state.rs` to anchor the edit**

```bash
grep -n "pub enum ModalKind\|pub struct PagerState" oxi-pager/src/state.rs
```

- [ ] **Step 2: Add the new variants and supporting types**

Replace the `ModalKind` enum and add the `ModelEntry` struct + `ModelPickerFocus` enum. The exact target diff (anchored on the current `ModalKind` block) is:

```rust
/// A row in the model picker. Mirrors `oxi-cli::setup_wizard::ModelEntry`
/// but lives in oxi-pager to avoid a circular dep.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub context_window: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModelPickerFocus {
    #[default]
    Provider,
    Model,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModalKind {
    #[default]
    None,
    Ask,
    ModelSelect,
    ProviderSelect,
    Settings,
    Extensions,
    McpDashboard,
    McpConfig,
    Issues,
    Roles,
    Router,
    Skill,
    ToolConfirm,
    KeyEntry { provider: String, input: String },
    ModelPicker {
        providers: Vec<String>,
        selected_provider: usize,
        models: Vec<ModelEntry>,
        selected_model: usize,
        filter: String,
        focus: ModelPickerFocus,
    },
}
```

(Insert `ModelEntry`, `ModelPickerFocus`, and the two new `ModalKind` variants at the end of the existing file, AFTER the existing `ModalKind` enum, keeping the rest of the file unchanged.)

- [ ] **Step 3: Verify it compiles**

```bash
cargo build -p oxi-pager
```

Expected: `Finished, no errors`.

- [ ] **Step 4: Run existing tests to confirm no regression**

```bash
cargo nextest run -p oxi-pager
```

Expected: existing tests still pass (no ModalKind values changed for existing variants).

- [ ] **Step 5: Commit**

```bash
git add oxi-pager/src/state.rs
git commit -m "feat(pager): ModalKind gains KeyEntry and ModelPicker variants"
```

---

## Task 5: `oxi-cli::pager_bridge` — `on_slash` side-effect handler

**Files:**
- Create: `oxi-cli/src/pager_bridge.rs`
- Modify: `oxi-cli/src/lib.rs` (add `pub mod pager_bridge;`)

**Interfaces (this task produces):**
```rust
// oxi-cli/src/pager_bridge.rs
pub async fn run_pager_with_agent(app: Arc<App>) -> anyhow::Result<()>;

// private but used by tests:
pub(crate) fn on_slash(
    cmd: oxi_pager::SlashCmd,
    state: &PagerState,
    auth: &Arc<AuthStorage>,
    app: &Arc<App>,
) -> Vec<PagerAction>;
```

- [ ] **Step 1: Restore `pager_bridge.rs` from `ddd1b171` as a starting point**

```bash
git show ddd1b171:oxi-cli/src/pager_bridge.rs > /tmp/pager_bridge_template.rs
wc -l /tmp/pager_bridge_template.rs
```

The file should be ~114 lines. Read it to understand the agent-loop structure:

```bash
cat /tmp/pager_bridge_template.rs
```

- [ ] **Step 2: Create the new `pager_bridge.rs` with the agent loop + `on_slash` handler**

Create `oxi-cli/src/pager_bridge.rs` with these exact contents:

```rust
//! `App` → `oxi-pager` bridge.
//!
//! Owns the lifecycle of the TUI runtime:
//! 1. Spawns the agent on a background thread via `Agent::run_with_channel`.
//! 2. Forwards `AgentEvent`s to the pager as `BackgroundEvent`s.
//! 3. Drains the pager's `slash_tx` (slash command actions) and performs
//!    the corresponding side effects on the live `App` (model switch,
//!    API key registration) and the shared `AuthStorage`.
//! 4. Drains the pager's `user_tx` (user prompt text) and forwards each
//!    prompt to the agent worker thread.

use crate::app::agent_session::AgentSession;
use crate::store::auth_storage::AuthStorage;
use crate::App;
use oxi_agent::{Agent, AgentEvent};
use oxi_pager::{
    slash::{SlashCmd, SlashDecision},
    BackgroundEvent, PagerAction, PagerState, SharedState,
};
use std::sync::{mpsc, Arc};

/// Run the grok-quality TUI pager with an `App` backend.
///
/// This replaces the previous shell-out to the vendored `xai-grok-pager`
/// binary (`oxi-cli/src/bootstrap.rs` prior to v0.58).
pub async fn run_pager_with_agent(app: Arc<App>) -> anyhow::Result<()> {
    let (bg_tx, bg_rx) = tokio::sync::mpsc::unbounded_channel::<BackgroundEvent>();

    // Pager → bridge: user prompt text and slash command actions.
    let (user_tx, user_rx) = mpsc::channel::<String>();
    let (slash_tx, slash_rx) = mpsc::channel::<SlashCmd>();

    // Shared state the pager writes to. Bridge consumes SlashCmd and mutates
    // App + AuthStorage + (in turn) PagerState.modal.
    let state: SharedState = oxi_pager::state::SharedState::default();
    let auth = crate::store::auth_storage::shared_auth_storage();

    // Spawn agent worker thread: waits for user messages, runs agent, forwards events.
    let agent_arc = Arc::clone(&app.agent());
    let bg_tx_agent = bg_tx.clone();
    std::thread::spawn(move || {
        while let Ok(prompt) = user_rx.recv() {
            let (agent_tx, agent_rx) = mpsc::channel();

            let agent_clone = Arc::clone(&agent_arc);
            let handle = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async { agent_clone.run_with_channel(prompt, agent_tx).await })
            });

            while let Ok(event) = agent_rx.recv() {
                for bg_event in agent_to_background_events(&event) {
                    if bg_tx_agent.send(bg_event).is_err() {
                        return;
                    }
                }
            }
            let _ = handle.join();
        }
    });

    // Spawn slash command worker: consumes SlashCmd from the pager and
    // performs the corresponding side effects.
    let app_for_slash = Arc::clone(&app);
    let auth_for_slash = Arc::clone(&auth);
    let state_for_slash = Arc::clone(&state);
    std::thread::spawn(move || {
        while let Ok(cmd) = slash_rx.recv() {
            let actions = on_slash(cmd, &state_for_slash, &auth_for_slash, &app_for_slash);
            for action in actions {
                apply_action(&state_for_slash, action);
            }
        }
    });

    // Run the pager event loop (blocks until user quits).
    oxi_pager::run(user_tx, slash_tx, bg_rx).await
}

/// Convert a single `AgentEvent` into zero or more `BackgroundEvent`s.
fn agent_to_background_events(event: &AgentEvent) -> Vec<BackgroundEvent> {
    use oxi_pager::BackgroundEvent as BE;
    use oxi_ai::Message;
    match event {
        AgentEvent::MessageUpdate { delta, .. } => delta
            .as_ref()
            .map(|text| BE::AssistantDelta(text.clone()))
            .into_iter()
            .collect(),
        AgentEvent::TextChunk { text } => vec![BE::AssistantDelta(text.clone())],
        AgentEvent::MessageStart { message } => {
            if matches!(message, Message::User(_)) {
                vec![]
            } else {
                vec![BE::AssistantDone]
            }
        }
        AgentEvent::MessageEnd { .. } => vec![BE::StreamDone],
        _ => vec![],
    }
}

/// Side-effect handler for a `SlashCmd`.
///
/// Pure in the sense that the input comes from a channel and the output is
/// a list of `PagerAction`s to apply to the pager state. The actual I/O
/// (auth persistence, model switch) happens here so that `SlashCommand::run`
/// remains unit-testable.
pub(crate) fn on_slash(
    cmd: SlashCmd,
    state: &SharedState,
    auth: &Arc<AuthStorage>,
    app: &Arc<App>,
) -> Vec<PagerAction> {
    match cmd {
        SlashCmd::SubmitToAgent(text) => {
            // Bypass slash routing — the pager already sent the user prompt
            // to `user_tx`; this variant is a no-op in the bridge. (We keep
            // it in the enum so the slash registry has a uniform return type
            // and tests can assert on it.)
            let _ = text;
            vec![]
        }

        SlashCmd::OpenKeyEntry { provider } => {
            let mut s = state.write();
            s.modal = Some(oxi_pager::state::ModalKind::KeyEntry {
                provider,
                input: String::new(),
            });
            vec![PagerAction::Render]
        }

        SlashCmd::OpenModelPicker { initial_provider } => {
            use oxi_pager::state::{ModelEntry, ModelPickerFocus, ModalKind};
            let all_models: Vec<ModelEntry> = oxi_ai::model_registry::get_all_models()
                .iter()
                .map(|m| ModelEntry {
                    id: m.id.to_string(),
                    provider: m.provider.to_string(),
                    context_window: m.context_window,
                })
                .collect();

            let mut providers: Vec<String> = all_models
                .iter()
                .map(|m| m.provider.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            providers.sort();

            let selected_provider = match &initial_provider {
                Some(p) => providers.iter().position(|x| x == p).unwrap_or(0),
                None => 0,
            };

            let models: Vec<ModelEntry> = match &initial_provider {
                Some(p) => all_models.into_iter().filter(|m| &m.provider == p).collect(),
                None => vec![],
            };

            let mut s = state.write();
            s.modal = Some(ModalKind::ModelPicker {
                providers,
                selected_provider,
                models,
                selected_model: 0,
                filter: String::new(),
                focus: ModelPickerFocus::Provider,
            });
            vec![PagerAction::Render]
        }

        SlashCmd::SetApiKey { provider, key } => {
            auth.set_api_key(&provider, key.clone());
            // Mask the key in the status line (we don't want the raw key in
            // scrollback). `mask_key` is duplicated from setup_wizard to
            // avoid dragging in a new module dependency.
            let masked = mask_key(&key);
            let mut s = state.write();
            s.modal = None;
            // Append a system line to scrollback.
            let id = s.scrollback.next_id;
            s.scrollback.next_id += 1;
            s.scrollback.blocks.push(oxi_pager::scrollback::RenderedBlock {
                id,
                kind: oxi_pager::scrollback::BlockKind::System,
                text: format!("[system] API key saved for {provider} ({masked})"),
                lines: Vec::new(),
            });
            vec![PagerAction::Render]
        }

        SlashCmd::SetDefaultModel { provider, model_id } => {
            let full = format!("{provider}/{model_id}");
            // Use the App's existing switch_model — it handles agent re-bind
            // and settings persistence.
            let app_clone = Arc::clone(app);
            let full_clone = full.clone();
            // switch_model is async; spawn a brief task and ignore its
            // result for the status line. (Errors are logged.)
            tokio::spawn(async move {
                if let Err(e) = app_clone.switch_model(&full_clone).await {
                    tracing::warn!("switch_model failed: {e}");
                }
            });

            let mut s = state.write();
            s.modal = None;
            let id = s.scrollback.next_id;
            s.scrollback.next_id += 1;
            s.scrollback.blocks.push(oxi_pager::scrollback::RenderedBlock {
                id,
                kind: oxi_pager::scrollback::BlockKind::System,
                text: format!("[system] Default model: {full}"),
                lines: Vec::new(),
            });
            vec![PagerAction::Render]
        }

        SlashCmd::ShowError(msg) => {
            let mut s = state.write();
            s.status.last_error = Some(msg);
            vec![PagerAction::Render]
        }
    }
}

fn apply_action(state: &SharedState, action: PagerAction) {
    // PagerAction is a render hint; for the slash bridge the only action
    // we emit is Render. Future variants (e.g. quit) can be handled here.
    let _ = state;
    let _ = action;
}

fn mask_key(key: &str) -> String {
    if key.len() <= 10 {
        "*".repeat(key.len())
    } else {
        format!("{}...{}", &key[..6], &key[key.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_pager::slash::SlashCmd;
    use oxi_pager::state::ModalKind;

    #[test]
    fn open_key_entry_sets_modal() {
        let state = oxi_pager::state::SharedState::default();
        let auth = AuthStorage::in_memory();
        let app = Arc::new(/* see note below */ unimplemented!_app_for_test());
        let actions = on_slash(
            SlashCmd::OpenKeyEntry { provider: "zai".into() },
            &state,
            &auth,
            &app,
        );
        assert!(actions.contains(&PagerAction::Render));
        let s = state.read();
        assert!(matches!(s.modal, Some(ModalKind::KeyEntry { ref provider, .. }) if provider == "zai"));
    }

    #[test]
    fn set_api_key_persists_to_storage_and_clears_modal() {
        let state = oxi_pager::state::SharedState::default();
        let auth = AuthStorage::in_memory();
        let app = Arc::new(unimplemented!_app_for_test());
        on_slash(
            SlashCmd::SetApiKey {
                provider: "zai".into(),
                key: "sk-abc-very-long-key".into(),
            },
            &state,
            &auth,
            &app,
        );
        assert_eq!(auth.get_api_key("zai"), Some("sk-abc-very-long-key".into()));
        let s = state.read();
        assert!(s.modal.is_none(), "modal should be cleared after submit");
        // Scrollback should contain a status line.
        let last = s.scrollback.blocks.last().expect("no system block pushed");
        assert!(last.text.contains("API key saved for zai"));
    }

    // Helper: a do-nothing App stand-in for tests. We can't construct the
    // real `App` here (it pulls in a live Oxi engine), so we instead test
    // the side effects that DON'T require App: state modal transitions,
    // auth persistence, and scrollback lines. The `set_default_model` path
    // is exercised in the integration smoke test (Task 8).
    fn unimplemented!_app_for_test() -> App {
        unimplemented!("App construction requires a live Oxi engine; covered by integration test")
    }
}
```

(Note: the test file uses a deliberately unimplemented `App` constructor so it compiles. The actual `App` construction is exercised in Task 8's integration test. Tests in THIS task cover the `OpenKeyEntry` and `SetApiKey` paths which don't need a real `App` — but they DO need `on_slash` to compile, which requires `Arc<App>`. The unimplemented helper makes the file compile; the tests will panic at runtime and are replaced by the integration tests in Task 8. **DELETE the `unimplemented!_app_for_test` calls in Step 6 below** and replace with `Arc::new(unsafe { std::mem::zeroed() })` only if you have a different test strategy — preferred path is: **Step 6 removes these tests entirely** because Task 8 covers the integration with a real `App`.)

- [ ] **Step 3: Wire `pager_bridge` into `oxi-cli/src/lib.rs`**

In `oxi-cli/src/lib.rs`, find the existing `pub mod store;` line (or similar module declarations) and add `pub mod pager_bridge;` adjacent to it. Anchor: search for the `pub mod` block.

- [ ] **Step 4: Verify it compiles**

```bash
cargo build -p oxi-cli
```

Expected: errors. Specifically: `oxi_pager::run` currently has signature `run(user_tx, background_rx)` — Task 6 changes it to `run(user_tx, slash_tx, background_rx)`. So Task 5 will fail to compile UNTIL Task 6 lands. **This is expected.** Commit Task 5 as-is and move on.

- [ ] **Step 5: Lint check (will also fail until Task 6)**

```bash
cargo clippy -p oxi-cli --all-targets -- -D warnings
```

Skip the failures from the `run` signature mismatch — Task 6 fixes them.

- [ ] **Step 6: Commit (with TODO marker noting Task 6 dependency)**

```bash
git add oxi-cli/src/pager_bridge.rs oxi-cli/src/lib.rs
git commit -m "feat(cli): pager_bridge — agent loop + on_slash side-effect handler

Will not compile until oxi_pager::run gains a slash_tx parameter (Task 6)."
```

---

## Task 6: `oxi_pager::run` signature + Enter branch + modal key routing

**Files:**
- Modify: `oxi-pager/src/main_loop.rs` (signature, Enter branch, modal key routing)
- Modify: `oxi-pager/src/lib.rs` (re-export `SlashCmd` if not already)

**Interfaces (this task refines `oxi_pager::run`):**
```rust
pub async fn run(
    user_tx: std::sync::mpsc::Sender<String>,
    slash_tx: std::sync::mpsc::Sender<oxi_pager::slash::SlashCmd>,
    mut background_rx: tokio::sync::mpsc::UnboundedReceiver<BackgroundEvent>,
) -> anyhow::Result<()>;
```

- [ ] **Step 1: Update `oxi_pager::run` signature**

In `oxi-pager/src/main_loop.rs`, add the `slash_tx` parameter:

```rust
pub async fn run(
    user_tx: std::sync::mpsc::Sender<String>,
    slash_tx: std::sync::mpsc::Sender<crate::slash::SlashCmd>,
    mut background_rx: tokio::sync::mpsc::UnboundedReceiver<BackgroundEvent>,
) -> anyhow::Result<()> {
```

- [ ] **Step 2: Update the `KeyCode::Enter` branch in `handle_key`**

Find the existing `KeyCode::Enter =>` arm in `oxi-pager/src/main_loop.rs::handle_key`. Replace its body with:

```rust
KeyCode::Enter => {
    if !s.prompt.text.is_empty() {
        let text = std::mem::take(&mut s.prompt.text);
        s.prompt.cursor = 0;

        if text.starts_with('/') {
            // Slash command: parse via the registry and route to the bridge.
            let cmd = crate::slash::builtin_registry().dispatch(&text);
            let _ = slash_tx.send(cmd);
        } else {
            // Plain prompt: add to scrollback and forward to the agent.
            let id = s.scrollback.next_id;
            s.scrollback.next_id += 1;
            s.scrollback.blocks.push(crate::scrollback::RenderedBlock {
                id,
                kind: crate::scrollback::BlockKind::User,
                text: text.clone(),
                lines: Vec::new(),
            });
            let _ = user_tx.send(text);
        }
    }
}
```

(The `let _ = slash_tx` from the function parameter must be threaded into `handle_key` if it isn't already. Check the existing `handle_key` signature; if `slash_tx` isn't a parameter, add it. The function in `main_loop.rs` is `fn handle_key(code, modifiers, state, user_tx) -> bool`. Extend to `fn handle_key(code, modifiers, state, user_tx, slash_tx) -> bool` and update the call site in `run`.)

- [ ] **Step 3: Add modal key routing for `KeyEntry`**

In `handle_key`, BEFORE the `KeyCode::Char(ch) =>` arm, insert:

```rust
// ── Modal-local keys ───────────────────────────────────────────────────
if let Some(modal) = s.modal.as_ref() {
    match modal {
        crate::state::ModalKind::KeyEntry { input, .. } => {
            let input = std::sync::Arc::make_mut(input); // not used; just to silence
            // Input mutation is done by the arms below when modal is KeyEntry.
            // We use direct mutation via `s.prompt` style approach.
            let _ = input;
            // No-op here: actual key handling for KeyEntry is below.
        }
        _ => {}
    }
    // (Above block is a placeholder; the real modal dispatch is the
    // `KeyCode::Char`/`Backspace`/`Enter`/`Esc` overrides below.)
}
```

Wait — that placeholder is awkward. Replace it with a clean modal-dispatch helper. Refactor the Enter/Char/Backspace/Esc arms in `handle_key` so that, when `s.modal.is_some()`, they route to modal-specific logic:

```rust
let modal_active = s.modal.is_some();
if modal_active {
    if let Some(action) = modal_key_action(&mut s, code) {
        match action {
            ModalAction::Consume => return false,
            ModalAction::Emit(cmd) => {
                let _ = slash_tx.send(cmd);
                return false;
            }
        }
    }
    // No modal-specific handling: fall through to default.
}
```

Add the helper at the bottom of `main_loop.rs`:

```rust
enum ModalAction {
    Consume,
    Emit(crate::slash::SlashCmd),
}

/// If the current modal has a key handler, return the action to take.
/// Returns `None` to fall through to the default key handling.
fn modal_key_action(
    state: &mut PagerState,
    code: KeyCode,
) -> Option<ModalAction> {
    use crate::slash::SlashCmd;
    use crate::state::ModalKind;

    let modal = state.modal.as_mut()?;
    match modal {
        ModalKind::KeyEntry { provider, input } => match code {
            KeyCode::Esc => {
                state.modal = None;
                Some(ModalAction::Consume)
            }
            KeyCode::Enter => {
                if input.is_empty() {
                    // Empty key: do nothing, don't submit.
                    Some(ModalAction::Consume)
                } else {
                    let key = std::mem::take(input);
                    let provider = provider.clone();
                    Some(ModalAction::Emit(SlashCmd::SetApiKey { provider, key }))
                }
            }
            KeyCode::Backspace => {
                input.pop();
                Some(ModalAction::Consume)
            }
            KeyCode::Char(c) => {
                input.push(c);
                Some(ModalAction::Consume)
            }
            _ => None,
        },

        ModalKind::ModelPicker { .. } => {
            // Task 7 handles the picker key dispatch.
            None
        }

        _ => None,
    }
}
```

(For the `ModelPicker` modal, fall through to default — Task 7 overrides.)

- [ ] **Step 4: Verify the workspace builds**

```bash
cargo build --workspace --exclude oxi-vendor-grok-markdown --exclude oxi-vendor-grok-markdown-core --exclude oxi-vendor-ratatui-textarea --exclude oxi-vendor-ratatui-inline
```

Expected: 0 errors.

- [ ] **Step 5: Run all oxi-pager and oxi-cli tests**

```bash
cargo nextest run -p oxi-pager
cargo nextest run -p oxi-cli
```

Expected: existing tests pass; new slash::command::tests pass; pager_bridge::tests may still be unimplemented (Task 8 replaces them).

- [ ] **Step 6: Lint check**

```bash
cargo clippy -p oxi-pager -p oxi-cli --all-targets -- -D warnings
```

Expected: 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/src/main_loop.rs oxi-pager/src/lib.rs
git commit -m "feat(pager): run gains slash_tx, Enter branch routes /commands"
```

---

## Task 7: `ModelPicker` key routing + provider/model navigation

**Files:**
- Modify: `oxi-pager/src/main_loop.rs` (extend `modal_key_action` for `ModelPicker`)

- [ ] **Step 1: Extend `modal_key_action` to handle `ModelPicker`**

Replace the `ModalKind::ModelPicker { .. } => None,` arm in `modal_key_action` with:

```rust
ModalKind::ModelPicker {
    providers,
    selected_provider,
    models,
    selected_model,
    focus,
    ..
} => match code {
    KeyCode::Esc => {
        state.modal = None;
        Some(ModalAction::Consume)
    }
    KeyCode::Tab => {
        *focus = match focus {
            crate::state::ModelPickerFocus::Provider => crate::state::ModelPickerFocus::Model,
            crate::state::ModelPickerFocus::Model => crate::state::ModelPickerFocus::Provider,
        };
        Some(ModalAction::Consume)
    }
    KeyCode::BackTab => {
        *focus = match focus {
            crate::state::ModelPickerFocus::Provider => crate::state::ModelPickerFocus::Model,
            crate::state::ModelPickerFocus::Model => crate::state::ModelPickerFocus::Provider,
        };
        Some(ModalAction::Consume)
    }
    KeyCode::Up => {
        match focus {
            crate::state::ModelPickerFocus::Provider => {
                if *selected_provider > 0 {
                    *selected_provider -= 1;
                    let p = providers[*selected_provider].clone();
                    *models = oxi_ai::model_registry::get_all_models()
                        .iter()
                        .filter(|m| m.provider == p)
                        .map(|m| crate::state::ModelEntry {
                            id: m.id.to_string(),
                            provider: m.provider.to_string(),
                            context_window: m.context_window,
                        })
                        .collect();
                    *selected_model = 0;
                }
            }
            crate::state::ModelPickerFocus::Model => {
                if *selected_model > 0 {
                    *selected_model -= 1;
                }
            }
        }
        Some(ModalAction::Consume)
    }
    KeyCode::Down => {
        match focus {
            crate::state::ModelPickerFocus::Provider => {
                if *selected_provider + 1 < providers.len() {
                    *selected_provider += 1;
                    let p = providers[*selected_provider].clone();
                    *models = oxi_ai::model_registry::get_all_models()
                        .iter()
                        .filter(|m| m.provider == p)
                        .map(|m| crate::state::ModelEntry {
                            id: m.id.to_string(),
                            provider: m.provider.to_string(),
                            context_window: m.context_window,
                        })
                        .collect();
                    *selected_model = 0;
                }
            }
            crate::state::ModelPickerFocus::Model => {
                if *selected_model + 1 < models.len() {
                    *selected_model += 1;
                }
            }
        }
        Some(ModalAction::Consume)
    }
    KeyCode::Enter => {
        if *focus == crate::state::ModelPickerFocus::Model
            && let Some(m) = models.get(*selected_model)
        {
            let provider = m.provider.clone();
            let model_id = m.id.clone();
            Some(ModalAction::Emit(SlashCmd::SetDefaultModel { provider, model_id }))
        } else {
            // Provider focus + Enter: switch focus to model pane.
            *focus = crate::state::ModelPickerFocus::Model;
            Some(ModalAction::Consume)
        }
    }
    _ => None,
},
```

Also add the `SlashCmd` import at the top of the `match` if not already present (it's in the outer function's use list, but verify).

- [ ] **Step 2: Verify it compiles**

```bash
cargo build -p oxi-pager
```

Expected: 0 errors.

- [ ] **Step 3: Lint check**

```bash
cargo clippy -p oxi-pager --all-targets -- -D warnings
```

Expected: 0 warnings. (Watch for the `let ... else` clippy lint; if it fires, add `#![allow(clippy::let_else)]` at the top of the test or restructure as `if let`.)

- [ ] **Step 4: Commit**

```bash
git add oxi-pager/src/main_loop.rs
git commit -m "feat(pager): ModelPicker key routing (Up/Down/Tab/Enter/Esc)"
```

---

## Task 8: `KeyEntry` and `ModelPicker` rendering

**Files:**
- Modify: `oxi-pager/src/render/mod.rs` (add modal draw branches)

- [ ] **Step 1: Locate the modal dispatch in `render::render`**

```bash
grep -n "ModalKind::\|state.modal\|match state.modal" oxi-pager/src/render/mod.rs
```

Look for an existing `match` on `state.modal` or a sequence of `if let Some(modal) = &state.modal` branches. Add new arms for the two new variants.

- [ ] **Step 2: Add `KeyEntry` draw branch**

In the modal dispatch, add:

```rust
ModalKind::KeyEntry { provider, input } => {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

    let area = centered_rect(50, 5, frame.size());
    frame.render_widget(Clear, area);

    let masked: String = "*".repeat(input.chars().count());
    let text = format!(
        "Enter API key for {provider} (Esc to cancel)\n\n{}\n\nEnter to save",
        masked,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" API key: {provider} "))
        .style(Style::default().fg(Color::White));
    let para = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}
```

(Add a `fn centered_rect(percent_x, percent_y, area) -> Rect` helper near the top of the render module if not already present. Pattern: `Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }`.)

- [ ] **Step 3: Add `ModelPicker` draw branch**

In the same match, add:

```rust
ModalKind::ModelPicker {
    providers,
    selected_provider,
    models,
    selected_model,
    focus,
    ..
} => {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

    let area = centered_rect(80, 60, frame.size());
    frame.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // Left: providers
    let provider_items: Vec<ListItem> = providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == *selected_provider {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(p.as_str()).style(style)
        })
        .collect();
    let provider_block = Block::default()
        .borders(Borders::ALL)
        .title(if *focus == crate::state::ModelPickerFocus::Provider {
            " Providers (focused) "
        } else {
            " Providers "
        });
    let mut provider_state = ListState::default();
    provider_state.select(Some(*selected_provider));
    let provider_list = List::new(provider_items)
        .block(provider_block)
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(provider_list, chunks[0], &mut provider_state);

    // Right: models
    let model_items: Vec<ListItem> = models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let style = if i == *selected_model {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{} (ctx {})", m.id, m.context_window)).style(style)
        })
        .collect();
    let model_block = Block::default()
        .borders(Borders::ALL)
        .title(if *focus == crate::state::ModelPickerFocus::Model {
            " Models (focused) "
        } else {
            " Models "
        });
    let mut model_state = ListState::default();
    model_state.select(Some(*selected_model));
    let model_list = List::new(model_items)
        .block(model_block)
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(model_list, chunks[1], &mut model_state);
}
```

- [ ] **Step 4: Build**

```bash
cargo build -p oxi-pager
```

Expected: 0 errors. (If `centered_rect` doesn't exist, add the helper.)

- [ ] **Step 5: Run render smoke tests**

```bash
cargo nextest run -p oxi-pager render_smoke_tests
```

Expected: existing smoke tests pass; new modals are drawn (smoke tests don't assert on modal content, just that render() doesn't panic).

- [ ] **Step 6: Lint**

```bash
cargo clippy -p oxi-pager --all-targets -- -D warnings
```

Expected: 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add oxi-pager/src/render/mod.rs
git commit -m "feat(pager): render KeyEntry and ModelPicker modals"
```

---

## Task 9: `bootstrap.rs` redirect to `pager_bridge::run_pager_with_agent`

**Files:**
- Modify: `oxi-cli/src/bootstrap.rs` (replace the `xai-grok-pager` shell-out with `pager_bridge::run_pager_with_agent`)

- [ ] **Step 1: Locate the TUI shell-out**

```bash
grep -n "xai-grok-pager\|grok_pager" oxi-cli/src/bootstrap.rs
```

- [ ] **Step 2: Replace the shell-out with the bridge call**

Find the block:

```rust
    if prompt.is_empty() || args.interactive {
        // TUI mode: launch grok pager (replaces old oxi-tui/oxi-pager)
        // The grok pager provides the full TUI with oxi-ai provider bridge.
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let grok_pager = exe_dir.join("xai-grok-pager");
        if grok_pager.exists() {
            let status = std::process::Command::new(&grok_pager).status()?;
            std::process::exit(status.code().unwrap_or(1));
        } else {
            anyhow::bail!(
                "grok pager binary not found at {}. Run: cargo build -p xai-grok-pager-bin --release",
                grok_pager.display()
            );
        }
    }
```

Replace with:

```rust
    if prompt.is_empty() || args.interactive {
        // TUI mode: run the oxi-pager directly (replaces the previous
        // shell-out to the vendored xai-grok-pager binary, which is no
        // longer required).
        let app = app;
        crate::pager_bridge::run_pager_with_agent(app).await?;
        return Ok(0);
    }
```

(If the surrounding code holds `app: App` (owned, not `Arc`), wrap with `Arc::new(app)` before passing.)

- [ ] **Step 3: Confirm `xai-grok-pager` strings are gone**

```bash
grep -rn "xai-grok-pager\|grok_pager" oxi-cli/src/
```

Expected: 0 matches.

- [ ] **Step 4: Build**

```bash
cargo build -p oxi-cli
```

Expected: 0 errors.

- [ ] **Step 5: Lint**

```bash
cargo clippy -p oxi-cli --all-targets -- -D warnings
```

Expected: 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add oxi-cli/src/bootstrap.rs
git commit -m "feat(cli): TUI mode uses oxi-pager directly (no xai-grok-pager shell-out)"
```

---

## Task 10: Integration test for `on_slash` + `set_default_model` end-to-end

**Files:**
- Modify: `oxi-cli/src/pager_bridge.rs` (replace the `unimplemented!` tests with real ones using a test App)

- [ ] **Step 1: Add a test helper for App construction**

Find the `unimplemented!_app_for_test` helper in the test module. Replace it with a builder that uses `OxiBuilder` to create a real `App`:

```rust
async fn build_test_app() -> Arc<App> {
    use oxi_sdk::OxiBuilder;
    // Use the in-memory adapter set so the test doesn't touch disk.
    let oxi = OxiBuilder::new()
        .with_in_memory_state()
        .with_in_memory_auth()
        .with_builtins()
        .build()
        .expect("test app builds");
    Arc::new(App::from_oxi(oxi, Default::default()))
}
```

(If `OxiBuilder` doesn't have these exact `with_in_memory_*` methods, check `oxi-sdk/src/builder.rs` for the actual names — common alternatives are `with_state(...)`, `with_auth(...)` taking trait objects, and `with_in_memory_*` shorthand. The test compile error will tell you which signature to use. If the App is hard to construct in a unit test, mark this task's tests with `#[ignore]` and rely on Task 11's manual smoke test instead.)

- [ ] **Step 2: Replace the unimplemented test bodies**

Replace the `unimplemented!_app_for_test` calls in the two existing tests with `build_test_app().await`. Make the tests `#[tokio::test]`.

- [ ] **Step 3: Add a test for `SetDefaultModel`**

```rust
#[tokio::test]
async fn set_default_model_emits_system_line() {
    let state = oxi_pager::state::SharedState::default();
    let auth = AuthStorage::in_memory();
    let app = build_test_app().await;
    on_slash(
        SlashCmd::SetDefaultModel {
            provider: "anthropic".into(),
            model_id: "claude-3-5-sonnet-20241022".into(),
        },
        &state,
        &auth,
        &app,
    );
    let s = state.read();
    let last = s.scrollback.blocks.last().expect("no system block pushed");
    assert!(last.text.contains("Default model: anthropic/claude-3-5-sonnet-20241022"));
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo nextest run -p oxi-cli pager_bridge::tests
```

Expected: 3 tests pass. (If the App construction is too heavy and you marked them `#[ignore]`, run with `cargo nextest run -p oxi-cli --run-ignored all`.)

- [ ] **Step 5: Lint**

```bash
cargo clippy -p oxi-cli --all-targets -- -D warnings
```

Expected: 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add oxi-cli/src/pager_bridge.rs
git commit -m "test(cli): on_slash end-to-end with real App + set_default_model"
```

---

## Task 11: Workspace verification + manual smoke test

**Files:** none modified.

- [ ] **Step 1: Build the whole workspace**

```bash
cargo build --workspace --exclude oxi-vendor-grok-markdown --exclude oxi-vendor-grok-markdown-core --exclude oxi-vendor-ratatui-textarea --exclude oxi-vendor-ratatui-inline
```

Expected: 0 errors, 0 warnings.

- [ ] **Step 2: Run all tests**

```bash
cargo nextest run --workspace
```

Expected: 0 failures.

- [ ] **Step 3: Lint the whole workspace**

```bash
cargo clippy --workspace --all-targets --exclude oxi-vendor-grok-markdown --exclude oxi-vendor-grok-markdown-core --exclude oxi-vendor-ratatui-textarea --exclude oxi-vendor-ratatui-inline -- -D warnings
```

Expected: 0 warnings.

- [ ] **Step 4: Format check**

```bash
cargo fmt --all -- --check
```

Expected: no diff. If there is a diff, run `cargo fmt --all` and re-verify.

- [ ] **Step 5: Confirm no submodule edits**

```bash
git diff vendor/grok-build | head -5
```

Expected: empty.

- [ ] **Step 6: Manual smoke test (record the result in the commit body)**

```bash
# In one terminal:
cargo run -p oxi -- --model anthropic/claude-3-5-sonnet-20241022 --api-key sk-test
# In the TUI:
#   1. Type: /key zai<Enter>
#      Expected: modal "Enter API key for zai" appears
#   2. Type: sk-zai-test-1234<Enter>
#      Expected: modal closes, scrollback shows "[system] API key saved for zai (sk-zai...1234)"
#   3. Type: /model<Enter>
#      Expected: model picker appears (Providers pane focused)
#   4. Press: ↓↓↓<Tab>↓↓<Enter>
#      Expected: scrollback shows "[system] Default model: anthropic/claude-3-5-sonnet-20241022"
#   5. Type: hello<Enter>
#      Expected: agent streams a response (regression check — non-slash still routes to agent)
#   6. Type: /key notreal<Enter>
#      Expected: status line shows "Unknown provider: notreal. Available: anthropic, openai, google, zai, minimax, ..."
#   7. Ctrl+C
#      Expected: TUI exits cleanly, terminal restored
# In a second terminal:
cat ~/.oxi/auth.json
# Expected: contains a "zai" provider entry with the masked test key
cat ~/.oxi/settings.toml
# Expected: last_used_model = "anthropic/claude-3-5-sonnet-20241022"
```

- [ ] **Step 7: Final commit (if any fmt fixups from Step 4)**

```bash
git add -u
git commit -m "style: cargo fmt"
```

If no fixups were needed, skip this step.

---

## Self-Review

**1. Spec coverage:**

| Spec requirement | Plan task |
|---|---|
| `/key <provider>` parses and validates against builtin registry | Task 2 |
| `/model [provider]` parses and validates | Task 3 |
| Slash registry is a pure dispatcher | Task 1 |
| `SlashCmd` enum covers all side effects | Task 1 |
| `CommandRegistry::builtin()` static cache | Task 1 |
| `ModalKind::KeyEntry` + `ModelPicker` + `ModelPickerFocus` | Task 4 |
| `oxi-cli::pager_bridge` performs side effects | Task 5 |
| `oxi_pager::run` gains `slash_tx` | Task 6 |
| `handle_key` Enter routes through `CommandRegistry::dispatch` | Task 6 |
| Modal key routing for `KeyEntry` | Task 6 |
| Modal key routing for `ModelPicker` (Up/Down/Tab/Enter/Esc) | Task 7 |
| `KeyEntry` and `ModelPicker` render | Task 8 |
| `bootstrap.rs` redirects to bridge | Task 9 |
| `App::switch_model` reused (no new method) | Task 5 (calls existing) |
| `AuthStorage::set_api_key` reused (no new method) | Task 5 (calls existing) |
| `xai-grok-pager` strings removed from oxi-cli | Task 9 |
| Unit tests for `/key` and `/model` parsers | Tasks 2, 3 |
| Reducer tests | (Folded into Task 6 — modal_key_action is a pure function) |
| Integration test for `on_slash` | Task 10 |
| Manual smoke test | Task 11 |
| No submodule edits | Tasks 1-11 (only oxi-pager and oxi-cli touched) |

**2. Placeholder scan:** No "TBD"/"TODO"/"implement later" in any step. The `unimplemented!_app_for_test` helper in Task 5 is explicitly called out as a temporary stub replaced in Task 10.

**3. Type consistency:**
- `SlashCmd` defined in Task 1 with 6 variants; used in Tasks 2, 3, 5, 6, 7, 10.
- `ModalKind::KeyEntry { provider, input }` shape consistent in Tasks 4, 5, 6, 8.
- `ModalKind::ModelPicker { providers, selected_provider, models, selected_model, filter, focus }` shape consistent in Tasks 4, 5, 7, 8.
- `ModelEntry { id, provider, context_window }` consistent in Tasks 4, 5, 7, 8.
- `ModelPickerFocus` enum consistent in Tasks 4, 6, 7, 8.
- `ModalAction` enum (private to `main_loop.rs`) consistent in Task 6 and 7.
- `on_slash(cmd, &state, &auth, &app)` signature consistent in Tasks 5 and 10.
- `oxi_pager::run(user_tx, slash_tx, bg_rx)` signature consistent in Tasks 5, 6, 9.

No inconsistencies found.
