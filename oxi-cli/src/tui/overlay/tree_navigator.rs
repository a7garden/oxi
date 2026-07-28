//! Interactive tree navigator overlay for the `/tree` command.
//!
//! Replaces the text-only dump with a navigable, foldable tree view
//! of session entries. Supports filtering by entry type and text search.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::store::session::{AgentMessage, SessionEntry};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_tui::Theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

use super::{OverlayAction, OverlayComponent, centered_layout};
use crate::app::agent_session::{AgentSession, AgentSessionHandle};

// ─────────────────────────────────────────────────────────────────────────
// SharedAppState (same pattern as factories.rs)
// ─────────────────────────────────────────────────────────────────────────

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

// ─────────────────────────────────────────────────────────────────────────
// Entry type discriminator
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryType {
    User,
    Assistant,
    Tool,
    System,
}

impl EntryType {
    fn from_message(msg: &AgentMessage) -> Self {
        match msg {
            AgentMessage::User { .. } => EntryType::User,
            AgentMessage::Assistant { .. } => EntryType::Assistant,
            AgentMessage::ToolResult { .. } => EntryType::Tool,
            AgentMessage::BashExecution { .. } => EntryType::Tool,
            AgentMessage::System { .. } => EntryType::System,
            AgentMessage::BranchSummary { .. } => EntryType::System,
            AgentMessage::CompactionSummary { .. } => EntryType::System,
            AgentMessage::Custom { .. } => EntryType::System,
        }
    }

