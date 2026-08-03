//! Welcome screen layout — vertically centered logo + menu + prompt.
//!
//! Ported from grok-build's `views/welcome/mod.rs` (`WelcomeLayout`).
//!
//! Two layout variants:
//! - **Stacked** (narrow): logo → gap → menu → gap → prompt → version
//! - **Hero box** (wide ≥ `HERO_BOX_MIN_WIDTH`): side-by-side logo + menu

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Padding};

use super::config::LayoutConfig;

/// Minimum terminal width for the hero-box layout.
pub const HERO_BOX_MIN_WIDTH: u16 = 90;

/// Prompt input height (shared across both variants).
pub const PROMPT_HEIGHT: u16 = 3;

const VERSION_GAP: u16 = 1;
const H_MARGIN: u16 = 2;
const H_MARGIN_COMPACT: u16 = 1;

fn logo_height_default() -> u16 {
    7
}

/// Which focus state the welcome prompt is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WelcomePromptFocus {
    /// Prompt is focused (accepting text input).
    #[default]
    Prompt,
    /// Session picker is focused.
    SessionPicker,
}

/// Computed areas for the welcome screen layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WelcomeLayout {
    /// Logo area.
    pub logo: Rect,
    /// Error / warning row.
    pub error: Rect,
    /// Menu items area.
    pub menu: Rect,
    /// Info slot below the menu (changelog / announcement).
    pub changelog: Rect,
    /// Tip row.
    pub tip: Rect,
    /// Prompt input widget.
    pub prompt: Rect,
    /// Version badge row.
    pub version: Rect,
    // Hero box sub-rects (all zero when inactive).
    /// Outer hero box border.
    pub hero_box: Rect,
    /// Logo inside the hero box.
    pub hero_logo: Rect,
    /// Version inside the hero box.
    pub hero_version: Rect,
    /// Subtitle row inside the hero box.
    pub hero_subtitle: Rect,
    /// Info slot inside the hero box.
    pub hero_info: Rect,
    /// Menu inside the hero box.
    pub hero_menu: Rect,
}

impl WelcomeLayout {
    /// Whether the hero box is active.
    #[must_use]
    pub fn has_hero_box(&self) -> bool {
        self.hero_box.width > 0 && self.hero_box.height > 0
    }

    /// Fixed rows below the menu+tip+prompt+version block.
    #[must_use]
    pub fn fixed_below(tip_height: u16) -> u16 {
        let tip_gap = u16::from(tip_height > 0);
        tip_height + tip_gap + PROMPT_HEIGHT + VERSION_GAP + 1
    }

    /// Minimum content height for the hero-box layout.
    #[must_use]
    pub fn min_hero_box_height(error_height: u16, menu_height: u16, tip_height: u16) -> u16 {
        let inner = menu_height.max(logo_height_default())
            + error_height
            + tip_height
            + Self::fixed_below(tip_height);
        inner + 2
    }

    /// Compute the welcome screen layout.
    #[must_use]
    pub fn compute(
        content_area: Rect,
        logo_height: u16,
        error_height: u16,
        menu_height: u16,
        tip_height: u16,
        changelog_height: u16,
        compact: bool,
    ) -> Self {
        let use_hero_box = !compact
            && content_area.width >= HERO_BOX_MIN_WIDTH
            && menu_height > 0
            && content_area.height
                >= Self::min_hero_box_height(error_height, menu_height, tip_height);

        if use_hero_box {
            return Self::compute_hero_box(content_area, error_height, menu_height, tip_height);
        }
        Self::compute_stacked(
            content_area,
            logo_height,
            error_height,
            menu_height,
            tip_height,
            changelog_height,
            compact,
        )
    }

