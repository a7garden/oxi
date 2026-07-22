//! Interactive extensions overlay for the `/extensions` command.
//!
//! Displays all tools and extensions grouped by category (essential, optional, WASM)
//! with toggle support for optional tools and detailed info for each entry.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, ListItem, ListState, Paragraph},
};

use super::{OverlayAction, OverlayComponent, centered_layout};
use crate::tui::app::NotificationKind;

// ─────────────────────────────────────────────────────────────────────────
// Extension/tool entry
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum EntryCategory {
    Essential,
    Optional,
    Wasm,
}

#[derive(Debug, Clone)]
struct ExtensionEntry {
    name: String,
    label: String,
    description: String,
    category: EntryCategory,
    enabled: bool,
    /// Whether this tool can be toggled on/off
    toggleable: bool,
}

impl ExtensionEntry {
    fn status_icon(&self, symbols: &oxi_tui_legacy::Symbols) -> &str {
        if !self.toggleable || self.enabled {
            symbols.dot_on
        } else {
            symbols.dot_off
        }
    }

    fn status_color(&self, theme: &oxi_tui_legacy::Theme) -> ratatui::style::Color {
        if !self.toggleable {
            theme.colors.muted
        } else if self.enabled {
            theme.colors.success
        } else {
            theme.colors.error
        }
    }

    fn category_label(&self) -> &str {
        match self.category {
            EntryCategory::Essential => "essential",
            EntryCategory::Optional => "optional",
            EntryCategory::Wasm => "wasm",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// List row — either a section header or a tool entry
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum ListRow {
    Section { label: &'static str },
    Entry { entry_idx: usize },
}

// ─────────────────────────────────────────────────────────────────────────
// Extensions overlay
// ─────────────────────────────────────────────────────────────────────────

pub fn extensions_overlay(
    session: &crate::app::agent_session::AgentSession,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    let entries = collect_entries(session);
    let session = session.clone_handle();
    let active_count = entries.iter().filter(|e| e.enabled).count();

    let rows = build_rows(&entries);

    Box::new(ExtensionsOverlay {
        all_entries: entries,
        rows,
        selected: 0,
        filter: String::new(),
        session,
        app_state: share_state(app_state),
        active_count,
        detail_scroll: 0,
    })
}

struct ExtensionsOverlay {
    all_entries: Vec<ExtensionEntry>,
    rows: Vec<ListRow>,
    selected: usize,
    filter: String,
    session: crate::app::agent_session::AgentSessionHandle,
    app_state: SharedAppState,
    active_count: usize,
    detail_scroll: usize,
}

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

#[allow(clippy::arc_with_non_send_sync)]
fn share_state(state: &mut crate::tui::app::AppState) -> SharedAppState {
    Arc::new(Mutex::new(state as *mut _))
}

impl std::fmt::Debug for ExtensionsOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionsOverlay")
            .field("entries", &self.all_entries.len())
            .field("rows", &self.rows.len())
            .field("active", &self.active_count)
            .finish()
    }
}

impl ExtensionsOverlay {
    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.rows = build_rows(&self.all_entries);
        } else {
            let lower = self.filter.to_lowercase();
            let filtered_indices: Vec<usize> = self
                .all_entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    entry.name.to_lowercase().contains(&lower)
                        || entry.label.to_lowercase().contains(&lower)
                        || entry.description.to_lowercase().contains(&lower)
                })
                .map(|(i, _)| i)
                .collect();

