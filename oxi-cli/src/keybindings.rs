//! Keybinding configuration and management.
//!
//! Provides keybinding definitions, defaults, and custom keybinding loading.
//! Supports hierarchical configuration:
//!   1. Built-in defaults (hardcoded)
//!   2. User overrides from `~/.oxi/keybindings.toml`
//!   3. Project overrides from `.oxi/keybindings.toml`
//!
//! Key notation: `"ctrl+c"`, `"alt+enter"`, `"shift+tab"`, `"f1"`, `"pageup"`, etc.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Key notation parsing
// ---------------------------------------------------------------------------

/// Parsed key name (modifiers handled separately).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyName {
/// char variant.
    Char(char),
/// enter variant.
    Enter,
/// escape variant.
    Escape,
/// tab variant.
    Tab,
/// backspace variant.
    Backspace,
/// delete variant.
    Delete,
/// up variant.
    Up,
/// down variant.
    Down,
/// left variant.
    Left,
/// right variant.
    Right,
/// home variant.
    Home,
/// end variant.
    End,
/// page up variant.
    PageUp,
/// page down variant.
    PageDown,
/// insert variant.
    Insert,
/// f variant.
    F(u8),
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyName::Char(c) => write!(f, "{}", c),
            KeyName::Enter => write!(f, "enter"),
            KeyName::Escape => write!(f, "escape"),
            KeyName::Tab => write!(f, "tab"),
            KeyName::Backspace => write!(f, "backspace"),
            KeyName::Delete => write!(f, "delete"),
            KeyName::Up => write!(f, "up"),
            KeyName::Down => write!(f, "down"),
            KeyName::Left => write!(f, "left"),
            KeyName::Right => write!(f, "right"),
            KeyName::Home => write!(f, "home"),
            KeyName::End => write!(f, "end"),
            KeyName::PageUp => write!(f, "pageup"),
            KeyName::PageDown => write!(f, "pagedown"),
            KeyName::Insert => write!(f, "insert"),
            KeyName::F(n) => write!(f, "f{}", n),
        }
    }
}

/// A fully parsed key sequence with modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySequence {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: KeyName,
}

impl KeySequence {
    /// Parse a key string like `"ctrl+c"`, `"alt+enter"`, `"shift+tab"`, `"ctrl+shift+f1"`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut remaining = s.trim();

        // Consume modifier prefixes (order-independent for ctrl/alt/shift)
        loop {
            if let Some(rest) = remaining
                .strip_prefix("ctrl+")
                .or_else(|| remaining.strip_prefix("control+"))
            {
                ctrl = true;
                remaining = rest;
                continue;
            }
            if let Some(rest) = remaining
                .strip_prefix("alt+")
                .or_else(|| remaining.strip_prefix("meta+"))
            {
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
                if num >= 1 && num <= 12 {
                    KeyName::F(num)
                } else {
                    return None;
                }
            }
            // Single character key
            _ if remaining.len() == 1 => {
                KeyName::Char(remaining.chars().next()?)
            }
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
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        let key_str = self.key.to_string();
        parts.push(&key_str);
        parts.join("+")
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_notation())
    }
}

// ---------------------------------------------------------------------------
// Keybinding action identifiers
// ---------------------------------------------------------------------------

/// Keybinding action identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyAction {
    // Editor actions
    CursorUp,
/// cursor down variant.
    CursorDown,
/// cursor left variant.
    CursorLeft,
/// cursor right variant.
    CursorRight,
/// cursor word left variant.
    CursorWordLeft,
/// cursor word right variant.
    CursorWordRight,
/// cursor line start variant.
    CursorLineStart,
/// cursor line end variant.
    CursorLineEnd,
/// jump forward variant.
    JumpForward,
/// jump backward variant.
    JumpBackward,
/// page up variant.
    PageUp,
/// page down variant.
    PageDown,
/// delete char backward variant.
    DeleteCharBackward,
/// delete char forward variant.
    DeleteCharForward,
/// delete word backward variant.
    DeleteWordBackward,
/// delete word forward variant.
    DeleteWordForward,
/// delete to line start variant.
    DeleteToLineStart,
/// delete to line end variant.
    DeleteToLineEnd,
/// yank variant.
    Yank,
/// yank pop variant.
    YankPop,
/// undo variant.
    Undo,
/// new line variant.
    NewLine,

    // Input actions
    Submit,
/// tab variant.
    Tab,
/// copy variant.
    Copy,
/// select up variant.
    SelectUp,
/// select down variant.
    SelectDown,
/// select page up variant.
    SelectPageUp,
/// select page down variant.
    SelectPageDown,
/// select confirm variant.
    SelectConfirm,
/// select cancel variant.
    SelectCancel,
/// interrupt variant.
    Interrupt,

    // App actions
    Clear,
/// exit variant.
    Exit,
/// suspend variant.
    Suspend,
/// cycle thinking level variant.
    CycleThinkingLevel,
/// cycle model forward variant.
    CycleModelForward,
/// cycle model backward variant.
    CycleModelBackward,
/// select model variant.
    SelectModel,
/// expand tools variant.
    ExpandTools,
/// toggle thinking variant.
    ToggleThinking,
/// toggle session named filter variant.
    ToggleSessionNamedFilter,
/// external editor variant.
    ExternalEditor,
/// follow up variant.
    FollowUp,
/// dequeue variant.
    Dequeue,
/// paste image variant.
    PasteImage,
/// new session variant.
    NewSession,
/// tree variant.
    Tree,
/// fork variant.
    Fork,
/// resume variant.
    Resume,
/// tree fold or up variant.
    TreeFoldOrUp,
/// tree unfold or down variant.
    TreeUnfoldOrDown,
