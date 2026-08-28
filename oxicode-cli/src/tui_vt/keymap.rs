//! Global keyboard shortcut resolver for the TUI.
//!
//! Tasks 5 and 6 (settings-panel keybindings) and T8 (per-feature shortcuts
//! merged from `feat/tui-omp-ideas`) consume [`GlobalAction`], [`KeyCombo`],
//! and [`Keymap`]. The resolver is fully additive: user overrides in
//! `Settings::keybindings` extend the default bindings rather than replacing
//! the whole table; a user override for an action replaces only that
//! action's combo list (and only when the override list parses to at least
//! one valid combo).
//!
//! `KeyCombo` parses and serializes the canonical textual form used in
//! settings files and in the `/settings` panel:
//!
//! - modifier prefix order is `Ctrl+`, `Alt+`, `Shift+`.
//! - the char payload is uppercased when `SHIFT` is set, mirroring crossterm's
//!   `KeyEvent::normalize_case` (shifted letter keys arrive as uppercase
//!   `KeyCode::Char`); `parse` normalizes the input the same way so a
//!   shift-letter combo parses canonically regardless of case.
//! - named keys (`Enter`, `Esc`, `Tab`, `Backspace`, `PageUp`, `PageDown`,
//!   arrow keys, `Home`, `End`, `BackTab`, `Delete`) keep their literal
//!   names; the parser maps lower-case variants too.
//!
//! Note: the original `feat/tui-omp-ideas` branch shipped its own
//! `KeyAction` enum + `~/.oxicode/keybindings.yml` loader + `KeyCombo`
//! struct + `matches`/`conflicts` API. The squash merged that into the
//! established `GlobalAction` resolver: every `KeyAction` variant maps to
//! a `GlobalAction` (Submit→Submit, SubmitNow→SendNow, QueueToggle→
//! ToggleQueuePanel, Interrupt→Interrupt, Clear→FoldAll, ModelPicker→
//! OpenCommandPalette, ToggleThinking→ToggleMultiline, Help→Help, ScrollUp→
//! ScrollUp, ScrollDown→ScrollDown). The YAML loader is dropped in favor
//! of `Settings::keybindings`.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Every remappable action in the TUI. The set is the union of the
/// settings-panel's `GlobalAction` (FoldAll/SendNow/etc.) and the
/// `feat/tui-omp-ideas` branch's `KeyAction` (Submit/ScrollUp/etc.). Each
/// variant maps to a single user-visible keybinding in the TUI; char-level
/// composer input (typing, vim insert mode) is NOT represented here and is
/// left untouched.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GlobalAction {
    Interrupt,
    Submit,
    SendNow,
    ToggleMultiline,
    OpenCommandPalette,
    ToggleQueuePanel,
    FoldAll,
    ScrollUp,
    ScrollDown,
    Clear,
    Help,
    ModelPicker,
    ToggleThinking,
}

/// Default bindings. Combos mirror the keys that were hardcoded in the
/// input dispatch before this module existed (settings-panel work) plus
/// the keys introduced by `feat/tui-omp-ideas` (Submit, ScrollUp/Down,
/// Clear, Help, ModelPicker, ToggleThinking). The order matters:
/// [`GlobalAction::all`] / [`Keymap::resolve`] iterate in this order, so
/// when two actions share a combo (only reachable via user overrides) the
/// earlier action wins — deterministically across runs.
pub const DEFAULT_KEYBINDINGS: &[(GlobalAction, &str)] = &[
    (GlobalAction::Interrupt, "Ctrl+c"),
    (GlobalAction::Submit, "Enter"),
    // Shift+Enter keeps the pre-keymap multiline muscle memory alive:
    // in multiline mode plain Enter inserts a newline while Shift+Enter
    // sends (the `feat/tui-omp-ideas` branch pinned both combos).
    (GlobalAction::Submit, "Shift+Enter"),
    (GlobalAction::SendNow, "Ctrl+Enter"),
    (GlobalAction::ToggleMultiline, "Ctrl+m"),
    (GlobalAction::OpenCommandPalette, "Ctrl+p"),
    (GlobalAction::ToggleQueuePanel, "Ctrl+;"),
    (GlobalAction::FoldAll, "Ctrl+e"),
    (GlobalAction::ScrollUp, "PageUp"),
    (GlobalAction::ScrollDown, "PageDown"),
    (GlobalAction::Clear, "Ctrl+l"),
    (GlobalAction::Help, "?"),
    (GlobalAction::ModelPicker, "Ctrl+g"),
    (GlobalAction::ToggleThinking, "Ctrl+t"),
];

