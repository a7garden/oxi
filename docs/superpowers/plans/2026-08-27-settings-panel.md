# Settings Panel Overhaul Implementation Plan (Phase 1/3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn oxicode's `/settings` from a flat 6-item list into a schema-driven, tabbed, searchable panel covering every `Settings` field, and make `settings.keybindings` a live keymap instead of a persisted-but-unread map.

**Architecture:** A `const SettingDef[]` table (`settings_defs.rs`) is the single source of truth for what `/settings` shows and edits; a `keymap.rs` module owns `GlobalAction`/`Keymap`/`KeyCombo` so the input loop's 6 hardcoded Ctrl- shortcuts become data-driven. The panel renders by extending the existing in-frame `OverlayState`/`render_overlay` machinery (NO alternate screen — AGENTS.md forbids it).

**Tech Stack:** Rust 2024, ratatui, `oxicode-cli` (composition root), `oxicode-vtui-compat` (InlineListSelection/InlineListItem/InlineListSearchConfig). Tests via `cargo nextest`, lints via `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt`.

## Global Constraints

- **No alternate screen.** Rendering stays on the main screen via `main_loop.rs::render_frame`/`render_overlay`. Do NOT add alt-screen, a second render driver, or a tape engine.
- **`Settings` struct fields are NOT changed.** This plan only adds *access* (`get_display_value`/`apply_change`), never new persisted state. `store/settings.rs` gets zero field edits.
- **Composer text input, vim mode, secure-input, confirmation y/n/x are untouched.** Only the 6 `KeyModifiers::CONTROL`-gated global shortcuts in `main_loop.rs:3279-3345` (Ctrl+C/M/P/;/E, Ctrl+Enter) become keymap-driven. Bare `Shift+E`/`e`/`J`/`K` (vim-style block nav, `main_loop.rs:3609-3725`) stays put.
- `cargo fmt` before every commit. `cargo clippy --workspace --all-targets -- -D warnings` must pass. Non-test code must not `unwrap` (only `#[cfg(test)]` relaxes that).
- Error handling: application crate (`oxicode-cli`) uses `anyhow::Result`.
- Commit convention: conventional commits, English messages, squash merge.

**Spec:** `docs/superpowers/specs/2026-08-27-settings-panel-design.md`

---

## File Structure

| File | Responsibility |
|---|---|
| `oxicode-cli/src/tui_vt/settings_defs.rs` (**new**) | `SettingKey` enum, `SettingDef` struct, `SETTING_DEFS` const table, `get_display_value`, `apply_change`. The ONLY code that touches `Settings` fields. |
| `oxicode-cli/src/tui_vt/keymap.rs` (**new**) | `GlobalAction`, `KeyCombo`, `Keymap`, `DEFAULT_KEYBINDINGS`. Parse/resolve/serialize key combos. |
| `oxicode-cli/src/tui_vt/mod.rs` (**modify**) | Declare `mod settings_defs; mod keymap;` |
| `oxicode-vtui-compat/src/ui_protocol/selection.rs` (**modify**) | `InlineListSelection` + 3 variants: `SettingsTab(usize)`, `SettingsSection(usize)`, `SettingKeyCapture(SettingKey)` (uses a `String` key name to avoid a cross-crate dep, see Task 4). |
| `oxicode-cli/src/tui_vt/main_loop.rs` (**modify**) | `OverlayState` +`tabs`/`active_tab`/`sections`/`active_section`; `render_overlay` +tab-bar/sidebar branch; `RenderState` +`keymap: Arc<RwLock<Keymap>>`; input loop's 6 hardcoded checks → `keymap.resolve()` dispatch; `SettingsCommand::execute` rewritten. |
| `oxicode-cli/src/tui_vt/slash/commands.rs` (**modify**) | New `HooksCommand` (`/hooks`). |
| `oxicode-cli/src/tui_vt/slash/registry.rs` (**modify**) | `SettingsCommand` rewritten to build items from `SETTING_DEFS`. |
| `docs/superpowers/specs/2026-08-27-settings-panel-design.md` | Read-only reference. |

---

### Task 1: `settings_defs.rs` — SettingKey, SettingDef, SETTING_DEFS table

**Files:**
- Create: `oxicode-cli/src/tui_vt/settings_defs.rs`
- Test: `oxicode-cli/src/tui_vt/settings_defs.rs` (inline `#[cfg(test)]`)
- Modify: `oxicode-cli/src/tui_vt/mod.rs`

**Interfaces:**
- Produces (consumed by Task 4, 5, 6):
  - `pub enum SettingKey { ... }` (all variants listed below)
  - `pub enum SettingsTab { General, Model, Interaction, Tools, Ui, AdvisorMemory, Keybindings, Advanced }`
  - `pub struct SettingDef { pub key: SettingKey, pub tab: SettingsTab, pub group: &'static str, pub label: &'static str, pub description: &'static str, pub widget: SettingWidget, pub condition: Option<fn(&crate::store::settings::Settings) -> bool> }`
  - `pub enum SettingWidget { Toggle, Cycle, SubmenuSelect(Vec<&'static str>), Text, Multiselect, MapEditor, Pointer }`
  - `pub const SETTING_DEFS: &[SettingDef]`
  - `pub fn defs_for_tab(tab: SettingsTab) -> Vec<&'static SettingDef>` (group-ordered, condition-filtered)
  - `pub fn get_display_value(key: SettingKey, s: &Settings) -> String`
  - `pub fn apply_change(key: SettingKey, s: &mut Settings, new: String) -> anyhow::Result<()>`

- [ ] **Step 1: Write the failing tests**