/// tree edit label variant.
    TreeEditLabel,
/// tree toggle label timestamp variant.
    TreeToggleLabelTimestamp,
/// toggle session path variant.
    ToggleSessionPath,
/// toggle session sort variant.
    ToggleSessionSort,
/// rename session variant.
    RenameSession,
/// delete session variant.
    DeleteSession,
/// delete session noninvasive variant.
    DeleteSessionNoninvasive,
/// save model selection variant.
    SaveModelSelection,
/// enable all models variant.
    EnableAllModels,
/// clear all models variant.
    ClearAllModels,
/// toggle provider variant.
    ToggleProvider,
/// reorder up variant.
    ReorderUp,
/// reorder down variant.
    ReorderDown,
/// tree filter default variant.
    TreeFilterDefault,
/// tree filter no tools variant.
    TreeFilterNoTools,
/// tree filter user only variant.
    TreeFilterUserOnly,
/// tree filter labeled only variant.
    TreeFilterLabeledOnly,
/// tree filter all variant.
    TreeFilterAll,
/// tree filter cycle forward variant.
    TreeFilterCycleForward,
/// tree filter cycle backward variant.
    TreeFilterCycleBackward,

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

            // Short aliases
            "submit" => KeyAction::Submit,
            "cancel" => KeyAction::SelectCancel,
            "historyUp" | "history_up" => KeyAction::SelectUp,
            "historyDown" | "history_down" => KeyAction::SelectDown,

            // Fallback to custom
            _ => KeyAction::Custom(s.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Keybinding definition and defaults
// ---------------------------------------------------------------------------

/// A keybinding definition with default keys and description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    /// The action identifier (e.g. "app.interrupt")
    pub action: String,
    /// Default key sequences
    pub default_keys: Vec<String>,
    /// Human-readable description
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

/// Complete set of default keybindings.
pub fn default_keybindings() -> HashMap<String, KeyBinding> {
    let mut m = HashMap::new();

    // --- TUI Editor ---
    m.insert("tui.editor.cursorUp".into(), KeyBinding::new("tui.editor.cursorUp", vec!["up"], "Move cursor up"));
    m.insert("tui.editor.cursorDown".into(), KeyBinding::new("tui.editor.cursorDown", vec!["down"], "Move cursor down"));
    m.insert("tui.editor.cursorLeft".into(), KeyBinding::new("tui.editor.cursorLeft", vec!["left", "ctrl+b"], "Move cursor left"));
    m.insert("tui.editor.cursorRight".into(), KeyBinding::new("tui.editor.cursorRight", vec!["right", "ctrl+f"], "Move cursor right"));
    m.insert("tui.editor.cursorWordLeft".into(), KeyBinding::new("tui.editor.cursorWordLeft", vec!["alt+left", "ctrl+left", "alt+b"], "Move cursor word left"));
    m.insert("tui.editor.cursorWordRight".into(), KeyBinding::new("tui.editor.cursorWordRight", vec!["alt+right", "ctrl+right", "alt+f"], "Move cursor word right"));
    m.insert("tui.editor.cursorLineStart".into(), KeyBinding::new("tui.editor.cursorLineStart", vec!["home", "ctrl+a"], "Move to line start"));
    m.insert("tui.editor.cursorLineEnd".into(), KeyBinding::new("tui.editor.cursorLineEnd", vec!["end", "ctrl+e"], "Move to line end"));
    m.insert("tui.editor.jumpForward".into(), KeyBinding::new("tui.editor.jumpForward", vec!["ctrl+]"], "Jump forward to character"));
    m.insert("tui.editor.jumpBackward".into(), KeyBinding::new("tui.editor.jumpBackward", vec!["ctrl+alt+]"], "Jump backward to character"));
    m.insert("tui.editor.pageUp".into(), KeyBinding::new("tui.editor.pageUp", vec!["pageup"], "Page up"));
    m.insert("tui.editor.pageDown".into(), KeyBinding::new("tui.editor.pageDown", vec!["pagedown"], "Page down"));
    m.insert("tui.editor.deleteCharBackward".into(), KeyBinding::new("tui.editor.deleteCharBackward", vec!["backspace"], "Delete character backward"));
    m.insert("tui.editor.deleteCharForward".into(), KeyBinding::new("tui.editor.deleteCharForward", vec!["delete", "ctrl+d"], "Delete character forward"));
    m.insert("tui.editor.deleteWordBackward".into(), KeyBinding::new("tui.editor.deleteWordBackward", vec!["ctrl+w", "alt+backspace"], "Delete word backward"));
    m.insert("tui.editor.deleteWordForward".into(), KeyBinding::new("tui.editor.deleteWordForward", vec!["alt+d", "alt+delete"], "Delete word forward"));
    m.insert("tui.editor.deleteToLineStart".into(), KeyBinding::new("tui.editor.deleteToLineStart", vec!["ctrl+u"], "Delete to line start"));
    m.insert("tui.editor.deleteToLineEnd".into(), KeyBinding::new("tui.editor.deleteToLineEnd", vec!["ctrl+k"], "Delete to line end"));
    m.insert("tui.editor.yank".into(), KeyBinding::new("tui.editor.yank", vec!["ctrl+y"], "Yank"));
    m.insert("tui.editor.yankPop".into(), KeyBinding::new("tui.editor.yankPop", vec!["alt+y"], "Yank pop"));
    m.insert("tui.editor.undo".into(), KeyBinding::new("tui.editor.undo", vec!["ctrl+-"], "Undo"));

    // --- TUI Input ---
    m.insert("tui.input.newLine".into(), KeyBinding::new("tui.input.newLine", vec!["shift+enter"], "Insert newline"));
    m.insert("tui.input.submit".into(), KeyBinding::new("tui.input.submit", vec!["enter"], "Submit input"));
    m.insert("tui.input.tab".into(), KeyBinding::new("tui.input.tab", vec!["tab"], "Tab / autocomplete"));
    m.insert("tui.input.copy".into(), KeyBinding::new("tui.input.copy", vec!["ctrl+c"], "Copy selection"));

    // --- TUI Select ---
    m.insert("tui.select.up".into(), KeyBinding::new("tui.select.up", vec!["up"], "Move selection up"));
    m.insert("tui.select.down".into(), KeyBinding::new("tui.select.down", vec!["down"], "Move selection down"));
    m.insert("tui.select.pageUp".into(), KeyBinding::new("tui.select.pageUp", vec!["pageup"], "Selection page up"));
    m.insert("tui.select.pageDown".into(), KeyBinding::new("tui.select.pageDown", vec!["pagedown"], "Selection page down"));
    m.insert("tui.select.confirm".into(), KeyBinding::new("tui.select.confirm", vec!["enter"], "Confirm selection"));
    m.insert("tui.select.cancel".into(), KeyBinding::new("tui.select.cancel", vec!["escape", "ctrl+c"], "Cancel selection"));

    // --- App actions ---
    m.insert("app.interrupt".into(), KeyBinding::new("app.interrupt", vec!["escape"], "Cancel or abort"));
    m.insert("app.clear".into(), KeyBinding::new("app.clear", vec!["ctrl+c"], "Clear editor"));
    m.insert("app.exit".into(), KeyBinding::new("app.exit", vec!["ctrl+d"], "Exit when editor is empty"));
    m.insert("app.suspend".into(), KeyBinding::new("app.suspend", vec!["ctrl+z"], "Suspend to background"));
    m.insert("app.thinking.cycle".into(), KeyBinding::new("app.thinking.cycle", vec!["shift+tab"], "Cycle thinking level"));
    m.insert("app.model.cycleForward".into(), KeyBinding::new("app.model.cycleForward", vec!["ctrl+p"], "Cycle to next model"));
    m.insert("app.model.cycleBackward".into(), KeyBinding::new("app.model.cycleBackward", vec!["shift+ctrl+p"], "Cycle to previous model"));
    m.insert("app.model.select".into(), KeyBinding::new("app.model.select", vec!["ctrl+l"], "Open model selector"));
    m.insert("app.tools.expand".into(), KeyBinding::new("app.tools.expand", vec!["ctrl+o"], "Toggle tool output"));
    m.insert("app.thinking.toggle".into(), KeyBinding::new("app.thinking.toggle", vec!["ctrl+t"], "Toggle thinking blocks"));
    m.insert("app.session.toggleNamedFilter".into(), KeyBinding::new("app.session.toggleNamedFilter", vec!["ctrl+n"], "Toggle named session filter"));
    m.insert("app.editor.external".into(), KeyBinding::new("app.editor.external", vec!["ctrl+g"], "Open external editor"));
    m.insert("app.message.followUp".into(), KeyBinding::new("app.message.followUp", vec!["alt+enter"], "Queue follow-up message"));
    m.insert("app.message.dequeue".into(), KeyBinding::new("app.message.dequeue", vec!["alt+up"], "Restore queued messages"));
    m.insert("app.clipboard.pasteImage".into(), KeyBinding::new("app.clipboard.pasteImage", vec!["ctrl+v"], "Paste image from clipboard"));
    m.insert("app.session.new".into(), KeyBinding::new("app.session.new", vec![], "Start a new session"));
    m.insert("app.session.tree".into(), KeyBinding::new("app.session.tree", vec![], "Open session tree"));
    m.insert("app.session.fork".into(), KeyBinding::new("app.session.fork", vec![], "Fork current session"));
    m.insert("app.session.resume".into(), KeyBinding::new("app.session.resume", vec![], "Resume a session"));
    m.insert("app.tree.foldOrUp".into(), KeyBinding::new("app.tree.foldOrUp", vec!["ctrl+left", "alt+left"], "Fold tree branch or move up"));
    m.insert("app.tree.unfoldOrDown".into(), KeyBinding::new("app.tree.unfoldOrDown", vec!["ctrl+right", "alt+right"], "Unfold tree branch or move down"));
    m.insert("app.tree.editLabel".into(), KeyBinding::new("app.tree.editLabel", vec!["shift+l"], "Edit tree label"));
    m.insert("app.tree.toggleLabelTimestamp".into(), KeyBinding::new("app.tree.toggleLabelTimestamp", vec!["shift+t"], "Toggle tree label timestamps"));
    m.insert("app.session.togglePath".into(), KeyBinding::new("app.session.togglePath", vec!["ctrl+p"], "Toggle session path display"));
    m.insert("app.session.toggleSort".into(), KeyBinding::new("app.session.toggleSort", vec!["ctrl+s"], "Toggle session sort mode"));
    m.insert("app.session.rename".into(), KeyBinding::new("app.session.rename", vec!["ctrl+r"], "Rename session"));
    m.insert("app.session.delete".into(), KeyBinding::new("app.session.delete", vec!["ctrl+d"], "Delete session"));
    m.insert("app.session.deleteNoninvasive".into(), KeyBinding::new("app.session.deleteNoninvasive", vec!["ctrl+backspace"], "Delete session when query is empty"));
    m.insert("app.models.save".into(), KeyBinding::new("app.models.save", vec!["ctrl+s"], "Save model selection"));
    m.insert("app.models.enableAll".into(), KeyBinding::new("app.models.enableAll", vec!["ctrl+a"], "Enable all models"));
    m.insert("app.models.clearAll".into(), KeyBinding::new("app.models.clearAll", vec!["ctrl+x"], "Clear all models"));
    m.insert("app.models.toggleProvider".into(), KeyBinding::new("app.models.toggleProvider", vec!["ctrl+p"], "Toggle all models for provider"));
    m.insert("app.models.reorderUp".into(), KeyBinding::new("app.models.reorderUp", vec!["alt+up"], "Move model up in order"));
    m.insert("app.models.reorderDown".into(), KeyBinding::new("app.models.reorderDown", vec!["alt+down"], "Move model down in order"));
    m.insert("app.tree.filter.default".into(), KeyBinding::new("app.tree.filter.default", vec!["ctrl+d"], "Tree filter: default view"));
    m.insert("app.tree.filter.noTools".into(), KeyBinding::new("app.tree.filter.noTools", vec!["ctrl+t"], "Tree filter: hide tool results"));
    m.insert("app.tree.filter.userOnly".into(), KeyBinding::new("app.tree.filter.userOnly", vec!["ctrl+u"], "Tree filter: user messages only"));
    m.insert("app.tree.filter.labeledOnly".into(), KeyBinding::new("app.tree.filter.labeledOnly", vec!["ctrl+l"], "Tree filter: labeled entries only"));
    m.insert("app.tree.filter.all".into(), KeyBinding::new("app.tree.filter.all", vec!["ctrl+a"], "Tree filter: show all entries"));
    m.insert("app.tree.filter.cycleForward".into(), KeyBinding::new("app.tree.filter.cycleForward", vec!["ctrl+o"], "Tree filter: cycle forward"));
    m.insert("app.tree.filter.cycleBackward".into(), KeyBinding::new("app.tree.filter.cycleBackward", vec!["shift+ctrl+o"], "Tree filter: cycle backward"));

    m
}

// ---------------------------------------------------------------------------
// TOML config format
// ---------------------------------------------------------------------------

/// TOML keybindings configuration file format.
///
/// Example `keybindings.toml`:
/// ```toml
/// [keybindings]
/// "app.interrupt" = "escape"
/// "app.clear" = "ctrl+c"
/// "tui.editor.cursorUp" = ["up", "k"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingsFile {
    /// User overrides for keybinding actions.
    /// A string value means a single key; an array means multiple keys.
    #[serde(default)]
    pub keybindings: HashMap<String, TomlKeyBinding>,
}

