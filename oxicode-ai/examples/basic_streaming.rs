//! Demonstrates how to use oxicode-ai to stream responses from a provider.
//!
//! Set environment variables before running:
//!   export ANTHROPIC_API_KEY="sk-ant-..."
//!
//! Run with: cargo run -p oxicode-ai --example basic_streaming

use oxicode_ai::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model_id =
        std::env::var("OXICODE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into());

    // Create a built-in provider by name (uses env vars for API keys).
    let provider = oxicode_ai::create_builtin_provider("anthropic")
        .ok_or_else(|| anyhow::anyhow!("Unknown provider: anthropic"))?;

    // Build a model descriptor.
    let model = oxicode_ai::Model::new(
        &model_id,
        "Claude Sonnet 4",
        oxicode_ai::Api::AnthropicMessages,
        "anthropic",
        "https://api.anthropic.com",
    );

    // Build a conversation context.
    let mut context = Context::new();
    context.add_message(Message::user("Explain Rust ownership in 3 sentences."));

    // Stream the response.
    println!("Streaming from anthropic/{model_id}:\n");

    use futures::StreamExt;
    let stream = provider.stream(&model, &context, None).await?;
    tokio::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::TextDelta { delta, .. } => {
                print!("{delta}");
                std::io::Write::flush(&mut std::io::stdout())?;
            }
            ProviderEvent::Done { .. } => {
                println!("\n\n[Done]");
            }
            ProviderEvent::Error { .. } => {
                eprintln!("\n[Error occurred]");
            }
            _ => {}
        }
    }

    Ok(())
}
