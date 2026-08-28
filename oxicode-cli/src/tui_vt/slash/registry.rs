//! Slash command registry for the VT TUI harness.
//!
//! Parsed in `crate::tui_vt::main_loop::handle_inline_event` when the
//! submitted text begins with `/`. Each command owns its name, aliases, and
//! execution. Adding a command = implement `SlashCommand` + register it in
//! [`SlashRegistry::builtins`].
//!
//! Slash commands for the VT TUI harness. Each command owns its name, aliases,
//! and execution; adding one = implement `SlashCommand` + register it in
//! [`SlashRegistry::builtins`] (`register_all`). Overlay-driving commands
//! (`/model`, `/settings`, `/sessions`, `/theme`) build an `InlineListSelection`
//! modal whose submission is handled in `main_loop.rs`'s overlay-submission arm.
//! `/issue` opens the issues panel (see `commands.rs::IssueCommand`).

use oxicode_vtui::tui::core::{
    InlineHandle, InlineListItem, InlineListSearchConfig, InlineListSelection, InlineMessageKind,
};

use crate::app::agent_session::AgentSessionHandle;
use crate::store::settings::Settings;
use crate::tui_vt::main_loop::{RenderState, plain_segment};
use crate::tui_vt::settings_defs::{SettingWidget, SettingsTab, defs_for_tab, get_display_value};

/// Outcome of dispatching a slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashOutcome {
    /// Command handled; remain in the event loop.
    Handled,
    /// Request application shutdown (`/quit`).
    Quit,
    /// No command matched; caller surfaces an "unknown command" notice.
    NotHandled,
}

/// Everything a slash command execution needs, bundled so adding a
/// dependency never changes every command's signature.
pub(crate) struct SlashCtx<'a> {
    pub session: &'a AgentSessionHandle,
    pub handle: &'a InlineHandle,
    pub state: &'a mut RenderState,
}

impl SlashCtx<'_> {
    /// Append one or more transcript lines via the harness command channel —
    /// the single source of truth (`apply_command` applies it to `RenderState`
    /// on the next loop iteration, so we must NOT also mutate `state` here).
    /// Newlines split into separate transcript lines.
    pub(crate) fn reply(&self, kind: InlineMessageKind, text: impl Into<String>) {
        let text = text.into();
        for line in text.split('\n') {
            self.handle
                .append_line(kind, vec![plain_segment(line.to_string())]);
        }
    }
}

/// One slash command owns its definition and execution.
///
/// Adding a command = implementing this trait + registering in `builtins()`.
/// Aliases live alongside the handler so the old "keep the table in sync with
/// the match" drift cannot recur.
pub(crate) trait SlashCommand: Send + Sync {
    /// Canonical name, no leading `/` (e.g. `"quit"`).
    fn name(&self) -> &'static str;
    /// Alternative names resolved alongside `name()` (e.g. `["exit", "q"]`).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    /// Short description shown in `/help` and RPC `get_commands`.
    fn description(&self) -> &'static str;
    /// Run the command. `args` is the trimmed text after the command token.
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome;

    /// Whether `token` (no leading `/`) names this command (case-insensitive).
    fn matches(&self, token: &str) -> bool {
        token.eq_ignore_ascii_case(self.name())
            || self.aliases().iter().any(|a| token.eq_ignore_ascii_case(a))
    }
}

/// Central registry of all built-in slash commands.
pub struct SlashRegistry {
    builtins: Vec<Box<dyn SlashCommand>>,
}

impl SlashRegistry {
    /// Assemble all built-in commands.
    pub fn builtins() -> Self {
        let mut registry = SlashRegistry {
            builtins: Vec::new(),
        };
        register_all(&mut registry);
        registry
    }

    /// Register one command.
    pub(crate) fn register(&mut self, cmd: Box<dyn SlashCommand>) {
        self.builtins.push(cmd);
    }

    /// Static catalog for RPC `get_commands`: `(name, description, aliases)`.
    /// Kept as a standalone associated fn so RPC can enumerate without
    /// constructing a session/handle context.
    pub fn builtin_commands() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
        Self::builtins()
            .builtins
            .iter()
            .map(|c| (c.name(), c.description(), c.aliases().to_vec()))
            .collect()
    }

    /// Try to dispatch `input` (full `/cmd args…`) to a command. `/help` is
    /// intercepted here because only the registry can enumerate its siblings.
    /// Returns [`SlashOutcome::NotHandled`] if nothing matches.
    pub(crate) fn dispatch(&self, input: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let trimmed = input.trim();
        let (cmd_token, arg) = match trimmed.find(' ') {
            Some(space) => (&trimmed[..space], trimmed[space + 1..].trim()),
            None => (trimmed, ""),
        };
        let token = cmd_token.strip_prefix('/').unwrap_or(cmd_token);

        if matches!(token, "help" | "?" | "commands") {
            self.render_help(ctx);
            return SlashOutcome::Handled;
        }

        for command in &self.builtins {
            if command.matches(token) {
                return command.execute(arg, ctx);
            }
        }
        SlashOutcome::NotHandled
    }

    fn render_help(&self, ctx: &mut SlashCtx<'_>) {
        let mut items: Vec<InlineListItem> = self
            .builtins
            .iter()
            .map(|c| {
                let mut title = format!("/{}", c.name());
                for alias in c.aliases() {
                    title.push_str(&format!(", /{alias}"));
                }
                InlineListItem {
                    title,
                    subtitle: Some(c.description().to_string()),
                    badge: None,
                    indent: 0,
                    selection: Some(InlineListSelection::SlashCommand(c.name().to_string())),
                    search_value: None,
                }
            })
            .collect();
        items.sort_by(|a, b| a.title.cmp(&b.title));
        ctx.handle.show_list_modal(
            "Commands".to_string(),
            vec!["Browse commands and insert one into the composer.".to_string()],
            items,
            None,
            None,
        );
    }
}

