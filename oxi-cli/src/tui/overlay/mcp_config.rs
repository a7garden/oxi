//! Interactive MCP server management overlay.
//!
//! Opened by the `/mcp` slash command. Unlike the read-only runtime
//! [`McpDashboardOverlay`](super::mcp_dashboard::McpDashboardOverlay),
//! this overlay lets you **add, edit, and remove** MCP servers, then
//! persist the change to a config file on disk and hot-reload the
//! running [`McpManager`].
//!
//! # Modes
//!
//! - [`Mode::List`] — browse servers in the target file with live
//!   connection status; add / edit / remove / reconnect / save.
//! - [`Mode::Edit`] — form editor for a single server (command, args,
//!   url, lifecycle, idle timeout, direct-tools).
//! - [`Mode::ConfirmRemove`] — confirm-before-delete prompt.
//! - [`Mode::QuickAdd`] — curated preset gallery. When the server list
//!   is empty, the overlay opens here by default so the user lands one
//!   keystroke away from a working config.
//!
//! # Config scope
//!
//! Writes target a single file, toggled with `Tab`:
//! - **Global** — `~/.config/oxi/mcp.json` (user-wide)
//! - **Project** — `<cwd>/.oxi/mcp.json` (shared via the config loader)
//!
//! On save the file is written atomically (temp + rename) and the new
//! [`McpConfig`] is pushed into the live manager via
//! [`McpManager::replace_config`], so newly added servers are reachable
//! through the `mcp` proxy tool without a restart. Direct-tool
//! registration still requires a restart (it runs once at boot).

use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_agent::mcp::{
    ConsentState, LifecycleMode, McpConfig, McpConnectionStatus, McpManager, ServerEntry,
    ToolPrefix, config,
};
use oxi_tui_legacy::{Theme, ThemeStyles};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use super::{OverlayAction, OverlayComponent, centered_layout};

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Which config file the overlay edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// User-wide: `~/.config/oxi/mcp.json`.
    Global,
    /// Project-local: `<cwd>/.oxi/mcp.json`.
    Project,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Scope::Global => "Global",
            Scope::Project => "Project",
        }
    }
}

// ---------------------------------------------------------------------------
// Editable server form model
// ---------------------------------------------------------------------------

/// Choice field backed lifecycle selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleField {
    Lazy,
    Eager,
    KeepAlive,
}

impl LifecycleField {
    const ALL: [Self; 3] = [Self::Lazy, Self::Eager, Self::KeepAlive];
    fn as_str(self) -> &'static str {
        match self {
            Self::Lazy => "lazy",
            Self::Eager => "eager",
            Self::KeepAlive => "keep-alive",
        }
    }
    fn to_mode(self) -> LifecycleMode {
        match self {
            Self::Lazy => LifecycleMode::Lazy,
            Self::Eager => LifecycleMode::Eager,
            Self::KeepAlive => LifecycleMode::KeepAlive,
        }
    }
    fn from_entry(entry: &ServerEntry) -> Self {
        match entry.lifecycle {
            Some(LifecycleMode::Eager) => Self::Eager,
            Some(LifecycleMode::KeepAlive) => Self::KeepAlive,
            _ => Self::Lazy,
        }
    }
}

/// Direct-tools selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectField {
    Off,
    All,
}

impl DirectField {
    const ALL: [Self; 2] = [Self::Off, Self::All];
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
        }
    }
    fn from_entry(entry: &ServerEntry) -> Self {
        match &entry.direct_tools {
            Some(oxi_agent::mcp::DirectToolsConfig::All(true)) => Self::All,
            _ => Self::Off,
        }
    }
    fn apply(self, entry: &mut ServerEntry) {
        match self {
            Self::Off => {
                entry.direct_tools = None;
            }
            Self::All => {
                entry.direct_tools = Some(oxi_agent::mcp::DirectToolsConfig::All(true));
            }
        }
    }
}

/// Which form field has focus in [`Mode::Edit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Transport,
    Command,
    Args,
    Url,
    Lifecycle,
    IdleTimeout,
    DirectTools,
    Env,
    Save,
    Cancel,
}

impl Field {
    /// Navigable order. `Transport` always sits right under `Name` so
    /// the user picks the transport first, then sees only the relevant
    /// fields for it. The `Env` slot doubles as the HTTP `Headers`
    /// editor; stdio shows command/args, http shows url.
    fn order(transport: Transport) -> &'static [Field] {
        match transport {
            Transport::Stdio => &[
                Field::Name,
                Field::Transport,
                Field::Command,
                Field::Args,
                Field::Lifecycle,
                Field::IdleTimeout,
                Field::DirectTools,
                Field::Env,
                Field::Save,
                Field::Cancel,
            ],
            Transport::Http => &[
                Field::Name,
                Field::Transport,
                Field::Url,
                Field::Lifecycle,
                Field::IdleTimeout,
                Field::DirectTools,
                Field::Env,
                Field::Save,
                Field::Cancel,
            ],
        }
    }
}

/// Transport inferred from which fields are populated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Stdio,
    Http,
}

/// A server being edited, stored as flat editable strings.
#[derive(Debug, Clone)]
struct EditableServer {
    /// Original name — `None` when adding a brand-new server.
    original_name: Option<String>,
    name: String,
    /// Explicitly chosen transport (not auto-detected), so the field
    /// layout is stable while editing and the user can reach the URL
    /// field without first filling in `command`.
    transport: Transport,
    command: String,
    args: String,
    url: String,
    env: String,
    lifecycle: LifecycleField,
    idle_timeout: String,
    direct_tools: DirectField,
}

impl EditableServer {
    fn empty() -> Self {
        Self {
            original_name: None,
            name: String::new(),
            transport: Transport::Stdio,
            command: String::new(),
            args: String::new(),
            url: String::new(),
            env: String::new(),
            lifecycle: LifecycleField::Lazy,
            idle_timeout: String::new(),
            direct_tools: DirectField::Off,
        }
    }

