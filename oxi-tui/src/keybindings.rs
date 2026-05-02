//! Declarative keyboard shortcut system.
//!
//! Provides a centralized, customizable keybinding registry inspired by
//! pi-mono's KeybindingsManager. Bindings are defined declaratively with
//! default keys and descriptions, and can be overridden via a JSON config
//! file (`~/.oxi/keybindings.json`).

use std::collections::HashMap;
use std::path::Path;

/// Unique identifier for a keybinding action.
pub type ActionId = &'static str;

/// Parsed key sequence (single key).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySequence {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: KeyName,
}

/// Key name (after modifiers stripped).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyName {
    Char(char),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl KeySequence {
    /// Parse a key string like `"ctrl+c"`, `"alt+enter"`, `"shift+tab"`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut remaining = s.trim();

        loop {
            if let Some(rest) = remaining.strip_prefix("ctrl+") {
                ctrl = true;
                remaining = rest;
                continue;
            }
            if let Some(rest) = remaining.strip_prefix("alt+") {
                alt = true;
                remaining = rest;
                continue;
            }
            if let Some(rest) = remaining.strip_prefix("shift+") {
                shift = true;
                remaining = rest;
                continue;
            }
            break;
        }

        let key = match remaining {
            "enter" => KeyName::Enter,
            "escape" | "esc" => KeyName::Escape,
            "tab" => KeyName::Tab,
            "backspace" | "bs" => KeyName::Backspace,
            "delete" | "del" => KeyName::Delete,
            "up" => KeyName::Up,
            "down" => KeyName::Down,
            "left" => KeyName::Left,
            "right" => KeyName::Right,
            "home" => KeyName::Home,
            "end" => KeyName::End,
            "pageup" | "page_up" => KeyName::PageUp,
            "pagedown" | "page_down" => KeyName::PageDown,
            s if s.starts_with('f') && s.len() == 2 => {
                if let Ok(n) = s[1..].parse::<u8>() {
                    KeyName::F(n)
                } else {
                    return None;
                }
            }
            s if s.len() == 1 => KeyName::Char(s.chars().next()?),
            _ => return None,
        };

        Some(Self { ctrl, alt, shift, key })
    }
}

// ---------------------------------------------------------------------------
// Well-known action IDs
// ---------------------------------------------------------------------------

/// Application-level keybinding actions.
pub mod actions {
    // Editor / input
    pub const INPUT_SUBMIT: &str = "app.input.submit";
    pub const INPUT_NEWLINE: &str = "app.input.newLine";
    pub const INPUT_TAB: &str = "app.input.tab";

    // Application control
    pub const INTERRUPT: &str = "app.interrupt";
    pub const CLEAR: &str = "app.clear";
    pub const EXIT: &str = "app.exit";
    pub const SUSPEND: &str = "app.suspend";

    // Model / thinking
    pub const CYCLE_THINKING: &str = "app.thinking.cycle";
    pub const TOGGLE_THINKING: &str = "app.thinking.toggle";
    pub const CYCLE_MODEL_FWD: &str = "app.model.cycleForward";
    pub const CYCLE_MODEL_BWD: &str = "app.model.cycleBackward";
    pub const SELECT_MODEL: &str = "app.model.select";

    // Session
    pub const NEW_SESSION: &str = "app.session.new";
    pub const RESUME_SESSION: &str = "app.session.resume";
    pub const TREE_SESSION: &str = "app.session.tree";
    pub const FORK_SESSION: &str = "app.session.fork";

    // Tools
    pub const EXPAND_TOOLS: &str = "app.tools.expand";

    // Clipboard
    pub const PASTE_IMAGE: &str = "app.clipboard.pasteImage";

    // Editor
    pub const EXTERNAL_EDITOR: &str = "app.editor.external";

    // Messages
    pub const FOLLOW_UP: &str = "app.message.followUp";
    pub const DEQUEUE: &str = "app.message.dequeue";