/// Build the `/settings` overlay rows for one tab from the
/// [`SETTING_DEFS`](crate::tui_vt::settings_defs::SETTING_DEFS) table —
/// never a hand-written list; adding a def is all it takes to show up.
///
/// Emits a non-interactive heading row (`selection: None`, no subtitle
/// or badge — the same convention the old read-only "Model" row used)
/// whenever the group label changes, then one row per def with its live
/// value from [`get_display_value`] as the badge. `Toggle` and `Cycle`
/// rows submit a `ConfigAction` carrying the `SettingKey` Debug name;
/// `Text` / `SubmenuSelect` / `Multiselect` / `Pointer` rows stay
/// read-only here (their editors route through the overlay-event path
/// in `main_loop.rs`).
///
/// The two `MapEditor` defs expand in place into per-entry rows:
/// - `Keybindings` → one action row per `GlobalAction` (Enter opens the
///   key-capture submenu) plus one indented row per bound combo.
/// - `ModelRoles` → one row per role.
///
/// Returns the items plus a parallel `SettingsMapRow` table (index
/// aligned, `None` for ordinary rows) that the settings panel's input
/// handling consults for `Enter` / `d` / `n` on map rows.
pub(crate) fn settings_overlay_items(
    tab: SettingsTab,
    settings: &Settings,
) -> (
    Vec<InlineListItem>,
    Vec<Option<crate::tui_vt::settings_defs::SettingsMapRow>>,
) {
    use crate::tui_vt::settings_defs::{SettingKey, SettingsMapRow};
    let mut items: Vec<InlineListItem> = Vec::new();
    let mut rows: Vec<Option<SettingsMapRow>> = Vec::new();
    let mut last_group: Option<&'static str> = None;
    for def in defs_for_tab(tab, settings) {
        if last_group != Some(def.group) {
            items.push(InlineListItem {
                title: def.group.to_string(),
                subtitle: None,
                badge: None,
                indent: 0,
                selection: None,
                search_value: None,
            });
            rows.push(None);
            last_group = Some(def.group);
        }
        // Map editors expand into per-entry rows; the generic def row
        // never renders for them.
        match (def.widget, def.key) {
            (SettingWidget::MapEditor, SettingKey::Keybindings) => {
                expand_keybinding_rows(settings, &mut items, &mut rows);
                continue;
            }
            (SettingWidget::MapEditor, SettingKey::ModelRoles) => {
                expand_model_role_rows(settings, &mut items, &mut rows);
                continue;
            }
            _ => {}
        }
        let selection = match def.widget {
            // Toggle/Cycle commit through the ConfigAction path (the
            // overlay-submission arm routes the SettingKey Debug name).
            SettingWidget::Toggle | SettingWidget::Cycle => {
                Some(InlineListSelection::ConfigAction(format!("{:?}", def.key)))
            }
            // Text/SubmenuSelect/Multiselect each get their own
            // selection variant: the editor opens on Enter with the
            // SettingKey Debug name as the payload, routed via the
            // matching arm in `handle_inline_event`.
            SettingWidget::Text => Some(InlineListSelection::SettingTextEdit(format!(
                "{:?}",
                def.key
            ))),
            SettingWidget::SubmenuSelect(_) => Some(InlineListSelection::SettingSubmenuOpen(
                format!("{:?}", def.key),
            )),
            SettingWidget::Multiselect => Some(InlineListSelection::SettingMultiselect(format!(
                "{:?}",
                def.key
            ))),
            // Pointer + MapEditor rows stay read-only here (MapEditor
            // is expanded above into per-entry rows with their own
            // selection variants; Pointer rows are owned by
            // slash commands).
            SettingWidget::MapEditor | SettingWidget::Pointer => None,
        };
        items.push(InlineListItem {
            title: def.label.to_string(),
            subtitle: Some(def.description.to_string()),
            badge: Some(get_display_value(def.key, settings)),
            indent: 0,
            selection,
            search_value: Some(format!("{} {} {:?}", def.label, def.description, def.key)),
        });
        rows.push(None);
    }
    (items, rows)
}

/// Expand the `Keybindings` MapEditor def: one action row per
/// `GlobalAction` (Enter → key-capture submenu, selection carries the
/// action name) followed by one indented row per bound combo. The
/// combo list comes from a keymap hydrated exactly like the live one
/// (defaults + user overrides), so the rows mirror what the next
/// keystroke actually resolves.
fn expand_keybinding_rows(
    settings: &Settings,
    items: &mut Vec<InlineListItem>,
    rows: &mut Vec<Option<crate::tui_vt::settings_defs::SettingsMapRow>>,
) {
    use crate::tui_vt::settings_defs::SettingsMapRow;
    let keymap = crate::tui_vt::keymap::Keymap::from_settings(&settings.keybindings);
    for action in crate::tui_vt::keymap::GlobalAction::all() {
        let name = action.name();
        items.push(InlineListItem {
            title: name.to_string(),
            subtitle: Some("Enter: capture a new combo".into()),
            badge: None,
            indent: 0,
            selection: Some(InlineListSelection::SettingKeyCapture(name.to_string())),
            search_value: Some(name.to_string()),
        });
        rows.push(Some(SettingsMapRow::KeybindingAction(action)));
        for combo in keymap.action_combos(action) {
            let combo = combo.to_string();
            items.push(InlineListItem {
                title: combo.clone(),
                subtitle: Some("d: remove".into()),
                badge: None,
                indent: 1,
                selection: None,
                search_value: Some(format!("{name} {combo}")),
            });
            rows.push(Some(SettingsMapRow::KeybindingCombo(action, combo)));
        }
    }
}

/// Expand the `ModelRoles` MapEditor def into one row per role
/// (sorted by role so the list — and tests — stay deterministic):
/// Enter edits the value, `d` deletes the role, `n` anywhere on the
/// Model tab starts a new role. An empty map renders a hint row.
fn expand_model_role_rows(
    settings: &Settings,
    items: &mut Vec<InlineListItem>,
    rows: &mut Vec<Option<crate::tui_vt::settings_defs::SettingsMapRow>>,
) {
    use crate::tui_vt::settings_defs::SettingsMapRow;
    if settings.model_roles.is_empty() {
        items.push(InlineListItem {
            title: "(no model roles)".into(),
            subtitle: Some("n: add a role".into()),
            badge: None,
            indent: 1,
            selection: None,
            search_value: Some("model roles".into()),
        });
        rows.push(None);
        return;
    }
    let mut roles: Vec<(&String, &String)> = settings.model_roles.iter().collect();
    roles.sort();
    for (role, model) in roles {
        items.push(InlineListItem {
            title: role.clone(),
            subtitle: Some("Enter: edit \u{b7} d: delete".into()),
            badge: Some(model.clone()),
            indent: 0,
            selection: None,
            search_value: Some(format!("{role} {model}")),
        });
        rows.push(Some(SettingsMapRow::ModelRole(role.clone())));
    }
}

/// `/settings` — open the settings panel overlay.
///
/// Rows come from the declarative settings table: a heading per group,
/// one row per setting, live values as badges. Selecting a `Toggle` or
/// `Cycle` row submits its `SettingKey` for an immediate apply.
struct SettingsCommand;

impl SlashCommand for SettingsCommand {
    fn name(&self) -> &'static str {
        "settings"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["config"]
    }
    fn description(&self) -> &'static str {
        "Open the settings panel"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let settings = Settings::load().unwrap_or_default();
        // `handle_inline_event`'s ShowOverlay hydration replaces this
        // flat list with the full tabbed panel (on the live tab); this
        // request just carries the first tab's rows for harnesses that
        // render the request verbatim.
        let (items, _rows) = settings_overlay_items(SettingsTab::General, &settings);

        let search = InlineListSearchConfig {
            label: "Filter settings".into(),
            placeholder: Some("Type to filter".into()),
        };
        ctx.handle.show_list_modal(
            "Settings".into(),
            vec!["Browse settings by group; filter with the search bar.".into()],
            items,
            None,
            Some(search),
        );
        SlashOutcome::Handled
    }
}

/// `/sessions` — open a session picker overlay listing recent sessions.
/// Selecting a session enqueues a resume that fires on the next Enter.
struct SessionsCommand;

/// Resolve the TUI's session storage directory (mirrors the CLI's
/// `~/.oxicode/sessions`).
pub(crate) fn sessions_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".oxicode").join("sessions"))
        .unwrap_or_else(|| std::path::PathBuf::from(".oxicode/sessions"))
}

