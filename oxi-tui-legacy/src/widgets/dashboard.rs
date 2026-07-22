//! Generic sectioned dashboard widget.
//!
//! Renders a vertically scrollable list of "sections", each containing
//! "items" with a status indicator and a set of badge strings. The
//! widget is domain-agnostic: the oxi-cli `McpDashboardOverlay` populates
//! it from MCP server / tool state, but the same widget could be used
//! for any other sectioned management view (extensions, skills, etc.).
//!
//! This keeps oxi-tui MCP-free: there is no `oxi-agent` or
//! `oxi_sdk::mcp::*` import anywhere in this file.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidget, Widget},
};

use crate::Theme;

// ── Data types ──────────────────────────────────────────────────────────

/// Display status for a single item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemStatus {
    /// Item is live and operational.
    Active,
    /// Item is configured but not currently running.
    Inactive,
    /// Item has a failure state with a human-readable message.
    Error(String),
}

impl ItemStatus {
    pub fn symbol(&self, symbols: &crate::symbols::Symbols) -> &'static str {
        match self {
            ItemStatus::Active => symbols.dot_on,
            ItemStatus::Inactive => symbols.dot_off,
            ItemStatus::Error(_) => symbols.status_error,
        }
    }

    pub fn style<'a>(&'a self, theme: &'a Theme) -> Style {
        match self {
            ItemStatus::Active => Style::default().fg(theme.colors.success),
            ItemStatus::Inactive => Style::default().fg(theme.colors.muted),
            ItemStatus::Error(_) => Style::default().fg(theme.colors.error),
        }
    }
}

/// A single item in a section (e.g. one MCP server, one tool).
#[derive(Debug, Clone)]
pub struct DashboardItem {
    /// Stable id (used for keyboard navigation / actions).
    pub id: String,
    /// One-line label.
    pub label: String,
    /// Optional detail shown in a second line.
    pub detail: String,
    /// Item status.
    pub status: ItemStatus,
    /// Optional badges (e.g. "DIRECT", "PROXY", "eager", "lazy").
    pub badges: Vec<String>,
}

/// A logical section (e.g. "Servers" or "Tools for chrome-devtools").
#[derive(Debug, Clone)]
pub struct DashboardSection {
    /// Section header text.
    pub title: String,
    /// Items in this section.
    pub items: Vec<DashboardItem>,
    /// Whether the section is currently collapsed (items hidden).
    pub collapsed: bool,
}

impl DashboardSection {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            collapsed: false,
        }
    }

    pub fn with_items(mut self, items: Vec<DashboardItem>) -> Self {
        self.items = items;
        self
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}

/// Input data for the dashboard widget.
#[derive(Debug, Clone, Default)]
pub struct DashboardData {
    pub sections: Vec<DashboardSection>,
    /// Optional header line (e.g. "MCP — 2/3 connected, 12 tools").
    pub header: Vec<String>,
    /// Optional footer line.
    pub footer: Vec<String>,
}

impl DashboardData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_header(mut self, header: Vec<String>) -> Self {
        self.header = header;
        self
    }

    pub fn with_footer(mut self, footer: Vec<String>) -> Self {
        self.footer = footer;
        self
    }

    pub fn add_section(mut self, section: DashboardSection) -> Self {
        self.sections.push(section);
        self
    }
}

/// Selection / interaction state.
#[derive(Debug, Clone, Default)]
pub struct DashboardState {
    /// Currently focused section.
    pub selected_section: usize,
    /// Currently focused item within the section.
    pub selected_item: usize,
    /// Optional filter substring.
    pub filter: String,
    /// Whether the filter is being edited.
    pub filter_editing: bool,
}

