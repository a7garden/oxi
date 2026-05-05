//! Declarative keyboard shortcut system.
//!
//! Provides a centralized, customizable keybinding registry.
//! Bindings are defined declaratively with
//! default keys and descriptions, and can be overridden via a JSON config
//! file (`~/.oxi/keybindings.json`).

use std::collections::HashMap;
use std::path::Path;

/// Unique identifier for a keybinding action.
pub type ActionId = &'static str;

/// Parsed key sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySequence {
    /// Whether Ctrl modifier is active.
    pub ctrl: bool,
    /// Whether Alt modifier is active.
    pub alt: bool,
    /// Whether Shift modifier is active.
    pub shift: bool,
    /// The key itself.
    pub key: KeyName,
}

/// Key name (after modifiers stripped).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyName {
    /// A printable ASCII character.
    Char(char),
    /// Enter / Return key.
    Enter,
    /// Escape key.
    Escape,
    /// Tab key.
    Tab,
    /// Backspace key.
    Backspace,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Arrow Up.
    Up,
    /// Arrow Down.
    Down,
    /// Arrow Left.
    Left,
    /// Arrow Right.
    Right,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    /// Function key F1-F12.
    F(u8),
}

impl KeySequence {
    /// Parse a key string like `"ctrl+c"`, `"alt+enter"`, `"shift+tab"`,
    /// `"ctrl+shift+f1"`, `"insert"`, `"pageup"`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut remaining = s.trim();

        // Consume modifier prefixes in any order
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

        let lower = remaining.to_lowercase();
        let key = match lower.as_str() {
            "enter" | "return" => KeyName::Enter,
            "escape" | "esc" => KeyName::Escape,
            "tab" => KeyName::Tab,
            "backspace" | "bs" => KeyName::Backspace,
            "delete" | "del" => KeyName::Delete,
            "insert" | "ins" => KeyName::Insert,
            "up" => KeyName::Up,
            "down" => KeyName::Down,
            "left" => KeyName::Left,
            "right" => KeyName::Right,
            "home" => KeyName::Home,
            "end" => KeyName::End,
            "pageup" | "page_up" => KeyName::PageUp,
            "pagedown" | "page_down" => KeyName::PageDown,
            // F-keys: f1..f12
            s if s.starts_with('f') && s.len() <= 3 => {
                let num: u8 = s[1..].parse().ok()?;
                if (1..=12).contains(&num) {
                    KeyName::F(num)
                } else {
                    return None;
                }
            }
            // Single character key
            _ if remaining.len() == 1 => KeyName::Char(remaining.chars().next()?),
            _ => return None,
        };

        Some(Self {
            ctrl,
            alt,
            shift,
            key,
        })
    }

    /// Format the key sequence back to string notation.
    pub fn to_notation(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("ctrl".to_string());
        }
        if self.alt {
            parts.push("alt".to_string());
        }
        if self.shift {
            parts.push("shift".to_string());
        }
        match self.key {
            KeyName::Char(c) => parts.push(c.to_string()),
            KeyName::Enter => parts.push("enter".to_string()),
            KeyName::Escape => parts.push("escape".to_string()),
            KeyName::Tab => parts.push("tab".to_string()),
            KeyName::Backspace => parts.push("backspace".to_string()),
            KeyName::Delete => parts.push("delete".to_string()),
            KeyName::Insert => parts.push("insert".to_string()),
            KeyName::Up => parts.push("up".to_string()),
            KeyName::Down => parts.push("down".to_string()),
            KeyName::Left => parts.push("left".to_string()),
            KeyName::Right => parts.push("right".to_string()),
            KeyName::Home => parts.push("home".to_string()),
            KeyName::End => parts.push("end".to_string()),
            KeyName::PageUp => parts.push("pageup".to_string()),
            KeyName::PageDown => parts.push("pagedown".to_string()),
            KeyName::F(n) => parts.push(format!("f{}", n)),
        }
        parts.join("+")
    }
}

