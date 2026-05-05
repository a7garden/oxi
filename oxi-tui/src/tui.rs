//! TUI - Main Terminal UI framework.
//!
//! This module provides the core TUI struct and event loop for building
//! terminal-based user interfaces with differential rendering.

use crate::{
    cell::Cell,
    component::Component,
    event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, ResizeEvent,
    },
    layout::{split, Constraint, Direction},
    overlay::{OverlayBox, OverlayContent, OverlayHandle, OverlayOptions},
    renderer::Renderer,
    surface::Surface,
    terminal::{CrosstermTerminal, Size, Terminal},
};
use anyhow::Result;
use std::io::{self, stdout, Write};

/// Rendering strategy based on change type.
enum RenderStrategy {
    /// Full redraw needed (first render, width change, or large changes).
    Full,
    /// Incremental update (only dirty lines).
    Incremental,
}

/// Main TUI struct - the entry point for building terminal UIs.
pub struct TUI {
    /// The terminal backend.
    terminal: Box<dyn Terminal>,
    /// Child components in z-order (0 = bottom).
    children: Vec<Box<dyn Component>>,
    /// Currently focused component index.
    focus_index: usize,
    /// Overlay stack.
    overlay_stack: Vec<OverlayHandleWrapper>,
    /// Whether a render is needed.
    dirty: bool,
    /// Previous surface for diff comparison.
    prev_surface: Option<Surface>,
    /// Renderer instance.
    renderer: Renderer,
    /// Surface size tracking.
    last_width: u16,
    last_height: u16,
    /// Running state.
    running: bool,
    /// Event handle callback.
    event_handler: Option<Box<dyn FnMut(crate::Event) + Send>>,
    /// Layout for arranging children.
    layout: Option<(Direction, Vec<Constraint>)>,
}

struct OverlayHandleWrapper {
    overlay: Box<dyn OverlayHandle>,
}

impl TUI {
    /// Create a new TUI instance with a default terminal.
    pub fn new(mut terminal: impl Terminal + 'static) -> Self {
        let size = terminal.size().unwrap_or(Size {
            width: 80,
            height: 24,
        });
        Self {
            terminal: Box::new(terminal),
            children: Vec::new(),
            focus_index: 0,
            overlay_stack: Vec::new(),
            dirty: true,
            prev_surface: None,
            renderer: Renderer::new(),
            last_width: size.width,
            last_height: size.height,
            running: false,
            event_handler: None,
            layout: None,
        }
    }

    /// Create with crossterm backend (convenience constructor).
    pub fn with_crossterm() -> Result<Self> {
        let terminal = CrosstermTerminal::new()?;
        Ok(Self::new(terminal))
    }

    /// Add a child component.
    pub fn add_child(&mut self, component: impl Component + 'static) -> usize {
        let index = self.children.len();
        self.children.push(Box::new(component));
        self.request_render();
        index
    }

    /// Remove a child component by index.
    pub fn remove_child(&mut self, index: usize) {
        if index < self.children.len() {
            self.children.remove(index);
            if self.focus_index >= self.children.len() && !self.children.is_empty() {
                self.focus_index = self.children.len() - 1;
            }
            self.request_render();
        }
    }

    /// Set focus to a component by index.
    pub fn set_focus(&mut self, index: usize) {
        if index < self.children.len() {
            // Unfocus previous
            if self.focus_index < self.children.len() {
                if let Some(child) = self.children.get_mut(self.focus_index) {
                    child.unfocus();
                }
            }
            self.focus_index = index;
            // Focus new
            if let Some(child) = self.children.get_mut(index) {
                child.focus();
            }
            self.request_render();
        }
    }

    /// Get current focus index.
    pub fn focus_index(&self) -> usize {
        self.focus_index
    }

    /// Get number of children.
    pub fn children_count(&self) -> usize {
        self.children.len()
    }

