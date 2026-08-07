use anstyle::{Color, RgbColor, Style};

use crate::theme::color_math::{
    MAX_DARK_BG_TEXT_LUMINANCE, MAX_LIGHT_BG_TEXT_LUMINANCE, MIN_DARK_BG_TEXT_LUMINANCE,
    contrast_ratio, relative_luminance,
};
use crate::theme::registry::all_theme_definitions;
use crate::*;

#[test]
fn test_mono_theme_exists() {
    let result = ensure_theme("mono");
    assert!(result.is_ok(), "Mono theme should be registered");
    assert_eq!(result.unwrap(), "Mono");
}

#[test]
fn test_mono_theme_contrast() {
    let result = validate_theme_contrast("mono");
    assert!(result.errors.is_empty(), "Mono theme should have no errors");
    assert!(result.is_valid);
}

#[test]
fn test_oxide_dark_theme_exists() {
    let result = ensure_theme("oxide-dark");
    assert!(result.is_ok(), "Oxide Dark theme should be registered");
    assert_eq!(result.unwrap(), "Oxide Dark");
}

#[test]
fn test_oxi_is_default() {
    // The oxi-design-system brand theme is the default — pure-black canvas,
    // warm ink, color-as-data. See docs/oxi-design-system-tui.md.
    assert_eq!(DEFAULT_THEME_ID, "oxi");
}

#[test]
fn test_oxi_theme_exists() {
    let result = ensure_theme("oxi");
    assert!(result.is_ok(), "Oxi theme should be registered");
    assert_eq!(result.unwrap(), "Oxi");
}

#[test]
fn test_oxi_contrast() {
    let result = validate_theme_contrast("oxi");
    assert!(
        result.errors.is_empty(),
        "Oxi theme should have no contrast errors"
    );
    assert!(result.is_valid);
}

#[test]
fn test_oxi_suite() {
    assert_eq!(theme_suite_id("oxi"), Some("oxi"));
    assert_eq!(theme_suite_label("oxi"), Some("Oxi"));
}

#[test]
fn test_oxi_palette_is_pure_black() {
    // The oxi theme mandates a pure-black canvas (#000000) — the sole
    // intentional deviation from the design system's cool near-black canvas.
    let theme = all_theme_definitions().get("oxi").expect("oxi exists");
    let RgbColor(r, g, b) = theme.palette.background;
    assert_eq!((r, g, b), (0, 0, 0), "oxi background must be pure black");
}

#[test]
fn test_oxide_dark_contrast() {
    let result = validate_theme_contrast("oxide-dark");
    assert!(
        result.errors.is_empty(),
        "Oxide Dark theme should have no contrast errors"
    );
    assert!(result.is_valid);
}

#[test]
fn test_oxide_dark_suite() {
    assert_eq!(theme_suite_id("oxide-dark"), Some("oxide"));
    assert_eq!(theme_suite_label("oxide-dark"), Some("Oxide"));
}

#[test]
fn test_oxide_dark_palette_is_neutral_gray() {
    // Regression: the old default (ciapre) used warm tan #aea47f as
    // foreground, making the whole TUI brown. oxide-dark must use a neutral
    // (near-achromatic) foreground and background.
    let theme = all_theme_definitions()
        .get("oxide-dark")
        .expect("oxide-dark exists");
    let RgbColor(fr, fg, fb) = theme.palette.foreground;
    let spread = (fr as i16 - fg as i16).unsigned_abs()
        + (fg as i16 - fb as i16).unsigned_abs()
        + (fr as i16 - fb as i16).unsigned_abs();
    assert!(
        spread <= 6,
        "foreground should be near-achromatic (neutral gray), got spread {spread} from ({fr},{fg},{fb})"
    );
    let RgbColor(br, bg, bb) = theme.palette.background;
    let bspread = (br as i16 - bg as i16).unsigned_abs()
        + (bg as i16 - bb as i16).unsigned_abs()
        + (br as i16 - bb as i16).unsigned_abs();
    assert!(
        bspread <= 6,
        "background should be near-achromatic (neutral gray), got spread {bspread} from ({br},{bg},{bb})"
    );
}