    fn from_stored(name: &str, entry: &ServerEntry) -> Self {
        let command = entry.command.clone().unwrap_or_default();
        let args = entry.args.clone().unwrap_or_default().join(" ");
        let url = entry.url.clone().unwrap_or_default();
        let env = entry
            .env
            .as_ref()
            .map(|m| {
                let mut pairs: Vec<String> = m.iter().map(|(k, v)| format!("{k}={v}")).collect();
                pairs.sort();
                pairs.join(", ")
            })
            .unwrap_or_default();
        // Infer transport from the stored entry: a URL means HTTP,
        // everything else (including a bare command) is stdio.
        let transport = if entry.url.is_some() {
            Transport::Http
        } else {
            Transport::Stdio
        };
        Self {
            original_name: Some(name.to_string()),
            name: name.to_string(),
            transport,
            command,
            args,
            url,
            env,
            lifecycle: LifecycleField::from_entry(entry),
            idle_timeout: entry
                .idle_timeout
                .map(|m| m.to_string())
                .unwrap_or_default(),
            direct_tools: DirectField::from_entry(entry),
        }
    }

    /// Build an EditableServer from a Quick Add preset. Behaves like
    /// `from_stored` for the entry fields, but uses the preset's name as
    /// the form default and clears `original_name` (it's a new server).
    ///
    /// `env_placeholder` is dropped into the env field as a hint so the
    /// user can see what variables they need to provide. It's not a
    /// valid value — `build()` will then either ignore it (no `=`) or
    /// require the user to replace it with a real value.
    fn from_preset(preset: &super::mcp_presets::McpPreset) -> Self {
        let entry = &preset.entry;
        let command = entry.command.clone().unwrap_or_default();
        let args = entry.args.clone().unwrap_or_default().join(" ");
        let url = entry.url.clone().unwrap_or_default();
        // If the preset advertises a placeholder (e.g. "API_KEY=…"),
        // seed the env field with that text so the user can see what
        // they need to fill in. Real values would be supplied by the
        // user before they hit Save.
        let env = preset.env_placeholder.unwrap_or_default().to_string();
        let transport = if entry.url.is_some() {
            Transport::Http
        } else {
            Transport::Stdio
        };
        Self {
            original_name: None,
            name: preset.name.to_string(),
            transport,
            command,
            args,
            url,
            env,
            lifecycle: LifecycleField::from_entry(entry),
            idle_timeout: entry
                .idle_timeout
                .map(|m| m.to_string())
                .unwrap_or_default(),
            direct_tools: DirectField::from_entry(entry),
        }
    }

    fn transport(&self) -> Transport {
        self.transport
    }

    /// Validate and convert into a `(name, ServerEntry)`. Returns
    /// `Err(message)` if the form is invalid.
    fn build(&self) -> Result<(String, ServerEntry), String> {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return Err("Server name is required.".into());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Name may only contain letters, digits, '-' and '_'.".into());
        }

        let mut entry = ServerEntry::default();

        match self.transport() {
            Transport::Stdio => {
                let cmd = self.command.trim().to_string();
                if cmd.is_empty() {
                    return Err("A `command` is required for stdio transport.".into());
                }
                entry.command = Some(cmd);
                let args: Vec<String> = shell_split(&self.args);
                if !args.is_empty() {
                    entry.args = Some(args);
                }
            }
            Transport::Http => {
                let url = self.url.trim().to_string();
                if url.is_empty() {
                    return Err("A `url` is required for HTTP transport.".into());
                }
                entry.url = Some(url);
            }
        }

        // env / headers
        let map = parse_kv(&self.env);
        if !map.is_empty() {
            match self.transport() {
                Transport::Stdio => entry.env = Some(map),
                Transport::Http => entry.headers = Some(map),
            }
        }

        entry.lifecycle = Some(self.lifecycle.to_mode());
        if let Ok(mins) = self.idle_timeout.trim().parse::<u64>() {
            entry.idle_timeout = Some(mins);
        }
        self.direct_tools.apply(&mut entry);

        Ok((name, entry))
    }
}

