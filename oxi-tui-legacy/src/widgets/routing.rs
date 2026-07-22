//! RoutingStatus widget — collapsible panel showing auto-routing state, fallback chain, and provider health.
//!
//! Displays:
//! - Auto-routing enabled/disabled
//! - Fallback enabled/disabled
//! - Current fallback chain (abbreviated names)
//! - Provider health indicators (colored dots based on circuit breaker state)
//!
//! Triggered by Ctrl+R, rendered as a bordered overlay panel.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
};

use crate::Theme;

// ── Data types ──────────────────────────────────────────────────────────

/// Circuit breaker / health state for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealth {
    /// Provider is healthy and accepting requests.
    Healthy,
    /// Provider has some failures, in degraded mode.
    Degraded,
    /// Provider has tripped the circuit breaker, marked as unavailable.
    Unavailable,
    /// Provider has been explicitly disabled by the user.
    Disabled,
}

impl ProviderHealth {
    fn symbol(&self, symbols: &crate::symbols::Symbols) -> &'static str {
        match self {
            ProviderHealth::Healthy => symbols.dot_on,
            ProviderHealth::Degraded => symbols.dot_on,
            ProviderHealth::Unavailable => symbols.dot_off,
            ProviderHealth::Disabled => symbols.dot_off,
        }
    }
}

/// One provider in the fallback chain.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// Display name (e.g. "anthropic", "google").
    pub name: String,
    /// Provider health state.
    pub health: ProviderHealth,
    /// Number of consecutive failures (for Degraded display).
    pub failures: u32,
    /// Whether this is the currently active provider.
    pub is_active: bool,
}

/// Routing status data — all state needed to render the panel.
#[derive(Debug, Clone)]
pub struct RoutingStatusData {
    /// Whether auto-routing is enabled.
    pub auto_routing_enabled: bool,
    /// Whether fallback is enabled.
    pub fallback_enabled: bool,
    /// Current fallback chain (in priority order).
    pub fallback_chain: Vec<ProviderInfo>,
    /// Currently selected/active provider index in fallback_chain.
    pub active_index: usize,
}

impl Default for RoutingStatusData {
    fn default() -> Self {
        Self {
            auto_routing_enabled: true,
            fallback_enabled: true,
            fallback_chain: Vec::new(),
            active_index: 0,
        }
    }
}

impl RoutingStatusData {
    /// Returns a short human-readable label for the active provider.
    pub fn active_provider_name(&self) -> Option<&str> {
        self.fallback_chain
            .get(self.active_index)
            .map(|p| p.name.as_str())
    }