/// A keybinding value in TOML: either a single key string or an array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TomlKeyBinding {
/// single variant.
    Single(String),
/// multiple variant.
    Multiple(Vec<String>),
}

impl TomlKeyBinding {
    /// Convert to a flat list of key strings.
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            TomlKeyBinding::Single(s) => vec![s.clone()],
            TomlKeyBinding::Multiple(v) => v.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

/// A detected keybinding conflict: the same key assigned to multiple actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingConflict {
    /// The key sequence that conflicts.
    pub key: String,
    /// The actions that all bind to this key.
    pub actions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Keybindings manager
// ---------------------------------------------------------------------------

/// Keybindings manager: loads defaults, merges user/project overrides,
/// resolves effective keys, and detects conflicts.
pub struct KeybindingsManager {
    /// All defined keybindings (defaults).
    bindings: HashMap<String, KeyBinding>,
    /// User overrides (action → keys).
    user_overrides: HashMap<String, Vec<String>>,
    /// Project overrides (action → keys).
    project_overrides: HashMap<String, Vec<String>>,
    /// Detected conflicts.
    conflicts: Vec<KeybindingConflict>,
}

impl KeybindingsManager {
    /// Create a new manager with defaults only.
    pub fn new() -> Self {
        let mut mgr = Self {
            bindings: default_keybindings(),
            user_overrides: HashMap::new(),
            project_overrides: HashMap::new(),
            conflicts: Vec::new(),
        };
        mgr.detect_conflicts();
        mgr
    }