#[test]
fn test_ansi_classic_theme_exists() {
    let result = ensure_theme("ansi-classic");
    assert!(result.is_ok(), "ANSI Classic theme should be registered");
    assert_eq!(result.unwrap(), "ANSI Classic");
}

#[test]
fn test_all_themes_resolvable() {
    for id in available_themes() {
        assert!(ensure_theme(id).is_ok(), "Theme {id} should be resolvable");
    }
}

#[test]
fn test_available_theme_suites_contains_expected_groups() {
    let suites = available_theme_suites();
    let suite_ids: Vec<&str> = suites.iter().map(|suite| suite.id).collect();
    assert!(suite_ids.contains(&"ciapre"));
    assert!(suite_ids.contains(&"vitesse"));
    assert!(suite_ids.contains(&"catppuccin"));
    assert!(suite_ids.contains(&"mono"));
}

#[test]
fn test_theme_suite_resolution() {
    assert_eq!(theme_suite_id("catppuccin-mocha"), Some("catppuccin"));
    assert_eq!(theme_suite_id("vitesse-light"), Some("vitesse"));
    assert_eq!(theme_suite_id("ciapre-dark"), Some("ciapre"));
    assert_eq!(theme_suite_id("mono"), Some("mono"));
    assert_eq!(theme_suite_id("unknown-theme"), None);
}

#[test]
#[ignore]
fn test_all_themes_have_readable_foreground_and_accents() {
    let accessibility = ColorAccessibilityConfig::default();
    let min_contrast = accessibility.minimum_contrast;
    for definition in all_theme_definitions().values() {
        let styles = definition
            .palette
            .build_styles_with_accessibility(&accessibility);
        let bg = definition.palette.background;

        for (name, color) in [
            ("foreground", style_rgb(styles.output)),
            ("primary", style_rgb(styles.primary)),
            ("secondary", style_rgb(styles.secondary)),
            ("user", style_rgb(styles.user)),
            ("response", style_rgb(styles.response)),
        ] {
            let color =
                color.unwrap_or_else(|| panic!("{} missing fg color for {}", name, definition.id));
            let ratio = contrast_ratio(color, bg);
            assert!(
                ratio >= min_contrast,
                "theme={} style={} contrast {:.2} < {:.1}",
                definition.id,
                name,
                ratio,
                min_contrast
            );

            let luminance = relative_luminance(color);
            if relative_luminance(bg) < 0.5 {
                assert!(
                    (MIN_DARK_BG_TEXT_LUMINANCE..=MAX_DARK_BG_TEXT_LUMINANCE).contains(&luminance),
                    "theme={} style={} luminance {:.3} outside dark-theme readability bounds",
                    definition.id,
                    name,
                    luminance
                );
            } else {
                assert!(
                    luminance <= MAX_LIGHT_BG_TEXT_LUMINANCE,
                    "theme={} style={} luminance {:.3} too bright for light theme",
                    definition.id,
                    name,
                    luminance
                );
            }
        }
    }
}

#[test]
fn test_syntax_theme_mapping_dark_themes() {
    assert_eq!(get_syntax_theme_for_ui_theme("dracula"), "Dracula");
    assert_eq!(
        get_syntax_theme_for_ui_theme("monokai-classic"),
        "monokai-classic"
    );
    assert_eq!(get_syntax_theme_for_ui_theme("github-dark"), "GitHub Dark");
    assert_eq!(get_syntax_theme_for_ui_theme("atom-one-dark"), "OneDark");
    assert_eq!(get_syntax_theme_for_ui_theme("ayu"), "ayu-dark");
    assert_eq!(get_syntax_theme_for_ui_theme("ayu-mirage"), "ayu-mirage");
}