/// Minimal shell-like splitter: splits on whitespace but honors double
/// quotes. Falls back to whitespace splitting for v1 simplicity.
fn shell_split(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;
    for ch in input.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            c => buf.push(c),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Parse `KEY=VAL, KEY2=VAL2` (comma and/or newline separated) into a map.
fn parse_kv(input: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for part in input.split([',', '\n']) {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                map.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Mode {
    /// Browsing the server list.
    List,
    /// Editing (or adding) a server.
    Edit {
        server: EditableServer,
        field: Field,
        error: Option<String>,
    },
    /// Confirming a destructive removal.
    ConfirmRemove { name: String },
    /// Browsing the curated Quick Add gallery. Pressing `Enter` on a
    /// preset transitions to `Edit` with the preset's fields pre-filled,
    /// so the user can confirm / tweak before saving.
    QuickAdd {
        selected: usize,
        list_state: ListState,
    },
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

/// The interactive MCP management overlay.
pub struct McpConfigOverlay {
    /// The live manager — used for live connection status and hot-reload.
    /// `None` is allowed for defensive operation: the overlay can still
    /// read/write the on-disk config, just without live status or
    /// hot-reload. This is rare in practice (the bootstrap normally wires
    /// it) but it keeps `/mcp` functional even if the manager is missing.
    manager: Option<Arc<McpManager>>,
    cwd: PathBuf,
    scope: Scope,
    /// The editable document for the target file (loaded fresh on
    /// construction and on every scope change).
    doc: McpConfig,
    selected: usize,
    list_state: ListState,
    mode: Mode,
    /// Unsaved edits exist in `doc` (relative to what's on disk).
    dirty: bool,
    /// Set on the first Esc/Tab-discard when `dirty`. A second Esc then
    /// actually closes/discards. Reset whenever the doc changes.
    discard_guard: bool,
    /// Status message shown at the bottom of the list view.
    notice: Option<(String, NoticeTone)>,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
enum NoticeTone {
    Success,
    Warning,
    Error,
    Info,
}

impl std::fmt::Debug for McpConfigOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConfigOverlay")
            .field("scope", &self.scope)
            .field("servers", &self.doc.mcp_servers.len())
            .field("selected", &self.selected)
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl McpConfigOverlay {
    /// Build the overlay, loading the current global config document.
    ///
    /// `manager` is the live runtime manager used for hot-reload and
    /// status display. It is `Option` so the overlay remains usable even
    /// in the rare case the bootstrap didn't wire it (e.g. a tool
    /// registry was assembled manually). When `None`, the overlay can
    /// still read / write the config file but skips live status and
    /// hot-reload.
    pub fn new(manager: Option<Arc<McpManager>>, cwd: PathBuf) -> Self {
        let scope = Scope::Global;
        let path = target_path(scope, &cwd);
        let doc = config::load_or_default(&path);
        let mut overlay = Self {
            manager,
            cwd,
            scope,
            doc,
            selected: 0,
            list_state: ListState::default(),
            mode: Mode::List,
            dirty: false,
            discard_guard: false,
            notice: None,
        };
        // When there are no servers configured yet, the user is most
        // likely here to add their first one. Drop them straight into
        // the Quick Add gallery so the path of least resistance is
        // one keystroke away (no need to read the empty-state hint).
        if overlay.server_names().is_empty() {
            overlay.enter_quick_add();
        }
        overlay
    }

    fn target_path(&self) -> PathBuf {
        target_path(self.scope, &self.cwd)
    }

    /// Sorted server names in the document (stable display order).
    fn server_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.doc.mcp_servers.keys().cloned().collect();
        names.sort();
        names
    }

    #[allow(dead_code)]
    fn reload_doc(&mut self) {
        let path = self.target_path();
        self.doc = config::load_or_default(&path);
        self.dirty = false;
        self.selected = self
            .selected
            .min(self.server_names().len().saturating_sub(1));
    }

    fn switch_scope(&mut self) {
        // Guard against silently discarding unsaved edits in the current scope.
        if self.dirty && !self.discard_guard {
            self.discard_guard = true;
            self.notice(
                "Unsaved changes in this scope — press s to save, or Tab again to discard.",
                NoticeTone::Warning,
            );
            return;
        }
        self.scope = match self.scope {
            Scope::Global => Scope::Project,
            Scope::Project => Scope::Global,
        };
        let path = self.target_path();
        self.doc = config::load_or_default(&path);
        self.dirty = false;
        self.discard_guard = false;
        self.selected = 0;
        self.notice = Some((
            format!("Scope: {} — {}", self.scope.label(), path.display()),
            NoticeTone::Info,
        ));
    }

    fn notice(&mut self, msg: impl Into<String>, tone: NoticeTone) {
        self.notice = Some((msg.into(), tone));
    }

    // ── List-mode actions ───────────────────────────────────────────

    fn begin_add(&mut self) {
        self.mode = Mode::Edit {
            server: EditableServer::empty(),
            field: Field::Name,
            error: None,
        };
    }

    fn begin_edit(&mut self) {
        let names = self.server_names();
        if let Some(name) = names.get(self.selected)
            && let Some(entry) = self.doc.mcp_servers.get(name)
        {
            let server = EditableServer::from_stored(name, entry);
            self.mode = Mode::Edit {
                server,
                field: Field::Name,
                error: None,
            };
        }
    }

    fn begin_remove(&mut self) {
        let names = self.server_names();
        if let Some(name) = names.get(self.selected).cloned() {
            self.mode = Mode::ConfirmRemove { name };
        }
    }

    /// Open the Quick Add gallery. Called from List mode (via `q` /
    /// `a`-when-empty) and from `new()` when the server list is empty.
    fn enter_quick_add(&mut self) {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        self.mode = Mode::QuickAdd {
            selected: 0,
            list_state,
        };
    }

    /// Apply the currently-selected preset to the Edit form.
    /// Returns `true` if a preset was applied.
    fn apply_preset(&mut self) -> bool {
        let idx = match &self.mode {
            Mode::QuickAdd { selected, .. } => *selected,
            _ => return false,
        };
        let Some(preset) = super::mcp_presets::presets().get(idx) else {
            return false;
        };
        let server = EditableServer::from_preset(preset);
        self.mode = Mode::Edit {
            server,
            field: Field::Name,
            error: None,
        };
        true
    }

    fn commit_remove(&mut self, name: &str) {
        if self.doc.mcp_servers.remove(name).is_some() {
            self.dirty = true;
            self.discard_guard = false;
            self.notice(format!("Removed '{name}' (unsaved)"), NoticeTone::Warning);
        }
        let count = self.server_names().len();
        if self.selected >= count && count > 0 {
            self.selected = count - 1;
        }
        self.mode = Mode::List;
    }

    /// Persist `doc` to the target file and hot-reload the manager.
    fn save_and_apply(&mut self) -> OverlayAction {
        let path = self.target_path();
        // Merge: the live manager sees the union of all config files, but
        // for hot-reload we re-read the full merged config from disk so
        // edits in *this* file compose with any other files the user has.
        match config::save_mcp_config(&path, &self.doc) {
            Ok(()) => {
                self.dirty = false;
                self.discard_guard = false;
                let merged = config::load_mcp_config_from(&self.cwd);
                let n = merged.mcp_servers.len();
                OverlayAction::McpConfigApplied {
                    config: merged,
                    message: format!(
                        "Saved {} ({} server{}) — applied live",
                        path.display(),
                        n,
                        if n == 1 { "" } else { "s" }
                    ),
                }
            }
            Err(e) => {
                self.notice(format!("Save failed: {e}"), NoticeTone::Error);
                OverlayAction::None
            }
        }
    }

    // ── Edit-mode helpers ───────────────────────────────────────────

    fn edit_field_mut(&mut self, field: Field) -> Option<&mut String> {
        let Mode::Edit { server, .. } = &mut self.mode else {
            return None;
        };
        match field {
            Field::Name => Some(&mut server.name),
            Field::Command => Some(&mut server.command),
            Field::Args => Some(&mut server.args),
            Field::Url => Some(&mut server.url),
            Field::Env => Some(&mut server.env),
            Field::IdleTimeout => Some(&mut server.idle_timeout),
            _ => None,
        }
    }

    fn move_field(&mut self, forward: bool) {
        let Mode::Edit { server, field, .. } = &mut self.mode else {
            return;
        };
        let order = Field::order(server.transport());
        let idx = order.iter().position(|f| *f == *field).unwrap_or(0);
        let next = if forward {
            (idx + 1) % order.len()
        } else {
            idx.saturating_sub(1)
        };
        *field = order[next];
    }

    fn cycle_choice(&mut self, field: Field, forward: bool) {
        let Mode::Edit {
            server,
            field: focus,
            ..
        } = &mut self.mode
        else {
            return;
        };
        let mut focus_moved_to = None;
        match field {
            Field::Transport => {
                // Toggle stdio ↔ http. When the transport flips, the field
                // layout changes too — jump focus to the first
                // transport-specific field so the user lands on the now-
                // relevant input (command for stdio, url for http).
                let new_t = match server.transport {
                    Transport::Stdio => Transport::Http,
                    Transport::Http => Transport::Stdio,
                };
                server.transport = new_t;
                focus_moved_to = Some(match new_t {
                    Transport::Stdio => Field::Command,
                    Transport::Http => Field::Url,
                });
            }
            Field::Lifecycle => {
                let all = LifecycleField::ALL;
                let i = all.iter().position(|l| *l == server.lifecycle).unwrap_or(0);
                let n = if forward {
                    (i + 1) % all.len()
                } else if i == 0 {
                    all.len() - 1
                } else {
                    i - 1
                };
                server.lifecycle = all[n];
            }
            Field::DirectTools => {
                let all = DirectField::ALL;
                let i = all
                    .iter()
                    .position(|d| *d == server.direct_tools)
                    .unwrap_or(0);
                let n = if forward {
                    (i + 1) % all.len()
                } else if i == 0 {
                    all.len() - 1
                } else {
                    i - 1
                };
                server.direct_tools = all[n];
            }
            _ => {}
        }
        if let Some(new_focus) = focus_moved_to {
            *focus = new_focus;
        }
    }

    /// Commit the edit form back into `doc` (stage only — does not write
    /// to disk). Returns `true` if the commit succeeded.
    fn stage_edit(&mut self) -> bool {
        // Extract what we need without holding a borrow across the write.
        let server = match &self.mode {
            Mode::Edit { server, .. } => server.clone(),
            _ => return false,
        };
        match server.build() {
            Ok((name, entry)) => {
                let original = server.original_name.clone();
                // If renaming, drop the old key.
                if let Some(orig) = &original
                    && orig != &name
                {
                    self.doc.mcp_servers.remove(orig);
                }
                self.doc.mcp_servers.insert(name, entry);
                self.dirty = true;
                self.discard_guard = false;
                self.mode = Mode::List;
                true
            }
            Err(msg) => {
                if let Mode::Edit { error, .. } = &mut self.mode {
                    *error = Some(msg);
                }
                false
            }
        }
    }

    /// Save button: stage the edit, then write to disk + apply live.
    /// Only writes/applies if staging succeeded (validation passed).
    fn commit_and_save(&mut self) -> OverlayAction {
        if self.stage_edit() {
            self.save_and_apply()
        } else {
            OverlayAction::None
        }
    }
}

/// Resolve the on-disk path a given scope writes to.
fn target_path(scope: Scope, cwd: &std::path::Path) -> PathBuf {
    match scope {
        Scope::Global => {
            config::default_write_path_global().unwrap_or_else(|| cwd.join(".oxi").join("mcp.json"))
        }
        Scope::Project => config::default_write_path_project(cwd),
    }
}

// ---------------------------------------------------------------------------
// OverlayComponent
// ---------------------------------------------------------------------------

impl OverlayComponent for McpConfigOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        match &mut self.mode {
            Mode::List => self.handle_list_key(key),
            Mode::Edit { .. } => self.handle_edit_key(key),
            Mode::ConfirmRemove { .. } => self.handle_confirm_key(key),
            Mode::QuickAdd { .. } => self.handle_quick_add_key(key),
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        // Capture the mode discriminant + any data the renderers need
        // *without* holding a borrow of `self.mode` across the render
        // calls (which require `&mut self`).
        enum RenderKind {
            List,
            Edit,
            ConfirmRemove(String),
            QuickAdd,
        }
        let kind = match &self.mode {
            Mode::List => RenderKind::List,
            Mode::Edit { .. } => RenderKind::Edit,
            Mode::ConfirmRemove { name } => RenderKind::ConfirmRemove(name.clone()),
            Mode::QuickAdd { .. } => RenderKind::QuickAdd,
        };
        match kind {
            RenderKind::List => self.render_list(frame, area, theme, &styles),
            RenderKind::Edit => self.render_edit(frame, area, theme, &styles),
            RenderKind::ConfirmRemove(name) => {
                self.render_list(frame, area, theme, &styles);
                self.render_confirm(frame, area, theme, &name);
            }
            RenderKind::QuickAdd => self.render_quick_add(frame, area, theme, &styles),
        }
    }

    fn hint(&self) -> &str {
        match self.mode {
            Mode::List => {
                if self.server_names().is_empty() {
                    // Empty state — the user is most likely here to add
                    // their first server. The Quick Add key is the
                    // path of least resistance.
                    "a:Quick Add │ c:Custom server │ Tab:Scope │ Esc:Close"
                } else {
                    "↑↓ Navigate │ a:Add │ e/Enter:Edit │ d:Remove │ r:Reconnect │ Tab:Scope │ s:Save │ Esc:Close"
                }
            }
            Mode::Edit { .. } => {
                "↑↓/Tab Next │ ←→ Transport/Choice │ Enter Save/Cancel │ Esc Cancel"
            }
            Mode::ConfirmRemove { .. } => "Enter:Confirm remove │ Esc/other:Cancel",
            Mode::QuickAdd { .. } => {
                "↑↓ Navigate │ Enter/a:Apply preset │ c:Custom server │ Esc:Back"
            }
        }
    }
}

// ── key handling ─────────────────────────────────────────────────────

impl McpConfigOverlay {
    fn handle_list_key(&mut self, key: KeyEvent) -> OverlayAction {
        let count = self.server_names().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                OverlayAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if count > 0 {
                    self.selected = (self.selected + 1).min(count - 1);
                }
                OverlayAction::None
            }
            KeyCode::Char('a') => {
                // 'a' now opens the Quick Add gallery, regardless of
                // whether the list is empty. The gallery includes a
                // 'c' shortcut to the blank custom form for power users.
                self.enter_quick_add();
                OverlayAction::None
            }
            KeyCode::Char('n') => {
                // 'n' keeps its old behaviour: jump straight to the
                // blank custom add form (no preset).
                self.begin_add();
                OverlayAction::None
            }
            KeyCode::Char('c') => {
                // 'c' = custom server: same as 'n'.
                self.begin_add();
                OverlayAction::None
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                if count > 0 {
                    self.begin_edit();
                } else {
                    // No servers — Enter on an empty list opens Quick
                    // Add (so the user can confirm their first server
                    // with a single key press).
                    self.enter_quick_add();
                }
                OverlayAction::None
            }
            KeyCode::Char('d') => {
                if count > 0 {
                    self.begin_remove();
                }
                OverlayAction::None
            }
            KeyCode::Char('r') => {
                let names = self.server_names();
                if let Some(name) = names.get(self.selected).cloned() {
                    OverlayAction::McpAction(super::mcp_dashboard::McpAction::Reconnect(name))
                } else {
                    OverlayAction::None
                }
            }
            KeyCode::Char('R') => {
                OverlayAction::McpAction(super::mcp_dashboard::McpAction::ReconnectAll)
            }
            KeyCode::Char('q') => {
                // 'q' is the Quick Add shortcut (was previously a
                // redundant alias for Esc — we don't need two close
                // keys when one does the job).
                self.enter_quick_add();
                OverlayAction::None
            }
            KeyCode::Tab => {
                self.switch_scope();
                OverlayAction::None
            }
            KeyCode::Char('s') => self.save_and_apply(),
            KeyCode::Esc => {
                if self.dirty && !self.discard_guard {
                    // First Esc: warn and arm the guard. Second Esc closes.
                    self.discard_guard = true;
                    self.notice(
                        "Unsaved changes — press s to save, or Esc again to discard.",
                        NoticeTone::Warning,
                    );
                    OverlayAction::None
                } else {
                    OverlayAction::Close
                }
            }
            _ => OverlayAction::None,
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) -> OverlayAction {
        // Read current field without holding a borrow across the match.
        let field = {
            let Mode::Edit { field, .. } = &self.mode else {
                return OverlayAction::None;
            };
            *field
        };

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::List;
                OverlayAction::None
            }
            KeyCode::Down | KeyCode::Tab => {
                self.move_field(true);
                OverlayAction::None
            }
            KeyCode::Up => {
                self.move_field(false);
                OverlayAction::None
            }
            KeyCode::Left
                if matches!(
                    field,
                    Field::Transport | Field::Lifecycle | Field::DirectTools
                ) =>
            {
                self.cycle_choice(field, false);
                OverlayAction::None
            }
            KeyCode::Right
                if matches!(
                    field,
                    Field::Transport | Field::Lifecycle | Field::DirectTools
                ) =>
            {
                self.cycle_choice(field, true);
                OverlayAction::None
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.edit_field_mut(field) {
                    buf.pop();
                }
                OverlayAction::None
            }
            KeyCode::Enter => {
                if field == Field::Save {
                    self.commit_and_save()
                } else if field == Field::Cancel {
                    self.mode = Mode::List;
                    OverlayAction::None
                } else {
                    // Enter on a normal field → jump to next.
                    self.move_field(true);
                    OverlayAction::None
                }
            }
            KeyCode::Char(c) => {
                // Guard `/` so it doesn't leak into the chat input.
                match field {
                    Field::Save => {
                        if c == 's' || c == 'S' {
                            return self.commit_and_save();
                        }
                        return OverlayAction::None;
                    }
                    // Choice fields are toggle-only (←/→); ignore chars.
                    Field::Cancel | Field::Transport | Field::Lifecycle | Field::DirectTools => {
                        return OverlayAction::None;
                    }
                    _ => {}
                }
                // Numeric guard for idle timeout.
                if field == Field::IdleTimeout && !(c.is_ascii_digit() || c == ' ') {
                    return OverlayAction::None;
                }
                // Name charset guard.
                if field == Field::Name && !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    return OverlayAction::None;
                }
                if let Some(buf) = self.edit_field_mut(field) {
                    buf.push(c);
                }
                OverlayAction::None
            }
            _ => OverlayAction::None,
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> OverlayAction {
        let name = match &self.mode {
            Mode::ConfirmRemove { name } => name.clone(),
            _ => return OverlayAction::None,
        };
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.commit_remove(&name);
                OverlayAction::None
            }
            _ => {
                self.mode = Mode::List;
                OverlayAction::None
            }
        }
    }

    /// Key handler for the Quick Add gallery.
    ///
    /// `Enter` / `a` applies the highlighted preset (jumps to the Edit
    /// form with fields pre-filled). `Esc` returns to List mode (which
    /// is a no-op when the server list is empty — the overlay will
    /// reopen on Quick Add next time). `c` switches to the blank
    /// add-server form (no preset).
    fn handle_quick_add_key(&mut self, key: KeyEvent) -> OverlayAction {
        let preset_count = super::mcp_presets::presets().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Mode::QuickAdd {
                    selected,
                    list_state,
                } = &mut self.mode
                {
                    *selected = selected.saturating_sub(1);
                    list_state.select(Some(*selected));
                }
                OverlayAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Mode::QuickAdd {
                    selected,
                    list_state,
                } = &mut self.mode
                    && preset_count > 0
                {
                    *selected = (*selected + 1).min(preset_count - 1);
                    list_state.select(Some(*selected));
                }
                OverlayAction::None
            }
            KeyCode::Enter | KeyCode::Char('a') => {
                self.apply_preset();
                OverlayAction::None
            }
            KeyCode::Char('c') => {
                // 'c' = "custom" — go straight to the blank add form
                // so power users can skip the gallery.
                self.begin_add();
                OverlayAction::None
            }
            KeyCode::Esc => {
                self.mode = Mode::List;
                OverlayAction::None
            }
            _ => OverlayAction::None,
        }
    }
}