impl DashboardState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move selection to the next item (wraps around within visible items).
    pub fn select_next(&mut self, data: &DashboardData) {
        let visible = visible_item_count(&data.sections, self.selected_section, &self.filter);
        if visible == 0 {
            return;
        }
        if self.selected_item + 1 < visible {
            self.selected_item += 1;
        } else {
            // Wrap to next non-empty section
            let n = data.sections.len();
            for offset in 1..=n {
                let next = (self.selected_section + offset) % n;
                if visible_item_count(&data.sections, next, &self.filter) > 0 {
                    self.selected_section = next;
                    self.selected_item = 0;
                    return;
                }
            }
        }
    }

    /// Move selection to the previous item.
    pub fn select_previous(&mut self, data: &DashboardData) {
        if self.selected_item > 0 {
            self.selected_item -= 1;
            return;
        }
        // Wrap to previous non-empty section
        let n = data.sections.len();
        for offset in 1..=n {
            let prev = (self.selected_section + n - offset) % n;
            if visible_item_count(&data.sections, prev, &self.filter) > 0 {
                self.selected_section = prev;
                self.selected_item = visible_item_count(&data.sections, prev, &self.filter) - 1;
                return;
            }
        }
    }

    /// Toggle filter editing mode.
    pub fn toggle_filter(&mut self) {
        self.filter_editing = !self.filter_editing;
    }

    /// Push a character into the filter.
    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
    }

    /// Pop the last character from the filter.
    pub fn filter_pop(&mut self) {
        self.filter.pop();
    }

    /// Clear the filter.
    pub fn filter_clear(&mut self) {
        self.filter.clear();
    }
}

/// Count the items visible in a section (after collapsing + filter).
fn visible_item_count(sections: &[DashboardSection], idx: usize, filter: &str) -> usize {
    sections
        .get(idx)
        .map(|s| {
            if s.collapsed {
                0
            } else {
                s.items
                    .iter()
                    .filter(|i| filter.is_empty() || item_matches(i, filter))
                    .count()
            }
        })
        .unwrap_or(0)
}

fn item_matches(item: &DashboardItem, filter: &str) -> bool {
    let f = filter.to_lowercase();
    item.label.to_lowercase().contains(&f)
        || item.id.to_lowercase().contains(&f)
        || item.detail.to_lowercase().contains(&f)
}

// ── Widget ─────────────────────────────────────────────────────────────

/// The dashboard widget itself.
pub struct DashboardWidget<'a> {
    data: DashboardData,
    theme: &'a Theme,
}

impl<'a> DashboardWidget<'a> {
    pub fn new(data: DashboardData, theme: &'a Theme) -> Self {
        Self { data, theme }
    }