#[test]
fn test_syntax_theme_mapping_light_themes() {
    assert_eq!(
        get_syntax_theme_for_ui_theme("solarized-light"),
        "Solarized (light)"
    );
    assert_eq!(
        get_syntax_theme_for_ui_theme("vitesse-light"),
        "base16-ocean.light"
    );
    assert_eq!(
        get_syntax_theme_for_ui_theme("apple-system-colors-light"),
        "base16-ocean.light"
    );
}

#[test]
fn test_syntax_theme_mapping_solarized() {
    assert_eq!(
        get_syntax_theme_for_ui_theme("solarized-dark"),
        "Solarized (dark)"
    );
    assert_eq!(
        get_syntax_theme_for_ui_theme("solarized-dark-hc"),
        "Solarized (dark)"
    );
}

#[test]
fn test_syntax_theme_mapping_gruvbox() {
    assert_eq!(
        get_syntax_theme_for_ui_theme("gruvbox-dark"),
        "gruvbox-dark"
    );
    assert_eq!(
        get_syntax_theme_for_ui_theme("gruvbox-light"),
        "gruvbox-light"
    );
    assert_eq!(
        get_syntax_theme_for_ui_theme("gruvbox-material"),
        "gruvbox-dark"
    );
    assert_eq!(
        get_syntax_theme_for_ui_theme("gruvbox-material-light"),
        "gruvbox-light"
    );
}

fn style_rgb(style: Style) -> Option<RgbColor> {
    match style.get_fg_color() {
        Some(Color::Rgb(rgb)) => Some(rgb),
        _ => None,
    }
}

#[test]
fn test_mix_blends_in_byte_space() {
    // Regression: mix() clamped each blended channel to [0,1] instead of
    // [0,255], collapsing almost every blend to (1,1,1). A 50/50 blend of
    // two grays must land mid-way in byte space.
    use crate::theme::color_math::{lighten, mix};
    let blended = mix(RgbColor(100, 100, 100), RgbColor(200, 200, 200), 0.5);
    assert_eq!(
        blended,
        RgbColor(150, 150, 150),
        "mid-blend must be ~(150,150,150), got {blended:?}"
    );
    // lighten() mixes toward white (255,255,255), not a muddy green.
    let lit = lighten(RgbColor(100, 100, 100), 0.5);
    assert_eq!(
        lit,
        RgbColor(178, 178, 178),
        "lighten toward white @0.5 must be ~(178,178,178), got {lit:?}"
    );
}

#[test]
fn test_default_theme_foreground_is_readable() {
    // Regression: the broken blend/luminance constants collapsed every theme
    // color to near-black (1,1,1), making the whole TUI invisible until the
    // user drag-selected text (which inverts colors). The default theme
    // foreground must be visibly lighter than its dark background, meet the
    // minimum contrast ratio, and not be degenerate near-black.
    use crate::theme::active_styles;
    let styles = active_styles();
    let fg = match styles.foreground {
        Color::Rgb(c) => c,
        ref other => panic!("foreground must be Rgb, got {other:?}"),
    };
    let bg = match styles.background {
        Color::Rgb(c) => c,
        ref other => panic!("background must be Rgb, got {other:?}"),
    };
    let fg_lum = relative_luminance(fg);
    let bg_lum = relative_luminance(bg);
    assert!(
        fg_lum > bg_lum + 0.3,
        "fg must be much lighter than bg: fg_lum={fg_lum} bg_lum={bg_lum}"
    );
    let ratio = contrast_ratio(fg, bg);
    assert!(
        ratio >= 3.0,
        "fg/bg contrast must meet min ratio 3.0, got {ratio}"
    );
    assert!(
        fg.0 > 50 && fg.1 > 50 && fg.2 > 50,
        "fg collapsed to near-black: {fg:?}"
    );
}
