//! Demonstrates the oxi-tui theme system and text utilities.
//!
//! Run with: cargo run -p oxi-tui --example theme_demo

fn main() {
    println!("oxi-tui Theme System Demo");
    println!("=========================");
    println!();

    // Load the default theme
    let theme = oxi_tui::Theme::default();
    println!("Default theme: {}", theme.name);
    println!("  Foreground:   {:?}", theme.colors.foreground);
    println!("  Background:   {:?}", theme.colors.background);
    println!("  Primary:      {:?}", theme.colors.primary);
    println!("  Error:        {:?}", theme.colors.error);
    println!();

    // Show all 28 ColorScheme slots for each built-in theme
    println!("Built-in themes (28 color slots each):");
    for &name in oxi_tui::THEME_NAMES {
        let t = oxi_tui::Theme::by_name(name);
        let c = &t.colors;
        println!();
        println!("  ── {} ──", name);
        println!("    background:     {:?}", c.background);
        println!("    response_bg:    {:?}", c.response_bg);
        println!("    thinking_bg:    {:?}", c.thinking_bg);
        println!("    surface_bg:     {:?}", c.surface_bg);
        println!("    user_bg:        {:?}", c.user_bg);
        println!("    panel_bg:       {:?}", c.panel_bg);
        println!("    code_fg/bg:     {:?} / {:?}", c.code_fg, c.code_bg);
        println!("    selection_bg:   {:?}", c.selection_bg);
        println!("    diff_add_bg:    {:?}", c.diff_add_bg);
        println!("    diff_remove_bg: {:?}", c.diff_remove_bg);
        println!("    diff_hunk_bg:   {:?}", c.diff_hunk_bg);
    }
    println!();

    // Custom themes: place TOML or JSON files in ~/.oxi/themes/
    println!("Custom themes: place TOML or JSON files in ~/.oxi/themes/");
    println!("See docs/THEME_GUIDE.md for the full schema.");
    println!();

    // Text truncation utility
    use oxi_tui::truncate_to_width;
    let text = "Hello, this is a long string that might not fit in the terminal";
    let truncated = truncate_to_width(text, 30);
    println!("Truncated (width 30): '{truncated}'");
    println!();

    // Fuzzy matching utility
    use oxi_tui::fuzzy_match;
    let result = fuzzy_match("hlo", "hello world");
    if let Some(fr) = result {
        println!("Fuzzy match 'hlo' in 'hello world': score={}", fr.score);
    }
}
