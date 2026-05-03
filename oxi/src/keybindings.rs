//! Keybinding configuration and management
//!
//! Provides keybinding definitions, defaults, and custom keybinding loading.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// Keybinding action identifiers
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyAction {
    // Editor actions
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorLineStart,
    CursorLineEnd,
    JumpForward,
    JumpBackward,
    PageUp,
    PageDown,
    DeleteCharBackward,
    DeleteCharForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Yank,
    YankPop,
    Undo,
    NewLine,

    // Input actions
    Submit,
    Tab,
    Copy,
    SelectUp,
    SelectDown,
    SelectPageUp,
    SelectPageDown,
    SelectConfirm,
    SelectCancel,
    Interrupt,

    // App actions
    Clear,
    Exit,
    Suspend,
    CycleThinkingLevel,
    CycleModelForward,
    CycleModelBackward,
    SelectModel,
    ExpandTools,
    ToggleThinking,
    ToggleSessionNamedFilter,
    ExternalEditor,
    FollowUp,
    Dequeue,
    PasteImage,
    NewSession,
    Tree,
    Fork,
    Resume,
    TreeFoldOrUp,
    TreeUnfoldOrDown,
    TreeEditLabel,
    TreeToggleLabelTimestamp,
    ToggleSessionPath,
    ToggleSessionSort,
    RenameSession,
    DeleteSession,
    DeleteSessionNoninvasive,
    SaveModelSelection,
    EnableAllModels,
    ClearAllModels,
    ToggleProvider,
    ReorderUp,
    ReorderDown,
    TreeFilterDefault,
    TreeFilterNoTools,
    TreeFilterUserOnly,
    TreeFilterLabeledOnly,
    TreeFilterAll,
    TreeFilterCycleForward,
    TreeFilterCycleBackward,
    ToggleRawMode,

    // Custom action (for extensions)
    Custom(String),
}

impl fmt::Display for KeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyAction::Custom(name) => write!(f, "{}", name),
            _ => write!(f, "{:?}", self),
        }
    }
}

