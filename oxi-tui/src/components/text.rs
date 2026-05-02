//! Text component for displaying text content with padding.

use std::string::String;

/// A text component that displays content with configurable padding.
pub struct Text {
    /// The text content to display.
    content: String,
    /// Horizontal padding (left and right).
    padding_x: usize,
    /// Vertical padding (top and bottom).
    padding_y: usize,
}

impl Text {
    /// Creates a new Text component with the given content.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            padding_x: 0,
            padding_y: 0,
        }
    }

    /// Sets the horizontal padding.
    pub fn padding_x(mut self, padding: usize) -> Self {
        self.padding_x = padding;
        self
    }

    /// Sets the vertical padding.
    pub fn padding_y(mut self, padding: usize) -> Self {
        self.padding_y = padding;
        self
    }

    /// Sets both horizontal and vertical padding.
    pub fn padding(mut self, padding: usize) -> Self {
        self.padding_x = padding;
        self.padding_y = padding;
        self
    }

    /// Gets the content of the text.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Trait for rendering components to a width.
pub trait Render {
    /// Render the component to lines of output with the given width.
    fn render(&self, width: usize) -> Vec<String>;
}

impl Render for Text {
    fn render(&self, width: usize) -> Vec<String> {
        use unicode_width::UnicodeWidthStr;

        let inner_width = width.saturating_sub(self.padding_x * 2);
        let mut lines = Vec::new();

        // Top padding
        for _ in 0..self.padding_y {
            lines.push(" ".repeat(width));
        }

        // Content with word wrapping
        let words: Vec<&str> = self.content.split_whitespace().collect();
        let mut current_line = String::new();
        let mut line_width: usize = 0;

        for word in words {
            let word_width = UnicodeWidthStr::width(word);
            let space_width = if current_line.is_empty() { 0 } else { 1 };

            if line_width + space_width + word_width > inner_width {
                if !current_line.is_empty() {
                    let padding = " ".repeat(self.padding_x);
                    lines.push(format!("{}{}{}", padding, current_line, " ".repeat(inner_width.saturating_sub(UnicodeWidthStr::width(current_line.as_str())))));
                    current_line.clear();
                    line_width = 0;
                }
            }

            if !current_line.is_empty() {
                current_line.push(' ');
                line_width += 1;
            }
            current_line.push_str(word);
            line_width += word_width;
        }

        // Remaining content
        if !current_line.is_empty() {
            let padding = " ".repeat(self.padding_x);
            lines.push(format!("{}{}{}", padding, current_line, " ".repeat(inner_width.saturating_sub(UnicodeWidthStr::width(current_line.as_str())))));
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            lines.push(" ".repeat(width));
        }

        lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self::new("")
    }
}