// ── rendering ─────────────────────────────────────────────────────────

impl McpConfigOverlay {
    fn render_list(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, styles: &ThemeStyles) {
        let popup = centered_layout(area, 0.82, 0.8);
        frame.render_widget(Clear, popup);

        let title = Line::from(vec![
            Span::styled(" \u{2699} MCP Servers ", styles.accent),
            Span::styled(
                format!(
                    "— {} ({})",
                    self.scope.label(),
                    self.target_path().display()
                ),
                styles.muted,
            ),
        ]);

        let border = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border.inner(popup);
        frame.render_widget(border, popup);

        // Build live status map from the manager. When the manager is
        // missing (defensive — bootstrap normally wires it), we skip
        // the live status and just show server entries in the muted
        // (disconnected) state. The list is still usable for add/edit.
        let mut status_by_name: std::collections::HashMap<String, (&str, Color)> =
            std::collections::HashMap::new();
        let (connected_count, total_count) = if let Some(mgr) = &self.manager {
            let dash = mgr.dashboard_data();
            let total = dash.settings.total_servers;
            let connected = dash.settings.connected_servers;
            for s in &dash.servers {
                let (label, color) = match &s.status {
                    McpConnectionStatus::Connected => (theme.symbols.dot_on, theme.colors.success),
                    McpConnectionStatus::Connecting => {
                        (theme.symbols.status_running, theme.colors.warning)
                    }
                    McpConnectionStatus::Error(_) => {
                        (theme.symbols.status_error, theme.colors.error)
                    }
                    McpConnectionStatus::Disconnected => {
                        (theme.symbols.dot_off, theme.colors.muted)
                    }
                };
                status_by_name.insert(s.name.clone(), (label, color));
            }
            (connected, total)
        } else {
            (0, 0)
        };

        let names = self.server_names();

        // Header line: counts + dirty indicator.
        let header = Line::from(vec![
            Span::styled(
                format!(
                    " {} server{}",
                    names.len(),
                    if names.len() == 1 { "" } else { "s" }
                ),
                styles.normal,
            ),
            Span::raw("   "),
            Span::styled(
                format!("live: {connected_count}/{total_count} connected"),
                styles.muted,
            ),
            if self.dirty {
                Span::raw("   ")
            } else {
                Span::raw("")
            },
            if self.dirty {
                Span::styled("● unsaved", Style::default().fg(theme.colors.warning))
            } else {
                Span::raw("")
            },
        ]);
        frame.render_widget(
            header,
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        let list_area = Rect {
            x: inner.x + 1,
            y: inner.y + 2,
            width: inner.width.saturating_sub(2),
            height: inner.height.saturating_sub(5),
        };

        let items: Vec<ListItem> = if names.is_empty() {
            // The overlay normally opens in Quick Add mode when the
            // list is empty, so this branch is only reached if the user
            // Esc'd back from Quick Add without applying a preset.
            // Still, keep the hint useful and consistent.
            vec![ListItem::new(Line::from(vec![
                Span::styled(" No servers yet. Press ", styles.muted),
                Span::styled("a", styles.accent),
                Span::styled(" for a curated Quick Add gallery, or ", styles.muted),
                Span::styled("c", styles.accent),
                Span::styled(" to enter a custom server.", styles.muted),
            ]))]
        } else {
            names
                .iter()
                .map(|name| {
                    let entry = self
                        .doc
                        .mcp_servers
                        .get(name)
                        .map(entry_summary)
                        .unwrap_or_default();
                    let (dot, dot_color) = status_by_name
                        .get(name)
                        .copied()
                        .unwrap_or((theme.symbols.dot_off, theme.colors.muted));
                    let mut spans = vec![
                        Span::styled(format!("{dot} "), Style::default().fg(dot_color)),
                        Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::styled(format!("   {entry}"), styles.muted),
                    ];
                    if let Some(e) = self.doc.mcp_servers.get(name)
                        && DirectField::from_entry(e) == DirectField::All
                    {
                        spans.push(Span::styled(
                            "  [direct]",
                            Style::default().fg(theme.colors.accent),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect()
        };

        self.list_state.select(if names.is_empty() {
            None
        } else {
            Some(self.selected)
        });
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(theme.colors.primary)
                    .fg(theme.colors.background),
            )
            .highlight_symbol(theme.symbols.cursor);
        frame.render_stateful_widget(list, list_area, &mut self.list_state);

        // Footer: notice + keymap.
        let notice_y = inner.y + inner.height.saturating_sub(2);
        if let Some((msg, tone)) = &self.notice {
            let color = match tone {
                NoticeTone::Success => theme.colors.success,
                NoticeTone::Warning => theme.colors.warning,
                NoticeTone::Error => theme.colors.error,
                NoticeTone::Info => theme.colors.accent,
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {msg}"),
                    Style::default().fg(color),
                ))),
                Rect {
                    x: inner.x + 1,
                    y: notice_y,
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " Tip: servers connect lazily — they start on first tool use.",
                    styles.muted,
                ))),
                Rect {
                    x: inner.x + 1,
                    y: notice_y,
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );
        }
    }

    fn render_edit(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, styles: &ThemeStyles) {
        let popup = centered_layout(area, 0.7, 0.78);
        frame.render_widget(Clear, popup);

        let is_new = matches!(
            &self.mode,
            Mode::Edit { server, .. } if server.original_name.is_none()
        );
        let verb = if is_new { "Add" } else { "Edit" };
        let border = Block::default()
            .title(Line::from(Span::styled(
                format!(" \u{2699} {verb} MCP Server "),
                styles.accent,
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border.inner(popup);
        frame.render_widget(border, popup);

        let (server, field, error) = match &self.mode {
            Mode::Edit {
                server,
                field,
                error,
            } => (server.clone(), *field, error.clone()),
            _ => return,
        };
        let transport = server.transport();

        let mut y = inner.y;
        let row = |y: &mut u16, inner: Rect| {
            let r = Rect {
                x: inner.x + 1,
                y: *y,
                width: inner.width.saturating_sub(2),
                height: 1,
            };
            *y += 1;
            r
        };

        let label_w = 14usize;
        let text_field = |frame: &mut Frame,
                          area: Rect,
                          label: &str,
                          value: &str,
                          placeholder: &str,
                          focused: bool,
                          theme: &Theme,
                          styles: &ThemeStyles| {
            let avail = (area.width as usize).saturating_sub(label_w + 4);
            let display = if value.is_empty() {
                placeholder.to_string()
            } else {
                value.to_string()
            };
            let value_style = if focused {
                Style::default()
                    .fg(theme.colors.background)
                    .bg(theme.colors.primary)
            } else if value.is_empty() {
                styles.muted
            } else {
                styles.normal
            };
            let label_span = Span::styled(
                format!("{:width$}", label, width = label_w),
                Style::default()
                    .fg(theme.colors.muted)
                    .add_modifier(Modifier::BOLD),
            );
            let cursor = if focused { "_" } else { " " };
            let value_span = Span::styled(
                format!(
                    " {}{:width$}",
                    display,
                    cursor,
                    width = avail.saturating_sub(1)
                ),
                value_style,
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![label_span, value_span])),
                area,
            );
        };

        let choice_field = |frame: &mut Frame,
                            area: Rect,
                            label: &str,
                            options: &[(&str, bool)],
                            focused: bool,
                            theme: &Theme,
                            styles: &ThemeStyles| {
            let label_color = if focused {
                theme.colors.accent
            } else {
                theme.colors.muted
            };
            let label_span = Span::styled(
                format!("{:width$}", label, width = label_w),
                Style::default()
                    .fg(label_color)
                    .add_modifier(Modifier::BOLD),
            );
            let mut spans = vec![label_span, Span::raw(" ")];
            for (opt, active) in options {
                if *active {
                    spans.push(Span::styled(
                        format!("[{opt}] "),
                        Style::default()
                            .fg(theme.colors.background)
                            .bg(theme.colors.primary),
                    ));
                } else {
                    spans.push(Span::styled(format!("{opt} "), styles.muted));
                }
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), area);
        };

        text_field(
            frame,
            row(&mut y, inner),
            "Name",
            &server.name,
            "my-server",
            field == Field::Name,
            theme,
            styles,
        );

        // Transport selector (←/→ toggles stdio ↔ http).
        choice_field(
            frame,
            row(&mut y, inner),
            "Transport",
            &[
                ("stdio", transport == Transport::Stdio),
                ("http", transport == Transport::Http),
            ],
            field == Field::Transport,
            theme,
            styles,
        );

        if transport == Transport::Stdio {
            text_field(
                frame,
                row(&mut y, inner),
                "Command",
                &server.command,
                "npx",
                field == Field::Command,
                theme,
                styles,
            );
            text_field(
                frame,
                row(&mut y, inner),
                "Args",
                &server.args,
                "-y @modelcontextprotocol/server-filesystem .",
                field == Field::Args,
                theme,
                styles,
            );
        } else {
            text_field(
                frame,
                row(&mut y, inner),
                "Url",
                &server.url,
                "https://example.com/mcp",
                field == Field::Url,
                theme,
                styles,
            );
        }

        // Separator
        y += 1;

        choice_field(
            frame,
            row(&mut y, inner),
            "Lifecycle",
            &[
                (
                    LifecycleField::Lazy.as_str(),
                    server.lifecycle == LifecycleField::Lazy,
                ),
                (
                    LifecycleField::Eager.as_str(),
                    server.lifecycle == LifecycleField::Eager,
                ),
                (
                    LifecycleField::KeepAlive.as_str(),
                    server.lifecycle == LifecycleField::KeepAlive,
                ),
            ],
            field == Field::Lifecycle,
            theme,
            styles,
        );
        text_field(
            frame,
            row(&mut y, inner),
            "Idle (min)",
            &server.idle_timeout,
            "(global default)",
            field == Field::IdleTimeout,
            theme,
            styles,
        );
        choice_field(
            frame,
            row(&mut y, inner),
            "Direct tools",
            &[
                (
                    DirectField::Off.as_str(),
                    server.direct_tools == DirectField::Off,
                ),
                (
                    DirectField::All.as_str(),
                    server.direct_tools == DirectField::All,
                ),
            ],
            field == Field::DirectTools,
            theme,
            styles,
        );
        text_field(
            frame,
            row(&mut y, inner),
            if transport == Transport::Stdio {
                "Env"
            } else {
                "Headers"
            },
            &server.env,
            if transport == Transport::Stdio {
                "API_KEY=xxx, DEBUG=true"
            } else {
                "Authorization=Bearer ..."
            },
            field == Field::Env,
            theme,
            styles,
        );

        y += 1;

        // Buttons
        let btn_area = row(&mut y, inner);
        let save_active = field == Field::Save;
        let cancel_active = field == Field::Cancel;
        let save_span = if save_active {
            Span::styled(
                " [Save & Apply] ",
                Style::default()
                    .bg(theme.colors.success)
                    .fg(theme.colors.background),
            )
        } else {
            Span::styled(" [Save & Apply] ", styles.normal)
        };
        let cancel_span = if cancel_active {
            Span::styled(
                " [Cancel] ",
                Style::default()
                    .bg(theme.colors.error)
                    .fg(theme.colors.background),
            )
        } else {
            Span::styled(" [Cancel] ", styles.muted)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![save_span, Span::raw("   "), cancel_span])),
            btn_area,
        );