impl From<&str> for KeyAction {
    fn from(s: &str) -> Self {
        match s {
            // Editor actions
            "tui.editor.cursorUp" => KeyAction::CursorUp,
            "tui.editor.cursorDown" => KeyAction::CursorDown,
            "tui.editor.cursorLeft" => KeyAction::CursorLeft,
            "tui.editor.cursorRight" => KeyAction::CursorRight,
            "tui.editor.cursorWordLeft" => KeyAction::CursorWordLeft,
            "tui.editor.cursorWordRight" => KeyAction::CursorWordRight,
            "tui.editor.cursorLineStart" => KeyAction::CursorLineStart,
            "tui.editor.cursorLineEnd" => KeyAction::CursorLineEnd,
            "tui.editor.jumpForward" => KeyAction::JumpForward,
            "tui.editor.jumpBackward" => KeyAction::JumpBackward,
            "tui.editor.pageUp" => KeyAction::PageUp,
            "tui.editor.pageDown" => KeyAction::PageDown,
            "tui.editor.deleteCharBackward" => KeyAction::DeleteCharBackward,
            "tui.editor.deleteCharForward" => KeyAction::DeleteCharForward,
            "tui.editor.deleteWordBackward" => KeyAction::DeleteWordBackward,
            "tui.editor.deleteWordForward" => KeyAction::DeleteWordForward,
            "tui.editor.deleteToLineStart" => KeyAction::DeleteToLineStart,
            "tui.editor.deleteToLineEnd" => KeyAction::DeleteToLineEnd,
            "tui.editor.yank" => KeyAction::Yank,
            "tui.editor.yankPop" => KeyAction::YankPop,
            "tui.editor.undo" => KeyAction::Undo,
            "tui.input.newLine" => KeyAction::NewLine,
            "tui.input.submit" => KeyAction::Submit,
            "tui.input.tab" => KeyAction::Tab,
            "tui.input.copy" => KeyAction::Copy,
            "tui.select.up" => KeyAction::SelectUp,
            "tui.select.down" => KeyAction::SelectDown,
            "tui.select.pageUp" => KeyAction::SelectPageUp,
            "tui.select.pageDown" => KeyAction::SelectPageDown,
            "tui.select.confirm" => KeyAction::SelectConfirm,
            "tui.select.cancel" => KeyAction::SelectCancel,

            // App actions
            "app.interrupt" => KeyAction::Interrupt,
            "app.clear" => KeyAction::Clear,
            "app.exit" => KeyAction::Exit,
            "app.suspend" => KeyAction::Suspend,
            "app.thinking.cycle" => KeyAction::CycleThinkingLevel,
            "app.model.cycleForward" => KeyAction::CycleModelForward,
            "app.model.cycleBackward" => KeyAction::CycleModelBackward,
            "app.model.select" => KeyAction::SelectModel,
            "app.tools.expand" => KeyAction::ExpandTools,
            "app.thinking.toggle" => KeyAction::ToggleThinking,
            "app.session.toggleNamedFilter" => KeyAction::ToggleSessionNamedFilter,
            "app.editor.external" => KeyAction::ExternalEditor,
            "app.message.followUp" => KeyAction::FollowUp,
            "app.message.dequeue" => KeyAction::Dequeue,
            "app.clipboard.pasteImage" => KeyAction::PasteImage,
            "app.session.new" => KeyAction::NewSession,
            "app.session.tree" => KeyAction::Tree,
            "app.session.fork" => KeyAction::Fork,
            "app.session.resume" => KeyAction::Resume,
            "app.tree.foldOrUp" => KeyAction::TreeFoldOrUp,
            "app.tree.unfoldOrDown" => KeyAction::TreeUnfoldOrDown,
            "app.tree.editLabel" => KeyAction::TreeEditLabel,
            "app.tree.toggleLabelTimestamp" => KeyAction::TreeToggleLabelTimestamp,
            "app.session.togglePath" => KeyAction::ToggleSessionPath,
            "app.session.toggleSort" => KeyAction::ToggleSessionSort,
            "app.session.rename" => KeyAction::RenameSession,
            "app.session.delete" => KeyAction::DeleteSession,
            "app.session.deleteNoninvasive" => KeyAction::DeleteSessionNoninvasive,
            "app.models.save" => KeyAction::SaveModelSelection,
            "app.models.enableAll" => KeyAction::EnableAllModels,
            "app.models.clearAll" => KeyAction::ClearAllModels,
            "app.models.toggleProvider" => KeyAction::ToggleProvider,
            "app.models.reorderUp" => KeyAction::ReorderUp,
            "app.models.reorderDown" => KeyAction::ReorderDown,
            "app.tree.filter.default" => KeyAction::TreeFilterDefault,
            "app.tree.filter.noTools" => KeyAction::TreeFilterNoTools,
            "app.tree.filter.userOnly" => KeyAction::TreeFilterUserOnly,
            "app.tree.filter.labeledOnly" => KeyAction::TreeFilterLabeledOnly,
            "app.tree.filter.all" => KeyAction::TreeFilterAll,
            "app.tree.filter.cycleForward" => KeyAction::TreeFilterCycleForward,
            "app.tree.filter.cycleBackward" => KeyAction::TreeFilterCycleBackward,

            // Default actions
            "submit" => KeyAction::Submit,
            "cancel" => KeyAction::SelectCancel,
            "historyUp" | "history_up" => KeyAction::SelectUp,
            "historyDown" | "history_down" => KeyAction::SelectDown,

            // Fallback to custom
            _ => KeyAction::Custom(s.to_string()),
        }
    }
}

/// A key binding
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    /// The action associated with this binding
    pub action: String,
    /// Default key sequences
    pub default_keys: Vec<String>,
    /// Description of the action
    pub description: String,
}

