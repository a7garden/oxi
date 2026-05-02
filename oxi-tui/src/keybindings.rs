//! Declarative keyboard shortcut system.
//!
//! Provides a centralized, customizable keybinding registry inspired by
//! pi-mono's KeybindingsManager. Bindings are defined declaratively with
//! default keys and descriptions, and can be overridden via a JSON config
//! file (`~/.oxi/keybindings.json`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Unique identifier for a keybinding action.
pub type ActionId = &'static str;

/// A resolved keybinding: action → keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keybinding {
    /// Human-readable description.
    pub description: &'static str,
    /// Default key sequences (e.g. `"ctrl+c"`, `"escape"`).
    pub default_keys: &'static [ &'static str ],
    /// User-overridden keys (loaded from config file).
    #[serde(skip)]
    pub user_keys: Option<Vec<String>>,
}

/// Full set of keybinding definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingDefinition {
    pub action: ActionId,
    pub description: &'static str,
    pub default_keys: &'static [ &'static str ],
}

/// Keyboard shortcut registry.
pub struct KeybindingRegistry {
    bindings: HashMap<ActionId, KeybindingEntry>,
}

/// Internal storage for a single binding.
struct KeybindingEntry {
    description: &'static str,
    default_keys: &'static [ &'static str ],
    user_keys: Option<Vec<String>>,
}

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
// Default bindings table
// ---------------------------------------------------------------------------

/// All built-in keybinding definitions.
const DEFAULT_BINDINGS: &[KeybindingDefinition] = &[
    // Input
    KeybindingDefinition { action: actions::INPUT_SUBMIT, description: "Submit input", default_keys: &["enter"] },
    KeybindingDefinition { action: actions::INPUT_NEWLINE, description: "Insert new line", default_keys: &["alt+enter"] },
    KeybindingDefinition { action: actions::INPUT_TAB, description: "Tab completion", default_keys: &["tab"] },

    // Application control
    KeybindingDefinition { action: actions::INTERRUPT, description: "Cancel or abort", default_keys: &["escape"] },
    KeybindingDefinition { action: actions::CLEAR, description: "Clear editor", default_keys: &["ctrl+c"] },
    KeybindingDefinition { action: actions::EXIT, description: "Exit when editor is empty", default_keys: &["ctrl+d"] },
    KeybindingDefinition { action: actions::SUSPEND, description: "Suspend to background", default_keys: &["ctrl+z"] },

    // Model / thinking
    KeybindingDefinition { action: actions::CYCLE_THINKING, description: "Cycle thinking level", default_keys: &["shift+tab"] },
    KeybindingDefinition { action: actions::TOGGLE_THINKING, description: "Toggle thinking blocks", default_keys: &["ctrl+t"] },
    KeybindingDefinition { action: actions::CYCLE_MODEL_FWD, description: "Cycle to next model", default_keys: &["ctrl+p"] },
    KeybindingDefinition { action: actions::CYCLE_MODEL_BWD, description: "Cycle to previous model", default_keys: &["shift+ctrl+p"] },
    KeybindingDefinition { action: actions::SELECT_MODEL, description: "Open model selector", default_keys: &["ctrl+l"] },

    // Session
    KeybindingDefinition { action: actions::NEW_SESSION, description: "Start a new session", default_keys: &[] },
    KeybindingDefinition { action: actions::RESUME_SESSION, description: "Resume a session", default_keys: &[] },
    KeybindingDefinition { action: actions::TREE_SESSION, description: "Open session tree", default_keys: &[] },
    KeybindingDefinition { action: actions::FORK_SESSION, description: "Fork current session", default_keys: &[] },

    // Tools
    KeybindingDefinition { action: actions::EXPAND_TOOLS, description: "Toggle tool output", default_keys: &["ctrl+o"] },

    // Clipboard
    KeybindingDefinition { action: actions::PASTE_IMAGE, description: "Paste image from clipboard", default_keys: &["ctrl+v"] },

    // Editor
    KeybindingDefinition { action: actions::EXTERNAL_EDITOR, description: "Open external editor", default_keys: &["ctrl+g"] },

    // Messages
    KeybindingDefinition { action: actions::FOLLOW_UP, description: "Queue follow-up message", default_keys: &["alt+enter"] },
    KeybindingDefinition { action: actions::DEQUEUE, description: "Restore queued messages", default_keys: &["alt+up"] },

    // Scrolling
    KeybindingDefinition { action: actions::SCROLL_UP, description: "Scroll up", default_keys: &["pageup"] },
    KeybindingDefinition { action: actions::SCROLL_DOWN, description: "Scroll down", default_keys: &["pagedown"] },
];

impl KeybindingRegistry {
    /// Create a registry with all default bindings.
    pub fn new() -> Self {
        let mut bindings = HashMap::new();
        for def in DEFAULT_BINDINGS {
            bindings.insert(def.action, KeybindingEntry {
                description: def.description,
                default_keys: def.default_keys,
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
            // Only apply overrides for known actions
            if !self.bindings.contains_key(action.as_str()) {
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

            if let Some(entry) = self.bindings.get_mut(action.as_str()) {
                entry.user_keys = Some(user_keys);
            }
        }

        Ok(())
    }

    /// Get the effective key sequences for an action.
    ///
    /// Returns user-overridden keys if set, otherwise the defaults.
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
            result.insert(action, keys);
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