        // Error line
        if let Some(err) = error {
            y += 1;
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" \u{26a0} {err}"),
                    Style::default().fg(theme.colors.error),
                ))),
                Rect {
                    x: inner.x + 1,
                    y,
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );
        }
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect, theme: &Theme, name: &str) {
        let popup = centered_layout(area, 0.5, 0.18);
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(Line::from(Span::styled(
                " Confirm removal ",
                Style::default().fg(theme.colors.warning),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.warning));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let lines = vec![
            Line::from(format!(
                " Remove server '{name}' from {} config?",
                self.scope.label()
            )),
            Line::from(""),
            Line::from(Span::styled(
                " Enter to confirm · Esc to cancel",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        frame.render_widget(
            Paragraph::new(lines),
            Rect {
                x: inner.x + 1,
                y: inner.y + 1,
                width: inner.width.saturating_sub(2),
                height: inner.height.saturating_sub(2),
            },
        );
    }

    /// Render the Quick Add gallery. Two-column layout: list of presets
    /// on the left, live preview / detail of the highlighted preset on
    /// the right.
    fn render_quick_add(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        styles: &ThemeStyles,
    ) {
        let popup = centered_layout(area, 0.86, 0.82);
        frame.render_widget(Clear, popup);

        let title = Line::from(vec![
            Span::styled(" Quick Add MCP Server ", styles.accent),
            Span::styled("— pick a preset to pre-fill the form, or ", styles.muted),
            Span::styled("c", styles.accent),
            Span::styled(" for custom.", styles.muted),
        ]);
        let border = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border.inner(popup);
        frame.render_widget(border, popup);

        // Pull selection out of self.mode without holding the borrow
        // through the rest of the function.
        let (selected_idx, list_state_ref) = match &mut self.mode {
            Mode::QuickAdd {
                selected,
                list_state,
            } => (*selected, list_state),
            _ => return,
        };

        let presets = super::mcp_presets::presets();
        // Update the cursor in the list state.
        list_state_ref.select(Some(selected_idx));

        // Split: list on the left, detail on the right.
        let list_width = inner.width.saturating_sub(2).saturating_mul(55) / 100;
        let list_area = Rect {
            x: inner.x + 1,
            y: inner.y + 1,
            width: list_width,
            height: inner.height.saturating_sub(2),
        };
        let detail_x = inner.x + 1 + list_width + 2;
        let detail_area = Rect {
            x: detail_x,
            y: inner.y + 1,
            width: inner.width.saturating_sub(2 + (detail_x - inner.x)),
            height: inner.height.saturating_sub(2),
        };

        // ── Left: preset list ────────────────────────────────────────
        let items: Vec<ListItem> = presets
            .iter()
            .map(|p| {
                let spans = vec![
                    Span::styled(
                        format!("{:<12}", p.name),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(format!("[{}]", p.tag), styles.muted),
                    if p.requires_oauth {
                        Span::styled("  oauth", styles.warning)
                    } else {
                        Span::raw("")
                    },
                ];
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(theme.colors.primary)
                    .fg(theme.colors.background),
            )
            .highlight_symbol(theme.symbols.cursor);
        frame.render_stateful_widget(list, list_area, list_state_ref);

        // ── Right: detail panel ──────────────────────────────────────
        if let Some(preset) = presets.get(selected_idx) {
            self.render_preset_detail(frame, detail_area, theme, styles, preset);
        } else {
            // Empty preset list shouldn't happen (PRESETS is a const
            // non-empty slice), but render a friendly placeholder
            // rather than panic.
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " (no presets available)",
                    styles.muted,
                ))),
                detail_area,
            );
        }
    }

    /// Render the right-hand detail panel for a highlighted preset:
    /// name + summary + transport + command/url + env hint + lifecycle.
    fn render_preset_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        styles: &ThemeStyles,
        preset: &super::mcp_presets::McpPreset,
    ) {
        let mut lines: Vec<Line> = Vec::new();
        // Header: name + tag
        lines.push(Line::from(vec![
            Span::styled(
                preset.name,
                Style::default()
                    .fg(theme.colors.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("({})", preset.tag),
                Style::default().fg(theme.colors.muted),
            ),
        ]));
        // Summary (wrap by hand: split on whitespace at width-2).
        let wrap_width = area.width.saturating_sub(2) as usize;
        let mut summary = preset.summary.to_string();
        if wrap_width > 0 {
            let mut chunks: Vec<String> = Vec::new();
            // Simple word-wrap: greedy line break.
            for word in summary.split_whitespace() {
                if let Some(last) = chunks.last_mut()
                    && last.len() + 1 + word.len() <= wrap_width
                {
                    last.push(' ');
                    last.push_str(word);
                    continue;
                }
                chunks.push(word.to_string());
            }
            summary = chunks.join("\n");
        }
        for line in summary.lines() {
            lines.push(Line::from(Span::styled(line, styles.normal)));
        }
        lines.push(Line::from(""));

        // Transport detail
        if let Some(cmd) = &preset.entry.command {
            lines.push(Line::from(vec![
                Span::styled("command:  ", styles.muted),
                Span::styled(cmd.clone(), styles.normal),
            ]));
            if let Some(args) = &preset.entry.args
                && !args.is_empty()
            {
                lines.push(Line::from(vec![
                    Span::styled("args:     ", styles.muted),
                    Span::styled(args.join(" "), styles.normal),
                ]));
            }
        }
        if let Some(url) = &preset.entry.url {
            lines.push(Line::from(vec![
                Span::styled("url:      ", styles.muted),
                Span::styled(url.clone(), styles.normal),
            ]));
        }
        if let Some(env) = preset.env_placeholder {
            lines.push(Line::from(vec![
                Span::styled("env:      ", styles.muted),
                Span::styled(env.to_string(), Style::default().fg(theme.colors.warning)),
            ]));
        }
        lines.push(Line::from(""));

        // Footer hint about post-install auth
        if preset.requires_oauth {
            lines.push(Line::from(Span::styled(
                "After saving, this server will prompt for OAuth.",
                Style::default().fg(theme.colors.warning),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "Press Enter to add this server.",
                styles.muted,
            )));
        }

        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(theme.colors.border));
        frame.render_widget(
            Paragraph::new(lines).block(block),
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height,
            },
        );
    }
}