impl GlobalAction {
    pub fn name(self) -> &'static str {
        match self {
            GlobalAction::Interrupt => "Interrupt",
            GlobalAction::Submit => "Submit",
            GlobalAction::SendNow => "SendNow",
            GlobalAction::ToggleMultiline => "ToggleMultiline",
            GlobalAction::OpenCommandPalette => "OpenCommandPalette",
            GlobalAction::ToggleQueuePanel => "ToggleQueuePanel",
            GlobalAction::FoldAll => "FoldAll",
            GlobalAction::ScrollUp => "ScrollUp",
            GlobalAction::ScrollDown => "ScrollDown",
            GlobalAction::Clear => "Clear",
            GlobalAction::Help => "Help",
            GlobalAction::ModelPicker => "ModelPicker",
            GlobalAction::ToggleThinking => "ToggleThinking",
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
    /// `"Alt+p"`, `"PageUp"`, `"?"` into a `KeyCombo`. Returns `None` on any
    /// unrecognized segment. Shift-letter payloads normalize to uppercase;
    /// named-key spelling is case-insensitive on input.
    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = KeyModifiers::NONE;
        let mut code: Option<KeyCode> = None;
        for part in s.split('+') {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            match part {
                "Ctrl" | "Control" => mods |= KeyModifiers::CONTROL,
                "Shift" => mods |= KeyModifiers::SHIFT,
                "Alt" => mods |= KeyModifiers::ALT,
                other => {
                    if code.is_some() {
                        // Reject "Enter+Esc" / "a+b" — too ambiguous for a
                        // user-facing override. `parse_key_string`-style
                        // split used to accept these; ours is stricter.
                        return None;
                    }
                    code = Some(parse_key_name(other, mods)?);
                }
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
            KeyCode::BackTab => out.push_str("BackTab"),
            KeyCode::Backspace => out.push_str("Backspace"),
            KeyCode::Delete => out.push_str("Delete"),
            KeyCode::Home => out.push_str("Home"),
            KeyCode::End => out.push_str("End"),
            KeyCode::PageUp => out.push_str("PageUp"),
            KeyCode::PageDown => out.push_str("PageDown"),
            KeyCode::Up => out.push_str("Up"),
            KeyCode::Down => out.push_str("Down"),
            KeyCode::Left => out.push_str("Left"),
            KeyCode::Right => out.push_str("Right"),
            KeyCode::Char(c) => {
                // Crossterm's `KeyEvent::normalize_case` already maps a
                // shifted letter to `Char(uppercase)` with `SHIFT` set, so we
                // emit the char as-is (which is uppercase for shifted letters).
                out.push(c);
            }
            // Fallback for named keys we don't handle explicitly (F-keys,
            // etc.) — defer to crossterm's own formatting.
            _ => return self.code.to_string(),
        }
        out
    }
}

/// Map a token in the position after the modifier prefixes to a
/// `KeyCode`. Single printable ASCII chars map to `KeyCode::Char` (with
/// the SHIFT-uppercase normalization); named keys (`Enter`, `Esc`,
/// `PageUp`, `?` if standalone, …) map to the corresponding variant.
/// Returns `None` on anything unrecognized.
fn parse_key_name(token: &str, mods: KeyModifiers) -> Option<KeyCode> {
    // 1. Named keys (case-insensitive) — list kept in lockstep with the
    //    emit side of `to_string` plus the spelled-out aliases that
    //    `feat/tui-omp-ideas` accepted (`backtab`, `pgup`, …).
    let lower = token.to_ascii_lowercase();
    if let Some(code) = match lower.as_str() {
        "enter" | "return" | "cr" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "tab" => Some(KeyCode::Tab),
        "backtab" | "shift-tab" | "shift+tab" => Some(KeyCode::BackTab),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" | "page_up" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "page_down" | "pgdn" => Some(KeyCode::PageDown),
        "backspace" | "bs" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        _ => None,
    } {
        return Some(code);
    }
    // 2. Single printable character. SHIFT-letter normalizes to uppercase
    //    to match crossterm's `KeyEvent::normalize_case`.
    if token.chars().count() == 1 {
        let ch = token.chars().next()?;
        let ch = if mods.contains(KeyModifiers::SHIFT) {
            ch.to_ascii_uppercase()
        } else {
            ch
        };
        return Some(KeyCode::Char(ch));
    }
    None
}

#[derive(Clone)]
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