    /// Create from hierarchical config: user + project paths.
    ///
    /// Priority: project overrides > user overrides > defaults.
    pub fn from_paths(user_path: Option<&Path>, project_path: Option<&Path>) -> Self {
        let mut mgr = Self::new();

        // Load user-level overrides (~/.oxi/keybindings.toml)
        if let Some(path) = user_path {
            if let Ok(keys) = Self::load_toml_overrides(path) {
                mgr.user_overrides = keys;
            } else if let Ok(keys) = Self::load_json_overrides(path) {
                // Fallback: legacy JSON format
                mgr.user_overrides = keys;
            }
        }

        // Load project-level overrides (.oxi/keybindings.toml)
        if let Some(path) = project_path {
            if let Ok(keys) = Self::load_toml_overrides(path) {
                mgr.project_overrides = keys;
            }
        }

        mgr.detect_conflicts();
        mgr
    }

    /// Convenience: create from the default user path.
    pub fn from_default_user_path() -> Self {
        let user_path = default_keybindings_path();
        Self::from_paths(user_path.as_deref(), None)
    }

    /// Register a new keybinding definition.
    pub fn register(&mut self, binding: KeyBinding) {
        self.bindings.insert(binding.action.clone(), binding);
        self.detect_conflicts();
    }

    /// Set user override for an action.
    pub fn set_user_override(&mut self, action: &str, keys: Vec<String>) {
        self.user_overrides.insert(action.to_string(), keys);
        self.detect_conflicts();
    }

    /// Set project override for an action.
    pub fn set_project_override(&mut self, action: &str, keys: Vec<String>) {
        self.project_overrides.insert(action.to_string(), keys);
        self.detect_conflicts();
    }

