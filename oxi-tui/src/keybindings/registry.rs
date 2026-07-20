//! Keybinding registry — maps Actions to key sequences.
//!
//! Provides a declarative keybinding system with default bindings,
//! user overrides, and lookup by key or action.

use super::keys::{KeyId, parse_key_id};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Action enum
// ---------------------------------------------------------------------------

/// All bindable actions in the TUI.
///
/// Organized by category:
/// - **Editor navigation**: cursor movement within the input area
/// - **Editor editing**: text modification operations
/// - **Input**: submit, newline, tab completion
/// - **View**: scrolling and viewport control
/// - **App**: application-level actions (quit, overlay, etc.)
/// - **Selection**: list/overlay navigation
/// - **Queue**: steering queue panel
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter, strum::Display)]
pub enum Action {
    // ── Editor navigation ──────────────────────────────────────
    /// Move cursor left one character.
    CursorLeft,
    /// Move cursor right one character.
    CursorRight,
    /// Move cursor left one word.
    CursorWordLeft,
    /// Move cursor right one word.
    CursorWordRight,
    /// Move cursor to the beginning of the line.
    CursorLineStart,
    /// Move cursor to the end of the line.
    CursorLineEnd,

    // ── Editor editing ─────────────────────────────────────────
    /// Delete character before cursor.
    DeleteCharBackward,
    /// Delete character after cursor.
    DeleteCharForward,
    /// Delete word before cursor.
    DeleteWordBackward,
    /// Delete word after cursor.
    DeleteWordForward,
    /// Delete from cursor to line start.
    DeleteToLineStart,
    /// Delete from cursor to line end.
    DeleteToLineEnd,
    /// Undo last edit.
    Undo,

    // ── Input ──────────────────────────────────────────────────
    /// Submit the current input.
    Submit,
    /// Insert a newline (soft break).
    NewLine,
    /// Accept completion / tab.
    Tab,
    /// Cycle thinking level.
    CycleThinking,

    // ── View ───────────────────────────────────────────────────
    /// Scroll the chat view up.
    ScrollUp,
    /// Scroll the chat view down.
    ScrollDown,
    /// Scroll the chat view up by a page.
    ScrollPageUp,
    /// Scroll the chat view down by a page.
    ScrollPageDown,
    /// Navigate to previous history entry.
    HistoryUp,
    /// Navigate to next history entry.
    HistoryDown,

    // ── App ────────────────────────────────────────────────────
    /// Quit / exit the application.
    Quit,
    /// Cancel current operation or dismiss overlay.
    Cancel,
    /// Open model selector overlay.
    OpenModelSelect,
    /// Open provider setup overlay.
    OpenProviderSetup,
    /// Toggle routing status overlay.
    ToggleRouting,
    /// Toggle steering queue panel.
    ToggleQueue,
    /// Toggle expand/collapse of the topmost visible thinking or tool block.
    ToggleExpand,
    /// Copy last code block to clipboard.
    CopyCodeBlock,
    /// Open last image in viewer.
    OpenImage,

    // ── Completion ─────────────────────────────────────────────
    /// Navigate to next completion suggestion.
    CompletionNext,
    /// Navigate to previous completion suggestion.
    CompletionPrev,
    /// Dismiss completion popup.
    CompletionDismiss,
    /// Accept selected completion.
    CompletionAccept,

    // ── Sticky panels (added in oxi-pager redesign, see spec §5.6) ─
    /// Toggle the todo sticky panel.
    ToggleTodo,
    /// Toggle the issues sticky panel.
    ToggleIssues,
    /// Toggle the agent hub overlay.
    ToggleHub,
    /// Toggle the LSP diagnostics panel.
    ToggleLsp,
}

// ---------------------------------------------------------------------------
// KeybindingsManager
// ---------------------------------------------------------------------------

/// Manages the mapping between Actions and KeyIds.
///
/// Supports layered bindings:
/// 1. **Defaults** — hardcoded sensible defaults
/// 2. **User overrides** — loaded from `settings.toml`
/// 3. **Resolved** — merged view (user overrides take priority)
pub struct KeybindingsManager {
    /// Default bindings (hardcoded).
    defaults: HashMap<Action, Vec<KeyId>>,
    /// User overrides from config.
    user_overrides: HashMap<Action, Vec<KeyId>>,
    /// Merged bindings (user overrides replace defaults for the same action).
    resolved: HashMap<Action, Vec<KeyId>>,
    /// Reverse lookup: KeyId → Action (for fast key dispatch).
    key_to_action: HashMap<KeyId, Action>,
}

impl fmt::Debug for KeybindingsManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeybindingsManager")
            .field("resolved_count", &self.resolved.len())
            .field("key_to_action_count", &self.key_to_action.len())
            .finish()
    }
}

