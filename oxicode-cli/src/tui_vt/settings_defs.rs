//! Declarative `SettingDef` table for the `/settings` TUI panel.
//!
//! Task 1 of the settings-panel rewrite. Owns the static metadata
//! (key, tab, group, label, description, widget kind, conditional
//! visibility) for every editable setting plus the typed accessors
//! `get_display_value` / `apply_change`. Later tasks (the renderer,
//! the per-widget editors, the map-editor screens) consume this table
//! and never reach into `Settings` fields directly.
//!
//! Constraints carried from the task brief:
//! - `Settings` itself is unchanged — this module adds ACCESS, not
//!   persisted state.
//! - `apply_change` for `DisabledTools` / `ModelRoles` / `Keybindings`
//!   must `bail!` so a stray scalar call is loud, never a silent no-op
//!   (their editors land in Tasks 5/6).

use crate::store::settings::{Settings, ThinkingLevel};
use std::str::FromStr;

/// Local Display helpers for `ThinkingLevel` / `EditFormat` —
/// kept here rather than on the source enums to avoid touching
/// `store::settings` (Task 1 adds access only, never new
/// persisted state or impls).
fn thinking_level_to_str(v: ThinkingLevel) -> &'static str {
    match v {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
}

fn edit_format_to_str(v: crate::store::settings::EditFormat) -> &'static str {
    use crate::store::settings::EditFormat;
    match v {
        EditFormat::Hashline => "hashline",
        EditFormat::StrReplace => "str_replace",
    }
}

/// Stable identifier for every editable setting in the panel.
///
/// `Pointer` rows (e.g. `Theme`, `Model`, `Hooks`, `*Paths`) are
/// read-only views into out-of-band state (slash commands, `oxicode
/// config`) and intentionally have no `apply_change` arm.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SettingKey {
    // Model / Defaults
    ThinkingLevel,
    AutoCompaction,
    GlyphSet,
    EditFormat,
    // General / Behaviour
    ExtensionsEnabled,
    SessionHistorySize,
    ToolTimeoutSecs,
    AskTimeoutSecs,
    DisabledTools,
    CommitToolEnabled,
    TodoPanelEnabled,
    AgentHubEnabled,
    MermaidRenderEnabled,
    SnapcompactEnabled,
    // Advisor & Memory
    MemoryEnabled,
    TtsrEnabled,
    TtsrInterruptMode,
    AdvisorEnabled,
    AdvisorSyncBacklog,
    AdvisorImmuneTurns,
    // Map editors (Tasks 5/6)
    ModelRoles,
    Keybindings,
    // Pointer rows (read-only)
    Theme,
    Model,
    CustomProviders,
    Hooks,
    ExtensionPaths,
    SkillPaths,
    PromptPaths,
    ThemePaths,
}

/// Top-level tab the row renders under.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SettingsTab {
    General,
    Model,
    Interaction,
    Tools,
    Ui,
    AdvisorMemory,
    Keybindings,
    Advanced,
}

/// Widget kind used to edit the row. The renderer dispatches on this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingWidget {
    /// Boolean on/off.
    Toggle,
    /// Discrete cycle (e.g. `Off` → `Minimal` → `Low` …).
    Cycle,
    /// Pick from a fixed enum-of-strings list (rendered as a submenu).
    SubmenuSelect(&'static [&'static str]),
    /// Free-form text input.
    Text,
    /// Set of strings toggled individually.
    Multiselect,
    /// `HashMap<String, String>` edited via a dedicated screen.
    MapEditor,
    /// Read-only summary pointing the user at a slash command.
    Pointer,
}

/// Declarative metadata for a single setting row.
pub struct SettingDef {
    pub key: SettingKey,
    pub tab: SettingsTab,
    /// Group label used to render a section header. Must be non-empty;
    /// groups are rendered contiguously in declaration order.
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub widget: SettingWidget,
    /// Optional visibility predicate. `None` ⇒ always visible.
    /// `fn` pointer (not closure) so the def table stays `const`.
    pub condition: Option<fn(&Settings) -> bool>,
}