/// Well-known action IDs.
pub mod actions {
    /// Submit current input line.
    pub const INPUT_SUBMIT: &str = "app.input.submit";
    /// Insert a newline in the editor.
    pub const INPUT_NEWLINE: &str = "app.input.newLine";
    /// Interrupt / cancel the current operation.
    pub const INTERRUPT: &str = "app.interrupt";
    /// Clear the editor content.
    pub const CLEAR: &str = "app.clear";
    /// Exit the application.
    pub const EXIT: &str = "app.exit";
    /// Suspend to background.
    pub const SUSPEND: &str = "app.suspend";
    /// Cycle thinking / reasoning depth.
    pub const CYCLE_THINKING: &str = "app.thinking.cycle";
    /// Toggle thinking block visibility.
    pub const TOGGLE_THINKING: &str = "app.thinking.toggle";
    /// Cycle to next model.
    pub const CYCLE_MODEL_FWD: &str = "app.model.cycleForward";
    /// Cycle to previous model.
    pub const CYCLE_MODEL_BWD: &str = "app.model.cycleBackward";
    /// Open the model selector.
    pub const SELECT_MODEL: &str = "app.model.select";
    /// Start a new session.
    pub const NEW_SESSION: &str = "app.session.new";
    /// Expand / collapse tool output panel.
    pub const EXPAND_TOOLS: &str = "app.tools.expand";
    /// Open external editor.
    pub const EXTERNAL_EDITOR: &str = "app.editor.external";
    /// Scroll the view up.
    pub const SCROLL_UP: &str = "app.scroll.up";
    /// Scroll the view down.
    pub const SCROLL_DOWN: &str = "app.scroll.down";
}

/// Internal binding definition.
struct BindingDef {
    description: &'static str,
    default_keys: &'static [&'static str],
    user_keys: Option<Vec<String>>,
}

/// All built-in keybinding definitions.
const DEFAULT_BINDINGS: &[(&str, &[&str], &str)] = &[
    (actions::INPUT_SUBMIT,    &["enter"],      "Submit input"),
    (actions::INPUT_NEWLINE,   &["alt+enter"],   "Insert new line"),
    (actions::INTERRUPT,       &["escape"],      "Cancel or abort"),
    (actions::CLEAR,           &["ctrl+c"],      "Clear editor"),
    (actions::EXIT,            &["ctrl+d"],      "Exit when editor is empty"),
    (actions::SUSPEND,         &["ctrl+z"],      "Suspend to background"),
    (actions::CYCLE_THINKING,  &["shift+tab"],   "Cycle thinking level"),
    (actions::TOGGLE_THINKING, &["ctrl+t"],      "Toggle thinking blocks"),
    (actions::CYCLE_MODEL_FWD, &["ctrl+p"],      "Cycle to next model"),
    (actions::CYCLE_MODEL_BWD, &["shift+ctrl+p"],"Cycle to previous model"),
    (actions::SELECT_MODEL,    &["ctrl+l"],      "Open model selector"),
    (actions::NEW_SESSION,     &[],              "Start a new session"),
    (actions::EXPAND_TOOLS,    &["ctrl+o"],      "Toggle tool output"),
    (actions::EXTERNAL_EDITOR, &["ctrl+g"],      "Open external editor"),
    (actions::SCROLL_UP,       &["pageup"],      "Scroll up"),
    (actions::SCROLL_DOWN,     &["pagedown"],    "Scroll down"),
];

/// Keyboard shortcut registry.
pub struct KeybindingRegistry {
    bindings: HashMap<ActionId, BindingDef>,
}

impl KeybindingRegistry {
    /// Create a new registry populated with default keybindings.
    pub fn new() -> Self {
        let mut bindings = HashMap::new();
        for &(action, keys, desc) in DEFAULT_BINDINGS {
            bindings.insert(action, BindingDef { description: desc, default_keys: keys, user_keys: None });
        }
        Self { bindings }
    }