            self.rows = build_rows_from_indices(&self.all_entries, &filtered_indices);
        }
        self.selected = 0;
        self.detail_scroll = 0;
    }

    fn entry_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, ListRow::Entry { .. }))
            .count()
    }

    /// Get the entry index of the nth selectable (Entry) row.
    fn selected_entry_idx(&self) -> Option<usize> {
        let mut entry_count = 0;
        for row in &self.rows {
            if let ListRow::Entry { entry_idx } = row {
                if entry_count == self.selected {
                    return Some(*entry_idx);
                }
                entry_count += 1;
            }
        }
        None
    }

    fn selected_entry(&self) -> Option<&ExtensionEntry> {
        let idx = self.selected_entry_idx()?;
        self.all_entries.get(idx)
    }

    fn toggle_selected(&mut self) {
        let Some(entry_idx) = self.selected_entry_idx() else {
            return;
        };
        let entry = &mut self.all_entries[entry_idx];

        if !entry.toggleable {
            return;
        }

        let registry = self.session.agent_ref().tools();
        let entry_name = entry.name.clone();

        if entry.enabled {
            // Disable
            if registry.get(&entry_name).is_some() {
                if let Some(tool) = registry.get(&entry_name)
                    && tool.essential()
                {
                    return;
                }
                registry.unregister(&entry_name);
                // Handle paired tools
                if entry_name == "web_search" {
                    registry.unregister("get_search_results");
                }
            }
            entry.enabled = false;
            self.active_count = self.all_entries.iter().filter(|e| e.enabled).count();
            self.notify(format!("Disabled: {}", entry_name), NotificationKind::Info);
        } else {
            // Enable
            let re_registered = try_re_register_tool(&entry_name, &registry);
            if re_registered {
                entry.enabled = true;
                self.active_count = self.all_entries.iter().filter(|e| e.enabled).count();
                self.notify(
                    format!("Enabled: {}", entry_name),
                    NotificationKind::Success,
                );
            } else {
                self.notify(
                    format!("Cannot re-enable: {}", entry_name),
                    NotificationKind::Warning,
                );
            }
        }
    }

    fn notify(&self, message: String, kind: NotificationKind) {
        if let Ok(ptr) = self.app_state.lock() {
            // SAFETY: We hold the Mutex lock, guaranteeing exclusive access.
            unsafe {
                if let Some(ref mut app) = (*ptr).as_mut() {
                    app.add_notification(message, kind);
                }
            }
        }
    }
}

/// Build the list rows from all entries with section headers.
fn build_rows(entries: &[ExtensionEntry]) -> Vec<ListRow> {
    let indices: Vec<usize> = (0..entries.len()).collect();
    build_rows_from_indices(entries, &indices)
}

/// Build the list rows from a subset of entry indices, inserting section headers.
fn build_rows_from_indices(entries: &[ExtensionEntry], indices: &[usize]) -> Vec<ListRow> {
    let mut rows = Vec::new();
    let mut current_section: Option<&str> = None;

    for &idx in indices {
        if let Some(entry) = entries.get(idx) {
            let section = match entry.category {
                EntryCategory::Essential => "ESSENTIAL",
                EntryCategory::Optional => "OPTIONAL",
                EntryCategory::Wasm => "WASM EXTENSIONS",
            };

            if current_section != Some(section) {
                current_section = Some(section);
                rows.push(ListRow::Section { label: section });
            }

            rows.push(ListRow::Entry { entry_idx: idx });
        }
    }

    rows
}

