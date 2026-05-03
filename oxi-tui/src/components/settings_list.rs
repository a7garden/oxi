//! Settings list component - key-value pairs with editable values.

use crate::{Cell, Color, Component, Event, KeyCode, KeyEvent, Rect, Size, Surface};

/// The type of a setting value.
#[derive(Debug, Clone)]
pub enum SettingValue {
    Toggle(bool),
    Text(String),
    Number(i64),
}

impl SettingValue {
    fn display(&self) -> String {
        match self {
            SettingValue::Toggle(v) => if *v { "on" } else { "off" }.to_string(),
            SettingValue::Text(s) => s.clone(),
            SettingValue::Number(n) => n.to_string(),
        }
    }
}

/// A single setting entry.
#[derive(Debug, Clone)]
pub struct SettingEntry {
    pub key: String,
    pub value: SettingValue,
    pub group: Option<String>,
}

/// Callback for when a setting value changes.
pub type OnSettingChangeFn = Box<dyn Fn(&str, &SettingValue) + Send>;

/// A list of settings with navigation and editing.
pub struct SettingsList {
    settings: Vec<SettingEntry>,
    selected: usize,
    editing: bool,
    edit_buffer: String,
    focused: bool,
    dirty: bool,
    on_change: Option<OnSettingChangeFn>,
}

impl SettingsList {
    pub fn new(settings: Vec<SettingEntry>) -> Self {
        Self {
            settings,
            selected: 0,
            editing: false,
            edit_buffer: String::new(),
            focused: false,
            dirty: true,
            on_change: None,
        }
    }

    pub fn on_change(mut self, f: impl Fn(&str, &SettingValue) + Send + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    pub fn selected_setting(&self) -> Option<&SettingEntry> {
        self.settings.get(self.selected)
    }

    fn navigate_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.dirty = true;
        }
    }

    fn navigate_next(&mut self) {
        if !self.settings.is_empty() && self.selected < self.settings.len() - 1 {
            self.selected += 1;
            self.dirty = true;
        }
    }

    fn activate(&mut self) {
        if self.settings.is_empty() {
            return;
        }
        let setting = &self.settings[self.selected];
        match &setting.value {
            SettingValue::Toggle(v) => {
                let new_val = !*v;
                let key = setting.key.clone();
                self.settings[self.selected].value = SettingValue::Toggle(new_val);
                if let Some(ref cb) = self.on_change {
                    cb(&key, &SettingValue::Toggle(new_val));
                }
                self.dirty = true;
            }
            SettingValue::Text(ref s) => {
                self.edit_buffer = s.clone();
                self.editing = true;
                self.dirty = true;
            }
            SettingValue::Number(_) => {
                self.edit_buffer = setting.value.display();
                self.editing = true;
                self.dirty = true;
            }
        }
    }

    fn confirm_edit(&mut self) {
        if !self.editing {
            return;
        }
        let key = self.settings[self.selected].key.clone();
        let old = &self.settings[self.selected].value;
        let new_value = match old {
            SettingValue::Number(_) => self.edit_buffer.parse::<i64>()
                .map(SettingValue::Number)
                .unwrap_or_else(|_| SettingValue::Number(0)),
            _ => SettingValue::Text(self.edit_buffer.clone()),
        };
        self.settings[self.selected].value = new_value;
        if let Some(ref cb) = self.on_change {
            cb(&key, &self.settings[self.selected].value);
        }
        self.editing = false;
        self.edit_buffer.clear();
        self.dirty = true;
    }

    fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buffer.clear();
        self.dirty = true;
    }

    /// Groups entries for display, inserting group headers.
    fn render_rows(&self) -> Vec<SettingsRow> {
        let mut rows = Vec::new();
        let mut last_group: Option<&str> = None;

        for (i, entry) in self.settings.iter().enumerate() {
            if entry.group.as_deref() != last_group {
                if let Some(ref g) = entry.group {
                    rows.push(SettingsRow::Group(g.clone()));
                }
                last_group = entry.group.as_deref();
            }
            rows.push(SettingsRow::Entry(i));
        }
        rows
    }
}

enum SettingsRow {
    Group(String),
    Entry(usize),
}

