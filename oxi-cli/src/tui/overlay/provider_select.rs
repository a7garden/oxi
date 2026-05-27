//! Provider selector overlay — scrollable categorized list with inline API key popup.
//!
//! Replaces the old `SetupStep::SelectProvider` + `SetupStep::EnterApiKey`
//! full-screen transitions with a single overlay component that shows:
//! - A scrollable, categorized provider list
//! - An inline popup for API key entry (no screen transition)

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_tui::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::{centered_layout, OverlayAction, OverlayComponent};

// ── Category definitions ─────────────────────────────────────────────────

/// (category_key, display_label, short_tag)
const CATEGORIES: &[(&str, &str, &str)] = &[
    ("primary", "Primary", "[P]"),
    ("chinese", "China", "[C]"),
    ("open", "Open Platform", "[O]"),
    ("cloud", "Cloud", "[L]"),
    ("enterprise", "Enterprise", "[E]"),
    ("specialized", "Specialized", "[S]"),
];

// ── Data types ────────────────────────────────────────────────────────────

/// A single provider entry for the selection list.
#[derive(Clone)]
pub struct ProviderEntry {
    pub name: String,
    pub display_name: String,
    pub has_key: bool,
    pub category: String,
    pub description: String,
}

/// Which sub-mode the overlay is in.
#[derive(Clone)]
pub enum SubMode {
    /// Browsing the provider list.
    Browsing,
    /// API key input popup is open.
    EnteringKey {
        provider_name: String,
        display_name: String,
        key_text: String,
    },
}

/// A visible row — either a category header or a provider item.
#[derive(Clone)]
enum VisibleRow {
    CategoryHeader {
        label: String,
        tag: String,
    },
    Provider {
        entry: ProviderEntry,
        global_index: usize,
    },
}

// ── Overlay state ─────────────────────────────────────────────────────────

/// Provider selector overlay component.
///
/// Shows a categorized, scrollable list of providers. On Enter, opens
/// an inline API key popup instead of transitioning to a separate screen.
/// After saving a key, returns to the list with updated status.
#[allow(clippy::type_complexity)]
pub struct ProviderSelectOverlay {
    pub providers: Vec<ProviderEntry>,
    /// Index into `providers` (skips category headers).
    pub selected: usize,
    /// Vertical scroll offset in `VisibleRow` units.
    scroll_offset: usize,
    pub sub_mode: SubMode,
    /// Whether this is the initial setup (show "done" after first key).
    is_initial_setup: bool,
    /// Callback when a key is saved (optional).
    on_key_saved: Option<Box<dyn FnMut(&str, &str)>>,
}

impl std::fmt::Debug for ProviderSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSelectOverlay")
            .field("providers", &self.providers.len())
            .field("selected", &self.selected)
            .field("scroll_offset", &self.scroll_offset)
            .field("sub_mode", &std::mem::discriminant(&self.sub_mode))
            .finish()
    }
}

impl ProviderSelectOverlay {
    /// Create a new provider select overlay.
    pub fn new(providers: Vec<ProviderEntry>, is_initial_setup: bool) -> Self {
        Self {
            providers,
            selected: 0,
            scroll_offset: 0,
            sub_mode: SubMode::Browsing,
            is_initial_setup,
            on_key_saved: None,
        }
    }

    /// Set a callback invoked when an API key is saved: `f(provider_name, api_key)`.
    #[allow(dead_code)]
    pub fn with_on_key_saved(mut self, f: impl FnMut(&str, &str) + 'static) -> Self {
        self.on_key_saved = Some(Box::new(f));
        self
    }

    // ── Visible item building ─────────────────────────────────────────

    fn build_visible_items(&self) -> Vec<VisibleRow> {
        let mut rows = Vec::new();
        for (cat_key, label, tag) in CATEGORIES {
            let cat_providers: Vec<(usize, &ProviderEntry)> = self
                .providers
                .iter()
                .enumerate()
                .filter(|(_, p)| p.category == *cat_key)
                .collect();

            if cat_providers.is_empty() {
                continue;
            }

            rows.push(VisibleRow::CategoryHeader {
                label: label.to_string(),
                tag: tag.to_string(),
            });

            for (idx, entry) in cat_providers {
                rows.push(VisibleRow::Provider {
                    entry: entry.clone(),
                    global_index: idx,
                });
            }
        }
        rows
    }

    // ── Scroll ────────────────────────────────────────────────────────