impl SessionsCommand {
    fn open_picker(&self, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        use oxicode_vtui::tui::core::{InlineListItem, InlineListSearchConfig};

        let session_dir = sessions_dir();
        let mut entries: Vec<(String, std::time::SystemTime)> = Vec::new();
        if let Ok(dir) = std::fs::read_dir(session_dir) {
            for entry in dir.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    let id = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let mtime = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    entries.push((id, mtime));
                }
            }
        }
        entries.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        entries.truncate(30);

        if entries.is_empty() {
            ctx.reply(InlineMessageKind::Info, "No saved sessions found.");
            return SlashOutcome::Handled;
        }

        let items: Vec<InlineListItem> = entries
            .iter()
            .map(|(id, mtime)| {
                let time_str = format_relative_time(*mtime);
                InlineListItem {
                    title: format!("{id}  \u{00b7}  {time_str}"),
                    subtitle: Some("Enter to resume".into()),
                    badge: None,
                    indent: 0,
                    selection: Some(InlineListSelection::Session(id.clone())),
                    search_value: Some(id.clone()),
                }
            })
            .collect();

        let search = InlineListSearchConfig {
            label: "Filter sessions".into(),
            placeholder: Some("Type to filter\u{2026}".into()),
        };
        ctx.handle.show_list_modal(
            "Sessions".into(),
            vec!["Select a session to resume (Esc to close)".into()],
            items,
            None,
            Some(search),
        );
        SlashOutcome::Handled
    }
}

impl SlashCommand for SessionsCommand {
    fn name(&self) -> &'static str {
        "sessions"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["resume"]
    }
    fn description(&self) -> &'static str {
        "Browse and resume past sessions"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let arg = args.trim();
        if arg.is_empty() {
            return self.open_picker(ctx);
        }

        if ctx.session.is_streaming() {
            ctx.reply(
                InlineMessageKind::Error,
                "Cannot resume while agent is running. Use /cancel first.",
            );
            return SlashOutcome::Handled;
        }
        let path = sessions_dir().join(format!("{arg}.jsonl"));
        if !path.is_file() {
            ctx.reply(
                InlineMessageKind::Error,
                format!("No session file: {}", path.display()),
            );
            return SlashOutcome::Handled;
        }
        ctx.state.pending_resume = Some(path);
        ctx.reply(InlineMessageKind::Info, format!("Resuming {arg}…"));
        SlashOutcome::Handled
    }
}
/// Format a `SystemTime` as a human-readable relative time (e.g. "2h ago").
fn format_relative_time(t: std::time::SystemTime) -> String {
    let now = std::time::SystemTime::now();
    match now.duration_since(t) {
        Ok(d) => {
            let mins = d.as_secs() / 60;
            if mins < 1 {
                "just now".into()
            } else if mins < 60 {
                format!("{mins}m ago")
            } else if mins < 60 * 24 {
                format!("{}h ago", mins / 60)
            } else if mins < 60 * 24 * 7 {
                format!("{}d ago", mins / (60 * 24))
            } else {
                format!("{}w ago", mins / (60 * 24 * 7))
            }
        }
        Err(_) => "unknown".into(),
    }
}

fn register_all(registry: &mut SlashRegistry) {
    registry.register(Box::new(QuitCommand));
    registry.register(Box::new(ClearCommand));
    registry.register(Box::new(CompactCommand));
    registry.register(Box::new(ModelCommand));
    registry.register(Box::new(CancelCommand));
    registry.register(Box::new(StatusCommand));
    registry.register(Box::new(SettingsCommand));
    registry.register(Box::new(VimCommand));
    registry.register(Box::new(AgentsCommand));
    registry.register(Box::new(ThemeCommand));
    registry.register(Box::new(FindCommand));
    registry.register(Box::new(SessionsCommand));
    registry.register(Box::new(ShortcutsCommand));
    registry.register(Box::new(HandoffCommand));
    registry.register(Box::new(MemoryCommand));
    registry.register(Box::new(super::todo_command::TodoCommand));
    super::commands::register_extra(registry);
}

/// `/vim` — toggle vim mode for prompt editing.
struct VimCommand;

impl SlashCommand for VimCommand {
    fn name(&self) -> &'static str {
        "vim"
    }
    fn description(&self) -> &'static str {
        "Toggle vim mode for prompt editing"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let enabled = !ctx.state.vim_state.enabled();
        ctx.state.vim_state.set_enabled(enabled);
        ctx.reply(
            InlineMessageKind::Info,
            if enabled {
                "Vim mode: ON — press Esc for Normal, i for Insert".to_string()
            } else {
                "Vim mode: OFF".to_string()
            },
        );
        SlashOutcome::Handled
    }
}

/// `/memory` — oxibrain durable-memory status: daemon health, space stats,
/// and recovery hints. Aliases: `/brain`, `/mem`.
struct MemoryCommand;

impl SlashCommand for MemoryCommand {
    fn name(&self) -> &'static str {
        "memory"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["brain", "mem"]
    }
    fn description(&self) -> &'static str {
        "Show oxibrain memory status (health, stats, recovery hints)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let args = _args.to_string();
        let handle = ctx.handle.clone();
        // The command runs inside the TUI's tokio runtime, so the daemon
        // round-trips go through `tokio::spawn`; replies land on the
        // transcript via the cloned `InlineHandle`.
        ctx.reply(InlineMessageKind::Info, "Brain memory — querying daemon…");
        tokio::spawn(async move {
            fn append(handle: &oxicode_vtui::tui::core::InlineHandle, text: String) {
                for line in text.split('\n') {
                    handle.append_line(
                        InlineMessageKind::Info,
                        vec![plain_segment(line.to_string())],
                    );
                }
            }

            let socket = crate::foundation::brain::default_socket_path();
            let backend = crate::foundation::brain::BrainMemoryBackend::new(socket.clone());
            let enabled = crate::store::settings::Settings::load()
                .map(|s| s.memory_enabled)
                .unwrap_or(true);
            let mut out = format!("socket:    {}", socket.display());
            out.push_str(if enabled {
                "\ntools:     enabled (memory_enabled)"
            } else {
                "\ntools:     disabled (memory_enabled = false in settings)"
            });
            let restart = args.trim().eq_ignore_ascii_case("restart");
            if restart {
                match crate::foundation::brain_control::revive().await {
                    Ok(msg) => out.push_str(&format!("\nrestart:   {msg}")),
                    Err(err) => out.push_str(&format!("\nrestart:   {err}")),
                }
            }
            match backend.ping().await {
                Ok(()) => {
                    out.push_str("\nhealth:    ok — oxibrain daemon connected");
                    if let Ok(stats) = backend.stats().await {
                        out.push_str(&format!(
                            "\nstats:     episodes {} · entities {} · statements {} · contradictions {}",
                            stats.get("episodes").and_then(|v| v.as_i64()).unwrap_or(-1),
                            stats.get("entities").and_then(|v| v.as_i64()).unwrap_or(-1),
                            stats.get("statements").and_then(|v| v.as_i64()).unwrap_or(-1),
                            stats.get("contradictions").and_then(|v| v.as_i64()).unwrap_or(-1),
                        ));
                    }
                    out.push_str(
                        "\nhints:     chat tools memory_retain / memory_recall / memory_reflect \
                         / memory_edit read+write this daemon",
                    );
                }
                Err(e) => {
                    out.push_str(&format!("\nhealth:    degraded — {e}"));
                    // Installed but stopped? Revive instead of just
                    // hinting: launchd bootstrap/kickstart when
                    // supervised, detached spawn otherwise.
                    match crate::foundation::brain_control::revive().await {
                        Ok(msg) => {
                            out.push_str(&format!("\nrevive:    {msg}"));
                            out.push_str("\n            the health chip refreshes within ~20s");
                        }
                        Err(err) => out.push_str(&format!("\nrevive:    {err}")),
                    }
                    out.push_str(
                        "\nhints:     set OXIBRAIN_SOCKET if the daemon lives elsewhere; \
                         no local fallback exists by design",
                    );
                }
            }
            if crate::foundation::migrate::default_legacy_path().exists() {
                out.push_str(
                    "\nlegacy:    a legacy local store exists — run `oxicode migrate brain` \
                     to move it into the daemon",
                );
            }
            append(&handle, out);
        });
        SlashOutcome::Handled
    }
}