    pub fn data(&self) -> &DashboardData {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut DashboardData {
        &mut self.data
    }

    pub fn set_data(&mut self, data: DashboardData) {
        self.data = data;
    }
}

impl StatefulWidget for DashboardWidget<'_> {
    type State = DashboardState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let theme = self.theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border))
            .title(Line::from(vec![Span::styled(
                " Dashboard ",
                Style::default()
                    .fg(theme.colors.primary)
                    .add_modifier(Modifier::BOLD),
            )]));
        let inner = block.inner(area);
        block.render(area, buf);

        // Build the rendered lines.
        let mut all: Vec<Line> = Vec::new();

        // Header
        for line in &self.data.header {
            all.push(Line::from(Span::styled(
                line.clone(),
                Style::default()
                    .fg(theme.colors.foreground)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        // Sections
        for (sec_idx, section) in self.data.sections.iter().enumerate() {
            all.push(Line::from(Span::styled(
                format!(
                    "{}{}{} {} ({}) {}",
                    theme.symbols.rule,
                    theme.symbols.rule,
                    theme.symbols.rule,
                    section.title,
                    section.items.len(),
                    if section.collapsed { "[+] " } else { "[-] " }
                ),
                Style::default()
                    .fg(theme.colors.accent)
                    .add_modifier(Modifier::BOLD),
            )));
            if !section.collapsed {
                for (item_idx, item) in section
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| state.filter.is_empty() || item_matches(i, &state.filter))
                {
                    let is_selected =
                        sec_idx == state.selected_section && item_idx == state.selected_item;
                    let mut spans = vec![
                        Span::styled(
                            format!("{} ", item.status.symbol(&theme.symbols)),
                            item.status.style(theme),
                        ),
                        Span::styled(
                            item.label.clone(),
                            if is_selected {
                                Style::default()
                                    .fg(theme.colors.primary)
                                    .bg(theme.colors.selection_bg)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(theme.colors.foreground)
                            },
                        ),
                    ];
                    for b in &item.badges {
                        spans.push(Span::styled(
                            format!(" [{}]", b),
                            Style::default().fg(theme.colors.muted),
                        ));
                    }
                    all.push(Line::from(spans));
                    if !item.detail.is_empty() {
                        all.push(Line::from(Span::styled(
                            format!("    {}", item.detail),
                            Style::default().fg(theme.colors.muted),
                        )));
                    }
                }
            }
        }

        // Footer
        if !self.data.footer.is_empty() {
            all.push(Line::from(""));
            for line in &self.data.footer {
                all.push(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(theme.colors.muted),
                )));
            }
        }

        // Filter display
        if state.filter_editing {
            all.push(Line::from(Span::styled(
                format!("Filter: {}_", state.filter),
                Style::default()
                    .fg(theme.colors.primary)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if !state.filter.is_empty() {
            all.push(Line::from(Span::styled(
                format!("Filter: '{}'", state.filter),
                Style::default().fg(theme.colors.muted),
            )));
        }

        // Truncate to inner area
        let visible: Vec<Line> = all.into_iter().take(inner.height as usize).collect();
        let paragraph = ratatui::widgets::Paragraph::new(visible)
            .style(Style::default())
            .block(Block::default());
        paragraph.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> DashboardData {
        DashboardData::new()
            .with_header(vec!["Test Dashboard".to_string()])
            .add_section(DashboardSection::new("Servers").with_items(vec![
                DashboardItem {
                    id: "a".into(),
                    label: "alpha".into(),
                    detail: "first".into(),
                    status: ItemStatus::Active,
                    badges: vec!["eager".into()],
                },
                DashboardItem {
                    id: "b".into(),
                    label: "beta".into(),
                    detail: "second".into(),
                    status: ItemStatus::Inactive,
                    badges: vec!["lazy".into()],
                },
            ]))
            .add_section(
                DashboardSection::new("Tools").with_items(vec![DashboardItem {
                    id: "t1".into(),
                    label: "tool_one".into(),
                    detail: "".into(),
                    status: ItemStatus::Active,
                    badges: vec!["DIRECT".into()],
                }]),
            )
    }

    #[test]
    fn select_next_wraps_sections() {
        let data = sample_data();
        let mut state = DashboardState::new();
        state.selected_section = 0;
        state.selected_item = 0;
        state.select_next(&data);
        assert_eq!(state.selected_section, 0);
        assert_eq!(state.selected_item, 1);
        state.select_next(&data);
        // Section 1, item 0 (after wrap)
        assert_eq!(state.selected_section, 1);
        assert_eq!(state.selected_item, 0);
        state.select_next(&data);
        // Wraps back to section 0
        assert_eq!(state.selected_section, 0);
        assert_eq!(state.selected_item, 0);
    }

    #[test]
    fn filter_matches_label() {
        assert!(item_matches(
            &DashboardItem {
                id: "x".into(),
                label: "chrome-devtools".into(),
                detail: "".into(),
                status: ItemStatus::Active,
                badges: vec![],
            },
            "Chrome"
        ));
        assert!(!item_matches(
            &DashboardItem {
                id: "x".into(),
                label: "github".into(),
                detail: "".into(),
                status: ItemStatus::Active,
                badges: vec![],
            },
            "chrome"
        ));
    }

    #[test]
    fn collapsed_section_has_no_visible_items() {
        let mut data = sample_data();
        data.sections[0].collapsed = true;
        assert_eq!(visible_item_count(&data.sections, 0, ""), 0);
        assert_eq!(visible_item_count(&data.sections, 1, ""), 1);
    }
}