impl Component for SettingsList {
    fn name(&self) -> &str {
        "SettingsList"
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
        if !self.focused {
            return false;
        }

        if self.editing {
            if let Event::Key(key) = event {
                match key.code {
                    KeyCode::Enter => {
                        self.confirm_edit();
                        return true;
                    }
                    KeyCode::Escape => {
                        self.cancel_edit();
                        return true;
                    }
                    KeyCode::Char(c) => {
                        self.edit_buffer.push(c);
                        self.dirty = true;
                        return true;
                    }
                    KeyCode::Backspace => {
                        self.edit_buffer.pop();
                        self.dirty = true;
                        return true;
                    }
                    _ => return false,
                }
            }
            return false;
        }

        match event {
            Event::Key(KeyEvent { code: KeyCode::Up, .. }) => {
                self.navigate_prev();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Down, .. }) => {
                self.navigate_next();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Char('k'), modifiers })
                if !modifiers.ctrl && !modifiers.alt =>
            {
                self.navigate_prev();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Char('j'), modifiers })
                if !modifiers.ctrl && !modifiers.alt =>
            {
                self.navigate_next();
                true
            }
            Event::Key(KeyEvent { code: KeyCode::Enter, .. })
            | Event::Key(KeyEvent { code: KeyCode::Char(' '), .. }) => {
                self.activate();
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let rows = self.render_rows();
        let mut row = area.y;
        let max_width = area.width as usize;

        for sr in &rows {
            if row >= area.y + area.height {
                break;
            }

            match sr {
                SettingsRow::Group(name) => {
                    let text: String = format!("── {} ──", name)
                        .chars()
                        .take(max_width)
                        .collect();
                    for (i, c) in text.chars().enumerate() {
                        let col = area.x + i as u16;
                        if col < area.x + area.width {
                            surface.set(
                                row,
                                col,
                                Cell::new(c).with_fg(Color::Indexed(8)).with_bold(),
                            );
                        }
                    }
                }
                SettingsRow::Entry(idx) => {
                    let entry = &self.settings[*idx];
                    let is_selected = *idx == self.selected;
                    let fg = if is_selected && self.focused {
                        Color::Black
                    } else {
                        Color::Default
                    };
                    let bg = if is_selected && self.focused {
                        Color::Indexed(12)
                    } else {
                        Color::Default
                    };

                    let indicator = if is_selected { ">" } else { " " };
                    surface.set(row, area.x, Cell::new(indicator.chars().next().unwrap()).with_fg(fg).with_bg(bg));

                    let key_max = max_width.saturating_sub(2).min(20);
                    let key_str: String = entry.key.chars().take(key_max).collect();
                    for (i, c) in key_str.chars().enumerate() {
                        let col = area.x + 2 + i as u16;
                        if col < area.x + area.width {
                            surface.set(row, col, Cell::new(c).with_fg(fg).with_bg(bg));
                        }
                    }

                    // Separator
                    let sep_col = area.x + 2 + key_str.len() as u16;
                    if sep_col < area.x + area.width {
                        surface.set(row, sep_col, Cell::new(':').with_fg(fg).with_bg(bg));
                    }

                    // Value
                    let val_start = sep_col + 2;
                    let val_display = if self.editing && is_selected {
                        self.edit_buffer.clone()
                    } else {
                        entry.value.display()
                    };
                    let available = (area.x + area.width).saturating_sub(val_start) as usize;
                    let val_str: String = val_display.chars().take(available).collect();
                    for (i, c) in val_str.chars().enumerate() {
                        let col = val_start + i as u16;
                        if col < area.x + area.width {
                            let cell_fg = if self.editing && is_selected {
                                Color::Yellow
                            } else {
                                fg
                            };
                            surface.set(row, col, Cell::new(c).with_fg(cell_fg).with_bg(bg));
                        }
                    }

                    // Cursor for editing
                    if self.editing && is_selected {
                        let cursor_col = val_start + val_str.len() as u16;
                        if cursor_col < area.x + area.width {
                            surface.set(row, cursor_col, Cell::new(' ').with_fg(Color::Black).with_bg(Color::White));
                        }
                    }

                    // Clear rest of line
                    let clear_start = val_start + val_str.len() as u16;
                    for col in clear_start..area.x + area.width {
                        surface.set(row, col, Cell::new(' ').with_fg(fg).with_bg(bg));
                    }
                }
            }
            row += 1;
        }

        // Clear remaining rows
        for r in row..area.y + area.height {
            for col in area.x..area.x + area.width {
                surface.set(r, col, Cell::new(' '));
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 20,
            height: 1,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        Some(Size {
            width: 60,
            height: (self.settings.len() as u16).min(20),
        })
    }

    fn on_focus(&mut self) {
        self.focused = true;
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        if self.editing {
            self.cancel_edit();
        }
        self.dirty = true;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}
