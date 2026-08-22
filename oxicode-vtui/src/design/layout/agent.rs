//! Agent view layout — pure geometry computation.
//!
//! Ported from grok-build's `views/agent.rs` (`AgentViewLayout`,
//! `ActivePane`, `PaneAreas`).  The layout is computed from screen area +
//! appearance config + per-pane heights, producing a set of [`Rect`]s that
//! widgets render into.
//!
//! ## Vertical stack (top → bottom)
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │ [Startup warnings]                optional  │
//! │ [Tasks pane]                      optional  │
//! │ [Catalog pane]                    optional  │
//! │ [Todo pane]                       optional  │
//! ├─────────────────────────────────────────────┤
//! │ Scrollback               Min(5) — dominant  │
//! ├─────────────────────────────────────────────┤
//! │ [BTW panel]                       optional  │
//! │ [Queue pane]                      optional  │
//! │ [Turn status]                     optional  │
//! │ [Banner / CTA / Follow-ups]       optional  │
//! ├─────────────────────────────────────────────┤
//! │ Prompt                fixed height          │
//! ├─────────────────────────────────────────────┤
//! │ ShortcutsBar            1 row               │
//! └─────────────────────────────────────────────┘
//! ```

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Padding};

use super::config::{LayoutConfig, ScrollbarConfig};

// ───────────────────────────────────────────────────────────────────────────
// ActivePane
// ───────────────────────────────────────────────────────────────────────────

/// Which pane is currently active (has keyboard focus) in the agent view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePane {
    /// Main conversation scrollback (default).
    #[default]
    Scrollback,
    /// Todo checklist side-pane.
    Todo,
    /// Prompt queue side-pane.
    Queue,
    /// Text input prompt.
    Prompt,
    /// Background tasks pane.
    Tasks,
    /// Subagent / extension catalog pane.
    Catalog,
}

impl ActivePane {
    /// Cycle to the next visible pane.  `visible` is the set of panes
    /// that currently have non-zero height (from [`PaneAreas`]).
    #[must_use]
    pub fn cycle(self, visible: &PaneAreas) -> Self {
        let order = [
            ActivePane::Scrollback,
            ActivePane::Todo,
            ActivePane::Queue,
            ActivePane::Tasks,
            ActivePane::Catalog,
            ActivePane::Prompt,
        ];
        let start = order.iter().position(|&p| p == self).unwrap_or(0);
        for i in 1..=order.len() {
            let candidate = order[(start + i) % order.len()];
            if visible.is_visible(candidate) {
                return candidate;
            }
        }
        self
    }
}

// ───────────────────────────────────────────────────────────────────────────
// PaneAreas (mouse hit-testing)
// ───────────────────────────────────────────────────────────────────────────

/// Cached pane rectangles from the last render, used for mouse hit-testing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaneAreas {
    /// Scrollback conversation area.
    pub scrollback: Rect,
    /// Todo side-pane area.
    pub todo: Rect,
    /// Queue side-pane area.
    pub queue: Rect,
    /// Prompt input area.
    pub prompt: Rect,
    /// Background tasks pane area.
    pub tasks: Rect,
    /// Subagent / extension catalog area.
    pub catalog: Rect,
}

impl PaneAreas {
    /// Determine which pane a screen position falls in, if any.
    #[must_use]
    pub fn hit_test(&self, col: u16, row: u16) -> Option<ActivePane> {
        let pos = (col, row).into();
        if self.tasks.area() > 0 && self.tasks.contains(pos) {
            return Some(ActivePane::Tasks);
        }
        if self.catalog.area() > 0 && self.catalog.contains(pos) {
            return Some(ActivePane::Catalog);
        }
        if self.todo.area() > 0 && self.todo.contains(pos) {
            return Some(ActivePane::Todo);
        }
        if self.queue.area() > 0 && self.queue.contains(pos) {
            return Some(ActivePane::Queue);
        }
        if self.scrollback.area() > 0 && self.scrollback.contains(pos) {
            return Some(ActivePane::Scrollback);
        }
        if self.prompt.area() > 0 && self.prompt.contains(pos) {
            return Some(ActivePane::Prompt);
        }
        None
    }