    /// Load user overrides from a JSON file.
    pub fn load_user_overrides(&mut self, path: &Path) -> Result<(), KeybindingError> {
        if !path.exists() { return Ok(()); }
        let content = std::fs::read_to_string(path).map_err(|e| KeybindingError::Io(e.to_string()))?;
        let overrides: serde_json::Value = serde_json::from_str(&content).map_err(|e| KeybindingError::Parse(e.to_string()))?;
        if let serde_json::Value::Object(map) = overrides {
            for (action, keys) in map {
                let action: &str = &action;
                if !self.bindings.contains_key(action) { continue; }
                let user_keys = match keys {
                    serde_json::Value::String(s) => vec![s],
                    serde_json::Value::Array(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    _ => continue,
                };
                if let Some(entry) = self.bindings.get_mut(action) { entry.user_keys = Some(user_keys); }
            }
        }
        Ok(())
    }

    /// Get the effective key sequences for an action.
    pub fn keys_for(&self, action: ActionId) -> Vec<KeySequence> {
        let entry = match self.bindings.get(action) { Some(e) => e, None => return Vec::new() };
        let raw = entry.user_keys.as_ref()
            .map(|k| k.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .unwrap_or_else(|| entry.default_keys.to_vec());
        raw.iter().filter_map(|k| KeySequence::parse(k)).collect()
    }

    /// Check if a key sequence matches an action.
    pub fn matches(&self, action: ActionId, seq: &KeySequence) -> bool {
        self.keys_for(action).iter().any(|k| k == seq)
    }

    /// Look up which action matches the given key sequence.
    pub fn action_for(&self, seq: &KeySequence) -> Option<ActionId> {
        self.bindings.keys().find(|&action| self.matches(action, seq)).map(|v| v as _)
    }

    /// Get the description for an action.
    pub fn description(&self, action: ActionId) -> Option<&'static str> {
        self.bindings.get(action).map(|e| e.description)
    }

    /// List all registered actions.
    pub fn actions(&self) -> Vec<ActionId> {
        let mut a: Vec<_> = self.bindings.keys().copied().collect(); a.sort(); a
    }
}

impl Default for KeybindingRegistry {
    fn default() -> Self { Self::new() }
}

/// Errors loading keybinding config.
#[derive(Debug, thiserror::Error)]
pub enum KeybindingError {
    #[error("IO error: {0}")] Io(String),
    #[error("Parse error: {0}")] Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_c() {
        let s = KeySequence::parse("ctrl+c").unwrap();
        assert!(s.ctrl);
        assert_eq!(s.key, KeyName::Char('c'));
    }

    #[test]
    fn parse_shift_tab() {
        let s = KeySequence::parse("shift+tab").unwrap();
        assert!(s.shift);
        assert_eq!(s.key, KeyName::Tab);
    }

    #[test]
    fn parse_escape() {
        let s = KeySequence::parse("escape").unwrap();
        assert_eq!(s.key, KeyName::Escape);
    }

    #[test]
    fn parse_f1_through_f12() {
        for n in 1..=12u8 {
            let key = format!("f{}", n);
            let s = KeySequence::parse(&key).unwrap();
            assert_eq!(s.key, KeyName::F(n));
        }
    }

    #[test]
    fn parse_f0_invalid() {
        assert!(KeySequence::parse("f0").is_none());
    }

    #[test]
    fn parse_f13_invalid() {
        assert!(KeySequence::parse("f13").is_none());
    }

    #[test]
    fn parse_insert() {
        let s = KeySequence::parse("insert").unwrap();
        assert_eq!(s.key, KeyName::Insert);
    }

    #[test]
    fn parse_ctrl_shift_f5() {
        let s = KeySequence::parse("ctrl+shift+f5").unwrap();
        assert!(s.ctrl);
        assert!(s.shift);
        assert_eq!(s.key, KeyName::F(5));
    }

    #[test]
    fn notation_round_trip() {
        let notations = vec![
            "ctrl+c", "alt+enter", "shift+tab", "escape", "f1", "f12",
            "pageup", "pagedown", "home", "end", "delete", "insert",
        ];
        for notation in notations {
            let seq = KeySequence::parse(notation)
                .unwrap_or_else(|| panic!("Failed to parse: {}", notation));
            let round = seq.to_notation();
            let re = KeySequence::parse(&round)
                .unwrap_or_else(|| panic!("Failed to re-parse: {}", round));
            assert_eq!(seq, re, "Round-trip failed for {}", notation);
        }
    }

    #[test]
    fn registry_matches() {
        let r = KeybindingRegistry::new();
        assert!(r.matches(actions::INTERRUPT, &KeySequence::parse("escape").unwrap()));
    }

    #[test]
    fn registry_action_for() {
        let r = KeybindingRegistry::new();
        assert_eq!(r.action_for(&KeySequence::parse("ctrl+d").unwrap()), Some(actions::EXIT));
    }
}