    fn label(self) -> &'static str {
        match self {
            EntryType::User => "user",
            EntryType::Assistant => "assistant",
            EntryType::Tool => "tool",
            EntryType::System => "system",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Filter mode
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterMode {
    Default,
    NoTools,
    UserOnly,
    All,
}

impl FilterMode {
    fn cycle(self) -> Self {
        match self {
            FilterMode::Default => FilterMode::NoTools,
            FilterMode::NoTools => FilterMode::UserOnly,
            FilterMode::UserOnly => FilterMode::All,
            FilterMode::All => FilterMode::Default,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FilterMode::Default => "default",
            FilterMode::NoTools => "no-tools",
            FilterMode::UserOnly => "user-only",
            FilterMode::All => "all",
        }
    }

    /// Whether to include an entry of the given type.
    fn includes(self, entry_type: EntryType) -> bool {
        match self {
            FilterMode::Default | FilterMode::All => true,
            FilterMode::NoTools => entry_type != EntryType::Tool,
            FilterMode::UserOnly => {
                entry_type == EntryType::User || entry_type == EntryType::Assistant
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// FlatNode — a single row in the flattened tree
// ─────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
struct FlatNode {
    entry_id: String,
    #[allow(dead_code)]
    parent_id: Option<String>,
    indent: usize,
    is_last_child: bool,
    is_folded: bool,
    has_children: bool,
    content: Line<'static>,
    #[allow(dead_code)]
    entry_type: EntryType,
}

// ─────────────────────────────────────────────────────────────────────────
// TreeNavigatorOverlay
// ─────────────────────────────────────────────────────────────────────────

pub struct TreeNavigatorOverlay {
    list_state: ListState,
    flat_nodes: Vec<FlatNode>,
    all_entries: Vec<SessionEntry>,
    folded: HashSet<String>,
    filter_mode: FilterMode,
    search_query: String,
    active_path: HashSet<String>,
    #[allow(dead_code)]
    current_leaf_id: Option<String>,
    #[allow(dead_code)]
    session_handle: AgentSessionHandle,
    #[allow(dead_code)]
    app_state: SharedAppState,
}

impl std::fmt::Debug for TreeNavigatorOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeNavigatorOverlay")
            .field("filter_mode", &self.filter_mode)
            .field("search_query", &self.search_query)
            .field("flat_nodes_count", &self.flat_nodes.len())
            .field("all_entries_count", &self.all_entries.len())
            .finish()
    }
}

impl TreeNavigatorOverlay {
    /// Create a new tree navigator overlay.
    pub fn new(
        entries: Vec<SessionEntry>,
        current_leaf: Option<String>,
        session_handle: AgentSessionHandle,
        app_state: SharedAppState,
    ) -> Self {
        let active_path = compute_active_path(&entries, current_leaf.as_deref());

        let mut overlay = Self {
            list_state: ListState::default(),
            flat_nodes: Vec::new(),
            all_entries: entries,
            folded: HashSet::new(),
            filter_mode: FilterMode::Default,
            search_query: String::new(),
            active_path,
            current_leaf_id: current_leaf,
            session_handle,
            app_state,
        };
        overlay.build_flat_tree();

        // Select first node if available
        if !overlay.flat_nodes.is_empty() {
            overlay.list_state.select(Some(0));
        }

        overlay
    }

    // ── Tree building ────────────────────────────────────────────────

    fn build_flat_tree(&mut self) {
        // Build adjacency: parent_id -> Vec<child entries>
        let mut children_map: HashMap<Option<String>, Vec<&SessionEntry>> = HashMap::new();
        for entry in &self.all_entries {
            let entry_type = EntryType::from_message(&entry.message);
            if !self.filter_mode.includes(entry_type) {
                continue;
            }
            // Apply search filter
            if !self.search_query.is_empty() {
                let content = entry.content();
                if !content
                    .to_lowercase()
                    .contains(&self.search_query.to_lowercase())
                {
                    continue;
                }
            }
            children_map
                .entry(entry.parent_id.clone())
                .or_default()
                .push(entry);
        }

        // Sort children by timestamp for deterministic order
        for children in children_map.values_mut() {
            children.sort_by_key(|e| e.timestamp);
        }

        self.flat_nodes.clear();

        // Find roots (parent_id = None or parent not in our filtered set)
        let filtered_ids: HashSet<String> = self
            .all_entries
            .iter()
            .filter(|e| {
                let et = EntryType::from_message(&e.message);
                self.filter_mode.includes(et)
            })
            .map(|e| e.id.clone())
            .collect();

        let roots = children_map.get(&None).cloned().unwrap_or_default();

        // Recursively flatten
        for (i, root) in roots.iter().enumerate() {
            let is_last = i == roots.len() - 1;
            let has_children = children_map
                .get(&Some(root.id.clone()))
                .is_some_and(|c| !c.is_empty());
            let is_folded = self.folded.contains(&root.id);

            self.flat_nodes.push(FlatNode {
                entry_id: root.id.clone(),
                parent_id: root.parent_id.clone(),
                indent: 0,
                is_last_child: is_last,
                is_folded: is_folded && has_children,
                has_children,
                content: make_content_line(root, &self.active_path),
                entry_type: EntryType::from_message(&root.message),
            });

            if has_children && !is_folded {
                // Pass current indent (0); flatten_children_impl decides
                // whether to increase based on child count.
                let next_indent = if roots.len() > 1 { 1 } else { 0 };
                Self::flatten_children_impl(
                    &root.id,
                    next_indent,
                    &children_map,
                    is_last,
                    &filtered_ids,
                    &self.folded,
                    &self.active_path,
                    &mut self.flat_nodes,
                );
            }
        }

        // Also pick up roots that had a parent_id pointing to a non-root
        // that got filtered out. Treat their parent as None.
        let root_ids: HashSet<String> = roots.iter().map(|r| r.id.clone()).collect();
        for entry in &self.all_entries {
            if root_ids.contains(&entry.id) {
                continue;
            }
            let entry_type = EntryType::from_message(&entry.message);
            if !self.filter_mode.includes(entry_type) {
                continue;
            }
            if !self.search_query.is_empty() {
                let content = entry.content();
                if !content
                    .to_lowercase()
                    .contains(&self.search_query.to_lowercase())
                {
                    continue;
                }
            }
            // If parent is not in filtered set, it's an orphan root
            let is_orphan = match &entry.parent_id {
                Some(pid) => !filtered_ids.contains(pid),
                None => true,
            };
            if is_orphan && !root_ids.contains(&entry.id) {
                let has_children = children_map
                    .get(&Some(entry.id.clone()))
                    .is_some_and(|c| !c.is_empty());
                let is_folded = self.folded.contains(&entry.id);
                self.flat_nodes.push(FlatNode {
                    entry_id: entry.id.clone(),
                    parent_id: entry.parent_id.clone(),
                    indent: 0,
                    is_last_child: true,
                    is_folded: is_folded && has_children,
                    has_children,
                    content: make_content_line(entry, &self.active_path),
                    entry_type: EntryType::from_message(&entry.message),
                });
            }
        }
    }

    /// Static version that accepts explicit parameters to avoid borrow conflicts.
    ///
    /// Linear chains (single-child) are collapsed at the same indent level so
    /// the tree only deepens at actual branch points (≥2 children).
    #[allow(clippy::too_many_arguments)]
    fn flatten_children_impl(
        parent_id: &str,
        indent: usize,
        children_map: &HashMap<Option<String>, Vec<&SessionEntry>>,
        _parent_is_last: bool,
        _filtered_ids: &HashSet<String>,
        folded: &HashSet<String>,
        active_path: &HashSet<String>,
        flat_nodes: &mut Vec<FlatNode>,
    ) {
        let children = match children_map.get(&Some(parent_id.to_string())) {
            Some(c) => c,
            None => return,
        };

        // Linear chains (single child) stay at the same indent level.
        // Only increase indent at actual branch points (≥2 children).
        let child_indent = if children.len() > 1 {
            indent + 1
        } else {
            indent
        };

        for (i, child) in children.iter().enumerate() {
            let is_last = i == children.len() - 1;
            let has_children = children_map
                .get(&Some(child.id.clone()))
                .is_some_and(|c| !c.is_empty());
            let is_folded = folded.contains(&child.id);

            flat_nodes.push(FlatNode {
                entry_id: child.id.clone(),
                parent_id: child.parent_id.clone(),
                indent: child_indent,
                is_last_child: is_last,
                is_folded: is_folded && has_children,
                has_children,
                content: make_content_line(child, active_path),
                entry_type: EntryType::from_message(&child.message),
            });

            if has_children && !is_folded {
                Self::flatten_children_impl(
                    &child.id,
                    child_indent,
                    children_map,
                    is_last,
                    _filtered_ids,
                    folded,
                    active_path,
                    flat_nodes,
                );
            }
        }
    }

    // ── Tree connectors ──────────────────────────────────────────────

    fn build_connector(node: &FlatNode) -> String {
        if node.indent == 0 {
            return String::new();
        }

        let mut connector = String::new();
        for level in 0..node.indent {
            if level < node.indent - 1 {
                // Intermediate levels: "│   " or "    "
                // We don't track per-level is_last, so use "│   " for all intermediates
                connector.push_str("│   ");
            } else {
                // Last level: "├─ " or "└─ "
                if node.is_last_child {
                    connector.push_str("└─ ");
                } else {
                    connector.push_str("├─ ");
                }
            }
        }
        connector
    }

    // ── Actions ──────────────────────────────────────────────────────

    fn toggle_fold(&mut self) {
        if let Some(node) = self.selected_node() {
            let id = node.entry_id.clone();
            if node.has_children {
                if self.folded.contains(&id) {
                    self.folded.remove(&id);
                } else {
                    self.folded.insert(id);
                }
                self.rebuild();
            }
        }
    }

    fn rebuild(&mut self) {
        let selected_id = self.selected_node().map(|n| n.entry_id.clone());
        self.build_flat_tree();
        // Re-select the same node if it still exists
        if let Some(id) = selected_id {
            let idx = self.flat_nodes.iter().position(|n| n.entry_id == id);
            if let Some(i) = idx {
                self.list_state.select(Some(i));
            } else if !self.flat_nodes.is_empty() {
                self.list_state.select(Some(0));
            }
        }
    }

    fn selected_node(&self) -> Option<&FlatNode> {
        self.list_state
            .selected()
            .and_then(|i| self.flat_nodes.get(i))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Helper functions
// ─────────────────────────────────────────────────────────────────────────

fn compute_active_path(entries: &[SessionEntry], leaf_id: Option<&str>) -> HashSet<String> {
    let mut path = HashSet::new();
    let Some(mut current_id) = leaf_id.map(|s| s.to_string()) else {
        return path;
    };

    let by_id: HashMap<&str, &SessionEntry> = entries.iter().map(|e| (e.id.as_str(), e)).collect();

    while let Some(entry) = by_id.get(current_id.as_str()) {
        path.insert(entry.id.clone());
        match &entry.parent_id {
            Some(pid) => current_id = pid.clone(),
            None => break,
        }
    }

    path
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

fn make_content_line(entry: &SessionEntry, active_path: &HashSet<String>) -> Line<'static> {
    let entry_type = EntryType::from_message(&entry.message);
    let is_active = active_path.contains(&entry.id);
    let content_text = entry.content();
    let display = truncate_str(&content_text, 80);

    let mut spans: Vec<Span<'static>> = Vec::new();

    // Active path marker
    if is_active {
        spans.push(Span::styled(
            "• ".to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }

    // Type tag
    let tag_style = match entry_type {
        EntryType::User => Style::default()
            .fg(ratatui::style::Color::Cyan)
            .add_modifier(Modifier::BOLD),
        EntryType::Assistant => Style::default(),
        EntryType::Tool => Style::default().fg(ratatui::style::Color::DarkGray),
        EntryType::System => Style::default().fg(ratatui::style::Color::Yellow),
    };

    spans.push(Span::styled(
        format!("[{}] ", entry_type.label()),
        tag_style,
    ));

    // Content
    let content_style = match entry_type {
        EntryType::User => Style::default()
            .fg(ratatui::style::Color::Cyan)
            .add_modifier(Modifier::BOLD),
        EntryType::Assistant => Style::default(),
        EntryType::Tool => Style::default().fg(ratatui::style::Color::DarkGray),
        EntryType::System => Style::default().fg(ratatui::style::Color::Yellow),
    };

    spans.push(Span::styled(display, content_style));

    Line::from(spans)
}

// ─────────────────────────────────────────────────────────────────────────
// OverlayComponent implementation
// ─────────────────────────────────────────────────────────────────────────

impl OverlayComponent for TreeNavigatorOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        match key.code {
            KeyCode::Up => {
                self.list_state.select_previous();
            }
            KeyCode::Down => {
                self.list_state.select_next();
            }
            KeyCode::PageUp => {
                self.list_state.scroll_up_by(10);
            }
            KeyCode::PageDown => {
                self.list_state.scroll_down_by(10);
            }
            KeyCode::Home if !self.flat_nodes.is_empty() => {
                self.list_state.select(Some(0));
            }
            KeyCode::End if !self.flat_nodes.is_empty() => {
                self.list_state.select(Some(self.flat_nodes.len() - 1));
            }
            KeyCode::Enter => {
                return OverlayAction::Close;
            }
            KeyCode::Char('f') => {
                self.toggle_fold();
            }
            KeyCode::Char('/') => {
                self.filter_mode = self.filter_mode.cycle();
                self.rebuild();
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.rebuild();
            }
            KeyCode::Esc => {
                return OverlayAction::Close;
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.rebuild();
            }
            _ => {}
        }

        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.9, 0.9);
        frame.render_widget(Clear, popup);

        // Title bar
        let title_text = format!(
            " Session Tree  │  filter: {}  │  search: {} ",
            self.filter_mode.label(),
            if self.search_query.is_empty() {
                "<none>"
            } else {
                &self.search_query
            }
        );
        let title_line = Line::styled(
            title_text,
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD),
        );

        let border_block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));

        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Build list items
        let items: Vec<ListItem> = self
            .flat_nodes
            .iter()
            .map(|node| {
                let connector = Self::build_connector(node);

                // Fold marker
                let fold_marker = if node.has_children {
                    if node.is_folded { "⊞ " } else { "⊟ " }
                } else {
                    "  "
                };

                let mut line_spans: Vec<Span<'static>> = Vec::new();

                // Connector (indent lines)
                line_spans.push(Span::styled(
                    connector,
                    Style::default().fg(ratatui::style::Color::DarkGray),
                ));

                // Fold marker
                line_spans.push(Span::styled(
                    fold_marker.to_string(),
                    Style::default().fg(ratatui::style::Color::DarkGray),
                ));

                // Content spans from node
                for span in node.content.spans.iter() {
                    line_spans.push(span.clone());
                }

                ListItem::new(Line::from(line_spans))
            })
            .collect();

        let highlight_style = Style::default()
            .fg(theme.colors.background)
            .bg(theme.colors.primary)
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_symbol(theme.symbols.cursor)
            .scroll_padding(3);

        frame.render_stateful_widget(list, inner, &mut self.list_state);

        // Bottom hint bar
        let hint_text = format!(
            " {} entries  │  Up/Down navigate  │  f fold  │  / filter  │  Enter select  │  Esc close ",
            self.flat_nodes.len()
        );
        frame.render_widget(
            ratatui::widgets::Paragraph::new(Span::styled(hint_text, styles.muted)),
            Rect {
                x: popup.x + 1,
                y: popup.y + popup.height.saturating_sub(1),
                width: popup.width.saturating_sub(2),
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " Up/Down navigate | f fold | / filter | Enter select | Esc close"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Factory function
// ─────────────────────────────────────────────────────────────────────────

#[allow(clippy::arc_with_non_send_sync)]
pub fn tree_navigator(
    entries: Vec<SessionEntry>,
    current_leaf: Option<String>,
    session: &AgentSession,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    let shared = Arc::new(Mutex::new(app_state as *mut _));
    Box::new(TreeNavigatorOverlay::new(
        entries,
        current_leaf,
        session.clone_handle(),
        shared,
    ))
}
