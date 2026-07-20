//! oxi — grok-build TUI with oxi backend.
//!
//! This binary launches the grok-build pager (xai-grok-pager) which provides
//! the full grok TUI experience. oxi-ai, oxi-agent, oxi-sdk provide the
//! underlying LLM provider abstraction, agent runtime, and multi-agent SDK
//! for programmatic consumers (oxios, extensions, etc.).
//!
//! The grok pager handles:
//! - CLI argument parsing (prompts, flags, subcommands)
//! - Terminal setup and the full TUI event loop
//! - LLM API calls via its SamplingClient (same APIs as oxi-ai: OpenAI, Anthropic, xAI)
//! - Session persistence, MCP, tools, slash commands
//!
//! API keys are read from the same environment variables oxi uses:
//!   XAI_API_KEY, OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY, etc.

use anyhow::Result;
use std::process::Command;

fn main() -> Result<()> {
    // Find the grok pager binary
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let grok_pager = exe_dir.join("xai-grok-pager");

    // If the grok pager binary exists alongside us, exec it
    if grok_pager.exists() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let status = Command::new(&grok_pager).args(&args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    // Fallback: try to find it in PATH or target/
    let args: Vec<String> = std::env::args().collect();
    eprintln!("oxi {} — grok-build TUI", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("The grok pager binary was not found at: {}", grok_pager.display());
    eprintln!();
    eprintln!("To build and run:");
    eprintln!("  cargo build --release -p xai-grok-pager-bin");
    eprintln!("  ./target/release/xai-grok-pager");
    eprintln!();
    eprintln!("Or install both binaries to the same directory.");

    Ok(())
}