    /// Get the resolved key sequences for an action.
    /// Priority: project > user > defaults.
    pub fn get_keys(&self, action: &str) -> Vec<String> {
        if let Some(keys) = self.project_overrides.get(action) {
            return keys.clone();
        }
        if let Some(keys) = self.user_overrides.get(action) {
            return keys.clone();
        }
        if let Some(binding) = self.bindings.get(action) {
            return binding.default_keys.clone();
        }
        Vec::new()
    }

    /// Get parsed `KeySequence`s for an action.
    pub fn get_key_sequences(&self, action: &str) -> Vec<KeySequence> {
        self.get_keys(action)
            .iter()
            .filter_map(|k| KeySequence::parse(k))
            .collect()
    }

    /// Check if a key sequence matches an action.
    pub fn matches(&self, action: &str, seq: &KeySequence) -> bool {
        self.get_key_sequences(action).iter().any(|k| k == seq)
    }

    /// Look up which action matches the given key sequence.
    pub fn action_for(&self, seq: &KeySequence) -> Option<String> {
        for action in self.bindings.keys() {
            if self.matches(action, seq) {
                return Some(action.clone());
            }
        }
        None
    }

    /// Get a binding definition by action name.
    pub fn get_binding(&self, action: &str) -> Option<&KeyBinding> {
        self.bindings.get(action)
    }

    /// Get all registered bindings.
    pub fn all_bindings(&self) -> &HashMap<String, KeyBinding> {
        &self.bindings
    }

    /// Get user overrides.
    pub fn user_overrides(&self) -> &HashMap<String, Vec<String>> {
        &self.user_overrides
    }

    /// Get project overrides.
    pub fn project_overrides(&self) -> &HashMap<String, Vec<String>> {
        &self.project_overrides
    }

    /// Get detected conflicts.
    pub fn conflicts(&self) -> &[KeybindingConflict] {
        &self.conflicts
    }

    /// Check whether there are any conflicts.
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    /// Reload overrides from files.
    pub fn reload(&mut self, user_path: Option<&Path>, project_path: Option<&Path>) {
        self.user_overrides.clear();
        self.project_overrides.clear();

        if let Some(path) = user_path {
            if let Ok(keys) = Self::load_toml_overrides(path) {
                self.user_overrides = keys;
            } else if let Ok(keys) = Self::load_json_overrides(path) {
                self.user_overrides = keys;
            }
        }
        if let Some(path) = project_path {
            if let Ok(keys) = Self::load_toml_overrides(path) {
                self.project_overrides = keys;
            }
        }
        self.detect_conflicts();
    }

    /// Export current effective overrides to a TOML file.
    pub fn export_to_toml(&self, path: &Path) -> std::io::Result<()> {
        let mut keybindings = HashMap::new();
        // Export only user-level overrides, not project overrides
        for (action, keys) in &self.user_overrides {
            if keys.len() == 1 {
                keybindings.insert(
                    action.clone(),
                    TomlKeyBinding::Single(keys[0].clone()),
                );
            } else {
                keybindings.insert(
                    action.clone(),
                    TomlKeyBinding::Multiple(keys.clone()),
                );
            }
        }
        let file = KeybindingsFile { keybindings };
        let content = toml::to_string_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, content)
    }

    // -- Private helpers --

    /// Detect conflicts where the same key is bound to multiple actions.
    fn detect_conflicts(&mut self) {
        self.conflicts.clear();
        let mut key_to_actions: HashMap<String, Vec<String>> = HashMap::new();

        for action in self.bindings.keys() {
            for key in self.get_keys(action) {
                key_to_actions
                    .entry(key)
                    .or_default()
                    .push(action.clone());
            }
        }

        for (key, actions) in key_to_actions {
            if actions.len() > 1 {
                // Sort for deterministic output
                let mut sorted = actions;
                sorted.sort();
                self.conflicts.push(KeybindingConflict {
                    key,
                    actions: sorted,
                });
            }
        }

        // Sort conflicts by key for deterministic output
        self.conflicts.sort_by(|a, b| a.key.cmp(&b.key));
    }

    /// Load overrides from a TOML file.
    fn load_toml_overrides(path: &Path) -> Result<HashMap<String, Vec<String>>, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("IO error: {}", e))?;
        let file: KeybindingsFile =
            toml::from_str(&content).map_err(|e| format!("TOML parse error: {}", e))?;
        let mut overrides = HashMap::new();
        for (action, binding) in file.keybindings {
            let keys = binding.to_vec();
            // Validate each key can be parsed
            for key in &keys {
                if KeySequence::parse(key).is_none() {
                    tracing::warn!(
                        "Ignoring unparseable key '{}' for action '{}'",
                        key,
                        action
                    );
                }
            }
            overrides.insert(action, keys);
        }
        Ok(overrides)
    }

    /// Load overrides from a legacy JSON file.
    fn load_json_overrides(path: &Path) -> Result<HashMap<String, Vec<String>>, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("IO error: {}", e))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("JSON parse error: {}", e))?;
        let mut overrides = HashMap::new();
        if let serde_json::Value::Object(map) = parsed {
            for (action, value) in map {
                let keys = match value {
                    serde_json::Value::String(s) => vec![s],
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    _ => continue,
                };
                overrides.insert(action, keys);
            }
        }
        Ok(overrides)
    }
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Get the default user keybindings file path (`~/.oxi/keybindings.toml`).
pub fn default_keybindings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".oxi").join("keybindings.toml"))
}

/// Get the project keybindings file path (`.oxi/keybindings.toml`).
pub fn project_keybindings_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".oxi").join("keybindings.toml")
}

// ---------------------------------------------------------------------------
// Legacy compatibility (preserving public API)
// ---------------------------------------------------------------------------

