use oxi::tui::v2_bridge::ClosureRoot;
use oxi_tui::pipeline::{CursorState, FrameOutcome, draw_frame};
use oxi_tui::theme::{TerminalCaps, Theme};
use oxi_tui::widget::{FocusTarget, RetainedTree};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn draw_frame_works_with_closure_root() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = Theme::dark();
    let caps = TerminalCaps::default();
    let mut cursor = CursorState::new();

    let mut tree = RetainedTree::new(Box::new(ClosureRoot::new(|frame| {
        frame.render_widget("Hello v2 pipeline", frame.area());
    })));

    let outcome = draw_frame(
        &mut terminal,
        &mut tree,
        &mut cursor,
        FocusTarget::None,
        &theme,
        &caps,
    )
    .unwrap();

    assert_eq!(outcome, FrameOutcome::Rendered);
}
