//! Slash command registry — the migration target for `handle_slash_command`.
//!
//! See `docs/designs/2026-06-17-slash-command-registry.md`.
//!
//! This file is introduced in step 1: the types and the registry exist and
//! compile, but `builtins()` is empty and nothing dispatches through it yet.
//! As commands are ported off the legacy match (step 2), they register here.

use super::{CompletionEntry, SlashCtx, SlashOutcome};
use crate::app::agent_session::AgentSession;
use crate::extensions::{WasmCommandDef, WasmExtensionManager};
use crate::tui::app::AppState;
use crate::tui::completion::CompletionItem;
use std::sync::Arc;

/// One slash command owns its definition, execution, and completion.
///
/// Adding a command = implementing this trait + registering in `builtins()`.
/// Aliases, usage, and argument completion live alongside the handler, so the
/// old "keep the table in sync with the match" drift cannot recur.
#[allow(dead_code)] // dispatched starting step 2
pub(crate) trait SlashCommand: Send + Sync {
    /// Canonical name, no leading `/` (e.g. `"mcp"`).
    fn name(&self) -> &str;
    /// Alternative names resolved alongside `name()` (e.g. `["ext"]`).
    fn aliases(&self) -> &[&str] {
        &[]
    }
    /// Short description shown in completions and help.
    fn description(&self) -> &str;
    /// Usage string (e.g. `"/mcp <dashboard|status>"`).
    fn usage(&self) -> &str {
        ""
    }

    /// Run the command. Synchronous; async work spawns onto the runtime and
    /// reports back via `ctx.ui_tx` (the existing `/share`, `/compact`
    /// pattern). Rationale: design Decision A.
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome;

    /// Argument completion (read-only). Default empty. Implementations return
    /// static subcommands, dynamic values (model ids, skill names…), or
    /// delegate to the path completer. Read-only access (`&AppState` / shared
    /// `&AgentSession`) keeps completion off any async boundary — see design
    /// §5.
    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        _state: &AppState,
    ) -> Vec<CompletionItem> {
        let _ = prefix;
        Vec::new()
    }

    /// Whether `token` (with or without leading `/`) names this command.
    fn matches(&self, token: &str) -> bool {
        let t = token.strip_prefix('/').unwrap_or(token);
        t.eq_ignore_ascii_case(self.name())
            || self.aliases().iter().any(|a| t.eq_ignore_ascii_case(a))
    }
}

/// Central registry of all slash commands: builtins + extension adapters.
pub(crate) struct SlashRegistry {
    builtin: Vec<Box<dyn SlashCommand>>,
    /// Extension commands, adapted to the trait at runtime via [`sync_extensions`].
    extensions: Vec<ExtensionCmdAdapter>,
}

/// Adapter that exposes a WASM extension command as a [`SlashCommand`].
///
/// The WASM host returns a `String`; for MVP we surface it as an info
/// notification attributed to the owning extension. The design's structured
/// outcome (§7.2) is a future enhancement — the `output`-only contract keeps
/// existing extensions working unchanged.
pub(crate) struct ExtensionCmdAdapter {
    /// Owning extension name (for `[ext] …` attribution).
    ext_name: String,
    def: WasmCommandDef,
    mgr: Arc<WasmExtensionManager>,
}

impl ExtensionCmdAdapter {
    fn matches_token(&self, token: &str) -> bool {
        let t = token.strip_prefix('/').unwrap_or(token);
        t.eq_ignore_ascii_case(&self.def.name)
    }
}

impl SlashCommand for ExtensionCmdAdapter {
    fn name(&self) -> &str {
        &self.def.name
    }
    fn description(&self) -> &str {
        &self.def.description
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let output = self
            .mgr
            .execute_command(&self.def.name, args)
            .unwrap_or_else(|e| format!("Error: {}", e));
        ctx.state.add_notification(
            format!("[{}] {}", self.ext_name, output),
            crate::tui::app::NotificationKind::Info,
        );
        SlashOutcome::Handled
    }
}

impl SlashRegistry {
    /// Assemble all built-in commands via `super::builtin::register_all`.
    pub(crate) fn builtins() -> Self {
        let mut registry = SlashRegistry {
            builtin: Vec::new(),
            extensions: Vec::new(),
        };
        super::builtin::register_all(&mut registry);
        registry
    }

    /// Register one command.
    pub(crate) fn register(&mut self, cmd: Box<dyn SlashCommand>) {
        self.builtin.push(cmd);
    }

    /// Rebuild the extension slice from a live WASM manager. Cheap: a handful
    /// of `Arc` clones.
    pub(crate) fn sync_extensions(&mut self, mgr: Option<&std::sync::Arc<WasmExtensionManager>>) {
        self.extensions.clear();
        if let Some(mgr) = mgr {
            for (ext_name, def) in mgr.all_command_defs() {
                self.extensions.push(ExtensionCmdAdapter {
                    ext_name: ext_name.to_string(),
                    def: def.clone(),
                    mgr: std::sync::Arc::clone(mgr),
                });
            }
        }
    }

