//! Cursor state with dedup — the core of cursor blink preservation.
//!
//! `reconcile()` is called every frame with the desired cursor state.
//! It emits cursor escape sequences to the terminal ONLY when something
//! actually changed:
//! - Visibility transition (Hide↔Show): emit `Hide`/`Show`
//! - Position change while visible: emit `MoveTo`
//! - Same position while visible: **emit nothing** ← this is the blink fix
//!
//! Contrast with ratatui's `apply_buffer_with_cursor` (render.rs:288-320)
//! which emits unconditionally based on whether `set_cursor_position` was
//! called in the render callback, regardless of whether anything moved.

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Position;

#[derive(Debug, Clone, Default)]
pub struct CursorState {
    last_pos: Option<Position>,
    visible: bool,
}

impl CursorState {
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

    /// `TestBackend` records cursor commands? Actually `TestBackend` doesn't record
    /// byte-level output. We test via observable state transitions on `CursorState`
    /// and a custom recording backend for byte-level tests.

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
    const P1_AGAIN: Position = Position { x: 5, y: 10 };
    const P2: Position = Position { x: 7, y: 12 };

    #[test]
    fn first_show_emits_show_and_moveto() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        assert_eq!(
            term.backend().commands,
            vec!["show".to_string(), "moveto(5,10)".to_string()]
        );
    }

    #[test]
    fn same_position_second_frame_emits_zero_bytes() {
        // ★ THE blink-preservation test
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(Some(P1_AGAIN), &mut term).unwrap();
        assert!(
            term.backend().commands.is_empty(),
            "second frame at same position must emit zero cursor bytes"
        );
    }

    #[test]
    fn position_change_emits_only_moveto() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(Some(P2), &mut term).unwrap();
        assert_eq!(term.backend().commands, vec!["moveto(7,12)".to_string()]);
    }

    #[test]
    fn hide_emits_hide() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(None, &mut term).unwrap();
        assert_eq!(term.backend().commands, vec!["hide".to_string()]);
    }

    #[test]
    fn hide_then_hide_again_emits_zero_bytes() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(None, &mut term).unwrap(); // first hide
        term.backend_mut().commands.clear();

        cursor.reconcile(None, &mut term).unwrap(); // second hide — should be no-op
        assert!(term.backend().commands.is_empty());
    }

    #[test]
    fn show_after_hide_emits_show_and_moveto() {
        let mut term = make_terminal();
        let mut cursor = CursorState::new();
        cursor.reconcile(Some(P1), &mut term).unwrap();
        cursor.reconcile(None, &mut term).unwrap();
        term.backend_mut().commands.clear();

        cursor.reconcile(Some(P2), &mut term).unwrap();
        assert_eq!(
            term.backend().commands,
            vec!["show".to_string(), "moveto(7,12)".to_string()]
        );
    }
}