    // Scrolling
    pub const SCROLL_UP: &str = "app.scroll.up";
    pub const SCROLL_DOWN: &str = "app.scroll.down";
}

// ---------------------------------------------------------------------------
// Binding definition
// ---------------------------------------------------------------------------

/// A single keybinding definition.
struct BindingDef {
    description: &'static str,
    default_keys: &'static [&'static str],
    user_keys: Option<Vec<String>>,
}

/// Keyboard shortcut registry.
pub struct KeybindingRegistry {
    bindings: HashMap<ActionId, BindingDef>,
}

/// All built-in keybinding definitions.
const DEFAULT_BINDINGS: &[(&str, &[&str], &str)] = &[
    // Input
    (actions::INPUT_SUBMIT, &["enter"], "Submit input"),
    (actions::INPUT_NEWLINE, &["alt+enter"], "Insert new line"),
    (actions::INPUT_TAB, &["tab"], "Tab completion"),
    // Application control
    (actions::INTERRUPT, &["escape"], "Cancel or abort"),
    (actions::CLEAR, &["ctrl+c"], "Clear editor"),
    (actions::EXIT, &["ctrl+d"], "Exit when editor is empty"),
    (actions::SUSPEND, &["ctrl+z"], "Suspend to background"),
    // Model / thinking
    (actions::CYCLE_THINKING, &["shift+tab"], "Cycle thinking level"),
    (actions::TOGGLE_THINKING, &["ctrl+t"], "Toggle thinking blocks"),
    (actions::CYCLE_MODEL_FWD, &["ctrl+p"], "Cycle to next model"),
    (actions::CYCLE_MODEL_BWD, &["shift+ctrl+p"], "Cycle to previous model"),
    (actions::SELECT_MODEL, &["ctrl+l"], "Open model selector"),
    // Session
    (actions::NEW_SESSION, &[], "Start a new session"),
    (actions::RESUME_SESSION, &[], "Resume a session"),
    (actions::TREE_SESSION, &[], "Open session tree"),
    (actions::FORK_SESSION, &[], "Fork current session"),
    // Tools
    (actions::EXPAND_TOOLS, &["ctrl+o"], "Toggle tool output"),
    // Clipboard
    (actions::PASTE_IMAGE, &["ctrl+v"], "Paste image from clipboard"),
    // Editor
    (actions::EXTERNAL_EDITOR, &["ctrl+g"], "Open external editor"),
    // Messages
    (actions::FOLLOW_UP, &["alt+enter"], "Queue follow-up message"),
    (actions::DEQUEUE, &["alt+up"], "Restore queued messages"),
    // Scrolling
    (actions::SCROLL_UP, &["pageup"], "Scroll up"),
    (actions::SCROLL_DOWN, &["pagedown"], "Scroll down"),
];

impl KeybindingRegistry {
    /// Create a registry with all default bindings.
    pub fn new() -> Self {
        let mut bindings = HashMap::new();
        for &(action, keys, desc) in DEFAULT_BINDINGS {
            bindings.insert(action, BindingDef {
                description: desc,
                default_keys: keys,
                user_keys: None,
            });
        }
        Self { bindings }
    }

