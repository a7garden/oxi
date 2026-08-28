//! Keymap for the git TUI overlay.
//!
//! Maps a [`crossterm::event::KeyEvent`] to a [`GitKeyAction`]. Pure — no
//! terminal I/O, no rendering. Consumed by the overlay's event loop.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// High-level action dispatched by the git TUI keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitKeyAction {
    /// Move cursor down by one visible line / entry.
    Down,
    /// Move cursor up by one visible line / entry.
    Up,
    /// Collapse / step left in the file tree.
    Left,
    /// Expand / step right in the file tree.
    Right,
    /// Jump to the top of the current list.
    GotoTop,
    /// Jump to the bottom of the current list.
    GotoBottom,
    /// Jump to the next hunk.
    HunkNext,
    /// Jump to the previous hunk.
    HunkPrev,
    /// Jump to the next file.
    FileNext,
    /// Jump to the previous file.
    FilePrev,
    /// Switch view mode: `1` = Split, `2` = Inline, `3` = Hunks, `4` = Files.
    /// The byte is the digit (`'1'..='4'`).
    ViewMode(u8),
    /// Toggle the file-tree sidebar.
    ToggleSidebar,
    /// Cycle whitespace mode: Off → IgnoreWhitespace → IgnoreFormatting.
    CycleWhitespace,
    /// Toggle soft-wrap of long lines.
    ToggleWrap,
    /// Stage the current hunk / file (`git add`).
    Stage,
    /// Unstage the current hunk / file (`git reset`).
    Unstage,
    /// Open the commit message composer.
    Commit,
    /// Re-run `git status` + `git diff` and refresh the view.
    Refresh,
    /// Close the git overlay.
    Close,
}