    fn ensure_selected_visible(&mut self, list_height: usize) {
        let items = self.build_visible_items();

        let selected_pos = items
            .iter()
            .position(|r| {
                matches!(
                    r,
                    VisibleRow::Provider { global_index, .. } if *global_index == self.selected
                )
            })
            .unwrap_or(0);

        // If the selected item is right after a category header, ensure the
        // header is also visible.
        let target_top = if selected_pos > 0
            && matches!(items[selected_pos - 1], VisibleRow::CategoryHeader { .. })
        {
            selected_pos - 1
        } else {
            selected_pos
        };

        if target_top < self.scroll_offset {
            self.scroll_offset = target_top;
        } else if selected_pos >= self.scroll_offset + list_height {
            let new_end = selected_pos + 1;
            let new_start = if new_end > list_height + 1
                && matches!(
                    items.get(new_end.saturating_sub(2)),
                    Some(VisibleRow::CategoryHeader { .. })
                ) {
                new_end.saturating_sub(list_height + 1)
            } else {
                new_end.saturating_sub(list_height)
            };
            self.scroll_offset = new_start;
        }
    }

    // ── Navigation ────────────────────────────────────────────────────

    fn prev_provider(&mut self) {
        if self.providers.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.providers.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    fn next_provider(&mut self) {
        if self.providers.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.providers.len();
    }

    // ── Static helpers ────────────────────────────────────────────────

    fn category_header_line(tag: &str, label: &str, width: usize) -> String {
        let inner = format!(" {} {} ", tag, label);
        let sep_len = width.saturating_sub(inner.len());
        format!("{}{}", inner, "\u{2500}".repeat(sep_len))
    }

    fn title_line() -> Line<'static> {
        Line::styled(
            " Select a Provider ",
            Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
        )
    }
}

// ── OverlayComponent impl ─────────────────────────────────────────────────

impl OverlayComponent for ProviderSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        let is_browsing = matches!(self.sub_mode, SubMode::Browsing);
        if is_browsing {
            return self.handle_browsing(key);
        }

        // EnteringKey mode — extract values, then process
        let (pname, ktext) = match &self.sub_mode {
            SubMode::EnteringKey {
                provider_name,
                key_text,
                ..
            } => (provider_name.clone(), key_text.clone()),
            _ => return OverlayAction::None,
        };

        let mut key_text = ktext;
        let mut close = false;
        let mut back_to_browsing = false;
        let mut do_save = false;

        match key.code {
            KeyCode::Char(c) => {
                key_text.push(c);
            }
            KeyCode::Backspace => {
                key_text.pop();
            }
            KeyCode::Enter => {
                if !key_text.is_empty() {
                    do_save = true;
                }
                back_to_browsing = true;
            }
            KeyCode::Esc => {
                back_to_browsing = true;
            }
            _ => {}
        }

        // Write back key_text
        if let SubMode::EnteringKey {
            key_text: ref mut kt,
            ..
        } = self.sub_mode
        {
            *kt = key_text.clone();
        }

        if do_save {
            let auth = oxi_store::auth_storage::shared_auth_storage();
            auth.set_api_key(&pname, key_text.clone());

            if let Some(entry) = self.providers.iter_mut().find(|p| p.name == pname) {
                entry.has_key = true;
            }

            if let Some(ref mut cb) = self.on_key_saved {
                cb(&pname, &key_text);
            }

            if self.is_initial_setup {
                let configured = self.providers.iter().filter(|p| p.has_key).count();
                if configured == 1 {
                    close = true;
                }
            }
        }

        if back_to_browsing && !close {
            self.sub_mode = SubMode::Browsing;
        }

        if close {
            OverlayAction::Close
        } else {
            OverlayAction::None
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.85, 0.90);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(Self::title_line())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Title row
        let title_style = Style::default()
            .fg(theme.colors.primary.to_ratatui())
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Up/Down: navigate", title_style),
                Span::styled("  |  ", styles.muted),
                Span::styled("Enter: set API key", title_style),
                Span::styled("  |  ", styles.muted),
                Span::styled("Esc: close", title_style),
            ])),
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        let list_height = inner.height.saturating_sub(3) as usize; // title + separator + hint

        self.render_list(frame, inner, theme, list_height);

        let items = self.build_visible_items();
        if items.len() > list_height {
            self.render_scrollbar(frame, inner, items.len(), list_height, theme);
        }

        // Bottom hint
        let configured = self.providers.iter().filter(|p| p.has_key).count();
        let hint = format!(
            " {} providers ({} configured)  |  Up/Down  |  Enter: set key  |  Esc: close",
            self.providers.len(),
            configured,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            Rect {
                x: inner.x + 1,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        // API key popup (on top of everything)
        if matches!(self.sub_mode, SubMode::EnteringKey { .. }) {
            self.render_api_key_popup(frame, area, theme);
        }
    }

    fn hint(&self) -> &str {
        match &self.sub_mode {
            SubMode::Browsing => " Up/Down navigate | Enter: set key | Esc: close",
            SubMode::EnteringKey { .. } => " Type API key | Enter: save | Esc: cancel",
        }
    }
}