/// The full table. Order matters: `defs_for_tab` preserves it and the
/// renderer renders groups contiguously, so visually related rows must
/// sit next to each other (and a new group must start with a fresh
/// header row).
pub const SETTING_DEFS: &[SettingDef] = &[
    // ── General ──────────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::ExtensionsEnabled,
        tab: SettingsTab::General,
        group: "Behavior",
        label: "Extensions",
        description: "Load WASM/native extensions",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::SessionHistorySize,
        tab: SettingsTab::General,
        group: "Behavior",
        label: "Session history size",
        description: "Entries kept in memory",
        widget: SettingWidget::Text,
        condition: None,
    },
    SettingDef {
        key: SettingKey::EditFormat,
        tab: SettingsTab::General,
        group: "Behavior",
        label: "Edit format",
        description: "str_replace or hashline",
        widget: SettingWidget::Cycle,
        condition: None,
    },
    // ── Model ────────────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::ThinkingLevel,
        tab: SettingsTab::Model,
        group: "Defaults",
        label: "Thinking level",
        description: "Reasoning effort",
        widget: SettingWidget::Cycle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::ModelRoles,
        tab: SettingsTab::Model,
        group: "Defaults",
        label: "Model roles",
        description: "role -> model pattern assignments",
        widget: SettingWidget::MapEditor,
        condition: None,
    },
    SettingDef {
        key: SettingKey::Theme,
        tab: SettingsTab::Model,
        group: "Pointers",
        label: "Theme",
        description: "Use /theme to change",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    SettingDef {
        key: SettingKey::Model,
        tab: SettingsTab::Model,
        group: "Pointers",
        label: "Model",
        description: "Use /model to change",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    // ── Interaction ──────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::AutoCompaction,
        tab: SettingsTab::Interaction,
        group: "Compaction",
        label: "Auto-compaction",
        description: "Compact when context exceeds window",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::ToolTimeoutSecs,
        tab: SettingsTab::Interaction,
        group: "Timeouts",
        label: "Tool timeout (s)",
        description: "Tool execution timeout",
        widget: SettingWidget::Text,
        condition: None,
    },
    SettingDef {
        key: SettingKey::AskTimeoutSecs,
        tab: SettingsTab::Interaction,
        group: "Timeouts",
        label: "Ask timeout (s)",
        description: "Ask overlay timeout",
        widget: SettingWidget::Text,
        condition: None,
    },
    // ── Tools ────────────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::DisabledTools,
        tab: SettingsTab::Tools,
        group: "Tools",
        label: "Disabled tools",
        description: "Tools turned off for the agent",
        widget: SettingWidget::Multiselect,
        condition: None,
    },
    SettingDef {
        key: SettingKey::CommitToolEnabled,
        tab: SettingsTab::Tools,
        group: "Tools",
        label: "Commit tool",
        description: "Enable the Commit tool",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::CustomProviders,
        tab: SettingsTab::Tools,
        group: "Pointers",
        label: "Custom providers",
        description: "Use /providers to manage",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    SettingDef {
        key: SettingKey::Hooks,
        tab: SettingsTab::Tools,
        group: "Pointers",
        label: "Hooks",
        description: "Use /hooks to view",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    // ── UI ───────────────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::GlyphSet,
        tab: SettingsTab::Ui,
        group: "Appearance",
        label: "Icons",
        description: "unicode / ascii / nerd glyph set",
        widget: SettingWidget::Cycle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::TodoPanelEnabled,
        tab: SettingsTab::Ui,
        group: "Panels",
        label: "Todo panel",
        description: "Sticky todo panel",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::AgentHubEnabled,
        tab: SettingsTab::Ui,
        group: "Panels",
        label: "Agent hub",
        description: "Ctrl+h /agents overlay",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::MermaidRenderEnabled,
        tab: SettingsTab::Ui,
        group: "Panels",
        label: "Mermaid",
        description: "Render mermaid diagrams",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::SnapcompactEnabled,
        tab: SettingsTab::Ui,
        group: "Panels",
        label: "Snapcompact",
        description: "PNG-frame compactor",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    // ── Advisor & Memory ─────────────────────────────────────────────
    SettingDef {
        key: SettingKey::AdvisorEnabled,
        tab: SettingsTab::AdvisorMemory,
        group: "Advisor",
        label: "Advisor",
        description: "Read-only reviewer shadowing the agent",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::AdvisorSyncBacklog,
        tab: SettingsTab::AdvisorMemory,
        group: "Advisor",
        label: "Sync backlog",
        description: "off / sync / async",
        widget: SettingWidget::SubmenuSelect(&["off", "sync", "async"]),
        condition: Some(|s| s.advisor.enabled),
    },
    SettingDef {
        key: SettingKey::AdvisorImmuneTurns,
        tab: SettingsTab::AdvisorMemory,
        group: "Advisor",
        label: "Immune turns",
        description: "Turns the advisor skips",
        widget: SettingWidget::Text,
        condition: Some(|s| s.advisor.enabled),
    },
    SettingDef {
        key: SettingKey::MemoryEnabled,
        tab: SettingsTab::AdvisorMemory,
        group: "Memory",
        label: "Memory",
        description: "Oxibrain durable memory",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::TtsrEnabled,
        tab: SettingsTab::AdvisorMemory,
        group: "Memory",
        label: "TTSR",
        description: "Time-traveling stream rules",
        widget: SettingWidget::Toggle,
        condition: None,
    },
    SettingDef {
        key: SettingKey::TtsrInterruptMode,
        tab: SettingsTab::AdvisorMemory,
        group: "Memory",
        label: "TTSR mode",
        description: "prose_only or rules",
        widget: SettingWidget::SubmenuSelect(&["prose_only", "rules"]),
        condition: Some(|s| s.ttsr_enabled),
    },
    // ── Keybindings ──────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::Keybindings,
        tab: SettingsTab::Keybindings,
        group: "Keybindings",
        label: "Keybindings",
        description: "Global shortcuts",
        widget: SettingWidget::MapEditor,
        condition: None,
    },
    // ── Advanced ─────────────────────────────────────────────────────
    SettingDef {
        key: SettingKey::ExtensionPaths,
        tab: SettingsTab::Advanced,
        group: "Resources",
        label: "Extensions",
        description: "Use `oxicode config` to manage",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    SettingDef {
        key: SettingKey::SkillPaths,
        tab: SettingsTab::Advanced,
        group: "Resources",
        label: "Skills",
        description: "Use `oxicode config` to manage",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    SettingDef {
        key: SettingKey::PromptPaths,
        tab: SettingsTab::Advanced,
        group: "Resources",
        label: "Prompts",
        description: "Use `oxicode config` to manage",
        widget: SettingWidget::Pointer,
        condition: None,
    },
    SettingDef {
        key: SettingKey::ThemePaths,
        tab: SettingsTab::Advanced,
        group: "Resources",
        label: "Themes",
        description: "Use `oxicode config` to manage",
        widget: SettingWidget::Pointer,
        condition: None,
    },
];

/// Return every def that belongs to `tab`, in declaration order, with
/// each row's `condition` evaluated against `s`.
///
/// Groups are preserved contiguously because `SETTING_DEFS` is
/// already sorted by (tab, group, declaration).
pub fn defs_for_tab(tab: SettingsTab, s: &Settings) -> Vec<&'static SettingDef> {
    SETTING_DEFS
        .iter()
        .filter(|d| d.tab == tab && d.condition.is_none_or(|c| c(s)))
        .collect()
}

/// Render the row's current value as a string. The renderer uses this
/// for both the right-hand summary cell and (for `Cycle` widgets) the
/// "next value" preview.
pub fn get_display_value(key: SettingKey, s: &Settings) -> String {
    use SettingKey::*;
    match key {
        ThinkingLevel => thinking_level_to_str(s.thinking_level).to_string(),
        AutoCompaction => s.auto_compaction.to_string(),
        GlyphSet => s.glyph_set.to_string(),
        EditFormat => edit_format_to_str(s.edit_format).to_string(),
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
        ExtensionPaths => format!("{}", s.extensions.len()),
        SkillPaths => format!("{}", s.skills.len()),
        PromptPaths => format!("{}", s.prompts.len()),
        ThemePaths => format!("{}", s.themes.len()),
    }
}

/// Apply a scalar edit.
///
/// `new` is the stringified new value:
/// - `Toggle`: `"true"` / `"false"`
/// - `Cycle`: the variant's `Display` form
/// - `SubmenuSelect`: one of the allowed strings
/// - `Text`: free-form, parsed via `FromStr` on the target field type
///
/// Returns `Err` on:
/// - parse failure (unknown cycle variant, non-numeric text)
/// - edits to `DisabledTools` / `ModelRoles` / `Keybindings` (their
///   structured editors are Tasks 5/6 — must not silently no-op)
/// - edits to any `Pointer` row (read-only by design)
pub fn apply_change(key: SettingKey, s: &mut Settings, new: String) -> anyhow::Result<()> {
    use SettingKey::*;
    match key {
        AutoCompaction => s.auto_compaction = new == "true",
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
            s.thinking_level = crate::store::settings::parse_thinking_level(&new)
                .ok_or_else(|| anyhow::anyhow!("invalid thinking level: {new}"))?;
        }
        GlyphSet => {
            s.glyph_set = crate::symbols::GlyphSet::from_str(&new)
                .map_err(|e| anyhow::anyhow!("invalid glyph set: {e}"))?
        }
        EditFormat => {
            use crate::store::settings::EditFormat as Ef;
            s.edit_format = match new.as_str() {
                "hashline" => Ef::Hashline,
                "str_replace" => Ef::StrReplace,
                _ => anyhow::bail!("invalid edit format: {new} (expected hashline|str_replace)"),
            };
        }
        SessionHistorySize => s.session_history_size = new.parse()?,
        ToolTimeoutSecs => s.tool_timeout_seconds = new.parse()?,
        AskTimeoutSecs => s.ask_timeout_secs = new.parse()?,
        AdvisorImmuneTurns => s.advisor.immune_turns = new.parse()?,
        TtsrInterruptMode => s.ttsr_interrupt_mode = new,
        AdvisorSyncBacklog => s.advisor.sync_backlog = new,
        // Editors land in Tasks 5/6 — a stray scalar call here must be
        // loud, never a silent no-op.
        DisabledTools => anyhow::bail!("disabled_tools edited via its multiselect (Task 5)"),
        ModelRoles => anyhow::bail!("model_roles edited via its map-editor (Task 6)"),
        Keybindings => anyhow::bail!("keybindings edited via its map-editor (Task 6)"),
        // Pointer rows are read-only by design (slash-command driven).
        Theme | Model | CustomProviders | Hooks | ExtensionPaths | SkillPaths | PromptPaths
        | ThemePaths => anyhow::bail!("read-only"),
    }
    Ok(())
}

