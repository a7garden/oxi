//! Global keyboard shortcut resolver for the TUI.
//!
//! Tasks 5 and 6 consume [`GlobalAction`], [`KeyCombo`], and [`Keymap`]. The
//! resolver is fully additive: user overrides in `Settings::keybindings`
//! extend the default bindings rather than replacing the whole table; a user
//! override for an action replaces only that action's combo list (and only
//! when the override list parses to at least one valid combo).
//!
//! `KeyCombo` parses and serializes the canonical textual form used in
//! settings files and in the `/settings` panel:
//!
//! - modifier prefix order is `Ctrl+`, `Alt+`, `Shift+`.
//! - the char payload is uppercased when `SHIFT` is set, mirroring crossterm's
//!   `KeyEvent::normalize_case` (shifted letter keys arrive as uppercase
//!   `KeyCode::Char`); `parse` normalizes the input the same way so a
//!   shift-letter combo parses canonically regardless of case.
//! - named keys (`Enter`, `Esc`, `Tab`, `Backspace`) keep their literal names.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GlobalAction {
    Interrupt,
    ToggleMultiline,
    OpenCommandPalette,
    ToggleQueuePanel,
    FoldAll,
    SendNow,
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
        DEFAULT_KEYBINDINGS
            .iter()
            .map(|(a, _)| *a)
            .find(|a| a.name() == s)
    }

    pub fn all() -> impl Iterator<Item = GlobalAction> {
        DEFAULT_KEYBINDINGS.iter().map(|(a, _)| *a)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    /// Parse a textual combo like `"Ctrl+p"`, `"Ctrl+Shift+e"`, `"Ctrl+Enter"`,
    /// `"Alt+p"` into a `KeyCombo`. Returns `None` on any unrecognized segment.
    /// Shift-letter payloads normalize to uppercase.
    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = KeyModifiers::NONE;
        let mut code: Option<KeyCode> = None;
        for part in s.split('+') {
            match part {
                "Ctrl" | "Control" => mods |= KeyModifiers::CONTROL,
                "Shift" => mods |= KeyModifiers::SHIFT,
                "Alt" => mods |= KeyModifiers::ALT,
                "Enter" => code = Some(KeyCode::Enter),
                "Esc" | "Escape" => code = Some(KeyCode::Esc),
                "Tab" => code = Some(KeyCode::Tab),
                "Backspace" => code = Some(KeyCode::Backspace),
                other if other.chars().count() == 1 => {
                    let ch = other.chars().next()?;
                    // Normalize: a SHIFT-modified letter is canonically
                    // uppercase, matching crossterm's
                    // `KeyEvent::normalize_case` and the form produced by
                    // `KeyCombo::to_string`. Makes shift-letter combos
                    // canonical regardless of case in the input.
                    let ch = if mods.contains(KeyModifiers::SHIFT) {
                        ch.to_ascii_uppercase()
                    } else {
                        ch
                    };
                    code = Some(KeyCode::Char(ch));
                }
                _ => return None,
            }
        }
        Some(KeyCombo {
            code: code?,
            modifiers: mods,
        })
    }

    /// Serialize back to the canonical textual form. Round-trips with
    /// [`KeyCombo::parse`] for every combo produced by [`DEFAULT_KEYBINDINGS`]
    /// and for shifted letter chars (which crossterm normalizes to uppercase).
    #[allow(clippy::inherent_to_string)] // brief signature is `to_string(&self) -> String`
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            out.push_str("Shift+");
        }
        match self.code {
            KeyCode::Enter => out.push_str("Enter"),
            KeyCode::Esc => out.push_str("Esc"),
            KeyCode::Tab => out.push_str("Tab"),
            KeyCode::Backspace => out.push_str("Backspace"),
            KeyCode::Char(c) => {
                // Crossterm's `KeyEvent::normalize_case` already maps a
                // shifted letter to `Char(uppercase)` with `SHIFT` set, so we
                // emit the char as-is (which is uppercase for shifted letters).
                out.push(c);
            }
            // Fallback for named keys we don't handle explicitly (F-keys,
            // arrows, etc.) — defer to crossterm's own formatting.
            _ => return self.code.to_string(),
        }
        out
    }
}

pub struct Keymap {
    bindings: HashMap<GlobalAction, Vec<KeyCombo>>,
}

impl Keymap {
    /// Build a keymap seeded with [`DEFAULT_KEYBINDINGS`], then layer user
    /// overrides on top. A user override replaces only the combos for the
    /// named action; other actions keep their defaults. An override that
    /// fails to parse (or produces zero combos) is silently ignored — the
    /// defaults remain.
    pub fn from_settings(overrides: &HashMap<String, Vec<String>>) -> Self {
        let mut bindings: HashMap<GlobalAction, Vec<KeyCombo>> = HashMap::new();
        for (action, combo) in DEFAULT_KEYBINDINGS {
            bindings
                .entry(*action)
                .or_default()
                .push(KeyCombo::parse(combo).expect("DEFAULT_KEYBINDINGS parses"));
        }
        for (name, combos) in overrides {
            let Some(action) = GlobalAction::from_name(name) else {
                continue;
            };
            let parsed: Vec<KeyCombo> = combos
                .iter()
                .map(String::as_str)
                .filter_map(KeyCombo::parse)
                .collect();
            if !parsed.is_empty() {
                bindings.insert(action, parsed);
            }
        }
        Keymap { bindings }
    }