/// Collect tool entries from the session's tool registry.
fn collect_entries(session: &crate::app::agent_session::AgentSession) -> Vec<ExtensionEntry> {
    let registry = session.agent_ref().tools();
    let names = registry.names();

    let builtin_names: std::collections::HashSet<&str> = [
        "read",
        "write",
        "edit",
        "bash",
        "grep",
        "find",
        "ls",
        "web_search",
        "get_search_results",
        "github",
        "subagent",
    ]
    .into_iter()
    .collect();

    let mut entries = Vec::new();

    for name in &names {
        if let Some(tool) = registry.get(name) {
            let is_essential = tool.essential();
            let is_builtin = builtin_names.contains(name.as_str());

            let category = if is_essential {
                EntryCategory::Essential
            } else if is_builtin {
                EntryCategory::Optional
            } else {
                EntryCategory::Wasm
            };

            entries.push(ExtensionEntry {
                name: name.clone(),
                label: tool.label().to_string(),
                description: tool.description().to_string(),
                category,
                enabled: true,
                toggleable: !is_essential,
            });
        }
    }

    // Sort: essential first, then optional, then wasm; alphabetical within each
    entries.sort_by(|a, b| {
        let ord = |cat: &EntryCategory| match cat {
            EntryCategory::Essential => 0,
            EntryCategory::Optional => 1,
            EntryCategory::Wasm => 2,
        };
        ord(&a.category)
            .cmp(&ord(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });

    entries
}

/// Try to re-register a previously disabled built-in tool.
fn try_re_register_tool(name: &str, registry: &std::sync::Arc<oxi_agent::ToolRegistry>) -> bool {
    use std::sync::Arc;

    match name {
        "read" | "write" | "edit" | "bash" | "grep" | "find" | "ls" => false,
        "web_search" => {
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::WebSearchTool::new(cache.clone()));
            registry.register(oxi_agent::GetSearchResultsTool::new(cache));
            true
        }
        "get_search_results" => {
            if registry.get("web_search").is_some() {
                false
            } else {
                let cache = Arc::new(oxi_agent::SearchCache::new());
                registry.register(oxi_agent::GetSearchResultsTool::new(cache));
                true
            }
        }
        "github" | "github_search" => {
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::GitHubTool::new(cache));
            true
        }
        "subagent" => {
            registry.register(oxi_agent::SubagentTool::with_cwd(std::path::PathBuf::from(
                ".",
            )));
            true
        }
        _ => false,
    }
}

impl OverlayComponent for ExtensionsOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        let count = self.entry_count();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = if count == 0 {
                    0
                } else {
                    self.selected.saturating_sub(1)
                };
                self.detail_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(count.saturating_sub(1));
                self.detail_scroll = 0;
            }
            KeyCode::PageUp => {
                self.selected = self
                    .selected
                    .saturating_sub(10)
                    .min(count.saturating_sub(1));
                self.detail_scroll = 0;
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + 10).min(count.saturating_sub(1));
                self.detail_scroll = 0;
            }
            KeyCode::Home => {
                self.selected = 0;
                self.detail_scroll = 0;
            }
            KeyCode::End => {
                self.selected = count.saturating_sub(1);
                self.detail_scroll = 0;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.toggle_selected();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                return OverlayAction::Close;
            }
            KeyCode::Char(c) if c != ' ' => {
                self.filter.push(c);
                self.apply_filter();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter();
            }
            KeyCode::Tab => {
                self.detail_scroll = self.detail_scroll.saturating_add(3);
            }
            KeyCode::BackTab => {
                self.detail_scroll = self.detail_scroll.saturating_sub(3);
            }
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui_legacy::Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.88, 0.88);
        frame.render_widget(Clear, popup);

        let total = self.all_entries.len();
        let active = self.active_count;
        let title_text = if self.filter.is_empty() {
            format!(" Extensions ({}/{}) ", active, total)
        } else {
            format!(
                " Extensions ({}/{}) filter: {} ",
                active, total, self.filter
            )
        };

        let border_block = Block::default()
            .title(title_line(&title_text))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Split inner into left (list) and right (detail) panels
        let list_width = (inner.width as f32 * 0.55).min(60.0) as u16;
        let detail_width = inner.width.saturating_sub(list_width).saturating_sub(2);

        let list_area = Rect {
            x: inner.x + 1,
            y: inner.y + 1,
            width: list_width,
            height: inner.height.saturating_sub(2),
        };

        // Vertical separator
        let sep_x = inner.x + list_width + 1;
        for y in (inner.y + 1)..(inner.y + inner.height.saturating_sub(1)) {
            frame.render_widget(
                Paragraph::new(Span::styled("│", Style::default().fg(theme.colors.border))),
                Rect {
                    x: sep_x,
                    y,
                    width: 1,
                    height: 1,
                },
            );
        }

        let detail_area = Rect {
            x: sep_x + 2,
            y: inner.y + 1,
            width: detail_width.saturating_sub(3),
            height: inner.height.saturating_sub(2),
        };

        // ── Build list items with section headers ──
        let list_items = self.build_list_items(theme, list_width);
        let highlight_style = Style::default()
            .fg(theme.colors.background)
            .bg(theme.colors.primary);

        let list_widget = ratatui::widgets::List::new(list_items)
            .highlight_style(highlight_style)
            .scroll_padding(2);

        let mut list_state = ListState::default();
        // Map logical selection (counting only entries) to list row index (including headers)
        let row_idx = self.selected_to_row_index();
        list_state.select(Some(row_idx));
        frame.render_stateful_widget(list_widget, list_area, &mut list_state);

        // ── Detail panel ──
        if let Some(entry) = self.selected_entry() {
            render_detail_panel(
                frame,
                detail_area,
                entry,
                theme,
                &styles,
                self.detail_scroll,
            );
        } else {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    " Select an extension to view details",
                    Style::default().fg(theme.colors.muted),
                )),
                detail_area,
            );
        }

        // ── Footer hint ──
        let hint = if self.filter.is_empty() {
            " \u{2191}/\u{2193} navigate  |  Enter/Space toggle  |  Type to filter  |  Esc close"
        } else {
            " \u{2191}/\u{2193} navigate  |  Enter/Space toggle  |  Backspace clear filter  |  Esc close"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            Rect {
                x: inner.x + 1,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " \u{2191}/\u{2193}  |  Enter toggle  |  Esc close"
    }
}

