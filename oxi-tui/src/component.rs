use std::thread;
use std::sync::Arc;

/// Component trait for UI elements that can be rendered
pub trait Component: Send + dyn {
    /// Render the component to a vector of strings (one per line)
    fn render(&self, width: usize) -> Vec<String>;
    /// Handle input data, returns true if the input was consumed
    fn handle_input(&mut self, _data: &str) -> bool { false }
    /// Mark the component as needing re-render
    fn invalidate(&mut self) {}
}

/// Focusable trait for components that can receive keyboard focus
pub trait Focusable: Component {
    /// Check if this component currently has focus
    fn focused(&self) -> bool;
    /// Set the focus state of this component
    fn set_focused(&mut self, focused: bool);
}

/// Container for managing multiple components
pub struct ComponentContainer {
    components: Vec<Box<dyn Component>>,
}

impl ComponentContainer {
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    pub fn add(&mut self, component: Box<dyn Component>) {
        self.components.push(component);
    }

    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for component in &self.components {
            lines.extend(component.render(width));
        }
        lines
    }

    pub fn handle_input(&mut self, data: &str) -> bool {
        for component in &mut self.components {
            if component.handle_input(data) {
                return true;
            }
        }
        false
    }

    pub fn invalidate(&mut self) {
        for component in &mut self.components {
            component.invalidate();
        }
    }
}

impl Default for ComponentContainer {
    fn default() -> Self {
        Self::new()
    }
}