impl KeyBinding {
    pub fn new(action: &str, default_keys: Vec<&str>, description: &str) -> Self {
        Self {
            action: action.to_string(),
            default_keys: default_keys.into_iter().map(String::from).collect(),
            description: description.to_string(),
        }
    }
}

/// User-configurable keybindings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserKeybindings {
    /// Map of action name to custom key sequence(s)
    #[serde(default)]
    pub bindings: HashMap<String, Vec<String>>,
}

/// Load user keybindings from a file
pub fn load_user_keybindings(path: &Path) -> Option<UserKeybindings> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save user keybindings to a file
pub fn save_user_keybindings(path: &Path, bindings: &UserKeybindings) -> std::io::Result<()> {
    let content = serde_json::to_string_pretty(bindings).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;
    std::fs::write(path, content)
}

/// Get the default keybindings configuration file path
pub fn default_keybindings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| {
        p.join("oxi")
            .join("keybindings.json")
    })
}

/// Vim-style keybindings
pub fn vim_keybindings() -> HashMap<String, KeyBinding> {
    let mut bindings = HashMap::new();

    bindings.insert(
        "tui.editor.cursorUp".to_string(),
        KeyBinding::new("tui.editor.cursorUp", vec!["k", "Up"], "Move cursor up"),
    );
    bindings.insert(
        "tui.editor.cursorDown".to_string(),
        KeyBinding::new("tui.editor.cursorDown", vec!["j", "Down"], "Move cursor down"),
    );
    bindings.insert(
        "tui.editor.cursorLeft".to_string(),
        KeyBinding::new("tui.editor.cursorLeft", vec!["h", "Left"], "Move cursor left"),
    );
    bindings.insert(
        "tui.editor.cursorRight".to_string(),
        KeyBinding::new("tui.editor.cursorRight", vec!["l", "Right"], "Move cursor right"),
    );
    bindings.insert(
        "tui.input.submit".to_string(),
        KeyBinding::new("tui.input.submit", vec!["Enter"], "Submit input"),
    );
    bindings.insert(
        "app.interrupt".to_string(),
        KeyBinding::new("app.interrupt", vec!["Escape"], "Cancel or abort"),
    );
    bindings.insert(
        "app.clear".to_string(),
        KeyBinding::new("app.clear", vec!["ctrl+c"], "Clear editor"),
    );

    bindings
}

/// Emacs-style keybindings
pub fn emacs_keybindings() -> HashMap<String, KeyBinding> {
    let mut bindings = HashMap::new();

    bindings.insert(
        "tui.editor.cursorUp".to_string(),
        KeyBinding::new("tui.editor.cursorUp", vec!["ctrl+p", "Up"], "Move cursor up"),
    );
    bindings.insert(
        "tui.editor.cursorDown".to_string(),
        KeyBinding::new("tui.editor.cursorDown", vec!["ctrl+n", "Down"], "Move cursor down"),
    );
    bindings.insert(
        "tui.editor.cursorLeft".to_string(),
        KeyBinding::new("tui.editor.cursorLeft", vec!["ctrl+b", "Left"], "Move cursor left"),
    );
    bindings.insert(
        "tui.editor.cursorRight".to_string(),
        KeyBinding::new("tui.editor.cursorRight", vec!["ctrl+f", "Right"], "Move cursor right"),
    );
    bindings.insert(
        "tui.input.newLine".to_string(),
        KeyBinding::new("tui.input.newLine", vec!["ctrl+j"], "New line"),
    );
    bindings.insert(
        "tui.input.submit".to_string(),
        KeyBinding::new("tui.input.submit", vec!["ctrl+m", "Enter"], "Submit input"),
    );
    bindings.insert(
        "app.interrupt".to_string(),
        KeyBinding::new("app.interrupt", vec!["ctrl+g"], "Cancel or abort"),
    );

    bindings
}