    /// True if `event` triggers `action` under the current bindings. An
    /// action matches when *any* of its registered combos matches.
    /// Convenience accessor for callers that already have a `&KeyEvent`
    /// and want a direct membership check (the original
    /// `feat/tui-omp-ideas` API surface).
    pub fn matches(&self, action: GlobalAction, event: &KeyEvent) -> bool {
        self.bindings.get(&action).is_some_and(|combos| {
            combos
                .iter()
                .any(|c| c.code == event.code && c.modifiers == event.modifiers)
        })
    }

    /// Replace the binding list for a single action (used by Task 6's
    /// map-editor when the user confirms a new combo).
    pub fn set_action(&mut self, action: GlobalAction, combos: Vec<KeyCombo>) {
        self.bindings.insert(action, combos);
    }

    /// The live combo list for `action` (defaults + user overrides,
    /// merged). The Keybindings map-editor reads this to append to or
    /// remove from the effective list.
    pub fn action_combos(&self, action: GlobalAction) -> &[KeyCombo] {
        self.bindings.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }
    fn ctrl(code: KeyCode) -> KeyEvent {
        press(code, KeyModifiers::CONTROL)
    }

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
        // PageUp/PageDown/named keys must also round-trip via the
        // canonical form used in DEFAULT_KEYBINDINGS.
        for s in [
            "Ctrl+c",
            "Ctrl+m",
            "Ctrl+Shift+E",
            "Ctrl+Enter",
            "Alt+p",
            "PageUp",
            "PageDown",
            "?",
            "Ctrl+l",
            "Ctrl+g",
            "Ctrl+t",
        ] {
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

    /// Ported from the `feat/tui-omp-ideas` `KeyAction` API: every default
    /// key from the branch's hardcoded dispatch must still resolve to the
    /// unified `GlobalAction` enum. This pins the wiring change so a future
    /// edit cannot silently rebind an action.
    #[test]
    fn defaults_match_unified_hardcoded_keys() {
        let km = Keymap::from_settings(&HashMap::new());

        // Ctrl+C → Interrupt
        assert!(km.matches(GlobalAction::Interrupt, &ctrl(KeyCode::Char('c'))));
        assert!(!km.matches(
            GlobalAction::Interrupt,
            &press(KeyCode::Char('c'), KeyModifiers::NONE),
        ));

        // Plain Enter → Submit (Enter must NOT match SendNow).
        assert!(km.matches(
            GlobalAction::Submit,
            &press(KeyCode::Enter, KeyModifiers::NONE),
        ));
        assert!(!km.matches(GlobalAction::Submit, &ctrl(KeyCode::Enter)));

        // Ctrl+Enter → SendNow (SubmitNow alias), plain Enter must not.
        assert!(km.matches(GlobalAction::SendNow, &ctrl(KeyCode::Enter)));
        assert!(!km.matches(
            GlobalAction::SendNow,
            &press(KeyCode::Enter, KeyModifiers::NONE),
        ));

        // Ctrl+; → ToggleQueuePanel (QueueToggle alias).
        assert!(km.matches(GlobalAction::ToggleQueuePanel, &ctrl(KeyCode::Char(';')),));
        assert!(!km.matches(
            GlobalAction::ToggleQueuePanel,
            &press(KeyCode::Char(';'), KeyModifiers::NONE),
        ));

        // PageUp → ScrollUp; the Up arrow must NOT match (no scroll-up
        // spill into single-line arrow keys).
        assert!(km.matches(
            GlobalAction::ScrollUp,
            &press(KeyCode::PageUp, KeyModifiers::NONE),
        ));
        assert!(!km.matches(
            GlobalAction::ScrollUp,
            &press(KeyCode::Up, KeyModifiers::NONE),
        ));

        // PageDown → ScrollDown; Down arrow must NOT match.
        assert!(km.matches(
            GlobalAction::ScrollDown,
            &press(KeyCode::PageDown, KeyModifiers::NONE),
        ));
        assert!(!km.matches(
            GlobalAction::ScrollDown,
            &press(KeyCode::Down, KeyModifiers::NONE),
        ));

        // Ctrl+L → Clear (the branch used Ctrl+E for Clear, but Ctrl+E is
        // already FoldAll on main; Ctrl+L is the canonical "clear screen"
        // chord and is the default we shipped).
        assert!(km.matches(GlobalAction::Clear, &ctrl(KeyCode::Char('l'))));
        assert!(!km.matches(
            GlobalAction::Clear,
            &press(KeyCode::Char('l'), KeyModifiers::NONE),
        ));

        // ? → Help; Ctrl+? must NOT match (modifiers are exact).
        assert!(km.matches(
            GlobalAction::Help,
            &press(KeyCode::Char('?'), KeyModifiers::NONE),
        ));
        assert!(!km.matches(GlobalAction::Help, &ctrl(KeyCode::Char('?'))));

        // Ctrl+G → ModelPicker.
        assert!(km.matches(GlobalAction::ModelPicker, &ctrl(KeyCode::Char('g'))));
        assert!(!km.matches(
            GlobalAction::ModelPicker,
            &press(KeyCode::Char('g'), KeyModifiers::NONE),
        ));

        // Ctrl+T → ToggleThinking.
        assert!(km.matches(GlobalAction::ToggleThinking, &ctrl(KeyCode::Char('t')),));
        assert!(!km.matches(
            GlobalAction::ToggleThinking,
            &press(KeyCode::Char('t'), KeyModifiers::NONE),
        ));

        // Ctrl+M → ToggleMultiline (legacy wiring from main).
        assert!(km.matches(GlobalAction::ToggleMultiline, &ctrl(KeyCode::Char('m')),));

        // Ctrl+E → FoldAll.
        assert!(km.matches(GlobalAction::FoldAll, &ctrl(KeyCode::Char('e'))));
    }

    /// Plain Enter and Ctrl+Enter must remain distinct. The original
    /// `feat/tui-omp-ideas` regression `matches_handles_ctrl_enter` covered
    /// this — re-asserted under the unified enum so a future parse change
    /// cannot collapse the two.
    #[test]
    fn matches_handles_ctrl_enter() {
        let km = Keymap::from_settings(&HashMap::new());
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        assert!(km.matches(GlobalAction::SendNow, &ctrl_enter));
        assert!(!km.matches(GlobalAction::Submit, &ctrl_enter));
        assert!(km.matches(GlobalAction::Submit, &enter));
        assert!(!km.matches(GlobalAction::SendNow, &enter));
    }

    /// `parse_key_name` rejects nonsense keys so a malformed user override
    /// is dropped (and the action stays on its default).
    #[test]
    fn parse_rejects_garbage_payload() {
        // Empty token in the middle of the combo.
        assert!(KeyCombo::parse("Ctrl+").is_none());
        // Unknown key name — `notakey` is not in the named-keys table and
        // is too long for the single-char arm.
        assert!(KeyCombo::parse("notakey").is_none());
        // Two key tokens — too ambiguous.
        assert!(KeyCombo::parse("a+b").is_none());
    }
}