    /// Returns abbreviated fallback chain names joined by " > ".
    pub fn chain_abbreviated(&self) -> String {
        self.fallback_chain
            .iter()
            .map(|p| {
                // Shorten provider name: "anthropic" -> "anth", "openai" -> "opn"
                let short = if p.name.len() > 4 {
                    &p.name[..4]
                } else {
                    &p.name
                };
                if p.is_active {
                    format!("[{}]", short)
                } else {
                    short.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

// ── State ────────────────────────────────────────────────────────────────

/// State for the RoutingStatus widget.
#[derive(Debug, Default)]
pub struct RoutingStatusState {
    /// Whether the panel is currently visible.
    pub visible: bool,
    /// Data to display.
    pub data: RoutingStatusData,
}

impl RoutingStatusState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle panel visibility.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Show the panel.
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Hide the panel.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Update the routing status data.
    pub fn set_data(&mut self, data: RoutingStatusData) {
        self.data = data;
    }
}

// ── Widget ───────────────────────────────────────────────────────────────

pub struct RoutingStatus<'a> {
    theme: &'a Theme,
}

impl<'a> RoutingStatus<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl StatefulWidget for RoutingStatus<'_> {
    type State = RoutingStatusState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if !state.visible || area.width < 10 || area.height < 4 {
            return;
        }
        // Fill the overlay area with panel_bg before drawing borders.
        buf.set_style(
            area,
            ratatui::style::Style::default().bg(self.theme.colors.panel_bg),
        );

        let styles = self.theme.to_styles();
        let d = &state.data;

        // Block with title
        let title = format!(" {} Routing Status ", styles.symbols.icon_thinking);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles.border)
            .title(Span::styled(
                title,
                styles.accent.add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(area);
        block.render(area, buf);

        let max_w = inner.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();

        // ── Auto-Routing row ──────────────────────────────────────────────
        {
            let enabled_ch = if d.auto_routing_enabled {
                styles.symbols.dot_on
            } else {
                styles.symbols.dot_off
            };
            let enabled_style = if d.auto_routing_enabled {
                styles.success
            } else {
                styles.muted
            };
            let enabled_label = if d.auto_routing_enabled { "ON" } else { "OFF" };

            let fallback_ch = if d.fallback_enabled {
                styles.symbols.dot_on
            } else {
                styles.symbols.dot_off
            };
            let fallback_style = if d.fallback_enabled {
                styles.success
            } else {
                styles.muted
            };
            let fallback_label = if d.fallback_enabled { "ON" } else { "OFF" };

            lines.push(Line::from(vec![
                Span::styled("  Auto-Route ", styles.muted),
                Span::styled(enabled_ch, enabled_style),
                Span::styled(format!(" {}  ", enabled_label), enabled_style),
                Span::styled("  Fallback ", styles.muted),
                Span::styled(fallback_ch, fallback_style),
                Span::styled(format!(" {}", fallback_label), fallback_style),
            ]));
        }

        // ── Separator ──────────────────────────────────────────────────────
        lines.push(Line::from(Span::styled(
            format!("  {}", styles.symbols.rule.repeat(max_w.saturating_sub(2))),
            styles.border,
        )));

        // ── Fallback Chain ─────────────────────────────────────────────────
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Fallback Chain:", styles.muted),
        ]));

        if d.fallback_chain.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("(none configured)", styles.muted),
            ]));
        } else {
            for provider in d.fallback_chain.iter() {
                let health_ch = provider.health.symbol(&styles.symbols);
                let health_style = match provider.health {
                    ProviderHealth::Healthy => styles.success,
                    ProviderHealth::Degraded => styles.warning,
                    ProviderHealth::Unavailable => styles.error,
                    ProviderHealth::Disabled => styles.muted,
                };

                let active_marker = if provider.is_active {
                    format!(" {}", styles.symbols.nav_selected)
                } else {
                    "  ".to_string()
                };

                // Build a single line: "  ● provider_name (n failures)"
                let mut line_spans: Vec<Span<'static>> = vec![
                    Span::raw("  "),
                    Span::styled(health_ch, health_style),
                    Span::raw(active_marker),
                    Span::styled(
                        provider.name.clone(),
                        if provider.is_active {
                            styles.primary
                        } else {
                            styles.muted
                        },
                    ),
                ];

                if provider.failures > 0 {
                    line_spans.push(Span::styled(
                        format!(
                            " ({} fail{})",
                            provider.failures,
                            if provider.failures == 1 { "" } else { "s" }
                        ),
                        styles.warning,
                    ));
                }

                lines.push(Line::from(line_spans));
            }
        }

        // ── Footer hint ─────────────────────────────────────────────────────
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Ctrl+R ", styles.primary),
            Span::styled("to close", styles.muted),
        ]));

        let text: ratatui::text::Text = lines.into_iter().collect();
        Paragraph::new(text).render(inner, buf);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_health_symbols() {
        let uni = crate::symbols::Symbols::unicode();
        let asc = crate::symbols::Symbols::ascii();
        // Healthy/degraded share the "on" dot; unavailable/disabled the "off".
        assert_eq!(ProviderHealth::Healthy.symbol(&uni), uni.dot_on);
        assert_eq!(ProviderHealth::Degraded.symbol(&uni), uni.dot_on);
        assert_eq!(ProviderHealth::Unavailable.symbol(&uni), uni.dot_off);
        assert_eq!(ProviderHealth::Disabled.symbol(&uni), uni.dot_off);
        // ASCII preset swaps the dots for 7-bit markers.
        assert_eq!(ProviderHealth::Healthy.symbol(&asc), asc.dot_on);
        assert_eq!(ProviderHealth::Unavailable.symbol(&asc), asc.dot_off);
    }

    #[test]
    fn routing_status_data_default() {
        let data = RoutingStatusData::default();
        assert!(data.auto_routing_enabled);
        assert!(data.fallback_enabled);
        assert!(data.fallback_chain.is_empty());
        assert_eq!(data.active_index, 0);
    }

    #[test]
    fn chain_abbreviated_empty() {
        let data = RoutingStatusData::default();
        assert_eq!(data.chain_abbreviated(), "");
    }

    #[test]
    fn chain_abbreviated_single() {
        let data = RoutingStatusData {
            fallback_chain: vec![ProviderInfo {
                name: "anthropic".to_string(),
                health: ProviderHealth::Healthy,
                failures: 0,
                is_active: true,
            }],
            ..Default::default()
        };
        assert_eq!(data.chain_abbreviated(), "[anth]");
    }

    #[test]
    fn chain_abbreviated_multiple() {
        let data = RoutingStatusData {
            fallback_chain: vec![
                ProviderInfo {
                    name: "anthropic".to_string(),
                    health: ProviderHealth::Healthy,
                    failures: 0,
                    is_active: true,
                },
                ProviderInfo {
                    name: "openai".to_string(),
                    health: ProviderHealth::Degraded,
                    failures: 3,
                    is_active: false,
                },
            ],
            ..Default::default()
        };
        assert_eq!(data.chain_abbreviated(), "[anth] > open");
    }

    #[test]
    fn routing_status_state_toggle() {
        let mut state = RoutingStatusState::new();
        assert!(!state.visible);
        state.toggle();
        assert!(state.visible);
        state.toggle();
        assert!(!state.visible);
    }
}
