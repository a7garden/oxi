// Modal dispatch — open/close overlay state.
use crate::state::ModalKind;

/// Open a modal: sets `state.modal = Some(kind)`.
pub fn open_modal(state: &mut crate::state::PagerState, kind: ModalKind) {
    state.modal = Some(kind);
}

/// Close the current modal: sets `state.modal = None`.
pub fn close_modal(state: &mut crate::state::PagerState) {
    state.modal = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PagerState;

    #[test]
    fn open_sets_modal() {
        let mut state = PagerState::default();
        assert_eq!(state.modal, None);
        open_modal(&mut state, ModalKind::Ask);
        assert_eq!(state.modal, Some(ModalKind::Ask));
    }

    #[test]
    fn close_clears_modal() {
        let mut state = PagerState::default();
        open_modal(&mut state, ModalKind::Ask);
        close_modal(&mut state);
        assert_eq!(state.modal, None);
    }
}