/// Get the default keybindings based on settings
pub fn get_default_keybindings() -> HashMap<String, KeyBinding> {
    // Default to vim-style on Unix, emacs-style elsewhere
    #[cfg(unix)]
    {
        vim_keybindings()
    }
    #[cfg(not(unix))]
    {
        emacs_keybindings()
    }
}

/// Keybindings manager for loading and resolving keybindings
pub struct KeybindingsManager {
    /// All defined keybindings
    bindings: HashMap<String, KeyBinding>,
    /// User overrides
    user_overrides: HashMap<String, Vec<String>>,
}

impl KeybindingsManager {
    /// Create a new keybindings manager
    pub fn new() -> Self {
        Self {
            bindings: get_default_keybindings(),
            user_overrides: HashMap::new(),
        }
    }

    /// Create from a config file
    pub fn from_file(path: &Path) -> Self {
        let mut manager = Self::new();
        if let Some(user) = load_user_keybindings(path) {
            manager.user_overrides = user.bindings;
        }
        manager
    }

    /// Create from settings
    pub fn from_settings(_settings: &crate::settings::Settings) -> Self {
        let manager = Self::new();
        // Settings-based keybindings would be applied here if available
        manager
    }

    /// Register a new keybinding
    pub fn register(&mut self, binding: KeyBinding) {
        self.bindings.insert(binding.action.clone(), binding);
    }

    /// Set user override for an action
    pub fn set_override(&mut self, action: &str, keys: Vec<String>) {
        self.user_overrides.insert(action.to_string(), keys);
    }

    /// Get the resolved key sequence for an action
    pub fn get_keys(&self, action: &str) -> Vec<String> {
        if let Some(user_keys) = self.user_overrides.get(action) {
            user_keys.clone()
        } else if let Some(binding) = self.bindings.get(action) {
            binding.default_keys.clone()
        } else {
            Vec::new()
        }
    }

    /// Get a binding by action name
    pub fn get_binding(&self, action: &str) -> Option<&KeyBinding> {
        self.bindings.get(action)
    }

    /// Get all registered bindings
    pub fn all_bindings(&self) -> &HashMap<String, KeyBinding> {
        &self.bindings
    }

    /// Get all user overrides
    pub fn user_overrides(&self) -> &HashMap<String, Vec<String>> {
        &self.user_overrides
    }

    /// Reload from file
    pub fn reload(&mut self, path: &Path) {
        if let Some(user) = load_user_keybindings(path) {
            self.user_overrides = user.bindings;
        }
    }

