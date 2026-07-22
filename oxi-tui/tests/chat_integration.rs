//! Integration coverage for ChatView memoization inside a composite retained tree.

use oxi_tui::content::MessageRole;
use oxi_tui::pipeline::{CursorState, FrameOutcome, draw_frame};
use oxi_tui::theme::{TerminalCaps, Theme};
use oxi_tui::widget::chat::ChatView;
use oxi_tui::widget::panel::Footer;
use oxi_tui::widget::{
    FocusTarget, RenderCtx, Renderable, RetainedChild, RetainedTree, Text, hash_combine,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

struct CompositeChatTree {
    chat: RetainedChild<ChatView>,
    footer: RetainedChild<Footer>,
    sticky: RetainedChild<Text>,
}

impl CompositeChatTree {
    fn stable_after_stream() -> Self {
        let mut chat = ChatView::new();
        let _ = chat.log_mut().append_message(MessageRole::Assistant);
        chat.log_mut().append_token("Hello");
        chat.log_mut().finalize_stream();
        chat.view_mut().set_viewport_height(22);

        Self {
            chat: RetainedChild::new(chat),
            footer: RetainedChild::new(Footer::new()),
            sticky: RetainedChild::new(Text::new("todo panel")),
        }
    }
}

impl Renderable for CompositeChatTree {
    fn content_hash(&self) -> u64 {
        hash_combine(
            self.chat.inner().content_hash(),
            hash_combine(
                self.footer.inner().content_hash(),
                self.sticky.inner().content_hash(),
            ),
        )
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx<'_, '_>) -> u16 {
        24
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx<'_, '_>) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);
        let _ = self.chat.render_if_changed(areas[0], ctx);
        let _ = self.footer.render_if_changed(areas[1], ctx);
        let _ = self.sticky.render_if_changed(areas[2], ctx);
    }
}

#[test]
fn streaming_updates_only_chat_subtree() {
    let mut chat = RetainedChild::new(ChatView::new());
    let mut footer = RetainedChild::new(Footer::new());
    let mut sticky = RetainedChild::new(Text::new("todo panel"));
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let theme = Theme::dark();
    let caps = TerminalCaps::default();

    term.draw(|frame| {
        let area = frame.area();
        let mut ctx = RenderCtx::new(frame, &theme, &caps);
        assert!(chat.render_if_changed(area, &mut ctx));
        assert!(footer.render_if_changed(area, &mut ctx));
        assert!(sticky.render_if_changed(area, &mut ctx));
    })
    .unwrap();

    let chat_hash_before = chat.inner().content_hash();
    let footer_hash_before = footer.inner().content_hash();
    let sticky_hash_before = sticky.inner().content_hash();
    let _ = chat
        .inner_mut()
        .log_mut()
        .append_message(MessageRole::Assistant);
    chat.inner_mut().log_mut().append_token("Hello");

    assert_ne!(chat.inner().content_hash(), chat_hash_before);
    assert_eq!(footer.inner().content_hash(), footer_hash_before);
    assert_eq!(sticky.inner().content_hash(), sticky_hash_before);

    term.draw(|frame| {
        let area = frame.area();
        let mut ctx = RenderCtx::new(frame, &theme, &caps);
        assert!(
            chat.render_if_changed(area, &mut ctx),
            "ChatView must re-render after token append"
        );
        assert!(
            !footer.render_if_changed(area, &mut ctx),
            "Footer must not re-render when its hash is unchanged"
        );
        assert!(
            !sticky.render_if_changed(area, &mut ctx),
            "Sticky must not re-render when its hash is unchanged"
        );
    })
    .unwrap();
}

#[test]
fn composite_tree_idle_when_chat_stable() {
    let mut tree = RetainedTree::new(Box::new(CompositeChatTree::stable_after_stream()));
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let mut cursor = CursorState::new();
    let theme = Theme::dark();
    let caps = TerminalCaps::default();

    let rendered = draw_frame(
        &mut term,
        &mut tree,
        &mut cursor,
        FocusTarget::None,
        &theme,
        &caps,
    )
    .unwrap();
    assert_eq!(rendered, FrameOutcome::Rendered);

    let idle = draw_frame(
        &mut term,
        &mut tree,
        &mut cursor,
        FocusTarget::None,
        &theme,
        &caps,
    )
    .unwrap();
    assert_eq!(idle, FrameOutcome::Idle);
}