/// `/agents` — open the Agent Hub overlay. Alias: `/hub`.
struct AgentsCommand;

impl SlashCommand for AgentsCommand {
    fn name(&self) -> &'static str {
        "agents"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["hub"]
    }
    fn description(&self) -> &'static str {
        "Open the Agent Hub overlay (alias: /hub)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.agent_hub_open = true;
        ctx.state.hub_entries = ctx.session.hub().snapshot();
        SlashOutcome::Handled
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Built-in commands
// ─────────────────────────────────────────────────────────────────────────

/// `/quit` — exit oxicode. Aliases: `/exit`, `/q`.
struct QuitCommand;

impl SlashCommand for QuitCommand {
    fn name(&self) -> &'static str {
        "quit"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["exit", "q"]
    }
    fn description(&self) -> &'static str {
        "Quit oxicode (aliases: /exit, /q)"
    }
    fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        SlashOutcome::Quit
    }
}

/// `/clear` — reset the conversation and wipe the transcript. Alias: `/cls`.
struct ClearCommand;

impl SlashCommand for ClearCommand {
    fn name(&self) -> &'static str {
        "clear"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["cls"]
    }
    fn description(&self) -> &'static str {
        "Clear the conversation and transcript (alias: /cls)"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        // `--yes` skips the confirmation dialog (used when re-dispatching
        // from the confirmation modal). Without it, open the dialog.
        if !args.split_whitespace().any(|a| a == "--yes") {
            ctx.state.confirmation = Some(super::super::main_loop::clear_confirmation());
            return SlashOutcome::Handled;
        }
        ctx.session.reset();
        ctx.state.transcript.clear();
        ctx.state.message_buffer.clear();
        ctx.state.scroll_offset = usize::MAX;
        ctx.reply(InlineMessageKind::Info, "Conversation cleared.");
        SlashOutcome::Handled
    }
}

/// `/compact` — manually trigger context compaction. Optional instructions.
struct CompactCommand;

impl SlashCommand for CompactCommand {
    fn name(&self) -> &'static str {
        "compact"
    }
    fn description(&self) -> &'static str {
        "Compact the context (optional: /compact <instructions>)"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let instructions = args.trim();
        let arg = if instructions.is_empty() {
            None
        } else {
            Some(instructions.to_string())
        };
        let session = ctx.session.clone();
        ctx.reply(InlineMessageKind::Info, "Compacting\u{2026}");
        tokio::spawn(async move {
            match session.compact(arg).await {
                Ok(result) => tracing::info!(?result, "manual compaction complete"),
                Err(err) => tracing::warn!(%err, "manual compaction failed"),
            }
        });
        SlashOutcome::Handled
    }
}

/// `/handoff` — generate a handoff document, start a fresh session, and
/// optionally auto-continue. Alias: `/hd`.
///
///   `/handoff`                generate + new session + auto-continue
///   `/handoff --review`       generate + new session, wait for user
///   `/handoff --dry-run`      generate doc only, don't start new session
///   `/handoff <slug>`         generate with a custom filename slug
struct HandoffCommand;

impl SlashCommand for HandoffCommand {
    fn name(&self) -> &'static str {
        "handoff"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["hd"]
    }
    fn description(&self) -> &'static str {
        "Generate a handoff doc and start a fresh session (alias: /hd)"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        use crate::app::handoff::{HandoffOptions, generate_and_apply_handoff};

        // Parse flags.
        let mut auto_continue = true;
        let mut dry_run = false;
        let mut slug = None;
        for arg in args.split_whitespace() {
            match arg {
                "--review" => auto_continue = false,
                "--dry-run" => dry_run = true,
                s if !s.starts_with('-') => slug = Some(s.to_string()),
                _ => {}
            }
        }

        // Gate: cannot hand off while agent is running.
        if ctx.session.is_streaming() {
            ctx.reply(
                InlineMessageKind::Error,
                "Cannot hand off while agent is running. Use /cancel first.",
            );
            return SlashOutcome::Handled;
        }

        // Gate: need enough conversation.
        let msg_count = ctx.session.messages().len();
        if msg_count < 2 {
            ctx.reply(
                InlineMessageKind::Error,
                "Not enough conversation to hand off (need at least 2 messages).",
            );
            return SlashOutcome::Handled;
        }

        let opts = HandoffOptions {
            slug,
            auto_continue,
            dry_run,
        };

        let msg = if dry_run {
            "Generating handoff document (dry run)\u{2026}"
        } else if auto_continue {
            "Generating handoff and starting new session\u{2026}"
        } else {
            "Generating handoff document\u{2026}"
        };
        ctx.reply(InlineMessageKind::Info, msg);

        // Show a spinner while the LLM call runs (10-30s). The handle is
        // Clone (cheap Arc) and fire-and-forget — no awaiting the worker.
        let handle = ctx.handle.clone();
        handle.set_reasoning_stage(Some("Generating handoff\u{2026}".to_string()));
        let session = ctx.session.clone();

        tokio::spawn(async move {
            let result = generate_and_apply_handoff(&session, &opts).await;
            // Always clear the spinner, even on failure, so the footer
            // doesn't stay stuck on "Generating handoff".
            handle.set_reasoning_stage(None);
            match result {
                Ok(path) => tracing::info!(%path, "handoff complete"),
                Err(err) => {
                    tracing::warn!(%err, "handoff failed");
                    session.emit_handoff_failed(err.to_string());
                }
            }
        });

        SlashOutcome::Handled
    }
}

