//! Command palette overlay component with fuzzy filtering.

use crate::autocomplete::FuzzyMatcher;
use crate::{Cell, Color, Component, Event, KeyCode, KeyEvent, Rect, Size, Surface};

/// A command entry in the palette.
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub shortcut: Option<String>,
    pub category: Option<String>,
}

impl Command {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            shortcut: None,
            category: None,
        }
    }

    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
}

/// Callback when a command is selected.
pub type OnCommandFn = Box<dyn Fn(&Command) + Send>;

/// A command palette with text input filtering and fuzzy matching.
pub struct CommandPalette {
    commands: Vec<Command>,
    filtered_indices: Vec<usize>,
    query: String,
    selected: usize,
    scroll_offset: usize,
    dirty: bool,
    visible: bool,
    on_command: Option<OnCommandFn>,
    matcher: FuzzyMatcher,
}

impl CommandPalette {
    pub fn new(commands: Vec<Command>) -> Self {
        let filtered_indices = (0..commands.len()).collect();
        Self {
            commands,
            filtered_indices,
            query: String::new(),
            selected: 0,
            scroll_offset: 0,
            dirty: true,
            visible: false,
            on_command: None,
            matcher: FuzzyMatcher::new(),
        }
    }

    pub fn on_command(mut self, f: impl Fn(&Command) + Send + 'static) -> Self {
        self.on_command = Some(Box::new(f));
        self
    }

    pub fn set_commands(&mut self, commands: Vec<Command>) {
        self.commands = commands;
        self.apply_filter();
        self.dirty = true;
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.apply_filter();
        self.dirty = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.dirty = true;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    fn apply_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.commands.len()).collect();
        } else {
            let mut scored: Vec<(usize, usize)> = self
                .commands
                .iter()
                .enumerate()
                .filter_map(|(i, cmd)| {
                    self.matcher
                        .matches(&self.query, &cmd.name)
                        .map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.dirty = true;
        }
    }

    fn select_next(&mut self) {
        if !self.filtered_indices.is_empty() && self.selected < self.filtered_indices.len() - 1 {
            self.selected += 1;
            self.dirty = true;
        }
    }

    fn confirm(&mut self) {
        if let Some(ref cb) = self.on_command {
            if let Some(&idx) = self.filtered_indices.get(self.selected) {
                if let Some(cmd) = self.commands.get(idx) {
                    cb(cmd);
                }
            }
        }
        self.hide();
    }
}