    fn compute_stacked(
        content_area: Rect,
        logo_height: u16,
        error_height: u16,
        menu_height: u16,
        tip_height: u16,
        changelog_height: u16,
        compact: bool,
    ) -> Self {
        let zero = Rect::default();
        let logo_rows = if compact { 0 } else { logo_height };
        let gap_after_logo = u16::from(error_height > 0);
        let tip_gap = u16::from(tip_height > 0);
        let fixed_below = Self::fixed_below(tip_height);
        let fixed_above = logo_rows + 1 + gap_after_logo + error_height;

        let (eff_cl_h, eff_cl_gap) = if !compact && changelog_height > 0 {
            let cg = 1u16;
            let needed = fixed_above + menu_height + cg + changelog_height + 1 + fixed_below;
            if content_area.height >= needed {
                (changelog_height, cg)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        let top_pad = if compact {
            0
        } else {
            let rem = content_area.height.saturating_sub(fixed_above);
            rem.saturating_sub(menu_height)
                .saturating_sub(eff_cl_gap + eff_cl_h)
                .saturating_sub(fixed_below)
                / 2
        };

        let [
            _,
            logo,
            _,
            _,
            error,
            menu,
            _,
            changelog,
            _,
            tip,
            _,
            prompt,
            _,
            version,
        ] = Layout::vertical([
            Constraint::Length(top_pad),
            Constraint::Length(logo_rows),
            Constraint::Length(1),
            Constraint::Length(gap_after_logo),
            Constraint::Length(error_height),
            Constraint::Length(menu_height),
            Constraint::Length(eff_cl_gap),
            Constraint::Length(eff_cl_h),
            Constraint::Min(1),
            Constraint::Length(tip_height),
            Constraint::Length(tip_gap),
            Constraint::Length(PROMPT_HEIGHT),
            Constraint::Length(VERSION_GAP),
            Constraint::Length(1),
        ])
        .areas(content_area);

        Self {
            logo,
            error,
            menu,
            changelog,
            tip,
            prompt,
            version,
            hero_box: zero,
            hero_logo: zero,
            hero_version: zero,
            hero_subtitle: zero,
            hero_info: zero,
            hero_menu: zero,
        }
    }

    fn compute_hero_box(
        content_area: Rect,
        error_height: u16,
        menu_height: u16,
        tip_height: u16,
    ) -> Self {
        let zero = Rect::default();
        let hero_inner_h = menu_height.max(logo_height_default()) + error_height + tip_height;
        let hero_h = hero_inner_h + 2;
        let hero_w = content_area.width.min(HERO_BOX_MIN_WIDTH);
        let total_needed = hero_h + 1 + PROMPT_HEIGHT + VERSION_GAP + 1;
        let top_pad = content_area.height.saturating_sub(total_needed) / 2;
        let hero_y = content_area.y + top_pad.min(content_area.height.saturating_sub(hero_h));
        let hero_x = content_area.x + (content_area.width.saturating_sub(hero_w)) / 2;
        let hero_box = Rect::new(hero_x, hero_y, hero_w, hero_h);
        let hero_inner = Block::default()
            .padding(Padding::symmetric(1, 0))
            .inner(hero_box);
        let left_w = (hero_inner.width / 3).max(20);
        let [hero_logo, hero_menu] =
            Layout::horizontal([Constraint::Length(left_w), Constraint::Min(1)]).areas(hero_inner);
        let below_y = hero_box.y + hero_box.height;
        let prompt = Rect::new(
            content_area.x + H_MARGIN,
            below_y + 1,
            content_area.width.saturating_sub(H_MARGIN * 2),
            PROMPT_HEIGHT,
        );
        let version = Rect::new(
            content_area.x + H_MARGIN,
            prompt.y + PROMPT_HEIGHT + VERSION_GAP,
            content_area.width.saturating_sub(H_MARGIN * 2),
            1,
        );

        Self {
            logo: zero,
            error: Rect::new(hero_inner.x, hero_logo.y, hero_inner.width, error_height),
            menu: zero,
            changelog: zero,
            tip: Rect::new(
                hero_inner.x,
                hero_logo.y + hero_logo.height.saturating_sub(tip_height),
                hero_inner.width,
                tip_height,
            ),
            prompt,
            version,
            hero_box,
            hero_logo,
            hero_version: zero,
            hero_subtitle: zero,
            hero_info: zero,
            hero_menu,
        }
    }

    /// Horizontal margin for the given compact flag.
    #[must_use]
    pub fn h_margin(compact: bool) -> u16 {
        if compact { H_MARGIN_COMPACT } else { H_MARGIN }
    }
}

/// Outer padding for the welcome screen.
#[must_use]
pub fn welcome_outer_padding(layout_cfg: &LayoutConfig, compact: bool) -> Padding {
    let h = if compact { 1 } else { layout_cfg.hpad_left };
    let v = if compact { 0 } else { layout_cfg.outer_vpad };
    Padding::new(h, h, v, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn stacked_layout_basic() {
        let l = WelcomeLayout::compute(screen(80, 30), 7, 0, 7, 0, 0, false);
        assert!(l.logo.height > 0);
        assert!(l.menu.height > 0);
        assert_eq!(l.prompt.height, PROMPT_HEIGHT);
        assert!(l.logo.y < l.menu.y);
        assert!(!l.has_hero_box());
    }

    #[test]
    fn hero_box_on_wide() {
        let l = WelcomeLayout::compute(screen(120, 30), 7, 0, 7, 0, 0, false);
        assert!(l.has_hero_box());
        assert!(l.hero_logo.width > 0);
    }

    #[test]
    fn compact_skips_logo() {
        let l = WelcomeLayout::compute(screen(80, 20), 7, 0, 7, 0, 0, true);
        assert_eq!(l.logo.height, 0);
    }

    #[test]
    fn version_below_prompt() {
        let l = WelcomeLayout::compute(screen(80, 30), 7, 0, 7, 0, 0, false);
        assert!(l.version.y > l.prompt.y);
    }
}
