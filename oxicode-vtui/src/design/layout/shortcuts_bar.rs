//! Shortcuts bar — bottom keyboard hint strip.
//!
//! Ported from grok-build's `views/shortcuts_bar.rs`.
//!
//! Styling is decoupled via the [`ShortcutBarStyling`] trait, mirroring the
//! existing `PanelStyleProvider` pattern.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

// ───────────────────────────────────────────────────────────────────────────
// ShortcutBarStyling trait
// ───────────────────────────────────────────────────────────────────────────

/// Trait for providing shortcuts bar styles.  Decouples the widget from any
/// specific theme type.
pub trait ShortcutBarStyling {
    /// Style for key labels (e.g., "Enter", "Esc").
    fn key_style(&self) -> Style;
    /// Style for human-readable labels (e.g., "send", "cancel").
    fn label_style(&self) -> Style;
    /// Style for separators between hints.
    fn separator_style(&self) -> Style;
    /// Background fill style.
    fn background_style(&self) -> Style;
    /// Style for the pending-action key.
    fn pending_key_style(&self) -> Style;
}

// ───────────────────────────────────────────────────────────────────────────
// HintItem
// ───────────────────────────────────────────────────────────────────────────

/// A single keyboard hint for the shortcuts bar.
#[derive(Debug, Clone)]
pub struct HintItem {
    /// Key display string (e.g., `"Enter"`, `"Ctrl+C"`).
    pub key: String,
    /// Optional secondary key for paired hints.
    pub alt_key: Option<String>,
    /// Human-readable label.
    pub label: String,
    /// Whether this hint is pinned (always visible in compact mode).
    pub pinned: bool,
}

impl HintItem {
    /// Create a new hint.
    #[must_use]
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            alt_key: None,
            label: label.into(),
            pinned: false,
        }
    }

    /// Create a paired hint (two keys, one label).
    #[must_use]
    pub fn paired(
        key: impl Into<String>,
        alt: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            alt_key: Some(alt.into()),
            label: label.into(),
            pinned: false,
        }
    }

    /// Mark this hint as pinned.
    #[must_use]
    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// Display width.
    #[must_use]
    pub fn display_width(&self) -> usize {
        let key_w = if let Some(alt) = &self.alt_key {
            self.key.chars().count() + 1 + alt.chars().count()
        } else {
            self.key.chars().count()
        };
        key_w + 2 + self.label.chars().count()
    }

    /// Build the styled spans for this hint.
    fn spans<S: ShortcutBarStyling>(&self, styles: &S) -> Vec<Span<'static>> {
        let mut spans = Vec::with_capacity(5);
        if let Some(alt) = &self.alt_key {
            spans.push(Span::styled(self.key.clone(), styles.key_style()));
            spans.push(Span::styled("/", styles.separator_style()));
            spans.push(Span::styled(alt.clone(), styles.key_style()));
        } else {
            spans.push(Span::styled(self.key.clone(), styles.key_style()));
        }
        spans.push(Span::styled(":", styles.separator_style()));
        spans.push(Span::styled(self.label.clone(), styles.label_style()));
        spans
    }
}

// ───────────────────────────────────────────────────────────────────────────
// CompactConfig + PendingHint
// ───────────────────────────────────────────────────────────────────────────

/// Compact-mode configuration for the shortcuts bar.
#[derive(Debug, Clone, Copy)]
pub struct CompactConfig {
    /// Maximum hints to display (pinned always included).
    pub max_visible: usize,
    /// Key for the trailing help hint.
    pub help_key: &'static str,
    /// Label for the trailing help hint.
    pub help_label: &'static str,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            max_visible: 8,
            help_key: "?",
            help_label: "help",
        }
    }
}

/// Info for the "press again to confirm" pending-action hint.
#[derive(Clone, Copy)]
pub struct PendingHint {
    /// Key to press again.
    pub key: &'static str,
    /// What action will be confirmed.
    pub label: &'static str,
}

// ───────────────────────────────────────────────────────────────────────────
// ShortcutsBar widget
// ───────────────────────────────────────────────────────────────────────────

/// Shortcuts bar widget.  Renders [`HintItem`]s in a single row.
pub struct ShortcutsBar<'a, S: ShortcutBarStyling> {
    hints: &'a [HintItem],
    styles: &'a S,
    compact: Option<&'a CompactConfig>,
    pending: Option<PendingHint>,
    right: Option<Line<'a>>,
}

impl<'a, S: ShortcutBarStyling> ShortcutsBar<'a, S> {
    /// Create a shortcuts bar with the given hints and styles.
    #[must_use]
    pub fn new(hints: &'a [HintItem], styles: &'a S) -> Self {
        Self {
            hints,
            styles,
            compact: None,
            pending: None,
            right: None,
        }
    }