    /// Export user overrides to file
    pub fn export_to_file(&self, path: &Path) -> std::io::Result<()> {
        let user_bindings = UserKeybindings {
            bindings: self.user_overrides.clone(),
        };
        save_user_keybindings(path, &user_bindings)
    }
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a key sequence string into components
pub fn parse_key_sequence(sequence: &str) -> Vec<(bool, bool, char)> {
    // Returns (ctrl, alt, key)
    let mut result = Vec::new();
    let parts: Vec<&str> = sequence.split('+').collect();

    let mut ctrl = false;
    let mut alt = false;

    for part in parts {
        let part_lower = part.to_lowercase();

        if part_lower == "ctrl" || part_lower == "control" {
            ctrl = true;
            continue;
        }
        if part_lower == "alt" || part_lower == "meta" {
            alt = true;
            continue;
        }

        let key_char = match part_lower.as_str() {
            "space" => " ",
            "tab" => "\t",
            "enter" | "return" => "\n",
            "escape" | "esc" => "\x1b",
            "backspace" => "\x7f",
            "delete" => "\x1b[3~",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            "home" => "\x1b[H",
            "end" => "\x1b[F",
            "pageup" => "\x1b[5~",
            "pagedown" => "\x1b[6~",
            _ => part,
        };

        let first_char = key_char.chars().next().unwrap_or(' ');
        result.push((ctrl, alt, first_char));
        // Reset modifiers after use
        ctrl = false;
        alt = false;
    }

    result
}

/// Format a key sequence for display
pub fn format_key_sequence(keys: &[String]) -> String {
    keys.iter()
        .map(|k| {
            let parts: Vec<&str> = k.split('+').collect();
            let mut formatted_parts = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                let lower = part.to_lowercase();
                if lower == "ctrl" {
                    formatted_parts.push("Ctrl".to_string());
                } else if lower == "alt" || lower == "meta" {
                    formatted_parts.push("Alt".to_string());
                } else if lower == "shift" {
                    formatted_parts.push("Shift".to_string());
                } else if i == parts.len() - 1 {
                    // Last part (the key) — uppercase single chars
                    let c = part.chars().next().unwrap_or(' ');
                    formatted_parts.push(c.to_uppercase().collect::<String>());
                } else {
                    formatted_parts.push(part.to_string());
                }
            }
            formatted_parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_action_from_str() {
        assert_eq!(KeyAction::from("app.interrupt"), KeyAction::Interrupt);
        assert_eq!(KeyAction::from("app.clear"), KeyAction::Clear);
        assert_eq!(KeyAction::from("submit"), KeyAction::Submit);
    }

    #[test]
    fn test_key_action_custom() {
        let action = KeyAction::from("my-extension.action");
        assert!(matches!(action, KeyAction::Custom(s) if s == "my-extension.action"));
    }

    #[test]
    fn test_keybindings_manager_new() {
        let manager = KeybindingsManager::new();
        assert!(!manager.all_bindings().is_empty());
    }

    #[test]
    fn test_get_keys_default() {
        let manager = KeybindingsManager::new();
        let keys = manager.get_keys("app.interrupt");
        assert!(!keys.is_empty());
    }

    #[test]
    fn test_set_override() {
        let mut manager = KeybindingsManager::new();
        manager.set_override("app.interrupt", vec!["ctrl+c".to_string()]);
        let keys = manager.get_keys("app.interrupt");
        assert_eq!(keys, vec!["ctrl+c"]);
    }

    #[test]
    fn test_vim_keybindings() {
        let bindings = vim_keybindings();
        assert!(bindings.contains_key("tui.editor.cursorUp"));
        assert!(bindings.contains_key("tui.input.submit"));
    }

    #[test]
    fn test_emacs_keybindings() {
        let bindings = emacs_keybindings();
        assert!(bindings.contains_key("tui.editor.cursorUp"));
        assert!(bindings.contains_key("tui.input.submit"));
    }

    #[test]
    fn test_parse_key_sequence() {
        let result = parse_key_sequence("ctrl+c");
        assert!(result.iter().any(|(ctrl, _, _)| *ctrl));

        let result = parse_key_sequence("alt+x");
        assert!(result.iter().any(|(_, alt, _)| *alt));
    }

    #[test]
    fn test_format_key_sequence() {
        let keys = vec!["ctrl+c".to_string(), "ctrl+x".to_string()];
        let formatted = format_key_sequence(&keys);
        assert!(formatted.contains("Ctrl+C"));
        assert!(formatted.contains("Ctrl+X"));
    }

    #[test]
    fn test_user_keybindings_serde() {
        let user = UserKeybindings {
            bindings: HashMap::from([
                ("app.interrupt".to_string(), vec!["ctrl+c".to_string()]),
            ]),
        };
        let json = serde_json::to_string(&user).unwrap();
        let parsed: UserKeybindings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bindings.get("app.interrupt"), Some(&vec!["ctrl+c".to_string()]));
    }

    #[test]
    fn test_keybinding_struct() {
        let binding = KeyBinding::new("test.action", vec!["a", "b"], "Test action");
        assert_eq!(binding.action, "test.action");
        assert_eq!(binding.default_keys, vec!["a", "b"]);
        assert_eq!(binding.description, "Test action");
    }
}