// ── Key handling helpers ──────────────────────────────────────────────────

impl ProviderSelectOverlay {
    fn handle_browsing(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Up => {
                self.prev_provider();
            }
            KeyCode::Down => {
                self.next_provider();
            }
            KeyCode::PageUp => {
                // Jump back by ~5 providers
                for _ in 0..5 {
                    self.prev_provider();
                }
            }
            KeyCode::PageDown => {
                for _ in 0..5 {
                    self.next_provider();
                }
            }
            KeyCode::Home => {
                self.selected = 0;
                self.scroll_offset = 0;
            }
            KeyCode::End if !self.providers.is_empty() => {
                self.selected = self.providers.len() - 1;
            }
            KeyCode::Enter => {
                if let Some(entry) = self.providers.get(self.selected).cloned() {
                    self.sub_mode = SubMode::EnteringKey {
                        provider_name: entry.name,
                        display_name: entry.display_name,
                        key_text: String::new(),
                    };
                }
            }
            KeyCode::Esc => {
                return OverlayAction::Close;
            }
            _ => {}
        }
        OverlayAction::None
    }
}

// ── Rendering helpers ─────────────────────────────────────────────────────

impl ProviderSelectOverlay {
    fn render_list(&mut self, frame: &mut Frame, inner: Rect, theme: &Theme, list_height: usize) {
        let styles = theme.to_styles();
        let items = self.build_visible_items();

        self.ensure_selected_visible(list_height);

        let avail_width = inner.width.saturating_sub(3); // leave room for scrollbar
        let start_y = inner.y + 2; // after title + separator

        // Separator after title
        let sep = "\u{2500}".repeat(inner.width.saturating_sub(2) as usize);
        frame.render_widget(
            Paragraph::new(Span::styled(
                sep,
                Style::default().fg(theme.colors.border.to_ratatui()),
            )),
            Rect {
                x: inner.x + 1,
                y: inner.y + 1,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        for (rendered, item) in items.iter().skip(self.scroll_offset).enumerate() {
            if rendered >= list_height {
                break;
            }
            let row_y = start_y + rendered as u16;

            match item {
                VisibleRow::CategoryHeader { label, tag } => {
                    let line = Self::category_header_line(tag, label, avail_width as usize);
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            line,
                            Style::default()
                                .fg(theme.colors.muted.to_ratatui())
                                .add_modifier(Modifier::BOLD),
                        )),
                        Rect {
                            x: inner.x + 1,
                            y: row_y,
                            width: avail_width,
                            height: 1,
                        },
                    );
                }
                VisibleRow::Provider {
                    entry,
                    global_index,
                } => {
                    let is_sel = *global_index == self.selected;
                    let check = if entry.has_key {
                        "\u{2713}"
                    } else {
                        "\u{25CB}"
                    };
                    let pointer = if is_sel { ">" } else { " " };

                    let row_style = if is_sel {
                        Style::default()
                            .fg(theme.colors.background.to_ratatui())
                            .bg(theme.colors.primary.to_ratatui())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        styles.normal
                    };

                    let check_style = if entry.has_key {
                        Style::default().fg(theme.colors.success.to_ratatui())
                    } else {
                        Style::default().fg(theme.colors.muted.to_ratatui())
                    };

                    let name_col_w = 18usize;
                    let display_name = truncate_str(&entry.display_name, name_col_w);
                    let desc_w = avail_width as usize;
                    let desc =
                        truncate_str(&entry.description, desc_w.saturating_sub(name_col_w + 10));

                    let mut spans = vec![
                        Span::styled(format!(" {} ", pointer), row_style),
                        Span::styled(
                            check.to_string(),
                            if is_sel { row_style } else { check_style },
                        ),
                        Span::styled(" ", row_style),
                        Span::styled(
                            format!(" {:<width$}", display_name, width = name_col_w),
                            row_style,
                        ),
                    ];

                    if entry.has_key {
                        spans.push(Span::styled(
                            truncate_str(
                                &entry.description,
                                desc_w.saturating_sub(name_col_w + 12),
                            ),
                            if is_sel { row_style } else { styles.muted },
                        ));
                        spans.push(Span::styled(
                            " [key set]",
                            if is_sel {
                                row_style
                            } else {
                                Style::default().fg(theme.colors.success.to_ratatui())
                            },
                        ));
                    } else {
                        spans.push(Span::styled(
                            desc,
                            if is_sel { row_style } else { styles.muted },
                        ));
                    }

                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect {
                            x: inner.x + 1,
                            y: row_y,
                            width: avail_width,
                            height: 1,
                        },
                    );
                }
            }
        }
    }

    fn render_api_key_popup(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if let SubMode::EnteringKey {
            display_name,
            key_text,
            provider_name,
        } = &self.sub_mode
        {
            let popup = centered_layout(area, 0.50, 0.32);
            frame.render_widget(Clear, popup);

            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", display_name))
                .border_style(Style::default().fg(theme.colors.accent.to_ratatui()));
            let inner = block.inner(popup);
            frame.render_widget(block, popup);

            let styles = theme.to_styles();

            // Storage hint + env var alternative
            if let Some(env_key) = oxi_ai::register_builtins::get_provider_env_key(provider_name) {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(
                        " Saved to ~/.oxi/auth.json",
                        styles.muted,
                    )])),
                    Rect {
                        x: inner.x + 1,
                        y: inner.y + 1,
                        width: inner.width.saturating_sub(2),
                        height: 1,
                    },
                );
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(" Or set env: ", styles.muted),
                        Span::styled(env_key, styles.normal),
                    ])),
                    Rect {
                        x: inner.x + 1,
                        y: inner.y + 2,
                        width: inner.width.saturating_sub(2),
                        height: 1,
                    },
                );
            }

            // Label
            frame.render_widget(
                Paragraph::new(Span::styled(
                    " API Key:",
                    Style::default()
                        .fg(theme.colors.foreground.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )),
                Rect {
                    x: inner.x + 1,
                    y: inner.y + 4,
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );

            // Input field (masked)
            let masked = "*".repeat(key_text.len());
            let field_width = (inner.width as usize).saturating_sub(4).max(20);
            let display: String = masked.chars().take(field_width).collect();

            let input_style = Style::default()
                .fg(theme.colors.foreground.to_ratatui())
                .bg(theme.colors.selection_bg.to_ratatui());

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<width$}", format!("{}_", display), width = field_width),
                        input_style,
                    ),
                ])),
                Rect {
                    x: inner.x + 1,
                    y: inner.y + 5,
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );

            // Hint
            frame.render_widget(
                Paragraph::new(Span::styled(" Enter: save  |  Esc: cancel", styles.muted)),
                Rect {
                    x: inner.x + 1,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );
        }
    }

    fn render_scrollbar(
        &self,
        frame: &mut Frame,
        inner: Rect,
        total: usize,
        visible: usize,
        theme: &Theme,
    ) {
        let track_x = inner.x + inner.width.saturating_sub(1);
        // Track starts below title + separator, ends above hint
        let track_top = inner.y + 2;
        let track_height = inner.height.saturating_sub(4) as usize; // title + sep + hint + bottom
        if track_height == 0 {
            return;
        }

        let thumb_size = ((visible as f32 / total as f32) * track_height as f32).max(1.0) as usize;
        let max_offset = total.saturating_sub(visible).max(1);
        let thumb_pos = ((self.scroll_offset as f32 / max_offset as f32)
            * (track_height.saturating_sub(thumb_size)) as f32) as usize;

        let thumb_style = Style::default().fg(theme.colors.primary.to_ratatui());
        let track_style = Style::default().fg(theme.colors.border.to_ratatui());

        for i in 0..track_height {
            let is_thumb = i >= thumb_pos && i < thumb_pos + thumb_size;
            let ch = if is_thumb { "\u{2588}" } else { "\u{2502}" };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    ch,
                    if is_thumb { thumb_style } else { track_style },
                )),
                Rect {
                    x: track_x,
                    y: track_top + i as u16,
                    width: 1,
                    height: 1,
                },
            );
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn truncate_str(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len > max_width {
        format!(
            "{}...",
            text.chars()
                .take(max_width.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        text.to_string()
    }
}

// ── Builder helper ────────────────────────────────────────────────────────

/// Build the provider list from builtins, sorted by category order.
pub fn build_provider_entries() -> Vec<ProviderEntry> {
    let auth = oxi_store::auth_storage::shared_auth_storage();

    let mut providers: Vec<ProviderEntry> = oxi_ai::register_builtins::get_builtin_providers()
        .iter()
        .map(|builtin| {
            let has_key = auth.get_api_key(builtin.name).is_some();
            ProviderEntry {
                name: builtin.name.to_string(),
                display_name: builtin.display_name.to_string(),
                has_key,
                category: builtin.category.to_string(),
                description: builtin.description.to_string(),
            }
        })
        .collect();

    // Sort by category order then by name.
    let category_rank = |cat: &str| -> usize {
        CATEGORIES
            .iter()
            .position(|(c, _, _)| *c == cat)
            .unwrap_or(CATEGORIES.len())
    };
    providers.sort_by(|a, b| {
        category_rank(&a.category)
            .cmp(&category_rank(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });

    providers
}