/// `/model` — inspect, set, or cycle the active model.
///   `/model`            show the current model
///   `/model <id>`       switch to `provider/model`
///   `/model next`       cycle to the next scoped model
struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &'static str {
        "model"
    }
    fn description(&self) -> &'static str {
        "Show or switch model (/model [<id>|next])"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        match args.trim() {
            "" => {
                let Some(catalog) = ctx.state.catalog.as_ref() else {
                    // Catalog never loaded — keep the existing read-only
                    // message so the user still gets *some* answer.
                    ctx.reply(
                        InlineMessageKind::Info,
                        format!("Current model: {}", ctx.session.model_id()),
                    );
                    return SlashOutcome::Handled;
                };

                let auth = crate::store::auth_storage::shared_auth_storage();
                let current = ctx.session.model_id();
                let (cur_provider, cur_model_id) = super::commands::split_model_id(&current);

                let (rows, used_fallback) =
                    model_picker_rows(catalog, &auth, cur_provider, cur_model_id);

                let keyed_provider_count = rows
                    .iter()
                    .filter(|e| auth.has(&e.provider))
                    .map(|e| e.provider.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                let filter_label = if used_fallback {
                    "Showing full catalog — no providers with keys configured yet".to_string()
                } else {
                    format!(
                        "Showing models from {keyed_provider_count} keyed provider{}",
                        if keyed_provider_count == 1 { "" } else { "s" },
                    )
                };

                ctx.state.overlay_model_ids = rows
                    .iter()
                    .map(|e| format!("{}/{}", e.provider, e.model_id))
                    .collect();

                let items: Vec<InlineListItem> = rows
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let id = format!("{}/{}", e.provider, e.model_id);
                        let mut sub = format!(
                            "{} \u{00b7} {} in / {} out",
                            super::commands::fmt_ctx(e.context_window),
                            super::commands::fmt_cost(e.cost_input),
                            super::commands::fmt_cost(e.cost_output),
                        );
                        if e.reasoning {
                            sub.push_str(" \u{00b7} reasoning");
                        }
                        if e.supports_vision {
                            sub.push_str(" \u{00b7} vision");
                        }
                        let badge = if id == current {
                            Some("active".to_string())
                        } else if used_fallback {
                            None
                        } else if !auth.has(&e.provider) {
                            Some("no-key".to_string())
                        } else {
                            None
                        };
                        InlineListItem {
                            title: id.clone(),
                            subtitle: Some(sub),
                            badge,
                            indent: 0,
                            selection: Some(InlineListSelection::Model(i)),
                            search_value: Some(
                                format!("{} {} {}", e.provider, e.model_id, e.name,),
                            ),
                        }
                    })
                    .collect();

                let total = items.len();
                let search = InlineListSearchConfig {
                    label: "Filter models".into(),
                    placeholder: Some("Type to filter (provider / model / name)\u{2026}".into()),
                };
                ctx.handle.show_list_modal(
                    format!("Models ({total})"),
                    vec![format!(
                        "{filter_label} \u{2014} Enter to switch, Esc to close"
                    )],
                    items,
                    None,
                    Some(search),
                );
            }
            "next" | "cycle" => match ctx.session.cycle_model() {
                Some(new_id) => {
                    crate::tui_vt::main_loop::sync_model_chips(ctx.state, ctx.session);
                    ctx.reply(InlineMessageKind::Info, format!("Switched to {new_id}"))
                }
                None => ctx.reply(
                    InlineMessageKind::Warning,
                    "No scoped models configured to cycle.",
                ),
            },
            id => match ctx.session.set_model(id) {
                Ok(()) => {
                    crate::tui_vt::main_loop::sync_model_chips(ctx.state, ctx.session);
                    ctx.reply(InlineMessageKind::Info, format!("Switched to {id}"))
                }
                Err(err) => ctx.reply(
                    InlineMessageKind::Error,
                    format!("Failed to set model {id}: {err}"),
                ),
            },
        }
        SlashOutcome::Handled
    }
}

/// Build the rows for the `/model` picker.
///
/// Rules:
/// 1. Models from every provider where `auth.has(p)` is true are
///    included (the current provider's models too — the user wants to
///    see every model they can call, including the other models from
///    their current provider).
/// 2. The active model is always present and pinned at index 0,
///    even if its provider has no key (e.g. key removed mid-session).
/// 3. If neither (1) nor (2) produces a row, fall back to the full
///    catalog and set `used_fallback = true` so the caller can drop
///    "no-key" badges (every row would be "no-key" in that state and
///    the footer already explains the fallback).
fn model_picker_rows(
    catalog: &std::sync::Arc<dyn oxicode_sdk::ports::catalog::ModelCatalog>,
    auth: &std::sync::Arc<crate::store::auth_storage::AuthStorage>,
    cur_provider: &str,
    cur_model_id: &str,
) -> (Vec<oxicode_sdk::CatalogModelEntry>, bool) {
    let all = catalog.search_sync("");

    // Models from every provider the user has a key for. The current
    // provider is NOT excluded — the user wants to see the other
    // models from their current provider, not just cross-provider
    // alternatives. The active model is deduplicated below.
    let mut keyed: Vec<_> = all
        .iter()
        .filter(|e| auth.has(&e.provider))
        .cloned()
        .collect();

    let current_entry = all
        .iter()
        .find(|e| e.provider == cur_provider && e.model_id == cur_model_id)
        .cloned();

    // Pin the active row at index 0. If the active model is already in
    // the keyed set (the normal case), drop the duplicate.
    let mut rows = Vec::with_capacity(keyed.len() + 1);
    if let Some(ce) = current_entry {
        keyed.retain(|e| !(e.provider == ce.provider && e.model_id == ce.model_id));
        rows.push(ce); // active row pinned to top
    }
    rows.append(&mut keyed);

    if rows.is_empty() {
        (all, true)
    } else {
        (rows, false)
    }
}
/// `/cancel` — abort any in-progress agent run. Alias: `/stop`.
struct CancelCommand;

impl SlashCommand for CancelCommand {
    fn name(&self) -> &'static str {
        "cancel"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["stop"]
    }
    fn description(&self) -> &'static str {
        "Abort the current run (alias: /stop)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let session = ctx.session.clone();
        tokio::spawn(async move {
            session.abort().await;
        });
        SlashOutcome::Handled
    }
}

/// `/status` — show the active model and session message counts.
struct StatusCommand;

impl SlashCommand for StatusCommand {
    fn name(&self) -> &'static str {
        "status"
    }
    fn description(&self) -> &'static str {
        "Show model and session stats"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let stats = ctx.session.session_stats();
        let model = ctx.session.model_id();
        let (provider, model_part) = super::commands::split_model_id(&model);
        let auth = crate::store::auth_storage::shared_auth_storage();
        let key = if auth.has(provider) { "set" } else { "missing" };
        let ctx_win = ctx
            .state
            .catalog
            .as_ref()
            .and_then(|c| c.get_model_sync(provider, model_part))
            .map(|e| super::commands::fmt_ctx(e.context_window))
            .unwrap_or_else(|| "?".to_string());
        let compaction = if ctx.session.auto_compaction_enabled() {
            "on"
        } else {
            "off"
        };
        let advisor = if ctx.session.is_advisor_enabled() {
            "on"
        } else {
            "off"
        };
        let thinking = ctx.session.thinking_level();
        ctx.reply(
            InlineMessageKind::Info,
            format!(
                "Model: {model}  (key: {key}, {ctx_win})\n\
                 Provider: {provider}  \u{00b7}  Thinking: {thinking:?}\n\
                 Compaction: {compaction}  \u{00b7}  Advisor: {advisor}\n\
                 Messages: {} user / {} assistant\n\
                 Tool calls: {} (results: {})\n\
                 Total: {}",
                stats.user_messages,
                stats.assistant_messages,
                stats.tool_calls,
                stats.tool_results,
                stats.total_messages,
            ),
        );
        SlashOutcome::Handled
    }
}

/// `/theme` — cycle, set, or pick a color theme.
///   `/theme`            cycle to the next theme
///   `/theme list`       open the theme picker overlay
///   `/theme <name>`     switch to a named theme
struct ThemeCommand;