    /// Whether a pane is currently visible (non-zero area).
    #[must_use]
    pub fn is_visible(&self, pane: ActivePane) -> bool {
        match pane {
            ActivePane::Scrollback => self.scrollback.area() > 0,
            ActivePane::Todo => self.todo.area() > 0,
            ActivePane::Queue => self.queue.area() > 0,
            ActivePane::Prompt => self.prompt.area() > 0,
            ActivePane::Tasks => self.tasks.area() > 0,
            ActivePane::Catalog => self.catalog.area() > 0,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Constants
// ───────────────────────────────────────────────────────────────────────────

/// Terminals at or below this height suppress optional rows above the prompt.
pub const SHORT_TERMINAL_ROWS: u16 = 16;

/// Auto-compact threshold.
pub const AUTO_COMPACT_MAX_ROWS: u16 = 20;

const _: () = assert!(SHORT_TERMINAL_ROWS < AUTO_COMPACT_MAX_ROWS);

/// Render-value derivation for compact mode.
#[must_use]
pub fn effective_compact(user_compact: bool, terminal_rows: u16) -> bool {
    user_compact || (terminal_rows > 0 && terminal_rows <= AUTO_COMPACT_MAX_ROWS)
}

// ───────────────────────────────────────────────────────────────────────────
// AgentViewLayout
// ───────────────────────────────────────────────────────────────────────────

/// Computed screen layout for the agent view.
///
/// Pure data — no rendering.  Computed from screen area + appearance config +
/// per-pane heights via [`compute`](Self::compute).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentViewLayout {
    /// Startup warning banner.
    pub startup_warnings: Rect,
    /// Background tasks pane.
    pub tasks: Rect,
    /// Subagent / extension catalog pane.
    pub catalog: Rect,
    /// Main conversation scrollback (dominant).
    pub scrollback: Rect,
    /// Todo checklist side-pane.
    pub todo: Rect,
    /// Prompt queue side-pane.
    pub queue: Rect,
    /// Inline side-question panel.
    pub btw: Rect,
    /// Turn status line.
    pub turn_status: Rect,
    /// Banner row above the prompt.
    pub banner: Rect,
    /// Inline CTA row.
    pub plugin_cta: Rect,
    /// Follow-up suggestion chips row.
    pub follow_ups: Rect,
    /// Voice recording indicator row.
    pub voice_recording: Rect,
    /// Prompt input widget.
    pub prompt: Rect,
    /// Bottom shortcuts bar.
    pub shortcuts: Rect,
    /// Scrollback area narrowed for scrollbar.
    pub scrollback_content: Rect,
    /// Scrollbar track x-coordinate.
    pub scrollbar_x: u16,
    /// Timeline rail left edge (0 = hidden).
    pub timeline_x: u16,
    /// Columns reserved for the timeline rail (0 = hidden).
    pub timeline_width: u16,
}

/// Inputs to [`AgentViewLayout::compute`], bundled so the call site names
/// each field — preventing accidental transposition of the many `u16`
/// height parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct LayoutInput {
    /// Prompt widget height (rows).
    pub prompt_height: u16,
    /// Background tasks pane height (0 = hidden).
    pub tasks_height: u16,
    /// Catalog pane height (0 = hidden).
    pub catalog_height: u16,
    /// Todo pane height (0 = hidden).
    pub todo_height: u16,
    /// Queue pane height (0 = hidden).
    pub queue_height: u16,
    /// BTW side-question panel height (0 = hidden).
    pub btw_height: u16,
    /// Turn status height (0 = hidden).
    pub turn_status_height: u16,
    /// Banner height (0 = hidden).
    pub banner_height: u16,
    /// Plugin CTA height (0 = hidden).
    pub cta_height: u16,
    /// Follow-up chips height (0 = hidden).
    pub follow_ups_height: u16,
    /// Startup warning height (0 = hidden).
    pub startup_warning_height: u16,
    /// Gap row between turn-status/scrollback and the prompt (0 or 1).
    pub prompt_gap: u16,
    /// Voice recording indicator height (0 = hidden).
    pub voice_recording_height: u16,
    /// Shortcuts bar height (always ≥ 1).
    pub shortcuts_height: u16,
    /// Timeline rail width (0 = hidden; requires scrollbar enabled).
    pub timeline_width: u16,
    /// Compact mode flag (affects padding).
    pub compact: bool,
}

impl AgentViewLayout {
    /// Compute layout from screen area, appearance config, and per-pane heights.
    ///
    /// When any optional pane height is `0`, both the pane and its separator
    /// gap are omitted from the constraint list.
    #[must_use]
    pub fn compute(
        area: Rect,
        layout_cfg: &LayoutConfig,
        scrollbar_cfg: &ScrollbarConfig,
        input: LayoutInput,
    ) -> Self {
        let compact = input.compact;
        let outer_vpad = layout_cfg.eff_outer_vpad(compact);
        let bottom_vpad = if area.height <= SHORT_TERMINAL_ROWS {
            0
        } else {
            outer_vpad
        };
        let cta_height = if area.height <= SHORT_TERMINAL_ROWS {
            0
        } else {
            input.cta_height
        };
        let follow_ups_height = if area.height <= SHORT_TERMINAL_ROWS {
            0
        } else {
            input.follow_ups_height
        };

        let top_vpad = outer_vpad;
        let outer_block = Block::default().padding(Padding::new(
            layout_cfg.eff_hpad_left(compact),
            layout_cfg.eff_hpad_right(compact),
            top_vpad,
            bottom_vpad,
        ));
        let inner_area = outer_block.inner(area);

        let mut constraints: Vec<Constraint> = Vec::new();

        if input.startup_warning_height > 0 {
            constraints.push(Constraint::Length(input.startup_warning_height));
        }

        let pane_gap: u16 = if top_vpad == 0 { 0 } else { 1 };
        if input.tasks_height > 0 {
            constraints.push(Constraint::Length(pane_gap));
            constraints.push(Constraint::Length(input.tasks_height));
        }
        if input.catalog_height > 0 {
            constraints.push(Constraint::Length(pane_gap));
            constraints.push(Constraint::Length(input.catalog_height));
        }
        if input.todo_height > 0 {
            constraints.push(Constraint::Length(pane_gap));
            constraints.push(Constraint::Length(input.todo_height));
        }

        let status_gap: u16 = if top_vpad == 0 { 0 } else { 1 };
        constraints.push(Constraint::Length(status_gap));
        constraints.push(Constraint::Min(5)); // Scrollback — dominant

        if input.btw_height > 0 {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(input.btw_height));
        }
        if input.queue_height > 0 {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(input.queue_height));
        }
        if input.turn_status_height > 0 {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(input.turn_status_height));
        }
        if input.banner_height > 0 {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(input.banner_height));
        }
        if cta_height > 0 {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(cta_height));
        }
        if follow_ups_height > 0 {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(follow_ups_height));
        }
        if input.prompt_gap > 0 {
            constraints.push(Constraint::Length(input.prompt_gap));
        }
        if input.voice_recording_height > 0 {
            constraints.push(Constraint::Length(input.voice_recording_height));
        }
        constraints.push(Constraint::Length(input.prompt_height));

        let shortcuts_gap: u16 = if input.shortcuts_height == 0 || bottom_vpad == 0 {
            0
        } else {
            1
        };
        if shortcuts_gap > 0 {
            constraints.push(Constraint::Length(shortcuts_gap));
        }
        if input.shortcuts_height > 0 {
            constraints.push(Constraint::Length(input.shortcuts_height));
        }

        let chunks = Layout::vertical(constraints).split(inner_area);

        let mut i = 0;

        let startup_warnings =
            Self::take_optional(&chunks, &mut i, input.startup_warning_height > 0);
        let tasks = Self::take_pane(&chunks, &mut i, input.tasks_height > 0);
        let catalog = Self::take_pane(&chunks, &mut i, input.catalog_height > 0);
        let todo = Self::take_pane(&chunks, &mut i, input.todo_height > 0);

        i += 1; // gap between the top panes and the scrollback
        let scrollback = chunks[i];
        i += 1;

        let btw = Self::take_section(&chunks, &mut i, input.btw_height > 0);
        let queue = Self::take_section(&chunks, &mut i, input.queue_height > 0);
        let turn_status = Self::take_section(&chunks, &mut i, input.turn_status_height > 0);
        let banner = Self::take_section(&chunks, &mut i, input.banner_height > 0);
        let plugin_cta = Self::take_section(&chunks, &mut i, cta_height > 0);
        let follow_ups = Self::take_section(&chunks, &mut i, follow_ups_height > 0);

        if input.prompt_gap > 0 {
            i += 1;
        }
        let voice_recording =
            Self::take_optional(&chunks, &mut i, input.voice_recording_height > 0);
        let prompt = chunks[i];
        i += 1;

        let shortcuts = if input.shortcuts_height > 0 {
            if shortcuts_gap > 0 {
                i += 1;
            }
            chunks[i]
        } else {
            Rect::ZERO
        };
        let scrollbar_x = area.right().saturating_sub(scrollbar_cfg.gap_right + 1);
        let timeline_width = if scrollbar_cfg.enabled {
            input.timeline_width
        } else {
            0
        };
        let timeline_x = (scrollbar_x + 1).saturating_sub(timeline_width);
        let content_end_x = if timeline_width > 0 {
            timeline_x.saturating_sub(scrollbar_cfg.gap_left)
        } else {
            scrollbar_x.saturating_sub(scrollbar_cfg.gap_left)
        };
        let scrollback_right = scrollback.x + scrollback.width;
        let scrollback_content = if !scrollbar_cfg.enabled || content_end_x >= scrollback_right {
            scrollback
        } else {
            Rect {
                width: content_end_x.saturating_sub(scrollback.x),
                ..scrollback
            }
        };

        Self {
            startup_warnings,
            tasks,
            catalog,
            scrollback,
            todo,
            queue,
            btw,
            turn_status,
            banner,
            plugin_cta,
            follow_ups,
            voice_recording,
            prompt,
            shortcuts,
            scrollback_content,
            scrollbar_x,
            timeline_x,
            timeline_width,
        }
    }

    /// Inner area width (for prompt height computation before full layout).
    #[must_use]
    pub fn inner_width(area: Rect, layout_cfg: &LayoutConfig, compact: bool) -> u16 {
        let vpad = layout_cfg.eff_outer_vpad(compact);
        let outer_block = Block::default().padding(Padding::new(
            layout_cfg.eff_hpad_left(compact),
            layout_cfg.eff_hpad_right(compact),
            vpad,
            vpad,
        ));
        outer_block.inner(area).width
    }

    /// Convert to [`PaneAreas`] for mouse hit-testing.
    #[must_use]
    pub fn pane_areas(&self) -> PaneAreas {
        PaneAreas {
            scrollback: self.scrollback,
            todo: self.todo,
            queue: self.queue,
            prompt: self.prompt,
            tasks: self.tasks,
            catalog: self.catalog,
        }
    }

    fn take_optional(chunks: &[Rect], i: &mut usize, present: bool) -> Rect {
        if present {
            let r = chunks[*i];
            *i += 1;
            r
        } else {
            Rect::default()
        }
    }

    fn take_section(chunks: &[Rect], i: &mut usize, present: bool) -> Rect {
        if present {
            *i += 1;
            let r = chunks[*i];
            *i += 1;
            r
        } else {
            Rect::default()
        }
    }

    fn take_pane(chunks: &[Rect], i: &mut usize, present: bool) -> Rect {
        if present {
            *i += 1;
            let r = chunks[*i];
            *i += 1;
            r
        } else {
            Rect::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(h: u16) -> Rect {
        Rect::new(0, 0, 80, h)
    }

    #[test]
    fn basic_layout_minimal() {
        let layout = AgentViewLayout::compute(
            screen(24),
            &LayoutConfig::default(),
            &ScrollbarConfig::default(),
            LayoutInput {
                prompt_height: 3,
                shortcuts_height: 1,
                ..Default::default()
            },
        );
        // No dedicated chrome row above the scrollback: it starts directly
        // under the outer vpad plus the pane/scrollback separator gap.
        let vpad = LayoutConfig::default().eff_outer_vpad(false);
        assert_eq!(
            layout.scrollback.y,
            vpad + u16::from(vpad > 0),
            "scrollback starts at the vpad (+gap), no status-bar row"
        );
        assert!(layout.scrollback.height >= 5);
        assert!(layout.prompt.y < layout.shortcuts.y);
    }

    #[test]
    fn all_panes_visible() {
        let layout = AgentViewLayout::compute(
            screen(60),
            &LayoutConfig::default(),
            &ScrollbarConfig::default(),
            LayoutInput {
                prompt_height: 3,
                shortcuts_height: 1,
                tasks_height: 5,
                catalog_height: 4,
                todo_height: 5,
                queue_height: 4,
                turn_status_height: 1,
                banner_height: 1,
                ..Default::default()
            },
        );
        assert!(layout.tasks.height > 0);
        assert!(layout.todo.height > 0);
        assert!(layout.queue.height > 0);
        assert!(layout.tasks.y < layout.scrollback.y);
        assert!(layout.queue.y > layout.scrollback.y);
    }

    #[test]
    fn optional_panes_collapse_to_zero() {
        let layout = AgentViewLayout::compute(
            screen(24),
            &LayoutConfig::default(),
            &ScrollbarConfig::default(),
            LayoutInput {
                prompt_height: 3,
                shortcuts_height: 1,
                ..Default::default()
            },
        );
        assert_eq!(layout.tasks, Rect::default());
        assert_eq!(layout.todo, Rect::default());
    }

    #[test]
    fn short_terminal_suppresses_cta_and_followups() {
        let layout = AgentViewLayout::compute(
            screen(SHORT_TERMINAL_ROWS),
            &LayoutConfig::default(),
            &ScrollbarConfig::default(),
            LayoutInput {
                prompt_height: 3,
                shortcuts_height: 1,
                cta_height: 1,
                follow_ups_height: 1,
                ..Default::default()
            },
        );
        assert_eq!(layout.plugin_cta, Rect::default());
        assert_eq!(layout.follow_ups, Rect::default());
    }

    #[test]
    fn pane_areas_hit_test() {
        let layout = AgentViewLayout::compute(
            screen(24),
            &LayoutConfig::default(),
            &ScrollbarConfig::default(),
            LayoutInput {
                prompt_height: 3,
                shortcuts_height: 1,
                todo_height: 5,
                ..Default::default()
            },
        );
        let areas = layout.pane_areas();
        assert_eq!(
            areas.hit_test(layout.scrollback.x, layout.scrollback.y),
            Some(ActivePane::Scrollback)
        );
        assert_eq!(
            areas.hit_test(layout.todo.x, layout.todo.y),
            Some(ActivePane::Todo)
        );
        assert_eq!(
            areas.hit_test(layout.prompt.x, layout.prompt.y),
            Some(ActivePane::Prompt)
        );
    }

    #[test]
    fn active_pane_cycle() {
        let areas = PaneAreas {
            scrollback: Rect::new(0, 0, 10, 10),
            prompt: Rect::new(0, 10, 10, 3),
            ..Default::default()
        };
        assert_eq!(ActivePane::Scrollback.cycle(&areas), ActivePane::Prompt);
        assert_eq!(ActivePane::Prompt.cycle(&areas), ActivePane::Scrollback);
    }

    #[test]
    fn effective_compact_logic() {
        assert!(!effective_compact(false, 0));
        assert!(effective_compact(false, AUTO_COMPACT_MAX_ROWS));
        assert!(effective_compact(false, 10));
        assert!(!effective_compact(false, 30));
        assert!(effective_compact(true, 100));
    }

    #[test]
    fn inner_width_without_padding() {
        let w =
            AgentViewLayout::inner_width(Rect::new(0, 0, 80, 24), &LayoutConfig::default(), false);
        assert_eq!(w, 76);
    }
}
