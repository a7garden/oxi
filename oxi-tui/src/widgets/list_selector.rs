//! Universal list-selector state — the single source of truth for all
//! list-selection UX (ask, model-select, resume, etc.).
//!
//! Ports omp's `HookSelectorComponent` into a stateful ratatui widget. The
//! caller owns the [`ListSelectorState`], calls [`ListSelectorState::handle_key`]
//! to mutate it, and [`ListSelectorState::render`] to produce display lines.
//!
//! ## Marker semantics (omp-faithful)
//!
//! - **Radio** (single-choice): the glyph is filled (`◉`) on the *cursor* row,
//!   hollow (`○`) elsewhere — the cursor previews the selection.
//! - **Checkbox** (multi): the glyph reflects the per-row checked state
//!   (`☑`/`☐`); the cursor row is colored accent.
//! - **None**: a plain cursor prefix (`❯`) on the highlighted row.
//!
//! ## Compact mode
//!
//! When `options.len() > max_visible` (default 12), the selector switches to
//! compact mode: descriptions are hidden except on the cursor row, and
//! type-to-search fuzzy filtering is enabled.

use crate::theme::ThemeStyles;
#[cfg(test)]
use crossterm::event::KeyModifiers;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use std::collections::HashSet;

// ── Public types ───────────────────────────────────────────────────────────

/// Row marker kind — determines the glyph drawn before each option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectorMarker {
    /// No marker — plain cursor prefix (model-select, session-select).
    #[default]
    None,
    /// Radio (single-choice) — filled on the cursor row.
    Radio,
    /// Checkbox (multi-choice) — reflects per-row checked state.
    Checkbox,
}

/// One selectable option.
#[derive(Debug, Clone)]
pub struct SelectorOption {
    /// Display label.
    pub label: String,
    /// Optional description shown indented below the label.
    pub description: Option<String>,
    /// When `true`, the row is skipped during cursor navigation.
    pub disabled: bool,
}

impl SelectorOption {
    /// Create a simple option with just a label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            disabled: false,
        }
    }

    /// Attach a description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// A control row appended after options (Other / Done).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRowKind {
    /// "Other (type your own)" — opens an inline editor.
    Other,
    /// "Done selecting" — multi-select terminator.
    Done,
}

/// Result of a key press — the caller interprets this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorAction {
    /// No action (cursor moved, search typed, etc.).
    None,
    /// A single-choice option was selected (Enter on a radio/none row).
    Select {
        /// Real index into the options vector.
        option_idx: usize,
    },
    /// A multi-choice option was toggled (Enter/Space on a checkbox row).
    Toggle {
        /// Real index into the options vector.
        option_idx: usize,
    },
    /// The "Other" control row was activated.
    Other,
    /// The "Done" control row was activated.
    Done,
    /// Navigate to the previous question (←).
    NavBack,
    /// Navigate to the next question (→).
    NavForward,
    /// Timeout expired.
    Timeout,
    /// User cancelled (Esc).
    Cancel,
}

// ── State ──────────────────────────────────────────────────────────────────

/// Complete display + interaction state for a list selector.
///
/// The caller owns this value, mutates it via [`handle_key`](Self::handle_key),
/// and renders it via [`render`](Self::render).
#[derive(Debug, Clone)]
pub struct ListSelectorState {
    // ── Configuration ──
    title: String,
    options: Vec<SelectorOption>,
    pub marker: SelectorMarker,
    checked: HashSet<usize>,
    /// Number of leading options that get markers (control rows excluded).
    markable_count: usize,
    control_rows: Vec<ControlRowKind>,
    pub timeout_secs: Option<u64>,
    pub progress: Option<String>,
    help_text: String,
    max_visible: usize,

    // ── Interaction state ──
    cursor: usize,
    search: String,
}

impl ListSelectorState {
    /// Create a new selector with the given title and options.
    pub fn new(title: impl Into<String>, options: Vec<SelectorOption>) -> Self {
        let markable = options.len();
        Self {
            title: title.into(),
            options,
            marker: SelectorMarker::None,
            checked: HashSet::new(),
            markable_count: markable,
            control_rows: Vec::new(),
            timeout_secs: None,
            progress: None,
            help_text: " \u{2191}\u{2193} navigate  enter select  esc cancel".to_string(),
            max_visible: 12,
            cursor: 0,
            search: String::new(),
        }
    }

