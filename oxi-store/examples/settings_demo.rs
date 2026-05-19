//! Demonstrates how to use oxi-store for settings management.
//!
//! Run with: cargo run -p oxi-store --example settings_demo

use oxi_store::settings::Settings;

fn main() {
    println!("oxi-store Settings Demo");
    println!("=======================");
    println!();

    // Load settings with the layered system:
    // defaults -> global config (~/.oxi/settings.toml) -> project config -> env vars
    match Settings::load() {
        Ok(settings) => {
            println!("Settings loaded successfully.");
            println!("  Default model: {:?}", settings.default_model);
            println!("  Default provider: {:?}", settings.default_provider);
            println!("  Temperature: {:?}", settings.default_temperature);
            println!("  Theme: {:?}", settings.theme);
            println!();

            // Validate settings
            let report = settings.validate();
            if report.is_valid() {
                println!("Settings validation: OK");
            } else {
                println!("Settings validation errors:");
                for err in &report.errors {
                    println!("  - {}: {}", err.field, err.message);
                }
            }
            for warn in &report.warnings {
                println!("  Warning - {}: {}", warn.field, warn.message);
            }
        }
        Err(e) => {
            println!("Could not load settings: {e}");
            println!("Using defaults is fine for a fresh install.");
        }
    }
}
