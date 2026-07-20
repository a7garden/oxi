// KeyRouter — bridges crossterm key events to the pager.
//
// `state.modal.is_some()` implies the focused widget is the modal; in
// that case the router returns `ModalLocal` for known modal keys
// (Enter/Esc/Arrow) and `PassThrough` for everything else.
//
// PR-3 ships a minimal router. PR-6 fills in modal-specific dispatch
// (each `ModalKind` decides what its keys mean).

use crossterm::event::KeyEvent;
use oxi_tui::keybindings::{Action, KeyId, KeybindingsManager};

use crate::state::ModalKind;

/// Where keyboard focus currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusTarget {
    #[default]
    Prompt,
    Chat,
    Modal(ModalKind),
    Status,
}

/// Outcome of resolving a raw key event through the router.
#[derive(Debug, Clone)]
pub enum ResolvedKey {
    Bind(Action),
    ModalLocal(ModalInput),
    PassThrough(KeyEvent),
    Ignored,
}

/// Coarse modal input events.
#[derive(Debug, Clone)]
pub enum ModalInput {
    Submit(String),
    Cancel,
    MoveUp,
    MoveDown,
}

/// Resolves key events to `ResolvedKey`.
pub struct KeyRouter {
    inner: KeybindingsManager,
    pub focused: FocusTarget,
}

impl KeyRouter {
    pub fn new(inner: KeybindingsManager) -> Self {
        Self {
            inner,
            focused: FocusTarget::default(),
        }
    }

    pub fn with_focus(mut self, focused: FocusTarget) -> Self {
        self.focused = focused;
        self
    }

    pub fn resolve(&self, ev: KeyEvent) -> ResolvedKey {
        if matches!(self.focused, FocusTarget::Modal(_)) {
            self.resolve_modal(ev)
        } else {
            let key_id = KeyId::from(ev);
            match self.inner.match_action(&key_id) {
                Some(action) => ResolvedKey::Bind(action),
                None => ResolvedKey::Ignored,
            }
        }
    }

    fn resolve_modal(&self, ev: KeyEvent) -> ResolvedKey {
        use crossterm::event::KeyCode;
        match ev.code {
            KeyCode::Enter => ResolvedKey::ModalLocal(ModalInput::Submit(String::new())),
            KeyCode::Esc => ResolvedKey::ModalLocal(ModalInput::Cancel),
            KeyCode::Up => ResolvedKey::ModalLocal(ModalInput::MoveUp),
            KeyCode::Down => ResolvedKey::ModalLocal(ModalInput::MoveDown),
            _ => ResolvedKey::PassThrough(ev),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn resolve_returns_ignored_for_unbound_key() {
        let router = KeyRouter::new(KeybindingsManager::default());
        let ev = key(KeyCode::F(24), KeyModifiers::NONE);
        assert!(matches!(router.resolve(ev), ResolvedKey::Ignored));
    }

    #[test]
    fn resolve_modal_local_takes_precedence() {
        let router = KeyRouter::new(KeybindingsManager::default())
            .with_focus(FocusTarget::Modal(ModalKind::Ask));
        let ev = key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            router.resolve(ev),
            ResolvedKey::ModalLocal(ModalInput::Submit(_))
        ));
    }
}
