//! Color blending and fading utilities.
//!
//! These utilities support smooth fade transitions (e.g., for sticky headers
//! being pushed off screen) by blending colors toward a base color.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};

/// The 6 channel values in the 256-color 6×6×6 cube.
const CUBE_VALUES: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Convert a 256-color indexed color to its (R, G, B) components.
///
/// Handles all three regions of the 256-color palette:
/// - 0–15:    standard/bright ANSI colors (uses common xterm defaults)
/// - 16–231:  6×6×6 color cube
/// - 232–255: 24-step grayscale ramp
pub fn indexed_to_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        // Standard colors (0–7) — common xterm defaults
        0 => (0, 0, 0),
        1 => (128, 0, 0),
        2 => (0, 128, 0),
        3 => (128, 128, 0),
        4 => (0, 0, 128),
        5 => (128, 0, 128),
        6 => (0, 128, 128),
        7 => (192, 192, 192),
        // Bright colors (8–15)
        8 => (128, 128, 128),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (0, 0, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        15 => (255, 255, 255),
        // 6×6×6 color cube (16–231)
        16..=231 => {
            let n = index - 16;
            let r = CUBE_VALUES[(n / 36) as usize];
            let g = CUBE_VALUES[((n % 36) / 6) as usize];
            let b = CUBE_VALUES[(n % 6) as usize];
            (r, g, b)
        }
        // Grayscale ramp (232–255): value = 8 + (index − 232) × 10
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    }
}

/// Map an RGB triplet to the nearest 256-color palette index (16–255).
///
/// Searches both the 6×6×6 color cube (16–231) and the 24-step grayscale
/// ramp (232–255), returning whichever has the smallest squared Euclidean
/// distance.
pub fn nearest_indexed(r: u8, g: u8, b: u8) -> u8 {
    // --- nearest in the 6×6×6 color cube (16–231) ---
    let ri = nearest_cube_channel(r);
    let gi = nearest_cube_channel(g);
    let bi = nearest_cube_channel(b);
    let cube_idx = 16 + 36 * ri as u16 + 6 * gi as u16 + bi as u16;
    let cube_dist = sq_dist(
        r,
        g,
        b,
        CUBE_VALUES[ri as usize],
        CUBE_VALUES[gi as usize],
        CUBE_VALUES[bi as usize],
    );

    // --- nearest in the grayscale ramp (232–255) ---
    // Ramp values: 8, 18, 28, …, 238  (24 entries)
    let lum = (r as u16 + g as u16 + b as u16) / 3;
    let gray_step = if lum <= 3 {
        0u8
    } else if lum >= 243 {
        23
    } else {
        ((lum as i16 - 8 + 5) / 10).clamp(0, 23) as u8
    };
    let gv = (8 + gray_step as u16 * 10) as u8;
    let gray_dist = sq_dist(r, g, b, gv, gv, gv);

    if gray_dist < cube_dist {
        232 + gray_step
    } else {
        cube_idx as u8
    }
}

/// Find the nearest index (0–5) into [`CUBE_VALUES`] for a single channel.
fn nearest_cube_channel(v: u8) -> u8 {
    let mut best = 0u8;
    let mut best_d = v.abs_diff(CUBE_VALUES[0]) as u16;
    for i in 1..6u8 {
        let d = v.abs_diff(CUBE_VALUES[i as usize]) as u16;
        if d < best_d {
            best = i;
            best_d = d;
        }
    }
    best
}

/// Squared Euclidean distance between two RGB colors.
fn sq_dist(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = r1 as i32 - r2 as i32;
    let dg = g1 as i32 - g2 as i32;
    let db = b1 as i32 - b2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// Extract (R, G, B) from a Color, supporting both Rgb and Indexed variants.
///
/// Returns `None` for named ANSI colors (Color::Red, etc.) and Color::Reset.
fn color_to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(n) => Some(indexed_to_rgb(n)),
        _ => None,
    }
}

/// Map every [`Color`] variant to an xterm-default RGB triple. `None`
/// only for `Color::Reset` (no defined RGB — caller chooses a fallback).
///
/// Useful when downstream code must produce RGB for *every* color value
/// — e.g. progress-bar gradients that lerp across named breakpoints, or
/// OSC 12 cursor-color updates that must emit an RGB triple regardless
/// of terminal color depth.
///
/// Named-color RGB matches the xterm 16-color palette used by
/// [`indexed_to_rgb`] for indices 0–15; the user's terminal may have
/// customised those entries, so the result is "approximate but
/// consistent with our other colorimetry".
pub fn resolve_to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    let idx: u8 = match color {
        Color::Rgb(r, g, b) => return Some((r, g, b)),
        Color::Indexed(n) => return Some(indexed_to_rgb(n)),
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset => return None,
    };
    Some(indexed_to_rgb(idx))
}

/// Blend a single color channel: lerp from base toward original based on opacity.
///
/// - `opacity = 0.0`: returns `base` (fully faded)
/// - `opacity = 1.0`: returns `original` (no change)
#[inline]
pub fn blend_channel(base: u8, original: u8, opacity: f32) -> u8 {
    // result = base + (original - base) * opacity
    //        = base * (1 - opacity) + original * opacity
    let result = base as f32 * (1.0 - opacity) + original as f32 * opacity;
    result.round() as u8
}