impl KeybindingsManager {
    /// Create a new manager with default bindings.
    pub fn new() -> Self {
        let mut mgr = Self {
            defaults: HashMap::new(),
            user_overrides: HashMap::new(),
            resolved: HashMap::new(),
            key_to_action: HashMap::new(),
        };
        mgr.init_defaults();
        mgr.rebuild();
        mgr
    }

    /// Apply user overrides from a config map.
    ///
    /// The config map keys are action names (matching `Action` variant names)
    /// and values are keybinding strings (e.g., "Ctrl+c") or arrays of strings.
    pub fn set_user_bindings(&mut self, config: &HashMap<String, Vec<String>>) {
        self.user_overrides.clear();
        for (action_name, key_strings) in config {
            if let Some(action) = parse_action(action_name) {
                let keys: Vec<KeyId> = key_strings.iter().filter_map(|s| parse_key_id(s)).collect();
                if !keys.is_empty() {
                    self.user_overrides.insert(action, keys);
                }
            }
        }
        self.rebuild();
    }

    /// Look up the action for a given key.
    ///
    /// Returns `None` if no binding matches.
    pub fn match_action(&self, key_id: &KeyId) -> Option<Action> {
        self.key_to_action.get(key_id).copied()
    }

    /// Get all key bindings for a given action.
    pub fn keys_for_action(&self, action: Action) -> &[KeyId] {
        self.resolved.get(&action).map_or(&[], |v| v)
    }

    /// Get the primary (first) key binding for an action, for display.
    pub fn primary_key_for(&self, action: Action) -> Option<&KeyId> {
        self.resolved.get(&action).and_then(|v| v.first())
    }

    /// Get all resolved bindings.
    pub fn all_bindings(&self) -> &HashMap<Action, Vec<KeyId>> {
        &self.resolved
    }

    /// Rebuild the resolved and reverse-lookup maps.
    fn rebuild(&mut self) {
        // Start from defaults
        self.resolved = self.defaults.clone();

        // Apply user overrides (replace entire key list for the action)
        for (action, keys) in &self.user_overrides {
            self.resolved.insert(*action, keys.clone());
        }

        // Build reverse lookup
        self.key_to_action.clear();
        for (action, keys) in &self.resolved {
            for key in keys {
                // First binding wins in case of conflict
                self.key_to_action.entry(key.clone()).or_insert(*action);
            }
        }
    }