impl ExtensionsOverlay {
    /// Build the styled list items for rendering.
    fn build_list_items(
        &self,
        theme: &oxi_tui_legacy::Theme,
        list_width: u16,
    ) -> Vec<ListItem<'static>> {
        let mut items = Vec::new();

        for row in &self.rows {
            match row {
                ListRow::Section { label } => {
                    let section_style = Style::default()
                        .fg(theme.colors.primary)
                        .add_modifier(Modifier::BOLD);
                    items.push(ListItem::new(Line::from(vec![Span::styled(
                        format!(" {} ", label),
                        section_style,
                    )])));
                }
                ListRow::Entry { entry_idx } => {
                    let entry = &self.all_entries[*entry_idx];
                    let is_selected = Some(*entry_idx) == self.selected_entry_idx();
                    let icon = entry.status_icon(&theme.symbols);
                    let icon_color = entry.status_color(theme);

                    let name_style = if is_selected {
                        Style::default()
                            .fg(theme.colors.background)
                            .bg(theme.colors.primary)
                            .add_modifier(Modifier::BOLD)
                    } else if entry.enabled {
                        Style::default().fg(theme.colors.foreground)
                    } else {
                        Style::default().fg(theme.colors.muted)
                    };

                    let tag_style = if is_selected {
                        Style::default()
                            .fg(theme.colors.background)
                            .bg(theme.colors.primary)
                    } else {
                        Style::default().fg(theme.colors.muted)
                    };

                    let max_label_len = (list_width as usize).saturating_sub(16);
                    let truncated_label: String = entry.name.chars().take(max_label_len).collect();
                    let padded_label =
                        format!("{:<width$}", truncated_label, width = max_label_len);

                    let mut spans = vec![
                        Span::styled(format!(" {} ", icon), Style::default().fg(icon_color)),
                        Span::styled(padded_label, name_style),
                    ];

                    if entry.toggleable {
                        let toggle_text = if entry.enabled { " on " } else { " off" };
                        spans.push(Span::styled(toggle_text, tag_style));
                    } else {
                        spans.push(Span::styled(
                            " fix",
                            Style::default().fg(theme.colors.muted),
                        ));
                    }

                    items.push(ListItem::new(Line::from(spans)));
                }
            }
        }

        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                " No matching extensions",
                Style::default().fg(theme.colors.muted),
            ))));
        }

        items
    }

    /// Convert the logical entry selection index to the row index in the list
    /// (accounting for section headers).
    fn selected_to_row_index(&self) -> usize {
        let target = self.selected;
        let mut entry_count = 0;
        for (i, row) in self.rows.iter().enumerate() {
            if let ListRow::Entry { .. } = row {
                if entry_count == target {
                    return i;
                }
                entry_count += 1;
            }
        }
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Detail panel renderer
// ─────────────────────────────────────────────────────────────────────────

fn render_detail_panel(
    frame: &mut Frame,
    area: Rect,
    entry: &ExtensionEntry,
    theme: &oxi_tui_legacy::Theme,
    styles: &oxi_tui_legacy::ThemeStyles,
    scroll: usize,
) {
    let title_style = Style::default()
        .fg(theme.colors.primary)
        .add_modifier(Modifier::BOLD);

    let status_text = if entry.enabled { "enabled" } else { "disabled" };
    let status_color = entry.status_color(theme);

    // Header: icon + name + status badge
    let header = Line::from(vec![
        Span::styled(
            format!(" {} ", entry.status_icon(&theme.symbols)),
            Style::default().fg(status_color),
        ),
        Span::styled(&entry.name, title_style),
        Span::styled(
            format!("  [{}]", status_text),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), area);

    // Category badge
    let cat_color = match entry.category {
        EntryCategory::Essential => theme.colors.muted,
        EntryCategory::Optional => theme.colors.accent,
        EntryCategory::Wasm => theme.colors.warning,
    };
    let badge_line = Line::from(vec![Span::styled(
        format!(" {} ", entry.category_label().to_uppercase()),
        Style::default()
            .fg(theme.colors.background)
            .bg(cat_color)
            .add_modifier(Modifier::BOLD),
    )]);
    frame.render_widget(
        Paragraph::new(badge_line),
        Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: 1,
        },
    );

    // Separator line
    frame.render_widget(
        Paragraph::new(Span::styled(
            "\u{2500}".repeat(area.width as usize),
            Style::default().fg(theme.colors.border),
        )),
        Rect {
            x: area.x,
            y: area.y + 2,
            width: area.width,
            height: 1,
        },
    );

    // Label row
    let desc_start = 3u16;
    if !entry.label.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Label:  ", Style::default().fg(theme.colors.muted)),
                Span::styled(&entry.label, styles.normal),
            ])),
            Rect {
                x: area.x,
                y: area.y + desc_start,
                width: area.width,
                height: 1,
            },
        );
    }

    // Full description with word wrap
    if !entry.description.is_empty() {
        let desc_y = area.y + desc_start + 1;
        let desc_height = area.height.saturating_sub(desc_start + 1);

        let desc_lines: Vec<Line> = wrap_text(&entry.description, area.width as usize);
        let visible_start = scroll.min(desc_lines.len().saturating_sub(1));
        let visible_end = (visible_start + desc_height as usize).min(desc_lines.len());
        let visible: Vec<Line> = desc_lines[visible_start..visible_end].to_vec();

        frame.render_widget(
            Paragraph::new(visible),
            Rect {
                x: area.x,
                y: desc_y,
                width: area.width,
                height: desc_height,
            },
        );
    }

    // Toggle action hint at the bottom of the detail panel
    if entry.toggleable {
        let hint_y = area.y + area.height.saturating_sub(2);
        let action = if entry.enabled { "disable" } else { "enable" };
        let hint_style = Style::default()
            .fg(theme.colors.accent)
            .add_modifier(Modifier::ITALIC);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" Press Enter to {}", action),
                hint_style,
            )),
            Rect {
                x: area.x,
                y: hint_y,
                width: area.width,
                height: 1,
            },
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

fn title_line(text: &str) -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        text.to_string(),
        Style::default()
            .fg(ratatui::style::Color::White)
            .bg(ratatui::style::Color::Rgb(30, 40, 60))
            .add_modifier(Modifier::BOLD),
    )
}

/// Word-wrap text to a given width, returning styled lines.
fn wrap_text(text: &str, max_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(Line::raw(String::new()));
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(Line::raw(std::mem::take(&mut current)));
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(Line::raw(current));
        }
    }
    lines
}
