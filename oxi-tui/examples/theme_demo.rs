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
    println!("  Foreground: {:?}", theme.colors.foreground);
    println!("  Background: {:?}", theme.colors.background);
    println!("  Primary:    {:?}", theme.colors.primary);
    println!("  Error:      {:?}", theme.colors.error);
    println!();

    // Theme can also be loaded from TOML or JSON files:
    //   let theme = ThemeManager::load_from_file("~/.oxi/themes/custom.toml");
    println!("Custom themes: place TOML or JSON files in ~/.oxi/themes/");
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