    // ── Builders ──

    /// Set the marker type.
    pub fn with_marker(mut self, marker: SelectorMarker) -> Self {
        self.marker = marker;
        self
    }

    /// Pre-check the given option indices (checkbox).
    pub fn with_checked(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.checked = indices.into_iter().collect();
        self
    }

    /// Append control rows (Other, Done).
    pub fn with_control_rows(mut self, rows: Vec<ControlRowKind>) -> Self {
        self.control_rows = rows;
        self
    }

    /// Set the timeout (seconds, for countdown display).
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }

    /// Set multi-step progress text (e.g. "(2/3)").
    pub fn with_progress(mut self, progress: impl Into<String>) -> Self {
        self.progress = Some(progress.into());
        self
    }

    /// Override the help text.
    pub fn with_help_text(mut self, text: impl Into<String>) -> Self {
        self.help_text = text.into();
        self
    }

    /// Override the compact-mode threshold.
    pub fn with_max_visible(mut self, n: usize) -> Self {
        self.max_visible = n;
        self
    }

    /// Set the initial cursor position (clamped, walks to nearest enabled).
    pub fn set_initial_cursor(&mut self, idx: usize) {
        self.cursor = self.coerce_cursor(idx);
    }

    /// Current cursor row index (into the combined option+control list).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Toggle a checkbox option.
    pub fn toggle(&mut self, option_idx: usize) {
        if self.checked.contains(&option_idx) {
            self.checked.remove(&option_idx);
        } else {
            self.checked.insert(option_idx);
        }
    }

    /// Return the set of checked option indices (for multi-select).
    pub fn checked_indices(&self) -> Vec<usize> {
        self.checked.iter().copied().collect()
    }

    /// Set the timeout display value (seconds remaining, for countdown).
    /// Called by the overlay each frame to sync the wall-clock countdown.
    pub fn set_timeout_display(&mut self, secs: u64) {
        self.timeout_secs = Some(secs);
    }

    /// Whether compact mode is active.
    pub fn is_compact(&self) -> bool {
        self.options.len() > self.max_visible
    }

    /// Current search query.
    pub fn search(&self) -> &str {
        &self.search
    }

    // ── Row model ──

    /// Filtered option indices (matching the search query). Empty query → all.
    fn filtered_options(&self) -> Vec<usize> {
        if self.search.trim().is_empty() {
            return (0..self.options.len()).collect();
        }
        let needle = self.search.to_lowercase();
        (0..self.options.len())
            .filter(|&i| {
                let opt = &self.options[i];
                opt.label.to_lowercase().contains(&needle)
                    || opt
                        .description
                        .as_deref()
                        .is_some_and(|d| d.to_lowercase().contains(&needle))
            })
            .collect()
    }

    /// Number of visible option rows (after filtering).
    pub fn visible_option_count(&self) -> usize {
        if self.is_compact() {
            self.filtered_options().len()
        } else {
            self.options.len()
        }
    }

    /// Total rows: visible options + control rows.
    pub fn total_rows(&self) -> usize {
        self.visible_option_count() + self.control_rows.len()
    }

    /// Map a display row index to a real option index. Returns `None` for
    /// control rows or out-of-range indices.
    pub fn display_to_option(&self, display_idx: usize) -> Option<usize> {
        if self.is_compact() {
            self.filtered_options().get(display_idx).copied()
        } else {
            (display_idx < self.options.len()).then_some(display_idx)
        }
    }

    /// The real option index under the cursor, or `None` if on a control row.
    pub fn cursor_option(&self) -> Option<usize> {
        self.display_to_option(self.cursor)
    }

    /// Whether the cursor is on a specific control row kind.
    fn cursor_on_control(&self, kind: ControlRowKind) -> bool {
        let vis = self.visible_option_count();
        // Cursor on an option row (below the control-row zone) → never a control row.
        if self.cursor < vis {
            return false;
        }
        let ctrl_offset = self.cursor - vis;
        self.control_rows.get(ctrl_offset) == Some(&kind)
    }

    // ── Cursor navigation ──

    /// Clamp `idx` into range, then walk to the nearest enabled option.
    fn coerce_cursor(&self, idx: usize) -> usize {
        let total = self.total_rows();
        if total == 0 {
            return 0;
        }
        let clamped = idx.min(total - 1);
        // Walk forward, then backward, to find a non-disabled option row.
        // Control rows are always navigable.
        for i in clamped..total {
            if self.row_is_navigable(i) {
                return i;
            }
        }
        for i in (0..clamped).rev() {
            if self.row_is_navigable(i) {
                return i;
            }
        }
        clamped
    }

    /// A row is navigable if it's a control row, or a non-disabled option.
    fn row_is_navigable(&self, display_idx: usize) -> bool {
        if let Some(opt_idx) = self.display_to_option(display_idx) {
            !self.options.get(opt_idx).is_some_and(|o| o.disabled)
        } else {
            // Control row — always navigable.
            true
        }
    }

    /// Move cursor by `delta` from its current position, skipping disabled
    /// rows. Stops at boundaries (no wrap-around). Safe against all-disabled
    /// input — bounded by `total_rows()` iterations.
    fn move_cursor(&mut self, delta: i32) {
        let total = self.total_rows();
        if total == 0 {
            return;
        }
        let max_idx = (total - 1) as i32;
        // Clamp first step to the valid range so we don't walk past bounds.
        let mut idx = (self.cursor as i32 + delta).clamp(0, max_idx);
        // Same position: nothing to do (or all-disabled from here).
        if idx as usize == self.cursor {
            return;
        }
        if self.row_is_navigable(idx as usize) {
            self.cursor = idx as usize;
            return;
        }
        // Disabled: step in direction, bounded by total iterations.
        let max_steps = total as i32;
        for _ in 0..max_steps {
            let next = idx + delta;
            if next < 0 || next > max_idx {
                // Hit boundary with disabled row — clamp to current boundary
                // cell (the row we just landed on) and stop.
                self.cursor = idx as usize;
                return;
            }
            idx = next;
            if self.row_is_navigable(idx as usize) {
                self.cursor = idx as usize;
                return;
            }
        }
        // All rows from here to boundary are disabled. Hold position.
    }

    // ── Key handling ──

    /// Process a key press. Mutates internal state and returns an action.
    pub fn handle_key(&mut self, key: KeyEvent) -> SelectorAction {
        if key.kind != KeyEventKind::Press {
            return SelectorAction::None;
        }

        // Search input (compact mode only).
        if self.is_compact() && self.handle_search_key(&key) {
            return SelectorAction::None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') if !self.is_compact() => {
                self.move_cursor(-1);
                SelectorAction::None
            }
            KeyCode::Down | KeyCode::Char('j') if !self.is_compact() => {
                self.move_cursor(1);
                SelectorAction::None
            }
            KeyCode::Enter => self.activate_cursor(false),
            KeyCode::Char(' ') => self.activate_cursor(true),
            KeyCode::Left => SelectorAction::NavBack,
            KeyCode::Right => SelectorAction::NavForward,
            KeyCode::Esc => SelectorAction::Cancel,
            _ => SelectorAction::None,
        }
    }

    /// Handle a key as search input. Returns `true` if consumed.
    fn handle_search_key(&mut self, key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Backspace => {
                self.search.pop();
                self.cursor = 0;
                true
            }
            KeyCode::Char(c) if !c.is_control() => {
                self.search.push(c);
                self.cursor = 0;
                true
            }
            _ => false,
        }
    }

    /// Activate the row under the cursor (Enter or Space).
    fn activate_cursor(&mut self, _is_space: bool) -> SelectorAction {
        // Control rows first.
        if self.cursor_on_control(ControlRowKind::Other) {
            return SelectorAction::Other;
        }
        if self.cursor_on_control(ControlRowKind::Done) {
            return SelectorAction::Done;
        }
        // Option row.
        match self.cursor_option() {
            Some(idx) => match self.marker {
                SelectorMarker::Checkbox => {
                    // Auto-toggle — the state owns checked, the action notifies the caller.
                    self.toggle(idx);
                    SelectorAction::Toggle { option_idx: idx }
                }
                SelectorMarker::Radio | SelectorMarker::None => {
                    SelectorAction::Select { option_idx: idx }
                }
            },
            None => SelectorAction::None,
        }
    }

    // ── Rendering ──

    /// Render the selector to display lines.
    pub fn render(&self, width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
        let sym = styles.symbols;
        let mut lines = Vec::new();

        // ── Title line ──
        let mut title_parts = vec![Span::styled(
            format!(" {} ", sym.tool_ask),
            styles.accent.add_modifier(Modifier::BOLD),
        )];
        let title_text = if let Some(prog) = &self.progress {
            format!("{} ({})", self.title, prog)
        } else {
            self.title.clone()
        };
        title_parts.push(Span::styled(
            title_text,
            styles.normal.add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(title_parts));

        // ── Countdown ──
        if let Some(secs) = self.timeout_secs {
            let timer_style = if secs <= 5 {
                styles.warning.add_modifier(Modifier::BOLD)
            } else {
                styles.muted
            };
            lines.push(Line::from(Span::styled(
                format!(" {} {}s remaining", sym.icon_time, secs),
                timer_style,
            )));
        }

        // ── Search status (compact mode) ──
        if self.is_compact() {
            let status = if self.search.trim().is_empty() {
                " Type to search".to_string()
            } else {
                format!(" Search: {}", self.search)
            };
            lines.push(Line::from(Span::styled(status, styles.muted)));
        }

        // ── Separator ──
        let sep = sym.rule.repeat(width.max(2) / sym.rule.len().max(1));
        lines.push(Line::from(Span::styled(
            sep,
            ratatui::style::Style::default().fg(ratatui::style::Color::Reset),
        )));

        // ── Option rows ──
        let vis = self.visible_option_count();
        for display_idx in 0..self.total_rows() {
            let is_cursor = display_idx == self.cursor;

            if let Some(opt_idx) = self.display_to_option(display_idx) {
                let opt = &self.options[opt_idx];
                let is_checked = self.checked.contains(&opt_idx);
                let is_disabled = opt.disabled;

                // Cursor prefix
                let prefix = if is_cursor {
                    Span::styled(
                        format!("{} ", sym.cursor),
                        styles.accent.add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::raw("  ")
                };

                // Marker
                let marker_span =
                    self.render_marker(opt_idx, is_cursor, is_checked, is_disabled, sym, styles);

                // Label
                let label_style = if is_disabled {
                    styles.muted
                } else if is_cursor {
                    styles.accent
                } else {
                    styles.normal
                };
                lines.push(Line::from(vec![
                    prefix,
                    marker_span,
                    Span::styled(opt.label.clone(), label_style),
                ]));

                // Description (non-compact: always; compact: cursor row only)
                if let Some(desc) = &opt.description
                    && (!self.is_compact() || is_cursor)
                {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(format!("{} {}", sym.nav_expand, desc), styles.muted),
                    ]));
                }
            } else {
                // Control row
                let ctrl_offset = display_idx - vis;
                let row = self.render_control_row(ctrl_offset, is_cursor, sym, styles);
                lines.push(row);
            }
        }

        // ── Bottom separator ──
        let sep2 = sym.rule.repeat(width.max(2) / sym.rule.len().max(1));
        lines.push(Line::from(Span::styled(
            sep2,
            ratatui::style::Style::default().fg(ratatui::style::Color::Reset),
        )));

        // ── Help text ──
        lines.push(Line::from(Span::styled(
            self.help_text.clone(),
            styles.muted,
        )));

        lines
    }

    /// Render the marker glyph for an option row.
    fn render_marker(
        &self,
        opt_idx: usize,
        is_cursor: bool,
        is_checked: bool,
        is_disabled: bool,
        sym: crate::symbols::Symbols,
        styles: &ThemeStyles,
    ) -> Span<'static> {
        // Options beyond markable_count get no marker.
        if opt_idx >= self.markable_count {
            return Span::raw("  ");
        }
        match self.marker {
            SelectorMarker::Radio => {
                let glyph = if is_cursor {
                    sym.radio_on
                } else {
                    sym.radio_off
                };
                let color = if is_disabled {
                    styles.muted
                } else if is_cursor {
                    styles.accent
                } else {
                    styles.muted
                };
                Span::styled(format!("{} ", glyph), color)
            }
            SelectorMarker::Checkbox => {
                let glyph = if is_checked {
                    sym.checkbox_on
                } else {
                    sym.checkbox_off
                };
                let color = if is_disabled {
                    styles.muted
                } else if is_cursor {
                    styles.accent
                } else if is_checked {
                    styles.success
                } else {
                    styles.muted
                };
                Span::styled(format!("{} ", glyph), color)
            }
            SelectorMarker::None => Span::raw(""),
        }
    }

    /// Render a control row (Other / Done).
    fn render_control_row(
        &self,
        ctrl_offset: usize,
        is_cursor: bool,
        sym: crate::symbols::Symbols,
        styles: &ThemeStyles,
    ) -> Line<'static> {
        let prefix = if is_cursor {
            Span::styled(
                format!("{} ", sym.cursor),
                styles.accent.add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("  ")
        };
        match self.control_rows.get(ctrl_offset) {
            Some(ControlRowKind::Other) => {
                let marker = if is_cursor {
                    Span::styled(format!("{} ", sym.radio_on), styles.accent)
                } else {
                    Span::styled(format!("{} ", sym.radio_off), styles.muted)
                };
                Line::from(vec![
                    prefix,
                    marker,
                    Span::styled(
                        "Other (type your own)".to_string(),
                        if is_cursor {
                            styles.accent
                        } else {
                            styles.muted
                        },
                    ),
                ])
            }
            Some(ControlRowKind::Done) => {
                let has_sel = !self.checked.is_empty();
                let style = if has_sel {
                    styles.success.add_modifier(Modifier::BOLD)
                } else {
                    styles.muted
                };
                let marker = if has_sel {
                    Span::styled(format!("{} ", sym.status_success), styles.success)
                } else {
                    Span::raw("  ")
                };
                Line::from(vec![
                    prefix,
                    marker,
                    Span::styled("Done selecting".to_string(), style),
                ])
            }
            None => Line::raw(""),
        }
    }

    /// Decrement the timeout (call every second from the overlay's poll).
    pub fn tick_timeout(&mut self) -> bool {
        if let Some(secs) = self.timeout_secs.as_mut() {
            if *secs == 0 {
                return true;
            }
            *secs -= 1;
            *secs == 0
        } else {
            false
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(n: usize) -> Vec<SelectorOption> {
        (0..n)
            .map(|i| SelectorOption::new(format!("Option {i}")))
            .collect()
    }

    fn press(state: &mut ListSelectorState, code: KeyCode) -> SelectorAction {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn radio_select_returns_select_action() {
        let mut s = ListSelectorState::new("Pick", opts(3)).with_marker(SelectorMarker::Radio);
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            SelectorAction::Select { option_idx: 0 }
        );
    }

    #[test]
    fn checkbox_enter_toggles() {
        let mut s = ListSelectorState::new("Pick", opts(3)).with_marker(SelectorMarker::Checkbox);
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            SelectorAction::Toggle { option_idx: 0 }
        );
        assert!(s.checked.contains(&0));
        // Move down, toggle second
        press(&mut s, KeyCode::Down);
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            SelectorAction::Toggle { option_idx: 1 }
        );
        assert!(s.checked.contains(&1));
    }

    #[test]
    fn none_marker_returns_select() {
        let mut s = ListSelectorState::new("Model", opts(3));
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            SelectorAction::Select { option_idx: 0 }
        );
    }

    #[test]
    fn down_moves_cursor() {
        let mut s = ListSelectorState::new("Pick", opts(3));
        press(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 1);
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            SelectorAction::Select { option_idx: 1 }
        );
    }

    #[test]
    fn up_at_top_stays() {
        let mut s = ListSelectorState::new("Pick", opts(3));
        press(&mut s, KeyCode::Up);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn esc_cancels() {
        let mut s = ListSelectorState::new("Pick", opts(2));
        assert_eq!(press(&mut s, KeyCode::Esc), SelectorAction::Cancel);
    }

    #[test]
    fn left_right_nav() {
        let mut s = ListSelectorState::new("Pick", opts(2));
        assert_eq!(press(&mut s, KeyCode::Left), SelectorAction::NavBack);
        assert_eq!(press(&mut s, KeyCode::Right), SelectorAction::NavForward);
    }

    #[test]
    fn disabled_rows_skipped_on_down() {
        let mut opts_disabled = opts(3);
        opts_disabled[1].disabled = true;
        let mut s = ListSelectorState::new("Pick", opts_disabled);
        // cursor at 0, down should skip 1 (disabled) → 2
        press(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn disabled_rows_skipped_on_up() {
        // cursor at 2, up should skip 1 (disabled) → 0. The previous
        // saturating_sub + coerce_cursor combo failed this — backward walk
        // wasn't symmetric.
        let mut opts_disabled = opts(3);
        opts_disabled[1].disabled = true;
        let mut s = ListSelectorState::new("Pick", opts_disabled);
        s.set_initial_cursor(2);
        press(&mut s, KeyCode::Up);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn down_stops_at_last_row() {
        // No wrap-around: down at last row stays. The previous coerce_cursor
        // call would jump to the first row because of its backward walk.
        let mut s = ListSelectorState::new("Pick", opts(3));
        s.set_initial_cursor(2);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn up_stops_at_first_row() {
        let mut s = ListSelectorState::new("Pick", opts(3));
        press(&mut s, KeyCode::Up);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn all_disabled_no_infinite_loop() {
        // Regression: move_cursor used to loop forever when every row was
        // disabled. It must return after bounded iterations.
        let mut opts_disabled = opts(4);
        for o in opts_disabled.iter_mut() {
            o.disabled = true;
        }
        let mut s = ListSelectorState::new("Pick", opts_disabled);
        // coerce_cursor returns clamped index when nothing is navigable.
        s.set_initial_cursor(1);
        // The exact end position is implementation-defined; the contract is
        // "no hang, no panic, cursor stays in [0, total)".
        press(&mut s, KeyCode::Down);
        assert!(s.cursor < s.total_rows());
        press(&mut s, KeyCode::Up);
        assert!(s.cursor < s.total_rows());
    }

    #[test]
    fn jk_navigation_skips_disabled() {
        let mut opts_disabled = opts(4);
        opts_disabled[2].disabled = true;
        let mut s = ListSelectorState::new("Pick", opts_disabled);
        s.set_initial_cursor(1);
        press(&mut s, KeyCode::Char('j'));
        // should skip 2 → land on 3
        assert_eq!(s.cursor, 3);
        press(&mut s, KeyCode::Char('k'));
        // should skip 2 backward → land on 1
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn control_row_other() {
        let mut s = ListSelectorState::new("Pick", opts(2))
            .with_marker(SelectorMarker::Radio)
            .with_control_rows(vec![ControlRowKind::Other]);
        // total rows = 2 options + 1 other = 3. Move to index 2 (Other).
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 2);
        assert_eq!(press(&mut s, KeyCode::Enter), SelectorAction::Other);
    }

    #[test]
    fn control_row_done() {
        let mut s = ListSelectorState::new("Pick", opts(2))
            .with_marker(SelectorMarker::Checkbox)
            .with_control_rows(vec![ControlRowKind::Other, ControlRowKind::Done]);
        // Toggle option 0 first
        press(&mut s, KeyCode::Enter);
        // Move to Done (index 3 = 2 options + 1 other)
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 3);
        assert_eq!(press(&mut s, KeyCode::Enter), SelectorAction::Done);
    }

    #[test]
    fn compact_mode_enables_search() {
        let mut s = ListSelectorState::new("Pick", opts(15)).with_max_visible(10);
        assert!(s.is_compact());
        // Type a search character
        s.handle_key(KeyEvent::new(KeyCode::Char('O'), KeyModifiers::NONE));
        assert_eq!(s.search, "O");
        // Filtered: all options contain "Option" so "O" matches all 15
        assert_eq!(s.visible_option_count(), 15);
        // Type a more specific query
        s.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        // "O1" matches "Option 1", "Option 10".."Option 14" = 6
        assert!(s.visible_option_count() < 15);
    }

    #[test]
    fn non_compact_does_not_search() {
        let mut s = ListSelectorState::new("Pick", opts(3));
        assert!(!s.is_compact());
        s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(s.search.is_empty());
    }

    #[test]
    fn initial_cursor_clamped() {
        let mut s = ListSelectorState::new("Pick", opts(3));
        s.set_initial_cursor(10);
        assert_eq!(s.cursor, 2); // clamped to last
    }

    #[test]
    fn render_produces_lines() {
        let s = ListSelectorState::new("Pick", opts(3)).with_marker(SelectorMarker::Radio);
        let lines = s.render(60, &ThemeStyles::default());
        // title + separator + 3 options + separator + help = 7 minimum
        assert!(lines.len() >= 7);
    }

    #[test]
    fn timeout_tick() {
        let mut s = ListSelectorState::new("Pick", opts(2)).with_timeout(3);
        assert!(!s.tick_timeout()); // 3→2
        assert!(!s.tick_timeout()); // 2→1
        assert!(s.tick_timeout()); // 1→0, expired
        assert_eq!(s.timeout_secs, Some(0));
    }
}