/// Toggle-helper used by the `DisabledTools` multiselect editor
/// (Task 5). `enabled = true` removes the tool from the disabled set;
/// `enabled = false` adds it if absent.
pub fn toggle_disabled_tool(s: &mut Settings, tool: &str, enabled: bool) {
    if enabled {
        s.disabled_tools.retain(|t| t != tool);
    } else if !s.disabled_tools.iter().any(|t| t == tool) {
        s.disabled_tools.push(tool.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry must have a non-empty group, AND within each tab
    /// every group label must appear in a single contiguous run —
    /// `[Defaults, Pointers, Defaults]` is the exact regression this
    /// test is meant to catch. Cross-group transitions is fine (e.g.
    /// `[Defaults, Pointers]`), as long as no group label reappears
    /// after a different one has been seen within the same tab.
    #[test]
    fn groups_are_contiguous_and_nonempty() {
        for def in SETTING_DEFS {
            assert!(
                !def.group.is_empty(),
                "SettingDef for {:?} has empty group",
                def.key,
            );
        }

        // For each tab, walk the groups in declaration order and
        // record which group labels we've already "completed".
        // Re-using a completed label splits the section.
        let mut completed: std::collections::HashMap<
            SettingsTab,
            std::collections::HashSet<&'static str>,
        > = std::collections::HashMap::new();
        let mut current_group_per_tab: std::collections::HashMap<SettingsTab, &'static str> =
            std::collections::HashMap::new();

        for def in SETTING_DEFS {
            if let Some(prev) = current_group_per_tab.get(&def.tab)
                && *prev != def.group
            {
                completed.entry(def.tab).or_default().insert(*prev);
            }
            let done = completed
                .get(&def.tab)
                .is_some_and(|c| c.contains(def.group));
            assert!(
                !done,
                "group {:?} reappears non-contiguously in tab {:?}",
                def.group, def.tab,
            );
            current_group_per_tab.insert(def.tab, def.group);
        }
    }

    /// Round-trip `Cycle` widget: parse → display preserves the variant.
    #[test]
    fn thinking_level_round_trips() {
        let mut s = Settings::default();
        apply_change(SettingKey::ThinkingLevel, &mut s, "high".into()).unwrap();
        assert_eq!(s.thinking_level, ThinkingLevel::High);
        assert_eq!(get_display_value(SettingKey::ThinkingLevel, &s), "high");

        // Invalid input is rejected.
        assert!(apply_change(SettingKey::ThinkingLevel, &mut s, "yikes".into()).is_err());
    }

    /// Round-trip `Toggle` widget.
    #[test]
    fn auto_compaction_round_trips() {
        let mut s = Settings::default();
        // Default is `true`; flip to `false` then back.
        apply_change(SettingKey::AutoCompaction, &mut s, "false".into()).unwrap();
        assert!(!s.auto_compaction);
        assert_eq!(get_display_value(SettingKey::AutoCompaction, &s), "false");
        apply_change(SettingKey::AutoCompaction, &mut s, "true".into()).unwrap();
        assert!(s.auto_compaction);
    }

    /// Round-trip `Text` widget: parse → display preserves the number.
    #[test]
    fn tool_timeout_secs_round_trips() {
        let mut s = Settings::default();
        apply_change(SettingKey::ToolTimeoutSecs, &mut s, "42".into()).unwrap();
        assert_eq!(s.tool_timeout_seconds, 42);
        assert_eq!(get_display_value(SettingKey::ToolTimeoutSecs, &s), "42");
        // Non-numeric input is rejected (no silent fallback).
        assert!(apply_change(SettingKey::ToolTimeoutSecs, &mut s, "abc".into()).is_err());
    }

    /// `defs_for_tab` respects the `condition` predicate: with the
    /// advisor disabled, the advisor's child rows are hidden; with it
    /// enabled, they reappear.
    #[test]
    fn defs_for_tab_respects_condition() {
        let mut s = Settings::default();
        // Default: advisor disabled → child rows hidden.
        let keys_disabled: Vec<SettingKey> = defs_for_tab(SettingsTab::AdvisorMemory, &s)
            .iter()
            .map(|d| d.key)
            .collect();
        assert!(!keys_disabled.contains(&SettingKey::AdvisorSyncBacklog));
        assert!(!keys_disabled.contains(&SettingKey::AdvisorImmuneTurns));

        // Enable the advisor → child rows appear.
        s.advisor.enabled = true;
        let keys_enabled: Vec<SettingKey> = defs_for_tab(SettingsTab::AdvisorMemory, &s)
            .iter()
            .map(|d| d.key)
            .collect();
        assert!(keys_enabled.contains(&SettingKey::AdvisorSyncBacklog));
        assert!(keys_enabled.contains(&SettingKey::AdvisorImmuneTurns));

        // Same predicate for TTSR.
        assert!(!keys_enabled.contains(&SettingKey::TtsrInterruptMode));
        s.ttsr_enabled = true;
        let keys_ttsr: Vec<SettingKey> = defs_for_tab(SettingsTab::AdvisorMemory, &s)
            .iter()
            .map(|d| d.key)
            .collect();
        assert!(keys_ttsr.contains(&SettingKey::TtsrInterruptMode));
    }

    /// Calling `apply_change` on the structured-editor rows must
    /// surface a loud error rather than silently no-op.
    #[test]
    fn structured_editor_rows_reject_scalar_edits() {
        let mut s = Settings::default();
        let err = apply_change(SettingKey::DisabledTools, &mut s, "anything".into()).unwrap_err();
        assert!(format!("{err}").contains("multiselect"));
        let err = apply_change(SettingKey::ModelRoles, &mut s, "anything".into()).unwrap_err();
        assert!(format!("{err}").contains("map-editor"));
        let err = apply_change(SettingKey::Keybindings, &mut s, "anything".into()).unwrap_err();
        assert!(format!("{err}").contains("map-editor"));
    }

    /// Pointer rows are read-only.
    #[test]
    fn pointer_rows_are_read_only() {
        let mut s = Settings::default();
        for key in [
            SettingKey::Theme,
            SettingKey::Model,
            SettingKey::CustomProviders,
            SettingKey::Hooks,
            SettingKey::ExtensionPaths,
            SettingKey::SkillPaths,
            SettingKey::PromptPaths,
            SettingKey::ThemePaths,
        ] {
            assert!(
                apply_change(key, &mut s, "anything".into()).is_err(),
                "{key:?} should reject scalar edits",
            );
        }
    }

    /// `toggle_disabled_tool` is idempotent: adding an already-absent
    /// tool adds it; toggling the same tool twice is a no-op.
    #[test]
    fn toggle_disabled_tool_idempotent() {
        let mut s = Settings::default();
        assert!(s.disabled_tools.is_empty());
        toggle_disabled_tool(&mut s, "bash", false);
        assert_eq!(s.disabled_tools, vec!["bash".to_string()]);
        toggle_disabled_tool(&mut s, "bash", false);
        assert_eq!(s.disabled_tools, vec!["bash".to_string()]);
        toggle_disabled_tool(&mut s, "bash", true);
        assert!(s.disabled_tools.is_empty());
    }
}