/// Map a key event to a [`GitKeyAction`]. Returns `None` for keys the
/// overlay does not handle (the caller decides whether to forward or drop).
pub fn match_git_key(key: &KeyEvent) -> Option<GitKeyAction> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // Number keys for view modes — handled before the catch-all Char arm.
    if let KeyCode::Char(c) = key.code
        && !alt
        && !key.modifiers.contains(KeyModifiers::CONTROL)
    {
        match c {
            '1' => return Some(GitKeyAction::ViewMode(1)),
            '2' => return Some(GitKeyAction::ViewMode(2)),
            '3' => return Some(GitKeyAction::ViewMode(3)),
            '4' => return Some(GitKeyAction::ViewMode(4)),
            _ => {}
        }
    }

    // Alt-modified cursor keys for hunk navigation.
    if alt {
        return match key.code {
            KeyCode::Down => Some(GitKeyAction::HunkNext),
            KeyCode::Up => Some(GitKeyAction::HunkPrev),
            _ => None,
        };
    }

    // Guard against Ctrl-modified keys hijacking the unmapped-char arms
    // below (Ctrl+C would otherwise trigger Commit, Ctrl+S → Stage, etc.).
    // Alt+down/up already short-circuits above for hunk nav.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }

    match key.code {
        KeyCode::Char('j') => Some(GitKeyAction::Down),
        KeyCode::Char('k') => Some(GitKeyAction::Up),
        KeyCode::Char('h') => Some(GitKeyAction::Left),
        KeyCode::Char('l') => Some(GitKeyAction::Right),
        KeyCode::Char('g') => Some(GitKeyAction::GotoTop),
        KeyCode::Char('G') => Some(GitKeyAction::GotoBottom),
        KeyCode::Char(']') => Some(GitKeyAction::FileNext),
        KeyCode::Char('[') => Some(GitKeyAction::FilePrev),
        KeyCode::Char('v') => Some(GitKeyAction::ToggleSidebar),
        KeyCode::Char('b') => Some(GitKeyAction::CycleWhitespace),
        KeyCode::Char('w') => Some(GitKeyAction::ToggleWrap),
        KeyCode::Char('s') => Some(GitKeyAction::Stage),
        KeyCode::Char('u') => Some(GitKeyAction::Unstage),
        KeyCode::Char('c') => Some(GitKeyAction::Commit),
        KeyCode::Char('r') => Some(GitKeyAction::Refresh),
        KeyCode::Char('q') => Some(GitKeyAction::Close),
        KeyCode::Esc => Some(GitKeyAction::Close),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn vim_motions_map() {
        assert_eq!(
            match_git_key(&k(KeyCode::Char('j'))),
            Some(GitKeyAction::Down)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('k'))),
            Some(GitKeyAction::Up)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('h'))),
            Some(GitKeyAction::Left)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('l'))),
            Some(GitKeyAction::Right)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('g'))),
            Some(GitKeyAction::GotoTop)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('G'))),
            Some(GitKeyAction::GotoBottom)
        );

        // Alt+down / Alt+Up → hunk nav.
        assert_eq!(
            match_git_key(&alt(KeyCode::Down)),
            Some(GitKeyAction::HunkNext)
        );
        assert_eq!(
            match_git_key(&alt(KeyCode::Up)),
            Some(GitKeyAction::HunkPrev)
        );

        // ] / [ → file nav.
        assert_eq!(
            match_git_key(&k(KeyCode::Char(']'))),
            Some(GitKeyAction::FileNext)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('['))),
            Some(GitKeyAction::FilePrev)
        );
    }

    #[test]
    fn number_keys_select_view() {
        assert_eq!(
            match_git_key(&k(KeyCode::Char('1'))),
            Some(GitKeyAction::ViewMode(1))
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('2'))),
            Some(GitKeyAction::ViewMode(2))
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('3'))),
            Some(GitKeyAction::ViewMode(3))
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('4'))),
            Some(GitKeyAction::ViewMode(4))
        );
        // Out-of-range digits return None.
        assert_eq!(match_git_key(&k(KeyCode::Char('5'))), None);
        assert_eq!(match_git_key(&k(KeyCode::Char('0'))), None);
    }

    #[test]
    fn whitespace_and_wrap_toggles() {
        assert_eq!(
            match_git_key(&k(KeyCode::Char('b'))),
            Some(GitKeyAction::CycleWhitespace)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('w'))),
            Some(GitKeyAction::ToggleWrap)
        );
        // Stage / Unstage / Commit / Refresh.
        assert_eq!(
            match_git_key(&k(KeyCode::Char('s'))),
            Some(GitKeyAction::Stage)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('u'))),
            Some(GitKeyAction::Unstage)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('c'))),
            Some(GitKeyAction::Commit)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('r'))),
            Some(GitKeyAction::Refresh)
        );
        // Sidebar toggle.
        assert_eq!(
            match_git_key(&k(KeyCode::Char('v'))),
            Some(GitKeyAction::ToggleSidebar)
        );
    }

    #[test]
    fn close_keys() {
        assert_eq!(
            match_git_key(&k(KeyCode::Char('q'))),
            Some(GitKeyAction::Close)
        );
        assert_eq!(match_git_key(&k(KeyCode::Esc)), Some(GitKeyAction::Close));
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_c_does_not_commit() {
        // Without the guard, Ctrl+C (a control-modifier key whose letter
        // happens to be 'c') would fall into the Char('c') arm and fire
        // Commit — hijacking the host cancel convention.
        assert_eq!(match_git_key(&ctrl(KeyCode::Char('c'))), None);
        // Plain 'c' still commits.
        assert_eq!(
            match_git_key(&k(KeyCode::Char('c'))),
            Some(GitKeyAction::Commit)
        );
    }

    #[test]
    fn ctrl_s_does_not_stage() {
        assert_eq!(match_git_key(&ctrl(KeyCode::Char('s'))), None);
        assert_eq!(
            match_git_key(&k(KeyCode::Char('s'))),
            Some(GitKeyAction::Stage)
        );
    }

    #[test]
    fn ctrl_r_and_ctrl_q_do_not_hijack() {
        // Ctrl+R is the host's redraw; Ctrl+Q is the host's quit. Both
        // must NOT be swallowed by the git overlay.
        assert_eq!(match_git_key(&ctrl(KeyCode::Char('r'))), None);
        assert_eq!(match_git_key(&ctrl(KeyCode::Char('q'))), None);
        assert_eq!(
            match_git_key(&k(KeyCode::Char('r'))),
            Some(GitKeyAction::Refresh)
        );
        assert_eq!(
            match_git_key(&k(KeyCode::Char('q'))),
            Some(GitKeyAction::Close)
        );
    }
}