/// Load user keybindings from a JSON file (legacy).
pub fn load_user_keybindings(path: &Path) -> Option<UserKeybindings> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save user keybindings to a JSON file (legacy).
pub fn save_user_keybindings(path: &Path, bindings: &UserKeybindings) -> std::io::Result<()> {
    let content = serde_json::to_string_pretty(bindings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, content)
}

/// User-configurable keybindings (legacy JSON format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserKeybindings {
    /// Map of action name to custom key sequence(s)
    #[serde(default)]
    pub bindings: HashMap<String, Vec<String>>,
}

/// Parse a key sequence string into components (legacy).
/// Returns `Vec<(ctrl, alt, key_char)>`.
pub fn parse_key_sequence(sequence: &str) -> Vec<(bool, bool, char)> {
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
            "space" => ' ',
            "tab" => '\t',
            "enter" | "return" => '\n',
            "escape" | "esc" => '\x1b',
            "backspace" => '\x7f',
            _ => part.chars().next().unwrap_or(' '),
        };

        result.push((ctrl, alt, key_char));
        ctrl = false;
        alt = false;
    }

    result
}

/// Format key sequences for display (legacy).
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // --- KeySequence parsing ---

    #[test]
    fn test_parse_ctrl_c() {
        let seq = KeySequence::parse("ctrl+c").unwrap();
        assert!(seq.ctrl);
        assert!(!seq.alt);
        assert!(!seq.shift);
        assert_eq!(seq.key, KeyName::Char('c'));
    }

    #[test]
    fn test_parse_alt_enter() {
        let seq = KeySequence::parse("alt+enter").unwrap();
        assert!(!seq.ctrl);
        assert!(seq.alt);
        assert_eq!(seq.key, KeyName::Enter);
    }

    #[test]
    fn test_parse_shift_tab() {
        let seq = KeySequence::parse("shift+tab").unwrap();
        assert!(seq.shift);
        assert_eq!(seq.key, KeyName::Tab);
    }

    #[test]
    fn test_parse_ctrl_shift_p() {
        let seq = KeySequence::parse("ctrl+shift+p").unwrap();
        assert!(seq.ctrl);
        assert!(seq.shift);
        assert_eq!(seq.key, KeyName::Char('p'));
    }

    #[test]
    fn test_parse_shift_ctrl_p() {
        // Modifier order should not matter
        let seq = KeySequence::parse("shift+ctrl+p").unwrap();
        assert!(seq.ctrl);
        assert!(seq.shift);
        assert_eq!(seq.key, KeyName::Char('p'));
    }

    #[test]
    fn test_parse_escape() {
        let seq = KeySequence::parse("escape").unwrap();
        assert_eq!(seq.key, KeyName::Escape);
        assert!(!seq.ctrl && !seq.alt && !seq.shift);
    }

    #[test]
    fn test_parse_esc() {
        let seq = KeySequence::parse("esc").unwrap();
        assert_eq!(seq.key, KeyName::Escape);
    }

    #[test]
    fn test_parse_f1_through_f12() {
        for n in 1..=12 {
            let key = format!("f{}", n);
            let seq = KeySequence::parse(&key).unwrap();
            assert_eq!(seq.key, KeyName::F(n), "Failed for {}", key);
        }
    }

    #[test]
    fn test_parse_f0_invalid() {
        assert!(KeySequence::parse("f0").is_none());
    }

    #[test]
    fn test_parse_f13_invalid() {
        assert!(KeySequence::parse("f13").is_none());
    }

    #[test]
    fn test_parse_pageup() {
        let seq = KeySequence::parse("pageup").unwrap();
        assert_eq!(seq.key, KeyName::PageUp);
    }

    #[test]
    fn test_parse_pagedown() {
        let seq = KeySequence::parse("pagedown").unwrap();
        assert_eq!(seq.key, KeyName::PageDown);
    }

    #[test]
    fn test_parse_home() {
        let seq = KeySequence::parse("home").unwrap();
        assert_eq!(seq.key, KeyName::Home);
    }

    #[test]
    fn test_parse_end() {
        let seq = KeySequence::parse("end").unwrap();
        assert_eq!(seq.key, KeyName::End);
    }

    #[test]
    fn test_parse_delete() {
        let seq = KeySequence::parse("delete").unwrap();
        assert_eq!(seq.key, KeyName::Delete);
    }

    #[test]
    fn test_parse_backspace() {
        let seq = KeySequence::parse("backspace").unwrap();
        assert_eq!(seq.key, KeyName::Backspace);
    }

    #[test]
    fn test_parse_insert() {
        let seq = KeySequence::parse("insert").unwrap();
        assert_eq!(seq.key, KeyName::Insert);
    }

    #[test]
    fn test_parse_ctrl_alt_delete() {
        let seq = KeySequence::parse("ctrl+alt+delete").unwrap();
        assert!(seq.ctrl);
        assert!(seq.alt);
        assert_eq!(seq.key, KeyName::Delete);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(KeySequence::parse("").is_none());
        assert!(KeySequence::parse("   ").is_none());
        assert!(KeySequence::parse("foobar").is_none());
    }

    #[test]
    fn test_parse_single_char() {
        let seq = KeySequence::parse("a").unwrap();
        assert_eq!(seq.key, KeyName::Char('a'));
    }

    // --- KeySequence display ---

    #[test]
    fn test_key_sequence_display() {
        let seq = KeySequence::parse("ctrl+alt+delete").unwrap();
        assert_eq!(seq.to_string(), "ctrl+alt+delete");
    }

    #[test]
    fn test_key_name_display() {
        assert_eq!(KeyName::Enter.to_string(), "enter");
        assert_eq!(KeyName::F(5).to_string(), "f5");
        assert_eq!(KeyName::PageUp.to_string(), "pageup");
    }

    // --- Round-trip notation ---

    #[test]
    fn test_notation_round_trip() {
        let notations = vec![
            "ctrl+c", "alt+enter", "shift+tab", "escape", "enter", "tab",
            "ctrl+shift+p", "alt+backspace", "f1", "f12", "pageup", "pagedown",
            "home", "end", "delete", "backspace",
        ];
        for notation in notations {
            let seq = KeySequence::parse(notation).unwrap_or_else(|| {
                panic!("Failed to parse: {}", notation)
            });
            let round_tripped = seq.to_notation();
            let re_parsed = KeySequence::parse(&round_tripped).unwrap_or_else(|| {
                panic!("Failed to re-parse: {}", round_tripped)
            });
            assert_eq!(seq, re_parsed, "Round-trip failed for {}", notation);
        }
    }

    // --- KeyAction ---

    #[test]
    fn test_key_action_from_str() {
        assert_eq!(KeyAction::from("app.interrupt"), KeyAction::Interrupt);
        assert_eq!(KeyAction::from("app.clear"), KeyAction::Clear);
        assert_eq!(KeyAction::from("submit"), KeyAction::Submit);
        assert_eq!(
            KeyAction::from("tui.editor.cursorUp"),
            KeyAction::CursorUp
        );
        assert_eq!(
            KeyAction::from("tui.input.submit"),
            KeyAction::Submit
        );
    }

    #[test]
    fn test_key_action_custom() {
        let action = KeyAction::from("my-extension.action");
        assert!(matches!(action, KeyAction::Custom(s) if s == "my-extension.action"));
    }

    // --- Default keybindings ---

    #[test]
    fn test_default_keybindings_not_empty() {
        let bindings = default_keybindings();
        assert!(!bindings.is_empty());
        // Verify key actions exist
        assert!(bindings.contains_key("app.interrupt"));
        assert!(bindings.contains_key("tui.input.submit"));
        assert!(bindings.contains_key("tui.editor.cursorUp"));
        assert!(bindings.contains_key("app.thinking.cycle"));
        assert!(bindings.contains_key("app.tree.filter.cycleForward"));
    }

    // --- KeybindingsManager basics ---

    #[test]
    fn test_keybindings_manager_new() {
        let manager = KeybindingsManager::new();
        assert!(!manager.all_bindings().is_empty());
    }

    #[test]
    fn test_get_keys_default() {
        let manager = KeybindingsManager::new();
        let keys = manager.get_keys("app.interrupt");
        assert_eq!(keys, vec!["escape"]);
    }

    #[test]
    fn test_set_user_override() {
        let mut manager = KeybindingsManager::new();
        manager.set_user_override("app.interrupt", vec!["ctrl+c".to_string()]);
        assert_eq!(
            manager.get_keys("app.interrupt"),
            vec!["ctrl+c"]
        );
    }

    #[test]
    fn test_project_override_takes_priority() {
        let mut manager = KeybindingsManager::new();
        manager.set_user_override("app.interrupt", vec!["ctrl+c".to_string()]);
        manager.set_project_override("app.interrupt", vec!["f1".to_string()]);
        // Project overrides user
        assert_eq!(manager.get_keys("app.interrupt"), vec!["f1"]);
    }

    #[test]
    fn test_matches() {
        let manager = KeybindingsManager::new();
        assert!(manager.matches("app.interrupt", &KeySequence::parse("escape").unwrap()));
        assert!(manager.matches("app.exit", &KeySequence::parse("ctrl+d").unwrap()));
    }

    #[test]
    fn test_action_for() {
        let manager = KeybindingsManager::new();
        // ctrl+z has a unique binding (no conflicts)
        assert_eq!(
            manager.action_for(&KeySequence::parse("ctrl+z").unwrap()),
            Some("app.suspend".to_string())
        );
    }

    #[test]
    fn test_get_key_sequences() {
        let manager = KeybindingsManager::new();
        let seqs = manager.get_key_sequences("tui.editor.cursorLeft");
        assert!(!seqs.is_empty());
        assert!(seqs.iter().any(|s| s.key == KeyName::Left));
        assert!(seqs.iter().any(|s| s.ctrl && s.key == KeyName::Char('b')));
    }

    // --- Conflict detection ---

    #[test]
    fn test_conflict_detection_on_create() {
        let manager = KeybindingsManager::new();
        // Many default bindings share keys (e.g. ctrl+c used in multiple places)
        // so conflicts should be detected
        if !manager.conflicts().is_empty() {
            // Verify conflict structure is valid
            for conflict in manager.conflicts() {
                assert!(!conflict.key.is_empty());
                assert!(conflict.actions.len() > 1);
            }
        }
    }

    #[test]
    fn test_conflict_after_override() {
        let mut manager = KeybindingsManager::new();
        // Create a conflict: bind two actions to the same key
        manager.set_user_override("app.interrupt", vec!["ctrl+x".to_string()]);
        manager.set_user_override("app.clear", vec!["ctrl+x".to_string()]);
        let conflicts: Vec<_> = manager
            .conflicts()
            .iter()
            .filter(|c| c.key == "ctrl+x")
            .collect();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].actions.contains(&"app.interrupt".to_string()));
        assert!(conflicts[0].actions.contains(&"app.clear".to_string()));
    }

    #[test]
    fn test_no_conflict_different_keys() {
        let mut manager = KeybindingsManager::new();
        manager.set_user_override("app.interrupt", vec!["f1".to_string()]);
        manager.set_user_override("app.clear", vec!["f2".to_string()]);
        let conflicts: Vec<_> = manager
            .conflicts()
            .iter()
            .filter(|c| c.key == "f1" || c.key == "f2")
            .collect();
        assert!(conflicts.is_empty());
    }

    // --- TOML loading ---

    #[test]
    fn test_load_toml_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[keybindings]
