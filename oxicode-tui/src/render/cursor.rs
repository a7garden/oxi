//! Cursor state with dedup — the core of cursor blink preservation.
//!
//! `reconcile()` is called every frame with the desired cursor state.
//! It emits cursor escape sequences to the terminal ONLY when something
//! actually changed:
//! - Visibility transition (Hide↔Show): emit `Hide`/`Show`
//! - Position change while visible: emit `MoveTo`
//! - Same position while visible: **emit nothing** ← this is the blink fix
//!
//! Contrast with ratatui's default cursor handling which emits
//! unconditionally based on whether `set_cursor_position` was called in
//! the render callback, regardless of whether anything moved.

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Position;

/// Cursor dedup state — tracks last cursor position/visibility to
/// avoid redundant escape sequences (blink preservation).
#[derive(Debug, Clone, Default)]
pub struct CursorState {
    last_pos: Option<Position>,
    visible: bool,
}

impl CursorState {
    /// Create a new cursor state with no known position (hidden).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply this frame's cursor request to the terminal.
    /// Emits zero bytes if nothing changed (same visibility AND same position).
    pub fn reconcile<B: Backend>(
        &mut self,
        want: Option<Position>,
        term: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let new_visible = want.is_some();

        // Visibility transition (rare): emit Show or Hide
        if new_visible != self.visible {
            if new_visible {
                term.show_cursor()?;
                self.visible = true;
            } else {
                term.hide_cursor()?;
                self.visible = false;
                self.last_pos = None;
            }
        }

        // Position change while visible: emit MoveTo. Same position → 0 bytes.
        if let (Some(new), Some(prev)) = (want, self.last_pos) {
            if new != prev {
                term.set_cursor_position(new)?;
                self.last_pos = Some(new);
            }
            // ★ new == prev: 0 bytes — blink timer preserved (core optimization)
        } else if let Some(new) = want {
            // Visibility just transitioned to Show — set initial position
            term.set_cursor_position(new)?;
            self.last_pos = Some(new);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;

    /// `TestBackend` doesn't record byte-level cursor output, so we use a
    /// custom recording backend that captures show/hide/moveto commands.

    #[derive(Default)]
    struct RecordingBackend {
        commands: Vec<String>,
        size: ratatui::layout::Size,
    }

    impl Backend for RecordingBackend {
        type Error = std::convert::Infallible;

        fn draw<'a, I>(&mut self, _content: I) -> Result<(), Self::Error>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Ok(())
        }

        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            self.commands.push("hide".into());
            Ok(())
        }

        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            self.commands.push("show".into());
            Ok(())
        }

        fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
            Ok(Position { x: 0, y: 0 })
        }

        fn set_cursor_position<P: Into<ratatui::layout::Position>>(
            &mut self,
            position: P,
        ) -> Result<(), Self::Error> {
            let p: Position = position.into();
            self.commands.push(format!("moveto({},{})", p.x, p.y));
            Ok(())
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn clear_region(
            &mut self,
            _clear_type: ratatui::backend::ClearType,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
            Ok(ratatui::backend::WindowSize {
                columns_rows: ratatui::layout::Size {
                    width: self.size.width,
                    height: self.size.height,
                },
                pixels: ratatui::layout::Size {
                    width: 0,
                    height: 0,
                },
            })
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn make_terminal() -> Terminal<RecordingBackend> {
        Terminal::new(RecordingBackend {
            commands: Vec::new(),
            size: ratatui::layout::Size {
                width: 80,
                height: 24,
            },
        })
        .unwrap()
    }

    const P1: Position = Position { x: 5, y: 10 };
    const P2: Position = Position { x: 7, y: 12 };

    #[test]
    fn first_show_emits_show_and_moveto() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        assert!(term.backend().commands.iter().any(|c| c == "show"));
        assert!(term.backend().commands.iter().any(|c| c == "moveto(5,10)"));
    }

    #[test]
    fn same_position_second_frame_emits_zero_bytes() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        let before = term.backend().commands.len();
        cs.reconcile(Some(P1), &mut term).unwrap();
        assert_eq!(
            term.backend().commands.len(),
            before,
            "same position should emit zero commands"
        );
    }

    #[test]
    fn position_change_emits_only_moveto() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        let before = term.backend().commands.len();
        cs.reconcile(Some(P2), &mut term).unwrap();
        let new_cmds = &term.backend().commands[before..];
        assert_eq!(new_cmds.len(), 1);
        assert!(new_cmds[0].starts_with("moveto"));
    }

    #[test]
    fn hide_emits_hide() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        cs.reconcile(None, &mut term).unwrap();
        assert!(term.backend().commands.iter().any(|c| c == "hide"));
    }

    #[test]
    fn hide_then_hide_again_emits_zero_bytes() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        cs.reconcile(None, &mut term).unwrap();
        let before = term.backend().commands.len();
        cs.reconcile(None, &mut term).unwrap();
        assert_eq!(term.backend().commands.len(), before);
    }

    #[test]
    fn show_after_hide_emits_show_and_moveto() {
        let mut cs = CursorState::new();
        let mut term = make_terminal();
        cs.reconcile(Some(P1), &mut term).unwrap();
        cs.reconcile(None, &mut term).unwrap();
        cs.reconcile(Some(P2), &mut term).unwrap();
        let cmds = &term.backend().commands;
        // Last two commands should be show + moveto
        assert!(cmds.iter().any(|c| c == "show"));
        assert!(cmds.iter().any(|c| c == "moveto(7,12)"));
    }
}