impl SlashCommand for ThemeCommand {
    fn name(&self) -> &'static str {
        "theme"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["t"]
    }
    fn description(&self) -> &'static str {
        "Cycle or pick a color theme (/theme [name|list])"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        use oxicode_vtui::theme::{
            active_theme_id, available_themes, set_active_theme, theme_label,
        };
        match args.trim() {
            "" | "next" | "cycle" => {
                let themes = available_themes();
                if themes.len() <= 1 {
                    ctx.reply(InlineMessageKind::Info, "Only one theme available.");
                } else {
                    let current = active_theme_id();
                    let pos = themes.iter().position(|t| *t == current).unwrap_or(0);
                    let next_id = &themes[(pos + 1) % themes.len()];
                    match set_active_theme(next_id) {
                        Ok(()) => {
                            let label = theme_label(next_id).unwrap_or(next_id.as_ref());
                            ctx.reply(InlineMessageKind::Info, format!("Theme: {label}"));
                        }
                        Err(e) => ctx.reply(
                            InlineMessageKind::Error,
                            format!("Failed to set theme: {e}"),
                        ),
                    }
                }
            }
            "list" | "picker" => {
                let themes = available_themes();
                let current = active_theme_id();
                let items: Vec<InlineListItem> = themes
                    .iter()
                    .map(|id| InlineListItem {
                        title: theme_label(id).unwrap_or(id.as_ref()).to_string(),
                        subtitle: Some(id.to_string()),
                        badge: if *id == current {
                            Some("active".to_string())
                        } else {
                            None
                        },
                        indent: 0,
                        selection: Some(InlineListSelection::Theme(id.to_string())),
                        search_value: Some(id.to_string()),
                    })
                    .collect();
                ctx.handle.show_list_modal(
                    "Themes".to_string(),
                    vec!["Select a theme (Esc to close, Enter to apply)".to_string()],
                    items,
                    None,
                    None,
                );
            }
            name => match set_active_theme(name) {
                Ok(()) => {
                    let label = theme_label(name).unwrap_or(name);
                    ctx.reply(InlineMessageKind::Info, format!("Theme: {label}"));
                }
                Err(e) => ctx.reply(
                    InlineMessageKind::Error,
                    format!("Unknown theme '{name}': {e}"),
                ),
            },
        }
        SlashOutcome::Handled
    }
}

/// `/find` — search within the transcript. Opens an inline search bar.
///   `/find <query>`   search for matches (n/N to navigate)
///   `/find`           clear search
struct FindCommand;

impl SlashCommand for FindCommand {
    fn name(&self) -> &'static str {
        "find"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["search", "/"]
    }
    fn description(&self) -> &'static str {
        "Search transcript (/find <query>, n/N to navigate)"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let query = args.trim();
        if query.is_empty() {
            ctx.state.search = None;
            ctx.reply(InlineMessageKind::Info, "Search cleared.");
        } else {
            ctx.state.start_search(query);
            let count = ctx
                .state
                .search
                .as_ref()
                .map(|s| s.matches.len())
                .unwrap_or(0);
            if count == 0 {
                ctx.reply(
                    InlineMessageKind::Warning,
                    format!("No matches for '{query}'."),
                );
            } else {
                ctx.reply(
                    InlineMessageKind::Info,
                    format!(
                        "{count} match{} for '{query}'",
                        if count == 1 { "" } else { "es" }
                    ),
                );
            }
        }
        SlashOutcome::Handled
    }
}

/// `/shortcuts` — show the keyboard shortcuts cheatsheet overlay.
struct ShortcutsCommand;

impl SlashCommand for ShortcutsCommand {
    fn name(&self) -> &'static str {
        "shortcuts"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["keys", "cheatsheet"]
    }
    fn description(&self) -> &'static str {
        "Show keyboard shortcuts (alias: ?)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.handle
            .show_modal("Keyboard Shortcuts".to_string(), shortcuts_lines(), None);
        SlashOutcome::Handled
    }
}