    /// Command-token completion: includes canonical names, aliases, and
    /// extension adapters. Returns entries whose canonical name or any alias
    /// starts with `query` (query has no leading `/`).
    pub(crate) fn complete_command(&self, query: &str) -> Vec<CompletionEntry> {
        let mut entries: Vec<CompletionEntry> = self
            .builtin
            .iter()
            .flat_map(|cmd| {
                let canonical = format!("/{}", cmd.name());
                let mut e = vec![CompletionEntry {
                    display: canonical.clone(),
                    canonical: canonical.clone(),
                    description: cmd.description().to_string(),
                    is_extension: false,
                }];
                for alias in cmd.aliases() {
                    let display = format!("/{}", alias);
                    e.push(CompletionEntry {
                        display,
                        canonical: canonical.clone(),
                        description: cmd.description().to_string(),
                        is_extension: false,
                    });
                }
                e
            })
            .collect();
        // Extension commands (no aliases in the MVP WASM contract).
        for ext in &self.extensions {
            entries.push(CompletionEntry {
                display: format!("/{}", ext.name()),
                canonical: format!("/{}", ext.name()),
                description: ext.description().to_string(),
                is_extension: true,
            });
        }
        entries
            .into_iter()
            .filter(|e| {
                e.display[1..]
                    .to_lowercase()
                    .starts_with(&query.to_lowercase())
            })
            .collect()
    }

    /// Try to dispatch `input` to the first matching command. Returns
    /// `NotHandled` if no command matches.
    pub(crate) fn dispatch(&self, input: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let trimmed = input.trim();
        let (cmd_token, arg) = match trimmed.find(' ') {
            Some(space) => (&trimmed[..space], Some(trimmed[space + 1..].trim())),
            None => (trimmed, None),
        };
        let token = cmd_token.strip_prefix('/').unwrap_or(cmd_token);
        for command in &self.builtin {
            if command.matches(token) {
                return command.execute(arg.unwrap_or(""), ctx);
            }
        }
        for ext in &self.extensions {
            if ext.matches_token(token) {
                return ext.execute(arg.unwrap_or(""), ctx);
            }
        }
        SlashOutcome::NotHandled
    }

    /// Argument completion: dispatch to the matched command's `complete_arg`.
    /// `cmd_token` has no leading `/`; `arg_prefix` is the text after the
    /// command + space. Read-only access (`&AppState` / `&AgentSession`).
    pub(crate) fn complete_arg(
        &self,
        cmd_token: &str,
        arg_prefix: &str,
        session: &AgentSession,
        state: &AppState,
    ) -> Vec<CompletionItem> {
        let token = cmd_token.strip_prefix('/').unwrap_or(cmd_token);
        for command in &self.builtin {
            if command.matches(token) {
                return command.complete_arg(arg_prefix, session, state);
            }
        }
        for ext in &self.extensions {
            if ext.matches_token(token) {
                return ext.complete_arg(arg_prefix, session, state);
            }
        }
        Vec::new()
    }

    /// Usage string for the command matching `cmd_token`, if any.
    #[allow(dead_code)] // surfaced in the argument-completion popup later
    pub(crate) fn usage_for(&self, cmd_token: &str) -> Option<&str> {
        let token = cmd_token.strip_prefix('/').unwrap_or(cmd_token);
        for command in &self.builtin {
            if command.matches(token) {
                let u = command.usage();
                return if u.is_empty() { None } else { Some(u) };
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyCommand {
        name: &'static str,
        aliases: Vec<&'static str>,
        description: &'static str,
    }

    impl SlashCommand for DummyCommand {
        fn name(&self) -> &str {
            self.name
        }
        fn aliases(&self) -> &[&str] {
            &self.aliases
        }
        fn description(&self) -> &str {
            self.description
        }
        fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> SlashOutcome {
            SlashOutcome::Handled
        }
    }

    fn registry_with(cmds: Vec<Box<dyn SlashCommand>>) -> SlashRegistry {
        SlashRegistry {
            builtin: cmds,
            extensions: Vec::new(),
        }
    }

    #[test]
    fn complete_command_includes_canonical_and_aliases() {
        let reg = registry_with(vec![
            Box::new(DummyCommand {
                name: "extensions",
                aliases: vec!["ext"],
                description: "List extensions",
            }),
            Box::new(DummyCommand {
                name: "model",
                aliases: vec![],
                description: "Select model",
            }),
        ]);
        // Empty query → all entries (canonical + alias).
        let all = reg.complete_command("");
        let names: Vec<&str> = all.iter().map(|e| e.display.as_str()).collect();
        assert!(names.contains(&"/extensions"));
        assert!(names.contains(&"/ext"));
        assert!(names.contains(&"/model"));
    }

    #[test]
    fn complete_command_filters_by_prefix_case_insensitive() {
        let reg = registry_with(vec![Box::new(DummyCommand {
            name: "Model",
            aliases: vec![],
            description: "d",
        })]);
        let res = reg.complete_command("mo");
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].display, "/Model");
    }

    #[test]
    fn matches_resolves_alias_case_insensitively() {
        let cmd = DummyCommand {
            name: "hotkeys",
            aliases: vec!["keys"],
            description: "d",
        };
        assert!(cmd.matches("/hotkeys"));
        assert!(cmd.matches("/KEYS"));
        assert!(cmd.matches("keys"));
        assert!(!cmd.matches("/unknown"));
    }

    #[test]
    fn alias_entries_point_at_canonical() {
        let reg = registry_with(vec![Box::new(DummyCommand {
            name: "extensions",
            aliases: vec!["ext"],
            description: "List extensions",
        })]);
        let ext = reg
            .complete_command("ext")
            .into_iter()
            .find(|e| e.display == "/ext")
            .expect("/ext present");
        assert_eq!(ext.canonical, "/extensions");
        assert!(!ext.is_extension);
    }
}