/// Blend a color toward a base color based on opacity.
///
/// - `opacity = 0.0`: returns `base` (fully faded)
/// - `opacity = 1.0`: returns `original` (no change)
///
/// Supports both `Color::Rgb` and `Color::Indexed` colors (indexed colors are
/// converted to their RGB equivalents for blending). When either input is
/// `Color::Indexed`, the blended result is quantized back to the nearest
/// 256-color index so the output stays terminal-compatible.
///
/// Returns `None` for named ANSI colors (Color::Red, etc.) since their RGB
/// values are terminal-dependent.
pub fn blend_color(base: Color, original: Color, opacity: f32) -> Option<Color> {
    let (base_r, base_g, base_b) = color_to_rgb(base)?;
    let (orig_r, orig_g, orig_b) = color_to_rgb(original)?;

    let r = blend_channel(base_r, orig_r, opacity);
    let g = blend_channel(base_g, orig_g, opacity);
    let b = blend_channel(base_b, orig_b, opacity);

    // When either input is indexed, quantize the blended result back to the
    // nearest 256-color index so the output stays terminal-compatible.
    // On 256-color terminals the theme quantizes all colors to Indexed at
    // startup, so any Indexed input signals that the terminal cannot handle
    // raw RGB — the output must stay in the indexed palette.
    Some(match (base, original) {
        (Color::Indexed(_), _) | (_, Color::Indexed(_)) => Color::Indexed(nearest_indexed(r, g, b)),
        _ => Color::Rgb(r, g, b),
    })
}

/// Blend all span colors in a line toward a base color.
///
/// This is useful for making content appear "faded" or "muted" by blending
/// its colors toward the background.
///
/// - `opacity = 0.0`: fully faded to base color
/// - `opacity = 1.0`: no change (original colors)
///
/// Named ANSI colors are left unchanged.
pub fn blend_line(line: Line<'static>, base: Color, opacity: f32) -> Line<'static> {
    let blended_spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| {
            let mut style = span.style;
            if let Some(fg) = style.fg
                && let Some(blended) = blend_color(base, fg, opacity)
            {
                style.fg = Some(blended);
            }
            Span::styled(span.content, style)
        })
        .collect();
    Line::from(blended_spans).style(line.style)
}

/// Blend all span colors in a line toward a base color, with default foreground.
///
/// Like `blend_line`, but spans without an explicit fg color are assigned
/// `default_fg` before blending. This ensures all text gets blended, not just
/// explicitly colored text.
///
/// - `opacity = 0.0`: fully faded to base color
/// - `opacity = 1.0`: no change (original colors)
///
/// Named ANSI colors are left unchanged.
pub fn blend_line_with_default(
    line: Line<'static>,
    base: Color,
    default_fg: Color,
    opacity: f32,
) -> Line<'static> {
    let blended_spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| {
            let mut style = span.style;
            // Use default_fg if no explicit fg color
            let fg = style.fg.unwrap_or(default_fg);
            if let Some(blended) = blend_color(base, fg, opacity) {
                style.fg = Some(blended);
            }
            Span::styled(span.content, style)
        })
        .collect();
    Line::from(blended_spans).style(line.style)
}

/// Fade a region of the buffer toward a base color.
///
/// This blends both foreground and background colors of each cell toward
/// `base_color` based on `opacity`:
/// - `opacity = 0.0`: fully faded (cells become base_color)
/// - `opacity = 1.0`: no change
///
/// Both RGB and Indexed colors are blended; named ANSI colors (Color::Red, etc.)
/// are left unchanged since their RGB values are terminal-dependent.
pub fn fade_region(buf: &mut Buffer, area: Rect, base_color: Color, opacity: f32) {
    blend_area(
        buf,
        area,
        Some((base_color, opacity)),
        Some((base_color, opacity)),
    );
}

/// Blend fg and/or bg of every cell in an area toward target colors.
///
/// Each parameter is `Option<(target, opacity)>`:
/// - `None`: leave that channel unchanged
/// - `Some((target, opacity))`: blend toward `target` at `opacity`
///   - `opacity = 0.0`: fully target (original gone)
///   - `opacity = 1.0`: no change (original kept)
///
/// Both RGB and Indexed colors are blended; named ANSI color cells are skipped.
pub fn blend_area(
    buf: &mut Buffer,
    area: Rect,
    fg: Option<(Color, f32)>,
    bg: Option<(Color, f32)>,
) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if let Some((target, opacity)) = fg
                    && let Some(blended) = blend_color(target, cell.fg, opacity)
                {
                    cell.set_fg(blended);
                }
                if let Some((target, opacity)) = bg
                    && let Some(blended) = blend_color(target, cell.bg, opacity)
                {
                    cell.set_bg(blended);
                }
            }
        }
    }
}

/// Dim a screen area: reset all modifiers then blend toward a background color.
///
/// This ensures no bold/italic/underline bleeds through the dimmed overlay.
pub fn dim_area(buf: &mut Buffer, area: Rect, blend_bg: ratatui::style::Color, blend_factor: f32) {
    use ratatui::style::Modifier;

    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                // Strip all modifiers (BOLD, ITALIC, UNDERLINE, etc.).
                cell.modifier = Modifier::empty();
            }
        }
    }
    // Then blend colors.
    crate::render::grok::color::blend_area(buf, area, Some((blend_bg, blend_factor)), None);
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