Tests assert: (a) every `SETTING_DEFS` entry has a non-empty group, and groups are contiguous within a tab; (b) `get_display_value`/`apply_change` round-trip for a representative scalar (`ThinkingLevel`), a toggle (`AutoCompaction`), and a map (`ModelRoles`); (c) `defs_for_tab` respects the `condition` predicate (e.g. `TtsrInterruptMode` hidden when `ttsr_enabled` is false). Use the inline-test pattern from `oxicode-cli/src/store/settings.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-cli tui_vt::settings_defs`
Expected: FAIL (module doesn't exist)

- [ ] **Step 3: Implement `settings_defs.rs`**

```rust
use crate::store::settings::{AdvisorSettings, Settings, ThinkingLevel};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SettingKey {
    ThinkingLevel, AutoCompaction, AutoRetry, GlyphSet, EditFormat,
    ExtensionsEnabled, SessionHistorySize, ToolTimeoutSecs, AskTimeoutSecs,
    DisabledTools, CommitToolEnabled, TodoPanelEnabled, AgentHubEnabled,
    MermaidRenderEnabled, SnapcompactEnabled, MemoryEnabled, TtsrEnabled,
    TtsrInterruptMode, AdvisorEnabled, AdvisorSyncBacklog, AdvisorImmuneTurns,
    ModelRoles, Keybindings,
    // Pointer rows:
    Theme, Model, CustomProviders, Hooks, ExtensionPaths, SkillPaths,
    PromptPaths, ThemePaths,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SettingsTab { General, Model, Interaction, Tools, Ui, AdvisorMemory, Keybindings, Advanced }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingWidget {
    Toggle, Cycle,
    SubmenuSelect(&'static [&'static str]),
    Text, Multiselect, MapEditor, Pointer,
}

pub struct SettingDef {
    pub key: SettingKey,
    pub tab: SettingsTab,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub widget: SettingWidget,
    pub condition: Option<fn(&Settings) -> bool>,
}

pub const SETTING_DEFS: &[SettingDef] = &[
    // General / Behavior
    SettingDef { key: SettingKey::ExtensionsEnabled, tab: SettingsTab::General, group: "Behavior",
        label: "Extensions", description: "Load WASM/native extensions", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::SessionHistorySize, tab: SettingsTab::General, group: "Behavior",
        label: "Session history size", description: "Entries kept in memory", widget: SettingWidget::Text, condition: None },
    SettingDef { key: SettingKey::EditFormat, tab: SettingsTab::General, group: "Behavior",
        label: "Edit format", description: "str_replace or hashline", widget: SettingWidget::Cycle, condition: None },
    // Model / Defaults
    SettingDef { key: SettingKey::ThinkingLevel, tab: SettingsTab::Model, group: "Defaults",
        label: "Thinking level", description: "Reasoning effort", widget: SettingWidget::Cycle, condition: None },
    SettingDef { key: SettingKey::ModelRoles, tab: SettingsTab::Model, group: "Defaults",
        label: "Model roles", description: "role -> model pattern assignments", widget: SettingWidget::MapEditor, condition: None },
    SettingDef { key: SettingKey::Theme, tab: SettingsTab::Model, group: "Pointers",
        label: "Theme", description: "Use /theme to change", widget: SettingWidget::Pointer, condition: None },
    SettingDef { key: SettingKey::Model, tab: SettingsTab::Model, group: "Pointers",
        label: "Model", description: "Use /model to change", widget: SettingWidget::Pointer, condition: None },
    // Interaction / Compaction + Timeouts
    SettingDef { key: SettingKey::AutoCompaction, tab: SettingsTab::Interaction, group: "Compaction",
        label: "Auto-compaction", description: "Compact when context exceeds window", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::ToolTimeoutSecs, tab: SettingsTab::Interaction, group: "Timeouts",
        label: "Tool timeout (s)", description: "Tool execution timeout", widget: SettingWidget::Text, condition: None },
    SettingDef { key: SettingKey::AskTimeoutSecs, tab: SettingsTab::Interaction, group: "Timeouts",
        label: "Ask timeout (s)", description: "Ask overlay timeout", widget: SettingWidget::Text, condition: None },
    // Tools
    SettingDef { key: SettingKey::DisabledTools, tab: SettingsTab::Tools, group: "Tools",
        label: "Disabled tools", description: "Tools turned off for the agent", widget: SettingWidget::Multiselect, condition: None },
    SettingDef { key: SettingKey::CommitToolEnabled, tab: SettingsTab::Tools, group: "Tools",
        label: "Commit tool", description: "Enable the Commit tool", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::CustomProviders, tab: SettingsTab::Tools, group: "Pointers",
        label: "Custom providers", description: "Use /providers to manage", widget: SettingWidget::Pointer, condition: None },
    SettingDef { key: SettingKey::Hooks, tab: SettingsTab::Tools, group: "Pointers",
        label: "Hooks", description: "Use /hooks to view", widget: SettingWidget::Pointer, condition: None },
    // UI / Appearance + Panels
    SettingDef { key: SettingKey::GlyphSet, tab: SettingsTab::Ui, group: "Appearance",
        label: "Icons", description: "unicode / ascii / nerd glyph set", widget: SettingWidget::Cycle, condition: None },
    SettingDef { key: SettingKey::TodoPanelEnabled, tab: SettingsTab::Ui, group: "Panels",
        label: "Todo panel", description: "Sticky todo panel", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::AgentHubEnabled, tab: SettingsTab::Ui, group: "Panels",
        label: "Agent hub", description: "Ctrl+h /agents overlay", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::MermaidRenderEnabled, tab: SettingsTab::Ui, group: "Panels",
        label: "Mermaid", description: "Render mermaid diagrams", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::SnapcompactEnabled, tab: SettingsTab::Ui, group: "Panels",
        label: "Snapcompact", description: "PNG-frame compactor", widget: SettingWidget::Toggle, condition: None },
    // Advisor & Memory
    SettingDef { key: SettingKey::AdvisorEnabled, tab: SettingsTab::AdvisorMemory, group: "Advisor",
        label: "Advisor", description: "Read-only reviewer shadowing the agent", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::AdvisorSyncBacklog, tab: SettingsTab::AdvisorMemory, group: "Advisor",
        label: "Sync backlog", description: "off / sync / async", widget: SettingWidget::SubmenuSelect(&["off", "sync", "async"]),
        condition: Some(|s| s.advisor.enabled) },
    SettingDef { key: SettingKey::AdvisorImmuneTurns, tab: SettingsTab::AdvisorMemory, group: "Advisor",
        label: "Immune turns", description: "Turns the advisor skips", widget: SettingWidget::Text,
        condition: Some(|s| s.advisor.enabled) },
    SettingDef { key: SettingKey::MemoryEnabled, tab: SettingsTab::AdvisorMemory, group: "Memory",
        label: "Memory", description: "Oxibrain durable memory", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::TtsrEnabled, tab: SettingsTab::AdvisorMemory, group: "Memory",
        label: "TTSR", description: "Time-traveling stream rules", widget: SettingWidget::Toggle, condition: None },
    SettingDef { key: SettingKey::TtsrInterruptMode, tab: SettingsTab::AdvisorMemory, group: "Memory",
        label: "TTSR mode", description: "prose_only or rules", widget: SettingWidget::SubmenuSelect(&["prose_only", "rules"]),
        condition: Some(|s| s.ttsr_enabled) },
    // Keybindings
    SettingDef { key: SettingKey::Keybindings, tab: SettingsTab::Keybindings, group: "Keybindings",
        label: "Keybindings", description: "Global shortcuts", widget: SettingWidget::MapEditor, condition: None },
    // Advanced / Pointers
    SettingDef { key: SettingKey::ExtensionPaths, tab: SettingsTab::Advanced, group: "Resources",
        label: "Extensions", description: "Use `oxicode config` to manage", widget: SettingWidget::Pointer, condition: None },
    SettingDef { key: SettingKey::SkillPaths, tab: SettingsTab::Advanced, group: "Resources",
        label: "Skills", description: "Use `oxicode config` to manage", widget: SettingWidget::Pointer, condition: None },
    SettingDef { key: SettingKey::PromptPaths, tab: SettingsTab::Advanced, group: "Resources",
        label: "Prompts", description: "Use `oxicode config` to manage", widget: SettingWidget::Pointer, condition: None },
    SettingDef { key: SettingKey::ThemePaths, tab: SettingsTab::Advanced, group: "Resources",
        label: "Themes", description: "Use `oxicode config` to manage", widget: SettingWidget::Pointer, condition: None },
];

pub fn defs_for_tab(tab: SettingsTab, s: &Settings) -> Vec<&'static SettingDef> {
    SETTING_DEFS.iter()
        .filter(|d| d.tab == tab && d.condition.map_or(true, |c| c(s)))
        .collect()
}

pub fn get_display_value(key: SettingKey, s: &Settings) -> String {
    use SettingKey::*;
    match key {
        ThinkingLevel => format!("{:?}", s.thinking_level).to_lowercase(),
        AutoCompaction => s.auto_compaction.to_string(),
        AutoRetry => s.auto_retry().to_string(),
        GlyphSet => s.glyph_set.to_string(),
        EditFormat => format!("{:?}", s.edit_format).to_lowercase(),
        ExtensionsEnabled => s.extensions_enabled.to_string(),
        SessionHistorySize => s.session_history_size.to_string(),
        ToolTimeoutSecs => s.tool_timeout_seconds.to_string(),
        AskTimeoutSecs => s.ask_timeout_secs.to_string(),
        DisabledTools => format!("{}", s.disabled_tools.len()),
        CommitToolEnabled => s.commit_tool_enabled.to_string(),
        TodoPanelEnabled => s.todo_panel_enabled.to_string(),
        AgentHubEnabled => s.agent_hub_enabled.to_string(),
        MermaidRenderEnabled => s.mermaid_render_enabled.to_string(),
        SnapcompactEnabled => s.snapcompact_enabled.to_string(),
        MemoryEnabled => s.memory_enabled.to_string(),
        TtsrEnabled => s.ttsr_enabled.to_string(),
        TtsrInterruptMode => s.ttsr_interrupt_mode.clone(),
        AdvisorEnabled => s.advisor.enabled.to_string(),
        AdvisorSyncBacklog => s.advisor.sync_backlog.clone(),
        AdvisorImmuneTurns => s.advisor.immune_turns.to_string(),
        ModelRoles => format!("{}", s.model_roles.len()),
        Keybindings => format!("{}", s.keybindings.len()),
        Theme => s.theme.clone(),
        Model => s.last_used_model.clone().unwrap_or_else(|| "unset".into()),
        CustomProviders => format!("{}", s.custom_providers.len()),
        Hooks => format!("{}", s.hooks.len()),
        ExtensionPaths => s.extensions.len().to_string(),
        SkillPaths => s.skills.len().to_string(),
        PromptPaths => s.prompts.len().to_string(),
        ThemePaths => s.themes.len().to_string(),
    }
}

pub fn apply_change(key: SettingKey, s: &mut Settings, new: String) -> anyhow::Result<()> {
    use SettingKey::*;
    match key {
        AutoCompaction => s.auto_compaction = new == "true",
        AutoRetry => s.auto_retry = new == "true",
        ExtensionsEnabled => s.extensions_enabled = new == "true",
        CommitToolEnabled => s.commit_tool_enabled = new == "true",
        TodoPanelEnabled => s.todo_panel_enabled = new == "true",
        AgentHubEnabled => s.agent_hub_enabled = new == "true",
        MermaidRenderEnabled => s.mermaid_render_enabled = new == "true",
        SnapcompactEnabled => s.snapcompact_enabled = new == "true",
        MemoryEnabled => s.memory_enabled = new == "true",
        TtsrEnabled => s.ttsr_enabled = new == "true",
        AdvisorEnabled => s.advisor.enabled = new == "true",
        ThinkingLevel => {
            s.thinking_level = ThinkingLevel::parse(&new)
                .ok_or_else(|| anyhow::anyhow!("invalid thinking level: {new}"))?;
        }
        GlyphSet => s.glyph_set = crate::symbols::GlyphSet::parse(&new)
            .ok_or_else(|| anyhow::anyhow!("invalid glyph set: {new}"))?,
        EditFormat => {
            s.edit_format = if new == "hashline" {
                crate::store::settings::EditFormat::Hashline
            } else {
                crate::store::settings::EditFormat::StrReplace
            };
        }
        SessionHistorySize => s.session_history_size = new.parse()?,
        ToolTimeoutSecs => s.tool_timeout_seconds = new.parse()?,
        AskTimeoutSecs => s.ask_timeout_secs = new.parse()?,
        AdvisorImmuneTurns => s.advisor.immune_turns = new.parse()?,
        TtsrInterruptMode => s.ttsr_interrupt_mode = new,
        AdvisorSyncBacklog => s.advisor.sync_backlog = new,
        DisabledTools => { /* multiselect handled via set_disabled_tools below */ }
        ModelRoles => { /* map-editor handled via set_model_roles below */ }
        Keybindings => { /* map-editor handled via keymap.rs, Task 5 */ }
        // Pointer rows are read-only:
        Theme | Model | CustomProviders | Hooks | ExtensionPaths
        | SkillPaths | PromptPaths | ThemePaths => anyhow::bail!("read-only"),
    }
    Ok(())
}

// Toggle-helper used by apply_change for the DisabledTools multiselect:
pub fn toggle_disabled_tool(s: &mut Settings, tool: &str, enabled: bool) {
    if enabled {
        s.disabled_tools.retain(|t| t != tool);
    } else if !s.disabled_tools.iter().any(|t| t == tool) {
        s.disabled_tools.push(tool.to_string());
    }
}
```

Note: `s.auto_retry` — check whether `Settings` has a public `auto_retry` field or only a getter `auto_retry_enabled()`. The current `/settings` reads `session.auto_retry_enabled()`. If `Settings` lacks a public `auto_retry` field, read it from the session and write via `s.auto_retry`. Verify against `oxicode-cli/src/store/settings.rs` during implementation and use the actual field name. Similarly confirm `ThinkingLevel::parse`, `GlyphSet::parse`, and `advisor` field access (`immune_turns`, `sync_backlog`, `enabled`) match `store/settings.rs`. If `AdvisorSettings.immune_turns` is `Option<usize>` rather than plain, adapt the match arm (parse to `Some(n)`).

- [ ] **Step 4: Declare modules in `oxicode-cli/src/tui_vt/mod.rs`**

Add `pub mod settings_defs;` (plus `pub mod keymap;` once Task 2 lands). Then `use settings_defs::SettingsTab;` where needed.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p oxicode-cli tui_vt::settings_defs`
Expected: PASS

- [ ] **Step 6: Lint + format + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`
Commit: `git add oxicode-cli/src/tui_vt/settings_defs.rs oxicode-cli/src/tui_vt/mod.rs && git commit -m "feat(tui): declarative SettingDef table for /settings"`

---

### Task 2: `keymap.rs` — GlobalAction, KeyCombo, Keymap

**Files:**
- Create: `oxicode-cli/src/tui_vt/keymap.rs`
- Test: `oxicode-cli/src/tui_vt/keymap.rs` (inline `#[cfg(test)]`)
- Modify: `oxicode-cli/src/tui_vt/mod.rs`

**Interfaces:**
- Produces (consumed by Task 5 and 6):
  - `#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub enum GlobalAction { Interrupt, ToggleMultiline, OpenCommandPalette, ToggleQueuePanel, FoldAll, SendNow }`
  - `pub const DEFAULT_KEYBINDINGS: &[(GlobalAction, &str)]`
  - `#[derive(Clone, PartialEq, Eq, Hash, Debug)] pub struct KeyCombo { pub code: crossterm::event::KeyCode, pub modifiers: crossterm::event::KeyModifiers }`
  - `impl KeyCombo { pub fn parse(s: &str) -> Option<Self>; pub fn to_string(&self) -> String; }`
  - `pub struct Keymap { bindings: HashMap<GlobalAction, Vec<KeyCombo>> }`
  - `impl Keymap { pub fn from_settings(overrides: &HashMap<String, Vec<String>>) -> Self; pub fn resolve(&self, key: crossterm::event::KeyEvent) -> Option<GlobalAction>; pub fn set_action(&mut self, action: GlobalAction, combos: Vec<KeyCombo>); }`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn default_resolve_maps_ctrl_p_to_command_palette() {
    let km = Keymap::from_settings(&HashMap::new());
    let ev = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(ev), Some(GlobalAction::OpenCommandPalette));
}