"app.interrupt" = "f1"
"tui.editor.cursorUp" = ["up", "k"]
"app.exit" = "ctrl+d"
"#
        )
        .unwrap();

        let overrides = KeybindingsManager::load_toml_overrides(&path).unwrap();
        assert_eq!(overrides.get("app.interrupt"), Some(&vec!["f1".to_string()]));
        assert_eq!(
            overrides.get("tui.editor.cursorUp"),
            Some(&vec!["up".to_string(), "k".to_string()])
        );
    }

    #[test]
    fn test_load_toml_invalid_key_warns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[keybindings]
"app.interrupt" = "!!!invalid!!!"
"#
        )
        .unwrap();

        // Should load but warn (tracing), not panic
        let overrides = KeybindingsManager::load_toml_overrides(&path).unwrap();
        assert!(overrides.contains_key("app.interrupt"));
    }

    #[test]
    fn test_load_json_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"app.interrupt": "f1", "app.clear": ["ctrl+c", "escape"]}}"#
        )
        .unwrap();

        let overrides = KeybindingsManager::load_json_overrides(&path).unwrap();
        assert_eq!(overrides.get("app.interrupt"), Some(&vec!["f1".to_string()]));
        assert_eq!(
            overrides.get("app.clear"),
            Some(&vec!["ctrl+c".to_string(), "escape".to_string()])
        );
    }

    // --- from_paths ---

    #[test]
    fn test_from_paths_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[keybindings]