    /// Load user overrides from a JSON file.
    ///
    /// The file format is `{ "app.action": "ctrl+x" }` or
    /// `{ "app.action": ["ctrl+x", "alt+x"] }`.
    pub fn load_user_overrides(&mut self, path: &Path) -> Result<(), KeybindingError> {
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| KeybindingError::Io(e.to_string()))?;
        let overrides: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| KeybindingError::Parse(e.to_string()))?;

        let map = match overrides {
            serde_json::Value::Object(m) => m,
            _ => return Ok(()),
        };

        for (action, keys) in map {
            let action: &str = &action;
            if !self.bindings.contains_key(action) {
                continue;
            }

            let user_keys = match keys {
                serde_json::Value::String(s) => vec![s],
                serde_json::Value::Array(arr) => {
                    arr.into_iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                }
                _ => continue,
            };

            if let Some(entry) = self.bindings.get_mut(action) {
                entry.user_keys = Some(user_keys);
            }
        }

        Ok(())
    }

    /// Get the effective key sequences for an action.
    pub fn keys_for(&self, action: ActionId) -> Vec<KeySequence> {
        let entry = match self.bindings.get(action) {
            Some(e) => e,
            None => return Vec::new(),
        };

        let raw_keys = entry.user_keys
            .as_ref()
            .map(|k| k.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .unwrap_or_else(|| entry.default_keys.to_vec());

        raw_keys.iter()
            .filter_map(|k| KeySequence::parse(k))
            .collect()
    }

    /// Check if a key sequence matches an action.
    pub fn matches(&self, action: ActionId, seq: &KeySequence) -> bool {
        self.keys_for(action).iter().any(|k| k == seq)
    }

    /// Look up which action (if any) matches the given key sequence.
    pub fn action_for(&self, seq: &KeySequence) -> Option<ActionId> {
        for action in self.bindings.keys() {
            if self.matches(action, seq) {
                return Some(action);
            }
        }
        None
    }

    /// Get the description for an action.
    pub fn description(&self, action: ActionId) -> Option<&'static str> {
        self.bindings.get(action).map(|e| e.description)
    }

    /// List all registered actions.
    pub fn actions(&self) -> Vec<ActionId> {
        let mut actions: Vec<ActionId> = self.bindings.keys().copied().collect();
        actions.sort();
        actions
    }

    /// Resolve the effective config as a map of action → key strings.
    pub fn effective_config(&self) -> HashMap<ActionId, Vec<String>> {
        let mut result = HashMap::new();
        for action in self.bindings.keys() {
            let entry = &self.bindings[action];
            let keys = entry.user_keys.clone().unwrap_or_else(|| {
                entry.default_keys.iter().map(|s| s.to_string()).collect()
            });
            result.insert(*action, keys);
        }
        result
    }
}

impl Default for KeybindingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur loading keybinding config.
#[derive(Debug, thiserror::Error)]
pub enum KeybindingError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_c() {
        let seq = KeySequence::parse("ctrl+c").unwrap();
        assert!(seq.ctrl);
        assert!(!seq.alt);
        assert!(!seq.shift);
        assert_eq!(seq.key, KeyName::Char('c'));
    }

    #[test]
    fn parse_shift_tab() {
        let seq = KeySequence::parse("shift+tab").unwrap();
        assert!(!seq.ctrl);
        assert!(seq.shift);
        assert_eq!(seq.key, KeyName::Tab);
    }

    #[test]
    fn parse_escape() {
        let seq = KeySequence::parse("escape").unwrap();
        assert!(!seq.ctrl);
        assert_eq!(seq.key, KeyName::Escape);
    }

    #[test]
    fn registry_default_bindings() {
        let reg = KeybindingRegistry::new();
        let keys = reg.keys_for(actions::INTERRUPT);
        assert!(keys.contains(&KeySequence::parse("escape").unwrap()));
    }

    #[test]
    fn registry_matches() {
        let reg = KeybindingRegistry::new();
        let seq = KeySequence::parse("escape").unwrap();
        assert!(reg.matches(actions::INTERRUPT, &seq));
    }

    #[test]
    fn registry_action_for() {
        let reg = KeybindingRegistry::new();
        let seq = KeySequence::parse("ctrl+d").unwrap();
        assert_eq!(reg.action_for(&seq), Some(actions::EXIT));
    }

    #[test]
    fn load_user_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        std::fs::write(&path, r#"{"app.interrupt": "ctrl+x"}"#).unwrap();

        let mut reg = KeybindingRegistry::new();
        reg.load_user_overrides(&path).unwrap();

        let keys = reg.keys_for(actions::INTERRUPT);
        assert!(keys.contains(&KeySequence::parse("ctrl+x").unwrap()));
        assert!(!keys.contains(&KeySequence::parse("escape").unwrap()));
    }
}