    /// Initialize the default keybindings.
    ///
    /// Port of pi's TUI_KEYBINDINGS defaults.
    fn init_defaults(&mut self) {
        use Action::*;

        let defaults: Vec<(Action, Vec<&str>)> = vec![
            // ── Editor navigation ──
            (CursorLeft, vec!["Left"]),
            (CursorRight, vec!["Right"]),
            (CursorWordLeft, vec!["Ctrl+Left"]),
            (CursorWordRight, vec!["Ctrl+Right"]),
            (CursorLineStart, vec!["Home"]),
            (CursorLineEnd, vec!["End"]),
            // ── Editor editing ──
            (DeleteCharBackward, vec!["Backspace"]),
            (DeleteCharForward, vec!["Delete"]),
            (DeleteWordBackward, vec!["Ctrl+Backspace"]),
            (DeleteWordForward, vec!["Ctrl+Delete"]),
            (DeleteToLineStart, vec!["Ctrl+u"]),
            (DeleteToLineEnd, vec!["Ctrl+k"]),
            (Undo, vec!["Ctrl+z"]),
            // ── Input ──
            (Submit, vec!["Enter"]),
            (NewLine, vec!["Alt+Enter", "Ctrl+j"]),
            (Tab, vec!["Tab"]),
            (CycleThinking, vec!["BackTab", "Shift+Tab"]),
            // ── View ──
            (ScrollUp, vec!["Up"]),
            (ScrollDown, vec!["Down"]),
            (ScrollPageUp, vec!["PageUp"]),
            (ScrollPageDown, vec!["PageDown"]),
            (HistoryUp, vec![]),   // Bound conditionally when input is empty
            (HistoryDown, vec![]), // Bound conditionally when input is empty
            // ── App ──
            (Quit, vec!["Ctrl+c"]),
            (Cancel, vec!["Esc"]),
            (OpenModelSelect, vec!["Ctrl+m"]),
            (OpenProviderSetup, vec!["Ctrl+o"]),
            (ToggleRouting, vec!["Ctrl+r"]),
            (ToggleQueue, vec!["Ctrl+q"]),
            (ToggleExpand, vec!["Ctrl+t"]),
            (CopyCodeBlock, vec!["Ctrl+y"]),
            (OpenImage, vec!["Ctrl+i"]),
            // ── Completion ──
            (CompletionNext, vec![]), // Handled via dedicated Tab/Enter logic
            (CompletionPrev, vec![]),
            (CompletionDismiss, vec![]), // Esc handled via Cancel
            (CompletionAccept, vec![]),
            // ── Sticky panels (oxi-pager redesign, spec §5.6) ────
            // F5-F8 chosen because Ctrl+T (ToggleExpand) and Ctrl+I
            // (OpenImage) are already taken by other actions.
            (ToggleTodo, vec!["F5"]),
            (ToggleIssues, vec!["F6"]),
            (ToggleHub, vec!["F7"]),
            (ToggleLsp, vec!["F8"]),
        ];
        for (action, key_strings) in defaults {
            let keys: Vec<KeyId> = key_strings.into_iter().filter_map(parse_key_id).collect();
            self.defaults.insert(action, keys);
        }
    }
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse an action name string into an Action variant.
///
/// Matches on the exact variant name (case-insensitive).
fn parse_action(s: &str) -> Option<Action> {
    use Action::*;
    match s.to_ascii_lowercase().as_str() {
        "cursorleft" => Some(CursorLeft),
        "cursorright" => Some(CursorRight),
        "cursorwordleft" => Some(CursorWordLeft),
        "cursorwordright" => Some(CursorWordRight),
        "cursorlinestart" => Some(CursorLineStart),
        "cursorlineend" => Some(CursorLineEnd),
        "deletecharbackward" => Some(DeleteCharBackward),
        "deletecharforward" => Some(DeleteCharForward),
        "deletewordbackward" => Some(DeleteWordBackward),
        "deletewordforward" => Some(DeleteWordForward),
        "deletetolinestart" => Some(DeleteToLineStart),
        "deletetolineend" => Some(DeleteToLineEnd),
        "undo" => Some(Undo),
        "submit" => Some(Submit),
        "newline" => Some(NewLine),
        "tab" => Some(Tab),
        "cyclethinking" => Some(CycleThinking),
        "scrollup" => Some(ScrollUp),
        "scrolldown" => Some(ScrollDown),
        "scrollpageup" => Some(ScrollPageUp),
        "scrollpagedown" => Some(ScrollPageDown),
        "historyup" => Some(HistoryUp),
        "historydown" => Some(HistoryDown),
        "quit" => Some(Quit),
        "cancel" => Some(Cancel),
        "openmodelselect" => Some(OpenModelSelect),
        "openprovidersetup" => Some(OpenProviderSetup),
        "togglerouting" => Some(ToggleRouting),
        "togglequeue" => Some(ToggleQueue),
        "copycodeblock" => Some(CopyCodeBlock),
        "openimage" => Some(OpenImage),
        "completionnext" => Some(CompletionNext),
        "completionprev" => Some(CompletionPrev),
        "completiondismiss" => Some(CompletionDismiss),
        "completionaccept" => Some(CompletionAccept),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_bindings_exist() {
        let mgr = KeybindingsManager::new();
        // Every action should have at least a default entry (may be empty)
        use strum::IntoEnumIterator;
        for action in Action::iter() {
            // Just verify we can query without panic
            let _ = mgr.keys_for_action(action);
        }
    }

    #[test]
    fn test_match_ctrl_c() {
        let mgr = KeybindingsManager::new();
        let key = parse_key_id("Ctrl+c").unwrap();
        assert_eq!(mgr.match_action(&key), Some(Action::Quit));
    }

    #[test]
    fn test_match_enter() {
        let mgr = KeybindingsManager::new();
        let key = parse_key_id("Enter").unwrap();
        assert_eq!(mgr.match_action(&key), Some(Action::Submit));
    }

    #[test]
    fn test_user_override() {
        let mut mgr = KeybindingsManager::new();

        // Default: Ctrl+c = Quit
        let ctrl_c = parse_key_id("Ctrl+c").unwrap();
        assert_eq!(mgr.match_action(&ctrl_c), Some(Action::Quit));

        // Override: Ctrl+c should now be Cancel
        let mut config = HashMap::new();
        config.insert("Quit".to_string(), vec!["Ctrl+x".to_string()]);
        config.insert("Cancel".to_string(), vec!["Ctrl+c".to_string()]);
        mgr.set_user_bindings(&config);

        assert_eq!(mgr.match_action(&ctrl_c), Some(Action::Cancel));
        let ctrl_x = parse_key_id("Ctrl+x").unwrap();
        assert_eq!(mgr.match_action(&ctrl_x), Some(Action::Quit));
    }

    #[test]
    fn test_parse_action() {
        assert_eq!(parse_action("Quit"), Some(Action::Quit));
        assert_eq!(parse_action("quit"), Some(Action::Quit));
        assert_eq!(parse_action("QUIT"), Some(Action::Quit));
        assert_eq!(parse_action("CursorLeft"), Some(Action::CursorLeft));
        assert_eq!(parse_action("Unknown"), None);
    }

    #[test]
    fn test_primary_key_display() {
        let mgr = KeybindingsManager::new();
        let key = mgr.primary_key_for(Action::Quit);
        assert!(key.is_some());
        assert_eq!(format!("{}", key.unwrap()), "Ctrl+c");
    }
}