/// One-line summary of a server entry for the list view.
fn entry_summary(entry: &ServerEntry) -> String {
    if let Some(cmd) = &entry.command {
        let args = entry.args.as_ref().map(|a| a.join(" ")).unwrap_or_default();
        format!("stdio · {cmd} {args}")
    } else if let Some(url) = &entry.url {
        format!("http · {url}")
    } else {
        "(unconfigured)".to_string()
    }
}

// Keep ToolPrefix referenced so unused-import warnings stay quiet when
// the overlay is compiled against a slim feature set.
const _: Option<ToolPrefix> = None;
// ConsentState is referenced by the dashboard action path; keep the
// import live even if this overlay doesn't set consent directly.
const _: Option<ConsentState> = None;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `EditableServer::from_preset` should produce a form that
    /// round-trips back to a `ServerEntry` matching the preset's
    /// transport-specific fields (command + args, or url).
    #[test]
    fn preset_roundtrips_through_editable_server() {
        for preset in crate::tui::overlay::mcp_presets::presets() {
            let editable = EditableServer::from_preset(preset);
            let (name, entry) = editable
                .build()
                .unwrap_or_else(|e| panic!("preset '{}' failed to build: {e}", preset.name));
            assert_eq!(name, preset.name, "name mismatch for {}", preset.name);
            // Transport sanity: the editable's transport (inferred from
            // which non-empty field is set) must match the preset's
            // transport.
            let editable_is_stdio = editable.url.trim().is_empty();
            let preset_is_stdio = preset.entry.url.is_none();
            assert_eq!(
                editable_is_stdio, preset_is_stdio,
                "transport mismatch for {} (editable: stdio={}, preset: stdio={})",
                preset.name, editable_is_stdio, preset_is_stdio
            );
            // Round-trip the transport-specific fields.
            if preset.entry.command.is_some() {
                assert_eq!(
                    entry.command, preset.entry.command,
                    "command mismatch for {}",
                    preset.name
                );
                assert_eq!(
                    entry.args, preset.entry.args,
                    "args mismatch for {}",
                    preset.name
                );
            }
            if preset.entry.url.is_some() {
                assert_eq!(
                    entry.url, preset.entry.url,
                    "url mismatch for {}",
                    preset.name
                );
            }
        }
    }

    /// Editing a preset (changing the name) should rename the entry.
    #[test]
    fn preset_can_be_renamed() {
        let preset = &crate::tui::overlay::mcp_presets::presets()[0];
        let mut editable = EditableServer::from_preset(preset);
        editable.name = "renamed-preset".to_string();
        let (name, _entry) = editable.build().expect("build ok");
        assert_eq!(name, "renamed-preset");
    }

    /// The lifecycle / direct-tools fields on the form should reflect
    /// the preset's choices after `from_preset`.
    #[test]
    fn preset_lifecycle_and_direct_tools_are_copied() {
        for preset in crate::tui::overlay::mcp_presets::presets() {
            let editable = EditableServer::from_preset(preset);
            // Both presets default to Lazy lifecycle + no direct_tools.
            // When a preset flips these in the future, the form will
            // reflect it.
            assert_eq!(
                editable.lifecycle,
                LifecycleField::from_entry(&preset.entry),
                "lifecycle mismatch for {}",
                preset.name
            );
            assert_eq!(
                editable.direct_tools,
                DirectField::from_entry(&preset.entry),
                "direct_tools mismatch for {}",
                preset.name
            );
        }
    }
}