    /// Add an overlay.
    pub fn add_overlay<T: OverlayContent + 'static>(
        &mut self,
        content: T,
        options: OverlayOptions,
    ) -> usize {
        let id = self.overlay_stack.len();
        let mut boxed = OverlayBox::new(content, options);
        boxed.set_id(id);

        self.overlay_stack.push(OverlayHandleWrapper {
            overlay: Box::new(boxed),
        });
        self.request_render();
        id
    }

    /// Remove an overlay by index.
    pub fn remove_overlay(&mut self, id: usize) {
        if id < self.overlay_stack.len() {
            self.overlay_stack.remove(id);
            self.request_render();
        }
    }

    /// Remove all overlays.
    pub fn clear_overlays(&mut self) {
        self.overlay_stack.clear();
        self.request_render();
    }

    /// Mark the TUI as needing a render.
    pub fn request_render(&mut self) {
        self.dirty = true;
    }

    /// Set an event handler callback.
    pub fn on_event(&mut self, handler: impl FnMut(crate::Event) + Send + 'static) {
        self.event_handler = Some(Box::new(handler));
    }

    /// Start the TUI event loop.
    ///
    /// This enters alternate screen mode and runs until `stop()` is called.
    pub fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }
        self.running = true;

        // Enter alternate screen
        crossterm::execute!(stdout(), crossterm::terminal::EnterAlternateScreen)?;
        crossterm::execute!(stdout(), crossterm::cursor::Hide)?;

        // Enable mouse reporting
        crossterm::execute!(stdout(), crossterm::event::EnableMouseCapture)?;

        // Initial render
        self.render()?;

        // Main event loop
        while self.running {
            // Poll for events with a timeout
            if let Some(event) = self.poll_event(std::time::Duration::from_millis(16)) {
                self.handle_event(event);
            }

            // Render if dirty
            if self.dirty {
                self.render()?;
            }
        }

        // Cleanup
        self.cleanup()?;

        Ok(())
    }

    /// Stop the TUI event loop.
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// Check if TUI is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Poll for a single event (non-blocking with timeout).
    fn poll_event(&self, timeout: std::time::Duration) -> Option<crate::Event> {
        if crossterm::event::poll(timeout).ok()? {
            crossterm::event::read().ok().map(Self::convert_event)
        } else {
            None
        }
    }

    /// Convert crossterm events to our Event type.
    fn convert_event(event: crossterm::event::Event) -> crate::Event {
        match event {
            crossterm::event::Event::Key(key) => {
                let code = match key.code {
                    crossterm::event::KeyCode::Enter => KeyCode::Enter,
                    crossterm::event::KeyCode::Esc => KeyCode::Escape,
                    crossterm::event::KeyCode::Tab => KeyCode::Tab,
                    crossterm::event::KeyCode::Backspace => KeyCode::Backspace,
                    crossterm::event::KeyCode::Delete => KeyCode::Delete,
                    crossterm::event::KeyCode::Up => KeyCode::Up,
                    crossterm::event::KeyCode::Down => KeyCode::Down,
                    crossterm::event::KeyCode::Left => KeyCode::Left,
                    crossterm::event::KeyCode::Right => KeyCode::Right,
                    crossterm::event::KeyCode::Home => KeyCode::Home,
                    crossterm::event::KeyCode::End => KeyCode::End,
                    crossterm::event::KeyCode::PageUp => KeyCode::PageUp,
                    crossterm::event::KeyCode::PageDown => KeyCode::PageDown,
                    crossterm::event::KeyCode::Insert => KeyCode::Insert,
                    crossterm::event::KeyCode::F(n) => KeyCode::F(n),
                    crossterm::event::KeyCode::Char(c) => KeyCode::Char(c),
                    _ => KeyCode::Enter, // Handle unknown keys
                };

                let modifiers = KeyModifiers {
                    shift: key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::SHIFT),
                    ctrl: key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL),
                    alt: key.modifiers.contains(crossterm::event::KeyModifiers::ALT),
                    meta: key.modifiers.contains(crossterm::event::KeyModifiers::META),
                };

                crate::Event::Key(KeyEvent::with_modifiers(code, modifiers))
            }
            crossterm::event::Event::Mouse(mouse) => {
                let kind = match mouse.kind {
                    crossterm::event::MouseEventKind::Down(_btn) => MouseEventKind::Press,
                    crossterm::event::MouseEventKind::Up(_btn) => MouseEventKind::Release,
                    crossterm::event::MouseEventKind::Drag(_btn) => MouseEventKind::Drag,
                    crossterm::event::MouseEventKind::Moved => MouseEventKind::Moved,
                    crossterm::event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
                    crossterm::event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
                    crossterm::event::MouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
                    crossterm::event::MouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
                };

                let button = match mouse.kind {
                    crossterm::event::MouseEventKind::Down(btn)
                    | crossterm::event::MouseEventKind::Up(btn)
                    | crossterm::event::MouseEventKind::Drag(btn) => match btn {
                        crossterm::event::MouseButton::Left => MouseButton::Left,
                        crossterm::event::MouseButton::Right => MouseButton::Right,
                        crossterm::event::MouseButton::Middle => MouseButton::Middle,
                    },
                    _ => MouseButton::None,
                };

                crate::Event::Mouse(MouseEvent {
                    kind,
                    button,
                    row: mouse.row,
                    col: mouse.column,
                })
            }
            crossterm::event::Event::Resize(cols, rows) => crate::Event::Resize(ResizeEvent {
                width: cols,
                height: rows,
            }),
            crossterm::event::Event::FocusGained => crate::Event::FocusGained,
            crossterm::event::Event::FocusLost => crate::Event::FocusLost,
            _ => crate::Event::None,
        }
    }

    /// Handle an input event.
    fn handle_event(&mut self, event: crate::Event) {
        // Handle overlay events first (for modals)
        if let Some(top) = self.overlay_stack.last_mut() {
            if top.overlay.is_hidden() {
                return;
            }
            // Try overlay first
            if top.overlay.handle_event(&event) {
                self.request_render();
                return;
            }
        }

        // Check Escape for closing overlays
        if let crate::Event::Key(ref key) = event {
            if key.code == KeyCode::Escape && !self.overlay_stack.is_empty() {
                self.overlay_stack.pop();
                self.request_render();
                return;
            }
        }

        // Pass to focused component
        if self.focus_index < self.children.len() {
            if self.children[self.focus_index].handle_event(&event) {
                self.request_render();
                return;
            }
        }

        // Global key handling
        if let crate::Event::Key(key) = &event {
            match key.code {
                // Tab cycles focus
                KeyCode::Tab => {
                    if self.children.len() > 1 {
                        let next = if key.modifiers.shift {
                            self.focus_index.saturating_sub(1)
                        } else {
                            (self.focus_index + 1) % self.children.len()
                        };
                        self.set_focus(next);
                    }
                }
                // Ctrl+C exits
                KeyCode::Char('c') if key.modifiers.ctrl => {
                    self.stop();
                }
                _ => {}
            }
        }

        // Call event handler if set
        if let Some(ref mut handler) = self.event_handler {
            handler(event);
        }
    }

    /// Render the current state.
    fn render(&mut self) -> Result<()> {
        let size = self.terminal.size()?;

        // Determine render strategy
        let strategy = self.determine_render_strategy(size);

        // Create surface for this frame
        let mut surface = Surface::new(size.width, size.height);

        // Clear to spaces
        let empty_cell = Cell::new(' ');
        surface.fill(empty_cell);

        // Render children
        let area = surface.area();
        for child in &mut self.children {
            child.render(&mut surface, area);
        }

        // Render overlays (on top)
        for overlay in &mut self.overlay_stack {
            if !overlay.overlay.is_hidden() {
                overlay.overlay.render(&mut surface, area);
            }
        }

        // Execute render based on strategy
        match strategy {
            RenderStrategy::Full => {
                self.renderer.begin_sync();
                self.renderer.clear_screen();
                for row in 0..size.height {
                    for col in 0..size.width {
                        if let Some(cell) = surface.get(row, col) {
                            self.renderer.render_cell(row, col, cell);
                        }
                    }
                }
                self.renderer.end_sync()?;
            }
            RenderStrategy::Incremental => {
                self.renderer.begin_sync();
                if let (Some(first), Some(last)) = (surface.first_dirty(), surface.last_dirty()) {
                    self.renderer.render_changed_lines(
                        &surface,
                        first,
                        last.min(size.height - 1),
                    )?;
                }
                self.renderer.end_sync()?;
            }
        }

        // Clear dirty state
        self.dirty = false;
        surface.clear_dirty();

        // Store for next diff
        self.prev_surface = Some(surface);

        // Hide cursor at end
        print!("\x1b[?25l");
        io::stdout().flush()?;

        Ok(())
    }

    /// Determine which rendering strategy to use.
    fn determine_render_strategy(&mut self, size: Size) -> RenderStrategy {
        // First render - full
        if self.prev_surface.is_none() {
            self.last_width = size.width;
            self.last_height = size.height;
            return RenderStrategy::Full;
        }

        // Width changed - full
        if size.width != self.last_width {
            self.last_width = size.width;
            self.last_height = size.height;
            return RenderStrategy::Full;
        }

        // Check if changes are above viewport (large scroll)
        // For now, always use incremental if there's a previous surface
        if let Some(ref prev) = self.prev_surface {
            if let (Some(_first), Some(last)) = (prev.first_dirty(), prev.last_dirty()) {
                // If change is in upper quarter of screen, full render
                if last > size.height / 4 * 3 {
                    return RenderStrategy::Full;
                }
            }
        }

        // Default to incremental
        RenderStrategy::Incremental
    }

    /// Cleanup on exit.
    fn cleanup(&mut self) -> Result<()> {
        // Show cursor
        crossterm::execute!(stdout(), crossterm::cursor::Show)?;

        // Disable mouse capture
        crossterm::execute!(stdout(), crossterm::event::DisableMouseCapture)?;

        // Leave alternate screen
        crossterm::execute!(stdout(), crossterm::terminal::LeaveAlternateScreen)?;

        // Flush output
        io::stdout().flush()?;

        Ok(())
    }

    /// Force a full redraw on next frame.
    pub fn force_redraw(&mut self) {
        if let Some(ref mut surf) = self.prev_surface {
            surf.mark_all_dirty();
        }
        self.dirty = true;
    }

    /// Get the current terminal size.
    pub fn size(&mut self) -> Result<Size> {
        self.terminal.size()
    }
}

impl Drop for TUI {
    fn drop(&mut self) {
        if self.running {
            // Ensure cleanup happens
            let _ = self.cleanup();
        }
    }
}
