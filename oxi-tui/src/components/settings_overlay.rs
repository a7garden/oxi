//! Settings overlay - modal overlay for editing settings groups.

use crate::components::SettingsList;
use crate::{Cell, Color, Component, Event, KeyCode, KeyEvent, Rect, Size, Surface};

/// Callback when settings are saved.
pub type OnSaveFn = Box<dyn Fn() + Send>;

/// A modal overlay that wraps a settings list with save/cancel actions.
pub struct SettingsOverlay {
    settings: SettingsList,
    title: String,
    visible: bool,
    focused: bool,
    dirty: bool,
    /// If true, focus is on the Save/Cancel buttons rather than the list.
    on_actions: bool,
    on_save: Option<OnSaveFn>,
}

impl SettingsOverlay {
    pub fn new(title: impl Into<String>, settings: SettingsList) -> Self {
        Self {
            settings,
            title: title.into(),
            visible: false,
            focused: false,
            dirty: true,
            on_actions: false,
            on_save: None,
        }
    }

    pub fn on_save(mut self, f: impl Fn() + Send + 'static) -> Self {
        self.on_save = Some(Box::new(f));
        self
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.focused = true;
        self.on_actions = false;
        self.settings.on_focus();
        self.dirty = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.focused = false;
        self.settings.on_unfocus();
        self.dirty = true;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn settings_mut(&mut self) -> &mut SettingsList {
        &mut self.settings
    }
}

impl Component for SettingsOverlay {
    fn name(&self) -> &str {
        "SettingsOverlay"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.settings.is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
        self.settings.clear_dirty();
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.visible || !self.focused {
            return false;
        }

        if self.on_actions {
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Left,
                    ..
                })
                | Event::Key(KeyEvent {
                    code: KeyCode::Right,
                    ..
                }) => {
                    // Toggle between save/cancel (we only have 2 buttons, flip state)
                    // For simplicity, we just let Enter trigger save
                    self.dirty = true;
                    true
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    if let Some(ref cb) = self.on_save {
                        cb();
                    }
                    self.hide();
                    true
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Escape,
                    ..
                })
                | Event::Key(KeyEvent {
                    code: KeyCode::Char('q'),
                    ..
                }) => {
                    self.hide();
                    true
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Tab, ..
                }) => {
                    self.on_actions = false;
                    self.settings.on_focus();
                    self.dirty = true;
                    true
                }
                _ => false,
            }
        } else {
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Escape,
                    ..
                }) => {
                    self.hide();
                    true
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Tab, ..
                }) => {
                    self.on_actions = true;
                    self.settings.on_unfocus();
                    self.dirty = true;
                    true
                }
                ev => {
                    if self.settings.handle_event(ev) {
                        self.dirty = true;
                        true
                    } else {
                        false
                    }
                }
            }
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        if !self.visible {
            return;
        }

        // Dim background
        let dim_bg = Color::Indexed(234);
        for r in area.y..area.y + area.height {
            for c in area.x..area.x + area.width {
                let cell = surface.get(r, c).cloned().unwrap_or_default();
                surface.set(r, c, Cell::new(cell.char).with_fg(cell.fg).with_bg(dim_bg));
            }
        }

        // Calculate overlay bounds
        let overlay_w = area.width.min(70);
        let overlay_h = area.height.min(24);
        let ox = area.x + (area.width.saturating_sub(overlay_w)) / 2;
        let oy = area.y + (area.height.saturating_sub(overlay_h)) / 2;

        // Fill overlay background
        let bg = Color::Indexed(236);
        for r in oy..oy + overlay_h {
            for c in ox..ox + overlay_w {
                if r < area.y + area.height && c < area.x + area.width {
                    surface.set(r, c, Cell::new(' ').with_bg(bg));
                }
            }
        }

        // Border
        let border_fg = Color::Indexed(12);
        surface.set(oy, ox, Cell::new('┌').with_fg(border_fg).with_bg(bg));
        surface.set(
            oy,
            ox + overlay_w - 1,
            Cell::new('┐').with_fg(border_fg).with_bg(bg),
        );
        surface.set(
            oy + overlay_h - 1,
            ox,
            Cell::new('└').with_fg(border_fg).with_bg(bg),
        );
        surface.set(
            oy + overlay_h - 1,
            ox + overlay_w - 1,
            Cell::new('┘').with_fg(border_fg).with_bg(bg),
        );
        for c in ox + 1..ox + overlay_w - 1 {
            surface.set(oy, c, Cell::new('─').with_fg(border_fg).with_bg(bg));
            surface.set(
                oy + overlay_h - 1,
                c,
                Cell::new('─').with_fg(border_fg).with_bg(bg),
            );
        }
        for r in oy + 1..oy + overlay_h - 1 {
            surface.set(r, ox, Cell::new('│').with_fg(border_fg).with_bg(bg));
            surface.set(
                r,
                ox + overlay_w - 1,
                Cell::new('│').with_fg(border_fg).with_bg(bg),
            );
        }

        // Title
        let title_str = format!(" {} ", self.title);
        let title_start = ox + (overlay_w.saturating_sub(title_str.len() as u16)) / 2;
        for (i, c) in title_str.chars().enumerate() {
            let col = title_start + i as u16;
            if col < ox + overlay_w - 1 {
                surface.set(
                    oy,
                    col,
                    Cell::new(c).with_fg(Color::White).with_bg(bg).with_bold(),
                );
            }
        }

        // Settings list area (inside border, below title, above actions)
        let inner = Rect::new(
            ox + 2,
            oy + 2,
            overlay_w.saturating_sub(4),
            overlay_h.saturating_sub(6),
        );
        if inner.is_valid() {
            self.settings.render(surface, inner);
        }

        // Action buttons row
        let btn_y = oy + overlay_h - 3;
        let save_label = "[ Save ]";
        let cancel_label = "[ Cancel ]";
        let save_x =
            ox + overlay_w / 2 - (save_label.len() as u16 + cancel_label.len() as u16 + 4) / 2;
        let cancel_x = save_x + save_label.len() as u16 + 4;

        let save_fg = if self.on_actions {
            Color::White
        } else {
            Color::Indexed(8)
        };
        let cancel_fg = if self.on_actions {
            Color::White
        } else {
            Color::Indexed(8)
        };

        for (i, c) in save_label.chars().enumerate() {
            let col = save_x + i as u16;
            if col < ox + overlay_w - 1 {
                surface.set(btn_y, col, Cell::new(c).with_fg(save_fg).with_bg(bg));
            }
        }
        for (i, c) in cancel_label.chars().enumerate() {
            let col = cancel_x + i as u16;
            if col < ox + overlay_w - 1 {
                surface.set(btn_y, col, Cell::new(c).with_fg(cancel_fg).with_bg(bg));
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 30,
            height: 10,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        Some(Size {
            width: 70,
            height: 24,
        })
    }

    fn on_focus(&mut self) {
        self.focused = true;
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        self.hide();
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}