    /// Enable compact mode.
    #[must_use]
    pub fn compact(mut self, cfg: &'a CompactConfig) -> Self {
        self.compact = Some(cfg);
        self
    }

    /// Set the pending-action confirmation hint.
    #[must_use]
    pub fn pending(mut self, hint: PendingHint) -> Self {
        self.pending = Some(hint);
        self
    }

    /// Set a right-aligned status line (e.g. scroll position).
    ///
    /// Skipped when it would overlap the left hints or while a pending
    /// confirmation hint owns the row.
    #[must_use]
    pub fn right(mut self, line: Line<'a>) -> Self {
        self.right = Some(line);
        self
    }
}

impl<S: ShortcutBarStyling> Widget for ShortcutsBar<'_, S> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        buf.set_style(area, self.styles.background_style());

        if let Some(pending) = self.pending {
            let line = Line::from(vec![
                Span::raw("press "),
                Span::styled(pending.key, self.styles.pending_key_style()),
                Span::raw(" again to "),
                Span::styled(pending.label, self.styles.label_style()),
            ]);
            buf.set_line(area.x, area.y, &line, area.width);
            return;
        }

        let effective = compute_effective_hints(self.hints, self.compact);
        let mut x = area.x;
        let separator = Span::styled("  ", self.styles.separator_style());

        for (i, hint) in effective.iter().enumerate() {
            if i > 0 {
                let sep_w = separator.width() as u16;
                if x + sep_w > area.x + area.width {
                    break;
                }
                buf.set_span(x, area.y, &separator, sep_w);
                x += sep_w;
            }
            let spans = hint.spans(self.styles);
            let line = Line::from(spans);
            let w = hint.display_width() as u16;
            if x + w > area.x + area.width {
                break;
            }
            buf.set_line(x, area.y, &line, w);
            x += w;
        }

        if let Some(right) = self.right {
            let w = right.width() as u16;
            let right_x = area.x + area.width.saturating_sub(w);
            if right_x > x {
                buf.set_line(right_x, area.y, &right, w);
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Hint computation
// ───────────────────────────────────────────────────────────────────────────

/// Compute the hint list the bar will actually render.
#[must_use]
pub fn compute_effective_hints<'a>(
    hints: &'a [HintItem],
    compact: Option<&'a CompactConfig>,
) -> Vec<&'a HintItem> {
    let Some(cfg) = compact else {
        return hints.iter().collect();
    };

    let mut result: Vec<&HintItem> = Vec::new();
    let pinned_count = hints.iter().filter(|h| h.pinned).count();

    for h in hints.iter().filter(|h| h.pinned) {
        result.push(h);
    }
    let remaining = cfg.max_visible.saturating_sub(pinned_count);
    for h in hints.iter().filter(|h| !h.pinned).take(remaining) {
        result.push(h);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStyles;
    impl ShortcutBarStyling for TestStyles {
        fn key_style(&self) -> Style {
            Style::default()
        }
        fn label_style(&self) -> Style {
            Style::default()
        }
        fn separator_style(&self) -> Style {
            Style::default()
        }
        fn background_style(&self) -> Style {
            Style::default()
        }
        fn pending_key_style(&self) -> Style {
            Style::default()
        }
    }

    #[test]
    fn hint_item_display_width() {
        assert_eq!(HintItem::new("Enter", "send").display_width(), 11); // "Enter: send" = 11 cols
    }

    #[test]
    fn compute_effective_compact_preserves_pinned() {
        let hints = vec![
            HintItem::new("a", "x").pinned(),
            HintItem::new("b", "y"),
            HintItem::new("c", "z"),
        ];
        let cfg = CompactConfig {
            max_visible: 2,
            ..Default::default()
        };
        let eff = compute_effective_hints(&hints, Some(&cfg));
        assert_eq!(eff.len(), 2);
        assert_eq!(eff[0].key, "a");
        assert_eq!(eff[1].key, "b");
    }

    #[test]
    fn shortcuts_bar_renders_without_panic() {
        let styles = TestStyles;
        let hints = vec![
            HintItem::new("Enter", "send"),
            HintItem::new("Esc", "cancel"),
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        ShortcutsBar::new(&hints, &styles).render(Rect::new(0, 0, 80, 1), &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "E");
    }

    #[test]
    fn shortcuts_bar_pending_mode() {
        let styles = TestStyles;
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        ShortcutsBar::new(&[], &styles)
            .pending(PendingHint {
                key: "q",
                label: "quit",
            })
            .render(Rect::new(0, 0, 80, 1), &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "p");
    }
}
