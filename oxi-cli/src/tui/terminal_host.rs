//! Terminal lifecycle for main-screen tape rendering and transient overlays.

use std::io::{self, Write};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use oxi_tui::tape::{FrameOutcome, LiveRegion, TapeEngine};
use ratatui::{Terminal, backend::CrosstermBackend};

use oxi_tui::render::DiffBackend;

/// Main-screen terminal owner. Alternate screen is entered only by `draw_overlay`.
pub(crate) struct TerminalHost<W: Write = io::Stdout> {
    tape: TapeEngine<W>,
    tty_ok: bool,
    overlay_active: bool,
    restored: bool,
}

impl TerminalHost<io::Stdout> {
    pub(crate) fn enter() -> Result<Self> {
        install_panic_hook();
        let tty_ok = enable_raw_mode().is_ok();
        let mut stdout = io::stdout();
        if tty_ok {
            let flags = keyboard_flags();
            let _ = execute!(
                stdout,
                Hide,
                EnableBracketedPaste,
                PushKeyboardEnhancementFlags(flags)
            );
            let _ = stdout.write_all(b"\x1b[?1000h\x1b[?1006h");
            let _ = stdout.flush();
        }
        let mut tape = TapeEngine::new(stdout);
        let caps = oxi_tui::render::terminal::TerminalCapabilities::detect();
        tape.set_synchronized_output(caps.synchronized_output);
        Ok(Self {
            tape,
            tty_ok,
            overlay_active: false,
            restored: false,
        })
    }
}

impl<W: Write> TerminalHost<W> {
    #[cfg(test)]
    fn with_writer(writer: W) -> Self {
        Self {
            tape: TapeEngine::new(writer),
            tty_ok: false,
            overlay_active: false,
            restored: false,
        }
    }

    #[cfg(test)]
    fn output(&self) -> &W {
        self.tape.writer()
    }

    pub(crate) fn paint_tape(
        &mut self,
        frame: &[String],
        live: LiveRegion,
        width: u16,
        height: u16,
    ) -> Result<FrameOutcome> {
        if self.overlay_active {
            self.leave_overlay()?;
        }
        Ok(self.tape.paint(frame, live, width, height)?)
    }

    pub(crate) fn clear_scrollback(&mut self) {
        self.tape.clear_scrollback();
    }

    pub(crate) fn draw_overlay<F>(&mut self, draw: F) -> Result<()>
    where
        F: FnOnce(&mut ratatui::Frame<'_>),
    {
        if !self.overlay_active {
            execute!(self.tape.writer_mut(), EnterAlternateScreen)?;
            self.overlay_active = true;
        }
        self.tape.flush()?;
        let backend = CrosstermBackend::new(self.tape.writer_mut());
        let mut terminal = Terminal::with_options(
            DiffBackend::new(backend),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fullscreen,
            },
        )?;
        terminal.draw(draw)?;
        terminal.backend_mut().flush()?;
        Ok(())
    }

    fn leave_overlay(&mut self) -> Result<()> {
        if self.overlay_active {
            execute!(self.tape.writer_mut(), LeaveAlternateScreen)?;
            self.overlay_active = false;
        }
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        let _ = self.leave_overlay();
        if self.tty_ok {
            let _ = self.tape.writer_mut().write_all(b"\x1b[?1000l\x1b[?1006l");
            let _ = execute!(
                self.tape.writer_mut(),
                PopKeyboardEnhancementFlags,
                DisableBracketedPaste,
                Show
            );
            let _ = self.tape.writer_mut().write_all(b"\r\n");
            let _ = self.tape.flush();
            disable_raw_mode()?;
            self.tty_ok = false;
        }
        self.restored = true;
        Ok(())
    }
}

impl<W: Write> Drop for TerminalHost<W> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn keyboard_flags() -> KeyboardEnhancementFlags {
    if std::env::var("OXI_KITTY_KEYBOARD").as_deref() == Ok("1") {
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
    } else {
        KeyboardEnhancementFlags::REPORT_EVENT_TYPES
    }
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = io::stdout();
        let _ = stdout.write_all(b"\x1b[?1000l\x1b[?1006l\x1b[?1049l\x1b[?25h");
        let _ = stdout.flush();
        let _ = disable_raw_mode();
        original(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_mode_sequence_has_no_alt_screen() {
        let ordinary = b"\x1b[?1000h\x1b[?1006h";
        assert!(!ordinary.windows(8).any(|w| w == b"\x1b[?1049"));
    }

    #[test]
    fn overlay_enters_alt_once_and_leaves_on_next_tape() {
        // This test needs terminal dimensions (ioctl TIOCGWINSZ) from
        // Viewport::Fullscreen. Headless CI runners don't have one.
        if crossterm::terminal::size().is_err() {
            eprintln!("skip: no terminal size (headless CI)");
            return;
        }
        let mut host = TerminalHost::with_writer(Vec::new());
        host.draw_overlay(|frame| {
            frame.render_widget(ratatui::widgets::Clear, frame.area());
        })
        .unwrap();
        host.draw_overlay(|frame| {
            frame.render_widget(ratatui::widgets::Clear, frame.area());
        })
        .unwrap();
        let output = String::from_utf8_lossy(host.output());
        assert_eq!(output.matches("\x1b[?1049h").count(), 1);
        assert_eq!(output.matches("\x1b[?1049l").count(), 0);

        host.paint_tape(&["row".to_string()], LiveRegion::None, 80, 24)
            .unwrap();
        let output = String::from_utf8_lossy(host.output());
        assert_eq!(output.matches("\x1b[?1049l").count(), 1);
        assert!(output.contains("row"));
    }
}
