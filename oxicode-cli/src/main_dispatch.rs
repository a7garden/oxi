//! Single-prompt helper shared by the binary entry and integration tests.
//!
//! Kept separate from `bootstrap.rs` because it is also called by tests.

use crate::App;
use anyhow::Result;

/// Run a single prompt and print the response.
pub async fn run_single_prompt(app: App, prompt: &str) -> Result<()> {
    let response = app
        .run_prompt_with_events(prompt.to_string(), |_event| {})
        .await?;
    println!("{response}");
    Ok(())
}