impl Component for CommandPalette {
    fn name(&self) -> &str {
        "CommandPalette"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.visible {
            return false;
        }

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Escape,
                ..
            }) => {
                self.hide();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => {
                self.select_prev();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) => {
                self.select_next();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                self.confirm();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                ..
            }) => {
                self.query.push(*c);
                self.apply_filter();
                self.selected = 0;
                self.dirty = true;
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                self.query.pop();
                self.apply_filter();
                self.selected = 0;
                self.dirty = true;
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        if !self.visible {
            return;
        }

        // Calculate palette dimensions: centered, max 60 wide, max 15 tall
        let palette_w = area.width.min(60);
        let list_height = 10u16;
        let total_height = 1 + list_height; // input + list
        let x = area.x + (area.width.saturating_sub(palette_w)) / 2;
        let y = area.y + 2; // slight offset from top

        // Dim background (draw a border around the palette area)
        let overlay_bg = Color::Indexed(236);
        for r in y..y + total_height + 2 {
            for c in x.saturating_sub(1)..x + palette_w + 1 {
                if r < area.y + area.height && c < area.x + area.width {
                    surface.set(r, c, Cell::new(' ').with_bg(overlay_bg));
                }
            }
        }

        // Draw border
        let border_fg = Color::Indexed(12);
        // Top
        surface.set(
            y,
            x.saturating_sub(1),
            Cell::new('┌').with_fg(border_fg).with_bg(overlay_bg),
        );
        surface.set(
            y,
            x + palette_w,
            Cell::new('┐').with_fg(border_fg).with_bg(overlay_bg),
        );
        // Bottom
        let bottom = y + total_height + 1;
        surface.set(
            bottom,
            x.saturating_sub(1),
            Cell::new('└').with_fg(border_fg).with_bg(overlay_bg),
        );
        surface.set(
            bottom,
            x + palette_w,
            Cell::new('┘').with_fg(border_fg).with_bg(overlay_bg),
        );
        // Sides
        for r in y + 1..bottom {
            if r < area.y + area.height {
                if x > 0 {
                    surface.set(
                        r,
                        x - 1,
                        Cell::new('│').with_fg(border_fg).with_bg(overlay_bg),
                    );
                }
                if x + palette_w < area.x + area.width {
                    surface.set(
                        r,
                        x + palette_w,
                        Cell::new('│').with_fg(border_fg).with_bg(overlay_bg),
                    );
                }
            }
        }
        // Horizontal lines
        for c in x..x + palette_w {
            surface.set(y, c, Cell::new('─').with_fg(border_fg).with_bg(overlay_bg));
            surface.set(
                bottom,
                c,
                Cell::new('─').with_fg(border_fg).with_bg(overlay_bg),
            );
        }
        // Separator after input
        let sep_y = y + 2;
        for c in x..x + palette_w {
            surface.set(
                sep_y,
                c,
                Cell::new('─').with_fg(border_fg).with_bg(overlay_bg),
            );
        }

        // Render input row
        let prompt = "> ";
        for (i, c) in prompt.chars().enumerate() {
            let col = x + i as u16;
            if col < x + palette_w {
                surface.set(
                    y + 1,
                    col,
                    Cell::new(c).with_fg(Color::White).with_bg(overlay_bg),
                );
            }
        }
        for (i, c) in self.query.chars().enumerate() {
            let col = x + prompt.len() as u16 + i as u16;
            if col < x + palette_w {
                surface.set(
                    y + 1,
                    col,
                    Cell::new(c).with_fg(Color::White).with_bg(overlay_bg),
                );
            }
        }
        // Cursor
        let cursor_col = x + prompt.len() as u16 + self.query.len() as u16;
        if cursor_col < x + palette_w {
            surface.set(
                y + 1,
                cursor_col,
                Cell::new(' ').with_fg(Color::Black).with_bg(Color::White),
            );
        }

        // Render filtered list
        let list_start = sep_y + 1;
        let visible_count = list_height as usize;
        let start = self.scroll_offset;
        let end = (start + visible_count).min(self.filtered_indices.len());

        for vi in start..end {
            let row = list_start + (vi - start) as u16;
            if row > y + total_height {
                break;
            }
            let cmd_idx = self.filtered_indices[vi];
            let cmd = &self.commands[cmd_idx];
            let is_selected = vi == self.selected;

            let (fg, bg) = if is_selected {
                (Color::Black, Color::Indexed(12))
            } else {
                (Color::White, overlay_bg)
            };

            // Command name
            let max_name = (palette_w as usize).saturating_sub(12);
            let name_str: String = cmd.name.chars().take(max_name).collect();
            for (i, c) in name_str.chars().enumerate() {
                let col = x + i as u16;
                if col < x + palette_w {
                    surface.set(row, col, Cell::new(c).with_fg(fg).with_bg(bg));
                }
            }

            // Shortcut (right-aligned)
            if let Some(ref shortcut) = cmd.shortcut {
                let sc_start = x + palette_w - shortcut.len() as u16 - 2;
                for (i, c) in shortcut.chars().enumerate() {
                    let col = sc_start + i as u16;
                    if col >= x && col < x + palette_w {
                        surface.set(
                            row,
                            col,
                            Cell::new(c).with_fg(Color::Indexed(8)).with_bg(bg),
                        );
                    }
                }
            }

            // Clear remainder
            let name_end = x + name_str.len() as u16;
            for col in name_end..x + palette_w {
                surface.set(row, col, Cell::new(' ').with_fg(fg).with_bg(bg));
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 30,
            height: 5,
        }
    }

    fn on_focus(&mut self) {
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.hide();
    }
}