/// Lines for the shortcuts cheatsheet overlay.
fn shortcuts_lines() -> Vec<String> {
    vec![
        "".into(),
        "  Navigation".into(),
        "  j / ↓      Scroll down (line)".into(),
        "  k / ↑      Scroll up (line)".into(),
        "  Shift+J    Next assistant turn".into(),
        "  Shift+K    Previous user turn".into(),
        "  PgDn       Scroll down (page)".into(),
        "  PgUp       Scroll up (page)".into(),
        "  G          Jump to bottom (follow)".into(),
        "  g          Jump to top".into(),
        "".into(),
        "  Blocks".into(),
        "  e          Cycle block (collapse/truncate/expand)".into(),
        "  Shift+E    Expand all blocks".into(),
        "  Ctrl+E     Collapse all blocks".into(),
        "".into(),
        "  Search".into(),
        "  /find <q>  Search transcript".into(),
        "  n          Next match".into(),
        "  N          Previous match".into(),
        "  Esc        Clear search".into(),
        "".into(),
        "  Input".into(),
        "  Ctrl+M     Toggle multiline input".into(),
        "  Ctrl+P     Command palette".into(),
        "  Ctrl+Enter Send now (abort + submit)".into(),
        "  Esc        Cancel run / quit (y to confirm)".into(),
        "".into(),
        "  Other".into(),
        "  ?          Show this cheatsheet".into(),
        "  /theme     Cycle color theme".into(),
        "  /model     Pick a model".into(),
        "  /models    Browse all models".into(),
        "  /providers Manage API keys".into(),
        "  /tools     List tools".into(),
        "  /mcp       MCP status".into(),
        "  /info      Diagnostics".into(),
        "  /export    Save as HTML".into(),
        "  /vim       Toggle vim mode".into(),
        "  Ctrl+C     Cancel run (then y to quit)".into(),
        "".into(),
    ]
}
// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_sdk::ports::catalog::{
        CatalogEvent, CatalogModelEntry, CatalogProtocol, CatalogSource, ModelCatalog,
    };
    use oxicode_sdk::{Model, Provider, ProviderError, ProviderEvent};
    use oxicode_vtui::tui::core::{InlineCommand, OverlayRequest};
    use std::pin::Pin;
    use std::task::{Context as TaskContext, Poll};

    #[test]
    fn builtins_register_expected_commands() {
        let reg = SlashRegistry::builtins();
        let names: Vec<&str> = reg.builtins.iter().map(|c| c.name()).collect();
        assert!(names.contains(&"quit"));
        assert!(names.contains(&"clear"));
        assert!(names.contains(&"memory"));
        assert!(names.contains(&"model"));
        assert!(names.contains(&"cancel"));
        assert!(names.contains(&"status"));
    }

    #[test]
    fn matches_resolves_aliases_case_insensitively() {
        let cmd = QuitCommand;
        assert!(cmd.matches("quit"));
        assert!(cmd.matches("EXIT"));
        assert!(cmd.matches("q"));
        assert!(!cmd.matches("quitter"));
    }

    #[test]
    fn builtin_commands_exposes_aliases_for_rpc() {
        let catalog = SlashRegistry::builtin_commands();
        let quit = catalog
            .iter()
            .find(|(name, _, _)| *name == "quit")
            .expect("quit command present");
        assert!(quit.2.contains(&"exit"));
        assert!(quit.2.contains(&"q"));
        assert!(!quit.1.is_empty(), "quit has a description");
    }

    #[test]
    fn handoff_command_metadata() {
        // /handoff must be registered and exposed via /help + RPC. Aliases
        // let power users type /hd instead.
        let reg = SlashRegistry::builtins();
        let names: Vec<&str> = reg.builtins.iter().map(|c| c.name()).collect();
        assert!(names.contains(&"handoff"), "handoff command registered");

        let catalog = SlashRegistry::builtin_commands();
        let handoff = catalog
            .iter()
            .find(|(name, _, _)| *name == "handoff")
            .expect("handoff present in catalog");
        assert!(
            handoff.2.contains(&"hd"),
            "handoff exposes /hd alias: got {:?}",
            handoff.2
        );
        assert!(!handoff.1.is_empty(), "handoff has a description");

        let cmd = HandoffCommand;
        assert!(cmd.matches("handoff"));
        assert!(cmd.matches("HD"));
        assert!(cmd.matches("Hd"));
    }

    /// `model_picker_rows` filters the catalog to providers with stored
    /// API keys, pins the active model at index 0, and falls back to the
    /// full catalog when nothing is keyed.
    #[test]
    fn model_picker_filters_by_keyed_providers() {
        use crate::store::auth_storage::AuthStorage;
        use std::sync::Arc;

        // Two providers, three models. The `anthropic` provider has a
        // key below; `google` does not.
        let entries = vec![
            CatalogModelEntry {
                provider: "anthropic".into(),
                model_id: "claude-sonnet".into(),
                name: "Claude Sonnet".into(),
                protocol: CatalogProtocol::AnthropicMessages,
                source: CatalogSource::Embedded,
                base_url: None,
                reasoning: false,
                supports_vision: true,
                cost_input: 3.0,
                cost_output: 15.0,
                cost_cache_read: 0.0,
                cost_cache_write: 0.0,
                context_window: 200_000,
                max_tokens: 8_192,
                input_modalities: vec!["text".into(), "image".into()],
                release_date: None,
                status: None,
            },
            CatalogModelEntry {
                provider: "anthropic".into(),
                model_id: "claude-opus".into(),
                name: "Claude Opus".into(),
                protocol: CatalogProtocol::AnthropicMessages,
                source: CatalogSource::Embedded,
                base_url: None,
                reasoning: false,
                supports_vision: true,
                cost_input: 15.0,
                cost_output: 75.0,
                cost_cache_read: 0.0,
                cost_cache_write: 0.0,
                context_window: 200_000,
                max_tokens: 8_192,
                input_modalities: vec!["text".into(), "image".into()],
                release_date: None,
                status: None,
            },
            CatalogModelEntry {
                provider: "google".into(),
                model_id: "gemini-2.5-pro".into(),
                name: "Gemini 2.5 Pro".into(),
                protocol: CatalogProtocol::OpenAiCompatible,
                source: CatalogSource::Embedded,
                base_url: None,
                reasoning: true,
                supports_vision: true,
                cost_input: 1.25,
                cost_output: 5.0,
                cost_cache_read: 0.0,
                cost_cache_write: 0.0,
                context_window: 1_000_000,
                max_tokens: 65_536,
                input_modalities: vec!["text".into(), "image".into()],
                release_date: None,
                status: None,
            },
        ];
        let catalog: Arc<dyn ModelCatalog> = Arc::new(StaticCatalog::new(entries));

        // Hermetic in-memory AuthStorage — no file I/O, no risk of
        // touching the user's real ~/.oxicode/auth.json. The production
        // `AuthStorage::default()` would point at that file and any
        // `set_api_key` would persist there. Always use `in_memory()`
        // in tests.
        let auth: Arc<AuthStorage> = Arc::new(AuthStorage::in_memory());
        auth.set_api_key("anthropic", "test-anthropic-key".to_string());

        let (rows, used_fallback) =
            model_picker_rows(&catalog, &auth, "anthropic", "claude-sonnet");

        assert!(
            !used_fallback,
            "keyed providers exist — no fallback expected"
        );
        // Active row pinned to index 0, the other keyed provider row
        // follows. google's models are excluded (no key).
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].provider, "anthropic");
        assert_eq!(rows[0].model_id, "claude-sonnet");
        assert_eq!(rows[1].model_id, "claude-opus");
    }
    /// When no providers are keyed AND there is no active model match
    /// in the catalog, the helper returns the full catalog with
    /// `used_fallback = true`.
    #[test]
    fn model_picker_falls_back_when_unkeyed_and_no_active_match() {
        use crate::store::auth_storage::AuthStorage;
        use std::sync::Arc;

        let entries = vec![CatalogModelEntry {
            provider: "openai".into(),
            model_id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            protocol: CatalogProtocol::OpenAiCompatible,
            source: CatalogSource::Embedded,
            base_url: None,
            reasoning: false,
            supports_vision: true,
            cost_input: 2.5,
            cost_output: 10.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            context_window: 128_000,
            max_tokens: 16_384,
            input_modalities: vec!["text".into(), "image".into()],
            release_date: None,
            status: None,
        }];
        let catalog: Arc<dyn ModelCatalog> = Arc::new(StaticCatalog::new(entries));

        // No keys at all.
        let auth: Arc<AuthStorage> = Arc::new(AuthStorage::in_memory());
        // Active model id is not in the catalog (impossible in practice
        // but the helper must handle it).
        let (rows, used_fallback) =
            model_picker_rows(&catalog, &auth, "anthropic", "claude-not-in-catalog");

        assert!(
            used_fallback,
            "no keys, no active match — fallback expected"
        );
        // The full catalog is returned.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model_id, "gpt-4o");
    }

    /// In-memory `ModelCatalog` test double holding a fixed list of
    /// entries. Only `search_sync` is exercised by `model_picker_rows`;
    /// the async methods are stubbed to keep the trait satisfied and
    /// are never invoked by the helper.
    struct StaticCatalog {
        entries: Vec<CatalogModelEntry>,
        tx: tokio::sync::broadcast::Sender<CatalogEvent>,
    }

    impl StaticCatalog {
        fn new(entries: Vec<CatalogModelEntry>) -> Self {
            let (tx, _) = tokio::sync::broadcast::channel(16);
            Self { entries, tx }
        }
    }

    impl std::fmt::Debug for StaticCatalog {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StaticCatalog")
                .field("entries", &self.entries.len())
                .finish_non_exhaustive()
        }
    }

    impl ModelCatalog for StaticCatalog {
        fn list_providers(
            &self,
        ) -> Pin<Box<dyn Future<Output = oxicode_sdk::SdkResult<Vec<String>>> + Send + '_>>
        {
            let mut providers: Vec<String> =
                self.entries.iter().map(|e| e.provider.clone()).collect();
            providers.sort();
            providers.dedup();
            Box::pin(async move { Ok(providers) })
        }
        fn get_provider(
            &self,
            _id: &str,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = oxicode_sdk::SdkResult<Option<oxicode_sdk::CatalogProviderEntry>>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(None) })
        }
        fn list_models(
            &self,
            provider_id: &str,
        ) -> Pin<Box<dyn Future<Output = oxicode_sdk::SdkResult<Vec<CatalogModelEntry>>> + Send + '_>>
        {
            let v: Vec<_> = self
                .entries
                .iter()
                .filter(|e| e.provider == provider_id)
                .cloned()
                .collect();
            Box::pin(async move { Ok(v) })
        }
        fn get_model(
            &self,
            provider: &str,
            model_id: &str,
        ) -> Pin<
            Box<dyn Future<Output = oxicode_sdk::SdkResult<Option<CatalogModelEntry>>> + Send + '_>,
        > {
            let hit = self.entries.iter().find_map(|e| {
                if e.provider == provider && e.model_id == model_id {
                    Some(e.clone())
                } else {
                    None
                }
            });
            Box::pin(async move { Ok(hit) })
        }
        fn search(
            &self,
            _pattern: &str,
        ) -> Pin<Box<dyn Future<Output = oxicode_sdk::SdkResult<Vec<CatalogModelEntry>>> + Send + '_>>
        {
            let v = self.entries.clone();
            Box::pin(async move { Ok(v) })
        }
        fn model_count(
            &self,
        ) -> Pin<Box<dyn Future<Output = oxicode_sdk::SdkResult<usize>> + Send + '_>> {
            let n = self.entries.len();
            Box::pin(async move { Ok(n) })
        }
        fn refresh(
            &self,
        ) -> Pin<
            Box<
                dyn Future<Output = oxicode_sdk::SdkResult<oxicode_sdk::RefreshOutcome>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(oxicode_sdk::RefreshOutcome::Unchanged) })
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CatalogEvent> {
            self.tx.subscribe()
        }
        fn search_sync(&self, _pattern: &str) -> Vec<CatalogModelEntry> {
            self.entries.clone()
        }
    }

    // ── /settings ────────────────────────────────────────────────────

    /// `/settings` items are built from the `SETTING_DEFS` General tab:
    /// a heading row at every group change, then one row per def in
    /// table order with the per-widget selection mapping.
    #[test]
    fn settings_items_mirror_general_tab_defs() {
        let settings = Settings::default();
        let defs = defs_for_tab(SettingsTab::General, &settings);
        assert!(!defs.is_empty(), "General tab must define settings");

        let items = settings_overlay_items(SettingsTab::General, &settings).0;
        assert!(!items.is_empty());

        // Walk the emitted items against the table: heading rows (no
        // subtitle/badge) must announce the next def's group and only
        // when the group changed; setting rows must match the next def
        // exactly. This pins both order and group contiguity.
        let mut next_def = 0usize;
        let mut last_group: Option<&str> = None;
        let mut heading_count = 0usize;
        for item in &items {
            if item.subtitle.is_none() && item.badge.is_none() {
                assert!(next_def < defs.len(), "heading with no def to follow");
                let def = defs[next_def];
                assert_eq!(item.title, def.group, "heading carries the group name");
                assert!(item.selection.is_none(), "headings are never interactive");
                assert_ne!(
                    last_group,
                    Some(def.group),
                    "heading emitted without a group change"
                );
                last_group = Some(def.group);
                heading_count += 1;
            } else {
                let def = defs[next_def];
                assert_eq!(item.title, def.label, "rows follow SETTING_DEFS order");
                assert_eq!(item.subtitle.as_deref(), Some(def.description));
                assert_eq!(
                    item.badge.as_deref(),
                    Some(get_display_value(def.key, &settings).as_str())
                );
                let expected = match def.widget {
                    SettingWidget::Toggle | SettingWidget::Cycle => {
                        Some(InlineListSelection::ConfigAction(format!("{:?}", def.key)))
                    }
                    SettingWidget::Text => Some(InlineListSelection::SettingTextEdit(format!(
                        "{:?}",
                        def.key
                    ))),
                    SettingWidget::SubmenuSelect(_) => Some(
                        InlineListSelection::SettingSubmenuOpen(format!("{:?}", def.key)),
                    ),
                    SettingWidget::Multiselect => Some(InlineListSelection::SettingMultiselect(
                        format!("{:?}", def.key),
                    )),
                    SettingWidget::MapEditor | SettingWidget::Pointer => None,
                };
                assert_eq!(item.selection.as_ref(), expected.as_ref());
                assert!(
                    item.search_value
                        .as_deref()
                        .is_some_and(|v| v.contains(def.label))
                );
                next_def += 1;
            }
        }
        assert_eq!(next_def, defs.len(), "exactly one row per General-tab def");
        assert!(heading_count > 0, "at least one group heading emitted");
    }

    /// Minimal provider double so a real `AgentSessionHandle` can back
    /// the `SlashCtx` — `/settings` itself never calls the model.
    struct NullProvider;

    struct EmptyStream;

    impl futures::Stream for EmptyStream {
        type Item = ProviderEvent;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(None)
        }
    }

    impl Provider for NullProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a oxicode_sdk::Context,
            _options: Option<oxicode_sdk::StreamOptions>,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
                            ProviderError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                Ok::<_, ProviderError>(Box::pin(EmptyStream)
                    as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
            })
        }
    }

    /// Dispatching `/settings` opens a "Settings" list overlay whose
    /// items are exactly the table-built General-tab rows.
    #[test]
    fn settings_command_opens_table_driven_overlay() {
        use crate::app::agent_session::AgentSession;
        use crate::store::session::SessionManager;
        use oxicode_agent::{Agent, AgentConfig, ToolRegistry};

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<InlineCommand>();
        let handle = InlineHandle::new_for_tests(cmd_tx);

        let session = AgentSession::new(
            std::sync::Arc::new(Agent::new(
                std::sync::Arc::new(NullProvider),
                AgentConfig::new("anthropic/claude-sonnet-4-20250514"),
                std::sync::Arc::new(ToolRegistry::new()),
            )),
            Settings::default(),
            SessionManager::in_memory("/tmp/test"),
            "/tmp/test".to_string(),
            crate::SessionState::default(),
        )
        .clone_handle();

        let mut state = RenderState::default();
        let mut ctx = SlashCtx {
            session: &session,
            handle: &handle,
            state: &mut state,
        };
        assert!(matches!(
            SlashRegistry::builtins().dispatch("/settings", &mut ctx),
            SlashOutcome::Handled
        ));

        // Drain the command channel for the overlay-open command.
        let list = loop {
            match cmd_rx.try_recv().expect("overlay command emitted") {
                InlineCommand::ShowOverlay { request } => match *request {
                    OverlayRequest::List(list) => break list,
                    _ => panic!("expected a list overlay request"),
                },
                _ => continue,
            }
        };
        assert_eq!(list.title, "Settings");
        assert!(list.search.is_some(), "settings overlay is filterable");

        // `execute` loads settings from disk, so badge VALUES may differ
        // from `Settings::default()` — but structure is table-determined.
        // Assert the row titles mirror the General defs in order, with a
        // heading row present.
        let defs = defs_for_tab(SettingsTab::General, &Settings::default());
        let rows: Vec<&str> = list
            .items
            .iter()
            .filter(|i| i.subtitle.is_some())
            .map(|i| i.title.as_str())
            .collect();
        let labels: Vec<&str> = defs.iter().map(|d| d.label).collect();
        assert_eq!(rows, labels, "overlay rows mirror the General tab defs");
        assert!(
            list.items
                .iter()
                .any(|i| i.subtitle.is_none() && i.badge.is_none()),
            "at least one group heading row present"
        );
    }
}