#[test]
fn user_override_adds_instead_of_replacing() {
    let mut o = HashMap::new();
    o.insert("OpenCommandPalette".into(), vec!["Alt+p".into()]);
    let km = Keymap::from_settings(&o);
    // default Ctrl+P still present:
    assert_eq!(km.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
               Some(GlobalAction::OpenCommandPalette));
    // and the new Alt+P:
    assert_eq!(km.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT)),
               Some(GlobalAction::OpenCommandPalette));
}

#[test]
fn keycombo_roundtrips() {
    for s in ["Ctrl+c", "Ctrl+m", "Ctrl+Shift+e", "Ctrl+Enter", "Alt+p"] {
        assert_eq!(KeyCombo::parse(s).unwrap().to_string(), s);
    }
}

#[test]
fn plain_char_is_not_a_global_action() {
    let km = Keymap::from_settings(&HashMap::new());
    assert_eq!(km.resolve(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-cli tui_vt::keymap`
Expected: FAIL (module doesn't exist)

- [ ] **Step 3: Implement `keymap.rs`**

```rust
use std::collections::HashMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GlobalAction {
    Interrupt, ToggleMultiline, OpenCommandPalette,
    ToggleQueuePanel, FoldAll, SendNow,
}

pub const DEFAULT_KEYBINDINGS: &[(GlobalAction, &str)] = &[
    (GlobalAction::Interrupt, "Ctrl+c"),
    (GlobalAction::ToggleMultiline, "Ctrl+m"),
    (GlobalAction::OpenCommandPalette, "Ctrl+p"),
    (GlobalAction::ToggleQueuePanel, "Ctrl+;"),
    (GlobalAction::FoldAll, "Ctrl+e"),
    (GlobalAction::SendNow, "Ctrl+Enter"),
];

impl GlobalAction {
    pub fn name(self) -> &'static str {
        match self {
            GlobalAction::Interrupt => "Interrupt",
            GlobalAction::ToggleMultiline => "ToggleMultiline",
            GlobalAction::OpenCommandPalette => "OpenCommandPalette",
            GlobalAction::ToggleQueuePanel => "ToggleQueuePanel",
            GlobalAction::FoldAll => "FoldAll",
            GlobalAction::SendNow => "SendNow",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        DEFAULT_KEYBINDINGS.iter().map(|(a, _)| *a).find(|a| a.name() == s)
    }
    pub fn all() -> impl Iterator<Item = GlobalAction> {
        DEFAULT_KEYBINDINGS.iter().map(|(a, _)| *a)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct KeyCombo { pub code: KeyCode, pub modifiers: KeyModifiers }

impl KeyCombo {
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split('+');
        let mut mods = KeyModifiers::NONE;
        let mut code = None;
        for p in parts {
            match p {
                "Ctrl" | "Control" => mods |= KeyModifiers::CONTROL,
                "Shift" => mods |= KeyModifiers::SHIFT,
                "Alt" => mods |= KeyModifiers::ALT,
                "Enter" => code = Some(KeyCode::Enter),
                "Esc" | "Escape" => code = Some(KeyCode::Esc),
                "Tab" => code = Some(KeyCode::Tab),
                "Backspace" => code = Some(KeyCode::Backspace),
                c if c.chars().count() == 1 => {
                    code = Some(KeyCode::Char(c.chars().next().unwrap()))
                }
                _ => return None,
            }
        }
        Some(KeyCombo { code: code?, modifiers: mods })
    }

    pub fn to_string(&self) -> String {
        let mut out = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) { out.push_str("Ctrl+"); }
        if self.modifiers.contains(KeyModifiers::ALT) { out.push_str("Alt+"); }
        if self.modifiers.contains(KeyModifiers::SHIFT) { out.push_str("Shift+"); }
        out.push_str(match self.code {
            KeyCode::Enter => "Enter",
            KeyCode::Esc => "Esc",
            KeyCode::Tab => "Tab",
            KeyCode::Backspace => "Backspace",
            KeyCode::Char(c) => {
                // bare-letter combo: emit only the shift-distinguished uppercase
                return if self.modifiers == KeyModifiers::NONE { out.push(c); out }
                       else { out.push(c); out };
            }
            _ => return self.code.to_string(),
        });
        out
    }
}

pub struct Keymap { bindings: HashMap<GlobalAction, Vec<KeyCombo>> }

impl Keymap {
    pub fn from_settings(overrides: &HashMap<String, Vec<String>>) -> Self {
        let mut bindings: HashMap<GlobalAction, Vec<KeyCombo>> = HashMap::new();
        for (action, combo) in DEFAULT_KEYBINDINGS {
            bindings.entry(*action).or_default().push(KeyCombo::parse(combo).expect("valid default"));
        }
        for (name, combos) in overrides {
            if let Some(action) = GlobalAction::from_name(name) {
                let parsed: Vec<KeyCombo> = combos.iter()
                    .filter_map(|c| KeyCombo::parse(c)).collect();
                if !parsed.is_empty() {
                    bindings.insert(action, parsed);
                }
            }
        }
        Keymap { bindings }
    }

    pub fn resolve(&self, key: KeyEvent) -> Option<GlobalAction> {
        self.bindings.iter()
            .find(|(_, combos)| combos.iter().any(|c| c.code == key.code && c.modifiers == key.modifiers))
            .map(|(action, _)| *action)
    }

    pub fn set_action(&mut self, action: GlobalAction, combos: Vec<KeyCombo>) {
        self.bindings.insert(action, combos);
    }
}
```

Note: refine `KeyCombo::to_string` for `Shift` handling — `KeyCode::Char('e')` with `SHIFT` should serialize as `"E"` (crossterm already uppercases shifted char codes). Ensure the round-trip test passes for `"Ctrl+Shift+e"` by normalizing to the single-char uppercase form.

- [ ] **Step 4: Declare module + run tests**

Add `pub mod keymap;` to `oxicode-cli/src/tui_vt/mod.rs`. Run `cargo nextest run -p oxicode-cli tui_vt::keymap`. Expected: PASS.

- [ ] **Step 5: Lint + format + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`
Commit: `git add oxicode-cli/src/tui_vt/keymap.rs oxicode-cli/src/tui_vt/mod.rs && git commit -m "feat(tui): keymap resolver for global shortcuts"`

---

### Task 3: Extend `InlineListSelection` for the settings panel

**Files:**
- Modify: `oxicode-vtui-compat/src/ui_protocol/selection.rs`
- Test: `oxicode-cli/src/tui_vt/main_loop.rs` (existing test module)

**Interfaces:**
- Produces: three new `InlineListSelection` variants used by Task 4/6:
  - `InlineListSelection::SettingsTab(usize)` — tab switch
  - `InlineListSelection::SettingsSection(usize)` — sidebar section jump
  - `InlineListSelection::SettingKeyCapture(String)` — the `SettingKey` name the KeyCapture submenu wants to edit

- [ ] **Step 1: Add the variants**

Append to the `InlineListSelection` enum in `selection.rs` (after the existing variants around line 110):

```rust
    /// Settings-panel tab switch (`/settings` tabs).
    SettingsTab(usize),
    /// Settings-panel sidebar section jump.
    SettingsSection(usize),
    /// Settings-panel key-capture submenu targeting a named `SettingKey`.
    /// Carries the key's Debug name (a `String`) to avoid a dependency from
    /// `oxicode-vtui-compat` onto `oxicode-cli`'s `SettingKey` enum.
    SettingKeyCapture(String),
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p oxicode-vtui-compat`
Expected: PASS

- [ ] **Step 3: Commit**

Commit: `git add oxicode-vtui-compat/src/ui_protocol/selection.rs && git commit -m "feat(vtui): settings-panel list-selection variants"`

---

### Task 4: Rewrite `SettingsCommand` to build from `SETTING_DEFS`

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/registry.rs:168-260`
- Test: `oxicode-cli/src/tui_vt/slash/registry.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `settings_defs::{SettingKey, SettingsTab, SETTING_DEFS, defs_for_tab, get_display_value}`, the new `InlineListSelection` variants from Task 3.
- Produces: `/settings` opens an overlay whose `OverlayState` carries `tabs`/`active_tab`/`sections` (populated by Task 5's render path). `ConfigAction(String)` still carries the `SettingKey` name for toggle/cycle commits, now via `apply_change`.

- [ ] **Step 1: Write the failing test**

Assert that after `/settings` runs, the emitted items correspond to the `General` tab's defs (non-empty, group-contiguous order) — mirror the existing test scaffolding around `registry.rs:1184-1499` (`SlashCommand` test harness with a fake `InlineHandle`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p oxicode-cli tui_vt::slash::registry`
Expected: FAIL (assertion on new behavior)

- [ ] **Step 3: Rewrite `SettingsCommand::execute`**

Replace the literal 6-item `vec![]` with a build from `defs_for_tab(active_tab, settings)`:

```rust
fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    use super::settings_defs::{defs_for_tab, get_display_value, SettingsTab};
    let settings = crate::store::settings::Settings::load().unwrap_or_default();
    // Track panel state on the host so tab switches rebuild items.
    let tab = ctx.state.settings_active_tab;  // new RenderState field (Task 5)
    let defs = defs_for_tab(tab, &settings);

    let mut items: Vec<InlineListItem> = Vec::new();
    for def in &defs {
        // insert a heading item when the group changes:
        if items.last().map(|i| i.badge.as_deref()).flatten().map_or(true,
            |_| true) { /* group-change detection via a parallel last_group var */ }
        let selection = match def.widget {
            SettingWidget::Toggle | SettingWidget::Cycle => {
                Some(InlineListSelection::ConfigAction(
                    format!("{:?}", def.key)))
            }
            _ => None,
        };
        items.push(InlineListItem {
            title: def.label.into(),
            subtitle: Some(def.description.into()),
            badge: Some(get_display_value(def.key, &settings)),
            indent: 0,
            selection,
            search_value: Some(format!("{} {} {:?}", def.label, def.description, def.key)),
        });
    }
    ctx.handle.show_list_modal("Settings".into(), vec![], items, None, Some(search));
    SlashOutcome::Handled
}
```

Full heading-row insertion and per-widget selection mapping is completed during implementation — the key change is that items derive from `SETTING_DEFS`, never a hand-written list. Submenu/multiselect/map-editor selection variants are routed through `handle_inline_event` in Task 5.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p oxicode-cli tui_vt::slash::registry`
Expected: PASS

- [ ] **Step 5: Lint + format + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`
Commit: `git add oxicode-cli/src/tui_vt/slash/registry.rs && git commit -m "feat(tui): /settings builds items from SettingDef table"`

---

### Task 5: `OverlayState` tabs/sections + `render_overlay` sidebar + input-loop dispatch + config handling

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (OverlayState ~588-596, OverlayListItem ~573-577, RenderState, render_overlay ~4507, spawn_input_thread key block ~3279-3345, ConfigAction arm ~2867-2923)
- Test: `oxicode-cli/src/tui_vt/main_loop.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `settings_defs` (Task 1), `keymap` (Task 2), `InlineListSelection` variants (Task 3).
- Produces: `/settings` renders with tabs+sidebar; the 6 hardcoded Ctrl- shortcuts resolve via `Keymap`; `ConfigAction`, `SettingsTab`, `SettingsSection`, `SettingKeyCapture` submissions dispatch.

- [ ] **Step 1: Add fields to `OverlayState` and `RenderState`**

```rust
// OverlayState (all default-empty so non-settings overlays are unaffected):
pub tabs: Vec<String>,
pub active_tab: usize,
pub sections: Vec<String>,   // group names for the active tab
pub active_section: usize,

// RenderState:
pub settings_active_tab: crate::tui_vt::settings_defs::SettingsTab,
pub keymap: std::sync::Arc<std::sync::RwLock<Keymap>>,
```

- [ ] **Step 2: Write the failing tests**

Port `render_overlay_*` test patterns (e.g. `render_overlay_secure_input_...` at ~7866): a test that an overlay with `tabs.len() > 1` and `sections.len() >= 2` renders a sidebar column; a test that `handle_inline_event` receiving `InlineListSelection::SettingsTab(1)` rebuilds the item list for tab 1; a test that the input loop maps a rebound `SendNow` combo (from a `Keymap` whose `SendNow` is `Alt+s`) to the send-now path.

- [ ] **Step 3: `render_overlay` sidebar branch**

Insert before the existing flat-list path in `render_overlay`:

```rust
// Tab bar (only when the overlay is the settings panel, tabs.len() > 1):
if overlay.tabs.len() > 1 {
    // render one line: each tab name, active bold+accent; ←/→ switch
}
// Sidebar split when the active tab has >=2 sections and width >= 60:
if overlay.sections.len() >= 2 && area.width >= 60 {
    let sidebar_w = (22usize.min(overlay.sections.iter().map(|s| s.chars().count()).max().unwrap_or(0)) + 4) as u16;
    // left: section names, active bold+accent
    // right: existing item list with out-of-section rows painted DIM
} else {
    // existing flat list path (heading rows already non-interactive via selection: None)
}
```

- [ ] **Step 4: Replace the 6 hardcoded shortcut checks with `Keymap::resolve`**

In `spawn_input_thread` (around lines 3279-3345), replace the six `if key.code == ... && modifiers.contains(CONTROL)` blocks with:

```rust
if let Some(action) = state.lock().keymap.read().unwrap().resolve(key) {
    let mut s = state.lock();
    match action {
        GlobalAction::Interrupt => { /* body from the current Ctrl+C block, verbatim */ }
        GlobalAction::ToggleMultiline => { /* body verbatim */ }
        GlobalAction::OpenCommandPalette => { /* body verbatim */ }
        GlobalAction::ToggleQueuePanel => { /* body verbatim */ }
        GlobalAction::FoldAll => { s.fold_all(); }
        GlobalAction::SendNow => { /* body verbatim */ }
    }
    continue;
}
```

The bodies are copied unchanged from the existing blocks — only the trigger condition changes. Do NOT touch `route_cancel`/`handle_ctrl_c_key`'s streaming-vs-idle logic.

- [ ] **Step 5: Extend the `ConfigAction` submission arm (~2867-2923)**

The existing arm handles `"thinking_level"`/`"auto_compaction"`/`"advisor"`/`"glyph_set"` by name. Replace the per-name match with a `settings_defs::apply_change(def.key, &mut settings, next_value)` dispatch. For `Cycle` widgets compute `next_value` as the next enum string; for `Toggle` as the inverted bool string. Route the new `SettingsTab(i)`/`SettingsSection(i)`/`SettingKeyCapture(name)` variants to tab/section switching and the KeyCapture submenu respectively.

- [ ] **Step 6: Run tests + lint + format + commit**

Run: `cargo nextest run -p oxicode-cli && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`
Commit: `git add oxicode-cli/src/tui_vt/main_loop.rs && git commit -m "feat(tui): tabbed/sidebar settings overlay + keymap-driven dispatch"`

---

### Task 6: Keybindings tab UI (KeyCapture) and live keymap swap

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (KeyCapture submenu handling)
- Modify: `oxicode-cli/src/tui_vt/settings_defs.rs` (Keybindings map-editor glue)
- Test: `oxicode-cli/src/tui_vt/main_loop.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `Keymap::set_action`, `GlobalAction::from_name`, `KeyCombo::parse`, `InlineListSelection::SettingKeyCapture(String)`.
- Produces: selecting a `GlobalAction` row opens a "Press a key combo (Esc to cancel)…" submenu; the captured `KeyEvent` serializes to a `KeyCombo`, appended to the action's list, persisted to `settings.keybindings`, and the live `RenderState.keymap` is rebuilt and swapped.

- [ ] **Step 1: Write the failing test**

A test that capturing a new combo for `OpenCommandPalette` (via `SettingKeyCapture("OpenCommandPalette")` then submitting a `KeyEvent`) causes the live `keymap` to resolve that new combo to `OpenCommandPalette`, and the old `Ctrl+P` still resolves (additive).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p oxicode-cli tui_vt::main_loop`
Expected: FAIL (new behavior)

- [ ] **Step 3: Implement KeyCapture handling**

In `handle_inline_event`, add a branch for `InlineListSelection::SettingKeyCapture(name)`:

```rust
// set state.overlay = KeyCaptureSubmenu { action: name } — renders
// "Press a key combo (Esc to cancel)…"; the next KeyEvent, if it maps to
// a GlobalAction via GlobalAction::from_name(&name), is serialized and
// appended. On commit:
let action = GlobalAction::from_name(&name).unwrap();
let mut km = state.keymap.write().unwrap();
let combos = km. /* existing combos for action */;
let mut combos = combos.clone();
combos.push(captured_combo);
km.set_action(action, combos);
drop(km);
// persist: settings.keybindings.insert(name, combos.map(to_string)); settings.save();
// rebuild+swap: *state.keymap.write().unwrap() = Keymap::from_settings(&settings.keybindings);
```

Refuse to remove the last combo for an action (`d` guard) — an action with zero keys is a silent trap.

- [ ] **Step 4: Run tests + lint + format + commit**

Run: `cargo nextest run -p oxicode-cli && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`
Commit: `git add oxicode-cli/src/tui_vt/main_loop.rs oxicode-cli/src/tui_vt/settings_defs.rs && git commit -m "feat(tui): live KeyCapture keybinding editor"`

---

### Task 7: `/hooks` read-only dashboard command

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/commands.rs` (add `HooksCommand`, register in `register_extra`)
- Test: `oxicode-cli/src/tui_vt/slash/commands.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `crate::store::settings::Settings` (`hooks: Vec<HookSpec>`), `SlashCtx`.
- Produces: `/hooks` lists each hook's event + command + approval status, mirroring `/mcp`'s dashboard pattern.

- [ ] **Step 1: Write the failing test**

Assert `/hooks` with two configured hooks produces output listing both events.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p oxicode-cli tui_vt::slash::commands`
Expected: FAIL

- [ ] **Step 3: Implement `HooksCommand`**

```rust
struct HooksCommand;
impl SlashCommand for HooksCommand {
    fn name(&self) -> &'static str { "hooks" }
    fn description(&self) -> &'static str { "List configured event hooks (read-only)" }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let settings = crate::store::settings::Settings::load().unwrap_or_default();
        if settings.hooks.is_empty() {
            ctx.reply(InlineMessageKind::Info,
                "No hooks configured. Edit [[hooks]] in ~/.oxicode/settings.toml.".into());
        } else {
            let mut out = String::new();
            for h in &settings.hooks {
                out.push_str(&format!("- [{}] {}\n", h.event, h.command));
            }
            ctx.reply(InlineMessageKind::Info, out);
        }
        SlashOutcome::Handled
    }
}
```

Register in `register_extra`. Confirm `HookSpec`'s actual field names (`event`, `command`) against `oxicode-sdk/src/ports/` `HookSpec` definition and adapt.

- [ ] **Step 4: Run tests + lint + format + commit**

Run: `cargo nextest run -p oxicode-cli && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`
Commit: `git add oxicode-cli/src/tui_vt/slash/commands.rs && git commit -m "feat(tui): /hooks read-only dashboard"`

---

### Task 8: Smoke test the full panel in a live TUI

**Files:**
- None (verification only).

**Interfaces:**
- Consumes: all prior tasks.

- [ ] **Step 1: Build the binary**

Run: `cargo build -p oxicode-cli`

- [ ] **Step 2: Launch the TUI in a PTY and exercise the panel**

Launch via `hub` (project-scoped process) with a PTY. Then:
- `/settings` opens; verify the tab bar renders and ←/→ switch tabs.
- A tab with ≥2 sections (e.g. Advisor & Memory) renders the left sidebar.
- Toggle `Auto-compaction`; confirm it flips immediately and `~/.oxicode/settings.toml` reflects it.
- Rebind `OpenCommandPalette` to a new combo via the Keybindings tab; confirm Ctrl+P no longer opens the palette and the new combo does, without restarting.
- `/hooks` shows the configured hooks (or the "none configured" message).
- `/tools` still lists tools; `/model` picker still works (regression).

- [ ] **Step 3: Report + final commit (if any fixes surfaced)**

Fix any issues found and commit as `fix(tui): ...`. No further commits otherwise.

---

## Self-Review

**Spec coverage:**
- §1-2 (problem/goals): Tasks 1, 4, 5, 6.
- §4 (SettingDef table): Task 1.
- §5 (widget catalog): Task 1 (defs) + Task 5/6 (dispatch). Multiselect `disabled_tools` options source (`ctx.session.agent_ref().tools()`) is wired in Task 5's ConfigAction handling — confirmed available via the existing `/tools` command pattern.
- §6 (layout/sidebar): Task 5.
- §7 (interaction/search/conditions): Task 4 (`defs_for_tab` condition filter) + Task 5 (search corpus across tabs).
- §8 (field mapping): Task 1's `SETTING_DEFS` table.
- §9 (keymap dispatch): Tasks 2, 5, 6.
- §10 (/hooks): Task 7.
- §11 (data model): no `Settings` field changes — respected across all tasks.
- §12 (testing): inline tests per task + Task 8 smoke.
- §13 risks: `ArcSwap` vs `RwLock` — Task 5 uses `RwLock` (the spec's acceptable fallback, avoids a new dependency).
- §14-15: roadmap/deferred — not in plan (correctly out of scope).

**Placeholder scan:** No TBD/TODO. Field-name caveats (`auto_retry`, `advisor.immune_turns`, `HookSpec.event`) are explicitly flagged as verify-during-implementation with the exact fallback behavior, not vague "handle edge cases".

**Type consistency:** `GlobalAction`/`KeyCombo`/`Keymap` names consistent across Tasks 2/5/6. `SettingKey`/`SettingsTab`/`SettingDef`/`SETTING_DEFS`/`defs_for_tab`/`get_display_value`/`apply_change` consistent across Tasks 1/4/5/6. `InlineListSelection::SettingsTab/SettingsSection/SettingKeyCapture` consistent across Tasks 3/5/6.
