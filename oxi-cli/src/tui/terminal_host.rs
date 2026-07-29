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
pub(crate) struct TerminalHost {
    tape: TapeEngine<io::Stdout>,
    tty_ok: bool,
    overlay_active: bool,
    restored: bool,
}

impl TerminalHost {
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
        let diff = DiffBackend::new(backend);
        let mut terminal = Terminal::new(diff)?;
        terminal.draw(draw)?;
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

impl Drop for TerminalHost {
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
    #[test]
    fn ordinary_mode_sequence_has_no_alt_screen() {
        let ordinary = b"\x1b[?1000h\x1b[?1006h";
        assert!(!ordinary.windows(8).any(|w| w == b"\x1b[?1049"));
    }
}
