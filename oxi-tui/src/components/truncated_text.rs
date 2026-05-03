//! Truncated text component with ellipsis support.

use crate::{Cell, Color, Component, Event, Rect, Size, Surface};

const TRUNCATION_SUFFIX: &str = "... [truncated]";

/// A text component that truncates content when it exceeds configured bounds.
pub struct TruncatedText {
    content: String,
    max_width: Option<u16>,
    max_height: Option<u16>,
    expand_on_focus: bool,
    expanded: bool,
    focused: bool,
    dirty: bool,
    fg_color: Option<Color>,
    bg_color: Option<Color>,
}

impl TruncatedText {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            max_width: None,
            max_height: Some(1),
            expand_on_focus: false,
            expanded: false,
            focused: false,
            dirty: true,
            fg_color: None,
            bg_color: None,
        }
    }

    pub fn with_max_width(mut self, width: u16) -> Self {
        self.max_width = Some(width);
        self
    }

    pub fn with_max_height(mut self, height: u16) -> Self {
        self.max_height = Some(height);
        self
    }

    pub fn with_expand_on_focus(mut self) -> Self {
        self.expand_on_focus = true;
        self
    }

    pub fn with_fg(mut self, color: Color) -> Self {
        self.fg_color = Some(color);
        self
    }

    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg_color = Some(color);
        self
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.dirty = true;
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// Compute the lines to display, applying truncation if needed.
    fn compute_lines(&self, area_width: u16) -> Vec<String> {
        let width_limit = self.max_width.map_or(area_width, |w| w.min(area_width)) as usize;
        let all_lines: Vec<String> = if self.content.contains('\n') {
            self.content.split('\n').map(|s| s.to_string()).collect()
        } else {
            // Wrap at width_limit
            let mut lines = Vec::new();
            let mut remaining = self.content.as_str();
            while !remaining.is_empty() {
                if remaining.len() <= width_limit {
                    lines.push(remaining.to_string());
                    break;
                }
                // Find a good break point
                let mut break_at = width_limit;
                if let Some(pos) = remaining[..width_limit].rfind(' ') {
                    break_at = pos;
                }
                lines.push(remaining[..break_at].to_string());
                remaining = remaining[break_at..].trim_start();
            }
            lines
        };

        let height_limit = if self.expand_on_focus && self.expanded {
            all_lines.len()
        } else {
            self.max_height.map_or(all_lines.len(), |h| h as usize)
        };

        let total = all_lines.len();
        if total <= height_limit {
            all_lines
        } else {
            let mut result: Vec<String> = all_lines.into_iter().take(height_limit).collect();
            // Append truncation suffix to last line
            if let Some(last) = result.last_mut() {
                let suffix_space = width_limit.min(TRUNCATION_SUFFIX.len());
                let suffix: String = TRUNCATION_SUFFIX.chars().take(suffix_space).collect();
                let max_last = width_limit.saturating_sub(suffix.len());
                last.truncate(max_last);
                last.push_str(&suffix);
            }
            result
        }
    }
}

impl Component for TruncatedText {
    fn name(&self) -> &str {
        "TruncatedText"
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
        if !self.focused || !self.expand_on_focus {
            return false;
        }

        if let crate::Event::Key(key) = event {
            match key.code {
                crate::KeyCode::Enter | crate::KeyCode::Char(' ') => {
                    self.expanded = !self.expanded;
                    self.dirty = true;
                    true
                }
                crate::KeyCode::Escape => {
                    if self.expanded {
                        self.expanded = false;
                        self.dirty = true;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let lines = self.compute_lines(area.width);

        for (row_idx, line) in lines.iter().enumerate() {
            let y = area.y + row_idx as u16;
            if y >= area.y + area.height {
                break;
            }

            for (i, c) in line.chars().enumerate() {
                let col = area.x + i as u16;
                if col >= area.x + area.width {
                    break;
                }
                let mut cell = Cell::new(c);
                if let Some(fg) = self.fg_color {
                    cell.fg = fg;
                }
                if let Some(bg) = self.bg_color {
                    cell.bg = bg;
                }
                surface.set(y, col, cell);
            }

            // Clear rest of row
            let clear_start = area.x + line.len().min(area.width as usize) as u16;
            for col in clear_start..area.x + area.width {
                let mut cell = Cell::new(' ');
                if let Some(bg) = self.bg_color {
                    cell.bg = bg;
                }
                surface.set(y, col, cell);
            }
        }

        // Clear remaining rows
        let used_rows = lines.len() as u16;
        for row in used_rows..area.height {
            let y = area.y + row;
            for col in area.x..area.x + area.width {
                let mut cell = Cell::new(' ');
                if let Some(bg) = self.bg_color {
                    cell.bg = bg;
                }
                surface.set(y, col, cell);
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 3,
            height: 1,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        let w = self.max_width.unwrap_or_else(|| self.content.len().min(80) as u16);
        let h = self.max_height.unwrap_or(1);
        Some(Size { width: w, height: h })
    }

    fn on_focus(&mut self) {
        self.focused = true;
        if self.expand_on_focus {
            self.expanded = true;
        }
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        self.expanded = false;
        self.dirty = true;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}