"app.interrupt" = "f1"
"#
        )
        .unwrap();

        let manager = KeybindingsManager::from_paths(Some(&path), None);
        assert_eq!(manager.get_keys("app.interrupt"), vec!["f1"]);
    }

    #[test]
    fn test_from_paths_project_overrides_user() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user.toml");
        let project_path = dir.path().join("project.toml");

        let mut f1 = std::fs::File::create(&user_path).unwrap();
        writeln!(f1, "[keybindings]\n\"app.interrupt\" = \"f1\"").unwrap();

        let mut f2 = std::fs::File::create(&project_path).unwrap();
        writeln!(f2, "[keybindings]\n\"app.interrupt\" = \"f2\"").unwrap();

        let manager = KeybindingsManager::from_paths(Some(&user_path), Some(&project_path));
        assert_eq!(manager.get_keys("app.interrupt"), vec!["f2"]);
    }

    // --- Export TOML ---

    #[test]
    fn test_export_to_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.toml");

        let mut manager = KeybindingsManager::new();
        manager.set_user_override("app.interrupt", vec!["f1".to_string()]);
        manager.set_user_override(
            "tui.editor.cursorUp",
            vec!["up".to_string(), "k".to_string()],
        );

        manager.export_to_toml(&path).unwrap();

        // Re-read and verify
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("app.interrupt"));
        assert!(content.contains("f1"));

        let parsed: KeybindingsFile = toml::from_str(&content).unwrap();
        assert_eq!(
            parsed.keybindings.get("app.interrupt"),
            Some(&TomlKeyBinding::Single("f1".to_string()))
        );
    }

    // --- TOML serde ---

    #[test]
    fn test_toml_key_binding_serde() {
        let single = TomlKeyBinding::Single("ctrl+c".to_string());
        let json = serde_json::to_string(&single).unwrap();
        let parsed: TomlKeyBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, single);

        let multi = TomlKeyBinding::Multiple(vec!["a".to_string(), "b".to_string()]);
        let json = serde_json::to_string(&multi).unwrap();
        let parsed: TomlKeyBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, multi);
    }

    #[test]
    fn test_toml_key_binding_to_vec() {
        let single = TomlKeyBinding::Single("ctrl+c".to_string());
        assert_eq!(single.to_vec(), vec!["ctrl+c"]);

        let multi = TomlKeyBinding::Multiple(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(multi.to_vec(), vec!["a", "b"]);
    }

    // --- Legacy API ---

    #[test]
    fn test_user_keybindings_serde() {
        let user = UserKeybindings {
            bindings: HashMap::from([("app.interrupt".to_string(), vec!["ctrl+c".to_string()])]),
        };
        let json = serde_json::to_string(&user).unwrap();
        let parsed: UserKeybindings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.bindings.get("app.interrupt"),
            Some(&vec!["ctrl+c".to_string()])
        );
    }

    #[test]
    fn test_keybinding_struct() {
        let binding = KeyBinding::new("test.action", vec!["a", "b"], "Test action");
        assert_eq!(binding.action, "test.action");
        assert_eq!(binding.default_keys, vec!["a", "b"]);
        assert_eq!(binding.description, "Test action");
    }

    #[test]
    fn test_parse_key_sequence_legacy() {
        let result = parse_key_sequence("ctrl+c");
        assert!(result.iter().any(|(ctrl, _, _)| *ctrl));

        let result = parse_key_sequence("alt+x");
        assert!(result.iter().any(|(_, alt, _)| *alt));
    }

    #[test]
    fn test_format_key_sequence_legacy() {
        let keys = vec!["ctrl+c".to_string(), "ctrl+x".to_string()];
        let formatted = format_key_sequence(&keys);
        assert!(formatted.contains("Ctrl+C"));
        assert!(formatted.contains("Ctrl+X"));
    }

    #[test]
    fn test_default_keybindings_path() {
        let path = default_keybindings_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains(".oxi"));
        assert!(p.to_string_lossy().contains("keybindings.toml"));
    }

    #[test]
    fn test_project_keybindings_path() {
        let p = project_keybindings_path(Path::new("/my/project"));
        assert_eq!(p, PathBuf::from("/my/project/.oxi/keybindings.toml"));
    }
}