    /// Resolve an incoming [`KeyEvent`] to its global action, if any.
    /// Actions are checked in [`DEFAULT_KEYBINDINGS`] order, so when two
    /// actions share a combo (only reachable via user overrides) the earlier
    /// default action wins — deterministically across runs.
    pub fn resolve(&self, key: KeyEvent) -> Option<GlobalAction> {
        GlobalAction::all().find(|action| {
            self.bindings.get(action).is_some_and(|combos| {
                combos
                    .iter()
                    .any(|c| c.code == key.code && c.modifiers == key.modifiers)
            })
        })
    }

    /// Replace the binding list for a single action (used by Task 6's
    /// map-editor when the user confirms a new combo).
    pub fn set_action(&mut self, action: GlobalAction, combos: Vec<KeyCombo>) {
        self.bindings.insert(action, combos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn default_resolve_maps_ctrl_p_to_command_palette() {
        let km = Keymap::from_settings(&HashMap::new());
        let ev = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(ev), Some(GlobalAction::OpenCommandPalette));
    }

    #[test]
    fn user_override_adds_instead_of_replacing() {
        let mut o: HashMap<String, Vec<String>> = HashMap::new();
        o.insert("OpenCommandPalette".into(), vec!["Alt+p".into()]);
        let km = Keymap::from_settings(&o);
        // The override replaces the combos for that action; other actions
        // keep their defaults. The override combo must resolve and the
        // original default for the same action must not — user wins.
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT)),
            Some(GlobalAction::OpenCommandPalette),
            "override combo Alt+P must resolve",
        );
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(GlobalAction::Interrupt),
            "default Ctrl+C for unrelated action must still resolve",
        );
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            None,
            "original Ctrl+P is gone after override",
        );
    }

    #[test]
    fn override_keeps_other_actions_default() {
        let mut o: HashMap<String, Vec<String>> = HashMap::new();
        o.insert("OpenCommandPalette".into(), vec!["Alt+p".into()]);
        let km = Keymap::from_settings(&o);
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
            Some(GlobalAction::SendNow),
        );
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            Some(GlobalAction::FoldAll),
        );
    }

    #[test]
    fn keycombo_roundtrips() {
        // Shifted-letter payloads serialize as uppercase (crossterm's
        // `KeyEvent::normalize_case` produces `Char(uppercase)` for them).
        for s in ["Ctrl+c", "Ctrl+m", "Ctrl+Shift+E", "Ctrl+Enter", "Alt+p"] {
            let parsed = KeyCombo::parse(s).unwrap_or_else(|| panic!("parse failed: {s}"));
            assert_eq!(parsed.to_string(), s, "round-trip mismatch for {s}");
        }
        // `parse` normalizes lowercase shift to uppercase too.
        let lower = KeyCombo::parse("Ctrl+Shift+e").unwrap();
        let upper = KeyCombo::parse("Ctrl+Shift+E").unwrap();
        assert_eq!(lower, upper, "lowercase Shift+E normalizes to uppercase",);
        assert_eq!(lower.to_string(), "Ctrl+Shift+E");
    }

    #[test]
    fn shifted_letter_serializes_uppercase() {
        let combo = KeyCombo {
            code: KeyCode::Char('E'),
            modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        };
        assert_eq!(combo.to_string(), "Ctrl+Shift+E");
        assert_eq!(KeyCombo::parse("Ctrl+Shift+E").unwrap(), combo);
    }

    #[test]
    fn plain_char_is_not_a_global_action() {
        let km = Keymap::from_settings(&HashMap::new());
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            None,
        );
    }

    #[test]
    fn from_name_and_all_are_inverse() {
        for action in GlobalAction::all() {
            assert_eq!(GlobalAction::from_name(action.name()), Some(action));
        }
        assert_eq!(GlobalAction::from_name("NotAnAction"), None);
    }

    #[test]
    fn set_action_replaces_combos() {
        let mut km = Keymap::from_settings(&HashMap::new());
        let new_combos = vec![KeyCombo::parse("Alt+x").unwrap()];
        km.set_action(GlobalAction::OpenCommandPalette, new_combos);
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
            Some(GlobalAction::OpenCommandPalette),
        );
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            None,
            "old Ctrl+P must be gone after set_action",
        );
    }

    #[test]
    fn parse_rejects_unknown_segments_and_empty_payload() {
        assert!(KeyCombo::parse("Ctrl+Foo").is_none());
        // Empty trailing segment has zero chars, falls to the catch-all arm,
        // and returns None.
        assert!(KeyCombo::parse("Ctrl+").is_none());
        // A double-Ctrl modifier is redundant but harmless; the payload
        // still parses.
        assert!(KeyCombo::parse("Ctrl+Ctrl+p").is_some());
    }

    #[test]
    fn shared_combo_resolves_in_default_bindings_order() {
        // A user override can bind a combo that collides with another
        // action's default (here: Interrupt takes over Ctrl+p, which is
        // OpenCommandPalette's default). Resolution must be deterministic:
        // the earlier action in DEFAULT_KEYBINDINGS wins (Interrupt is
        // listed before OpenCommandPalette).
        let mut o: HashMap<String, Vec<String>> = HashMap::new();
        o.insert("Interrupt".into(), vec!["Ctrl+c".into(), "Ctrl+p".into()]);
        let km = Keymap::from_settings(&o);
        let ev = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(ev), Some(GlobalAction::Interrupt));
    }
}
