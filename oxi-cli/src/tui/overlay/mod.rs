//! Overlay component trait and shared types.
//!
//! Each overlay (model selector, logout, resume) implements `OverlayComponent`
//! to encapsulate its own state, event handling, and rendering.
//! This follows ratatui's StatefulWidget philosophy at the overlay level.

use crossterm::event::{KeyEvent, MouseEvent};
use oxi_tui::Theme;
use ratatui::{Frame, layout::Rect};

pub mod anchor;
pub mod extensions;
pub mod factories;
pub mod fork_select;
pub mod issues_panel;
pub mod mcp_config;
pub mod mcp_dashboard;
pub mod mcp_presets;
pub mod model_select_inline;
pub mod provider_select;
pub mod questionnaire;
pub mod router_integration;
pub mod router_setup;
pub mod settings;
pub mod text_viewer;
pub mod tree_navigator;
#[allow(unused_imports)]
pub use extensions::extensions_overlay;
#[allow(unused_imports)]
pub use factories::{logout_select, model_select, resume_select, routing_status};
#[allow(unused_imports)]
pub use fork_select::ForkSelectOverlay;
#[allow(unused_imports)]
pub use issues_panel::IssuesPanelOverlay;
#[allow(unused_imports)]
pub use mcp_config::McpConfigOverlay;
#[allow(unused_imports)]
pub use mcp_dashboard::{McpAction, McpDashboardOverlay};
pub use router_setup::{RouterSetupData, router_setup};
#[allow(unused_imports)]
pub use settings::settings_overlay;
#[allow(unused_imports)]
pub use text_viewer::{changelog_overlay, help_overlay, hotkeys_overlay, tools_overlay};
#[allow(unused_imports)] // re-exported for downstream consumers; not used in this crate
pub use tree_navigator::{TreeNavigatorOverlay, tree_navigator};

// ---------------------------------------------------------------------------
// Overlay action
// ---------------------------------------------------------------------------

/// Actions an overlay can request after handling a key event.
#[derive(Debug)]
#[allow(dead_code)]
pub enum OverlayAction {
    /// No action needed.
    None,
    /// Close the overlay.
    Close,
    /// Switch to a different session.
    SwitchSession(String),
    /// Start a new session.
    NewSession,
    /// Execute a slash command by name.
    ExecuteSlashCommand(String),
    /// Send a user prompt.
    SendPrompt(String),
    /// Open the router setup overlay.
    OpenRouterSetup {
        initial: crate::tui::overlay::RouterSetupData,
        models: Vec<String>,
    },
    /// Fork session from the selected entry ID.
    ForkFromEntry { entry_id: String },
    /// Navigate to the selected tree node entry ID.
    NavigateToEntry { entry_id: String },
    /// A provider's API key was saved during initial setup.
    /// Caller should open the model selector for this provider next.
    ProviderKeySaved { provider_name: String },
    /// A model was selected from the inline model-selector overlay.
    /// The handler should apply it to the session and persist settings.
    ModelSelected {
        provider_name: String,
        model_id: String,
    },
    /// Action emitted by the MCP dashboard overlay.
    McpAction(McpAction),
    /// A new MCP config was written to disk by the management overlay
    /// and should be hot-loaded into the running `McpManager`.
    /// `config` is the freshly-merged disk view; `message` is shown as
    /// a success notification.
    McpConfigApplied {
        config: oxi_agent::mcp::McpConfig,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for self-contained overlay components.
///
/// Each overlay owns its state, handles its own key events, and renders itself.
/// The app only needs to dispatch — no match sprawl in handlers/render.
pub trait OverlayComponent: std::fmt::Debug {
    /// Handle a key press. Return an action if the app needs to do something.
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;

    /// Called after an async action completes. Default: no-op.
    fn mark_refresh(&mut self) {}

    /// Handle a clipboard paste event.
    /// Default implementation returns `OverlayAction::None` (no-op).
    fn handle_paste(&mut self, _text: &str) -> OverlayAction {
        OverlayAction::None
    }

    /// Handle a mouse event.
    /// Default implementation returns `OverlayAction::None` (no-op).
    #[allow(dead_code)]
    fn handle_mouse(&mut self, _event: MouseEvent) -> OverlayAction {
        OverlayAction::None
    }

    /// Render the overlay into the given area.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme);

    /// Footer hint text for this overlay.
    fn hint(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Helper to center a popup in an area with given size ratios.
/// Kept for backward compatibility — prefer `centered_layout`.
#[allow(dead_code)]
pub fn centered_popup(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    let w = (area.width as f32 * width_pct) as u16;
    let h = (area.height as f32 * height_pct) as u16;
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

/// Create a centered layout using the new anchor system.
/// Drop-in replacement for `centered_popup` that delegates to `resolve_overlay_layout`.
pub fn centered_layout(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    use oxi_tui::overlay_anchor::{
        OverlayAnchor, OverlayLayout, SizeValue, resolve_overlay_layout,
    };
    let layout = OverlayLayout {
        anchor: OverlayAnchor::Center,
        width: SizeValue::Percent(width_pct),
        max_height: Some((area.height as f32 * height_pct) as u16),
        min_width: None,
        margin: 0,
        ..Default::default()
    };
    resolve_overlay_layout(&layout, area.width, area.height)
}
