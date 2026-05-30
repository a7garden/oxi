//! Text viewer overlay — displays scrollable text content.
//! Used for help, hotkeys, changelog, tools list, etc.

use super::{centered_layout, OverlayAction, OverlayComponent};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
// use oxi_tui::Theme; // unused — kept for future use
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation},
    Frame,
};

pub struct TextViewerOverlay {
    title: String,
    lines: Vec<Line<'static>>,
    scroll: usize,
    total_lines: usize,
}

impl TextViewerOverlay {
    pub fn new(title: impl Into<String>, content: String) -> Self {
        let lines: Vec<Line<'static>> = content.lines().map(|l| Line::raw(l.to_string())).collect();
        let total_lines = lines.len();
        Self {
            title: title.into(),
            lines,
            scroll: 0,
            total_lines,
        }
    }
}

impl std::fmt::Debug for TextViewerOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextViewerOverlay")
            .field("title", &self.title)
            .field("total_lines", &self.total_lines)
            .field("scroll", &self.scroll)
            .finish()
    }
}

impl OverlayComponent for TextViewerOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j')
                if self.scroll < self.total_lines.saturating_sub(1) =>
            {
                self.scroll += 1;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(20);
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + 20).min(self.total_lines.saturating_sub(1));
            }
            KeyCode::Home => {
                self.scroll = 0;
            }
            KeyCode::End => {
                self.scroll = self.total_lines.saturating_sub(1);
            }
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                return OverlayAction::Close;
            }
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        let popup = centered_layout(area, 0.85, 0.85);
        let title_style = Style::default()
            .fg(theme.colors.primary.to_ratatui())
            .add_modifier(Modifier::BOLD);

        // Clear background
        frame.render_widget(Clear, popup);

        // Border and title
        let border_block = Block::default()
            .title(Span::styled(&self.title, title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Calculate visible area
        let visible_height = (inner.height.saturating_sub(2)) as usize;
        let start = self.scroll;
        let end = (start + visible_height).min(self.lines.len());
        let visible_lines: Vec<Line<'_>> = self.lines[start..end].to_vec();

        // Content
        let content_height = (end - start) as u16;
        let content_area = Rect {
            x: inner.x + 1,
            y: inner.y + 1,
            width: inner.width.saturating_sub(2),
            height: content_height,
        };

        let paragraph =
            Paragraph::new(visible_lines.clone()).wrap(ratatui::widgets::Wrap { trim: false });

        frame.render_widget(paragraph, content_area);

        // Scrollbar
        if self.total_lines > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(
                scrollbar,
                Rect {
                    x: inner.x + inner.width - 1,
                    y: inner.y + 1,
                    width: 1,
                    height: inner.height.saturating_sub(1),
                },
                &mut ratatui::widgets::ScrollbarState::new(self.total_lines).position(self.scroll),
            );
        }

        // Footer hint
        frame.render_widget(
            Paragraph::new(Span::styled(
                " ↑/↓ scroll  |  PgUp/PgDn  |  Esc/Enter/q close",
                Style::default().fg(theme.colors.muted.to_ratatui()),
            )),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " ↑/↓ scroll  |  Esc close"
    }
}

// ── Convenience constructors for common text viewers ─────────────────────

pub fn help_overlay() -> Box<dyn OverlayComponent> {
    Box::new(TextViewerOverlay::new(" Help ", HELP_CONTENT.to_string()))
}

pub fn hotkeys_overlay() -> Box<dyn OverlayComponent> {
    Box::new(TextViewerOverlay::new(
        " Key Shortcuts ",
        HOTKEYS_CONTENT.to_string(),
    ))
}

pub fn changelog_overlay(entries: Vec<(String, String)>) -> Box<dyn OverlayComponent> {
    let content = format_changelog_entries(entries);
    Box::new(TextViewerOverlay::new(" Changelog ", content))
}

pub fn tools_overlay(content: String) -> Box<dyn OverlayComponent> {
    Box::new(TextViewerOverlay::new(" Tools ", content))
}

pub fn extensions_overlay(content: String) -> Box<dyn OverlayComponent> {
    Box::new(TextViewerOverlay::new(" Extensions & WASM Tools ", content))
}

fn format_changelog_entries(entries: Vec<(String, String)>) -> String {
    let mut out = String::new();
    for (version, changelog) in entries {
        out.push_str(&format!("## {}\n\n", version));
        out.push_str(changelog.trim());
        out.push_str("\n\n");
    }
    out
}

// ── Static content ────────────────────────────────────────────────────────

pub const HELP_CONTENT: &str = r#" Session
  /new              Start a new session
  /clone            Duplicate current session
  /resume           Resume a previous session
  /import <path>    Import session from JSONL
  /tree             Show session tree
  /fork             List messages to fork from
  /fork <number>    Fork from a message by list number
  /fork <id>        Fork from a specific message ID
  /session          Show session info
  /name <name>      Set session name

 Model
  /model [id]       Switch or show model
  /scoped-models    Models for Ctrl+P cycling
  /router           Configure model router

 Context
  /compact [instr]  Compact context

 Tools
  /tools            List active tools
  /tools <name>     Toggle tool on/off
  /extensions       List extensions & WASM tools
  /ext              Alias for /extensions

 Export
  /export [path]    Export to HTML
  /copy             Copy code block / last reply
  /share            Share session as GitHub Gist

 Auth
  /provider [name]  Configure API key
  /logout [name]    Remove key

 Info
  /help             This help
  /hotkeys          Key shortcuts
  /changelog        Changelog
  /settings         Current settings
  /reload           Reload config
  /quit             Quit

 Keys
  Enter             Send
  Ctrl+C            Interrupt / Quit
  PageUp/Down       Scroll
  /                 Slash commands"#;

pub const HOTKEYS_CONTENT: &str = r#" Navigation
  Enter              Submit input
  Escape             Cancel
  PageUp/PageDown    Scroll chat

 Editor
  ←/→                Move cursor
  Home/End           Start/End of line
  Backspace          Delete char
  Ctrl+←/→           Move by word

 Session
  Ctrl+C             Interrupt / Quit
  Ctrl+Y             Copy last code block
  Ctrl+P             Cycle models
  Shift+Ctrl+P       Cycle models (reverse)"#;
