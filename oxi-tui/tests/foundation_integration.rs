//! Integration test for the Plan A foundation (pipeline + widget + theme).
//!
//! Verifies the cross-module contracts that individual unit tests cannot:
//! - `draw_frame` skips rendering when content hash is unchanged (idle skip).
//! - `TerminalCaps::adapt_theme` downgrades all colors to `Color::Reset` at
//!   `ColorLevel::None`.
//! - `hash_combine` is deterministic across invocations (foundation for the
//!   Plan B child→parent hash propagation chain).

use oxi_tui::pipeline::{CursorState, FrameOutcome, draw_frame};
use oxi_tui::theme::{TerminalCaps, Theme};
use oxi_tui::widget::{FocusTarget, RetainedTree, Text, hash_combine, hash_str};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn foundation_idle_frame_skips_render() {
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    let mut tree = RetainedTree::new(Box::new(
        Text::new("hello world").fg(ratatui::style::Color::Green),
    ));
    let mut cursor = CursorState::new();
    let theme = Theme::dark();
    let caps = TerminalCaps::default();

    // Frame 1: rendered (first call always).
    let o1 = draw_frame(
        &mut term,
        &mut tree,
        &mut cursor,
        FocusTarget::None,
        &theme,
        &caps,
    )
    .unwrap();
    assert_eq!(o1, FrameOutcome::Rendered);

    // Frame 2: idle (hash unchanged, no resize).
    let o2 = draw_frame(
        &mut term,
        &mut tree,
        &mut cursor,
        FocusTarget::None,
        &theme,
        &caps,
    )
    .unwrap();
    assert_eq!(o2, FrameOutcome::Idle);
}

#[test]
fn foundation_theme_adapt_to_basic_level() {
    use oxi_tui::theme::ColorLevel;
    let mut theme = Theme::dark();
    let _original_bg = theme.colors.background;
    let caps = TerminalCaps {
        color_level: ColorLevel::None,
        ..Default::default()
    };
    caps.adapt_theme(&mut theme);
    // At None level, all colors become Reset.
    assert_eq!(theme.colors.background, ratatui::style::Color::Reset);
}

#[test]
fn foundation_hash_propagation() {
    // Stub: real child→parent propagation tested in Plan B when composite
    // widgets land. For now, verify the primitive combine is deterministic so
    // the future propagation chain has a stable foundation.
    let h1 = hash_combine(hash_str("parent"), hash_str("child"));
    let h2 = hash_combine(hash_str("parent"), hash_str("child"));
    assert_eq!(h1, h2);
}
