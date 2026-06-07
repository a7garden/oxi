//! Benchmarks for SSE (Server-Sent Events) parsing.
//!
//! Measures throughput for both OpenAI and Anthropic event stream formats.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

// We access the internal parsing functions directly via crate-level re-exports.
// Since the parse functions are private, we replicate the parsing logic here
// for benchmarking, or we benchmark via the public stream API.
//
// For direct access, we'll use a small trick: declare a local module that
// re-uses the same deserialization types.

use oxi_ai::ProviderEvent;
use std::sync::Arc;

/// Minimal replicated OpenAI SSE chunk for benchmarking parse performance.
/// This mirrors `oxi_ai::providers::openai::parse_sse_events`.
mod openai_parser {
    use oxi_ai::{Api, AssistantMessage, ProviderEvent, StopReason, Usage};
    use serde::Deserialize;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct SSEChunk {
        #[allow(dead_code)]
        id: Option<String>,
        choices: Vec<Choice>,
        usage: Option<UsageInfo>,
    }

    #[derive(Debug, Deserialize)]
    struct Choice {
        #[allow(dead_code)]
        index: usize,
        delta: Option<Delta>,
        finish_reason: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct Delta {
        content: Option<String>,
        tool_calls: Option<Vec<ToolCallDelta>>,
    }

    #[derive(Debug, Deserialize)]
    struct ToolCallDelta {
        #[allow(dead_code)]
        index: Option<usize>,
        function: Option<FunctionDelta>,
    }

    #[derive(Debug, Deserialize)]
    struct FunctionDelta {
        #[allow(dead_code)]
        name: Option<String>,
        arguments: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    struct UsageInfo {
        prompt_tokens: usize,
        completion_tokens: usize,
        total_tokens: usize,
        prompt_tokens_details: Option<PromptTokensDetails>,
    }

    #[derive(Debug, Deserialize, Clone)]
    struct PromptTokensDetails {
        cached_tokens: usize,
    }

    /// Parse SSE text into events (replicates openai::parse_sse_events).
    pub fn parse_sse_events(text: &str, provider: &str, model_id: &str) -> Vec<ProviderEvent> {
        let mut events = Vec::new();
        let partial_message = AssistantMessage::new(Api::OpenAiCompletions, provider, model_id);

        let estimated_events = text.split('\n').filter(|l| l.starts_with("data: ")).count();
        events.reserve(estimated_events);

        let mut accumulated_usage = Usage::default();

        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || !line.starts_with("data: ") {
                continue;
            }

            let data = &line[6..];

            if data == "[DONE]" {
                break;
            }

            if data.is_empty() {
                continue;
            }

            let chunk = match serde_json::from_str::<SSEChunk>(data) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for choice in &chunk.choices {
                if let Some(delta) = &choice.delta {
                    if let Some(content) = &delta.content {
                        events.push(ProviderEvent::TextDelta {
                            content_index: choice.index,
                            delta: content.clone(),
                            partial: Arc::new(partial_message.clone()),
                        });
                    }
                }

                if choice.finish_reason.is_some() {
                    let reason = match choice.finish_reason.as_deref() {
                        Some("stop") => StopReason::Stop,
                        Some("length") => StopReason::Length,
                        Some("tool_calls") => StopReason::ToolUse,
                        _ => StopReason::Stop,
                    };

                    let mut done_msg = partial_message.clone();
                    done_msg.usage = accumulated_usage.clone();
                    events.push(ProviderEvent::Done {
                        reason,
                        message: done_msg,
                    });
                }
            }

            if let Some(chunk_usage) = chunk.usage {
                accumulated_usage.input = chunk_usage.prompt_tokens;
                accumulated_usage.output = chunk_usage.completion_tokens;
                accumulated_usage.cache_read = chunk_usage
                    .prompt_tokens_details
                    .as_ref()
                    .map(|d| d.cached_tokens)
                    .unwrap_or(0);
                accumulated_usage.total_tokens = chunk_usage.total_tokens;
            }
        }

        events
    }
}

/// Anthropic SSE parser replica for benchmarking.
mod anthropic_parser {
    use oxi_ai::{Api, AssistantMessage, ProviderEvent, StopReason, Usage};
    use serde::Deserialize;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct AnthropicEvent {
        #[serde(rename = "type")]
        type_: Option<String>,
        index: Option<usize>,
        content_block: Option<ContentBlockStart>,
        delta: Option<Delta>,
        usage: Option<AnthropicUsage>,
    }

    #[derive(Debug, Deserialize)]
    struct ContentBlockStart {
        #[serde(rename = "type")]
        type_: Option<String>,
        #[allow(dead_code)]
        index: Option<usize>,
    }

    #[derive(Debug, Deserialize)]
    struct Delta {
        #[serde(rename = "type")]
        type_: Option<String>,
        text: Option<String>,
        #[allow(dead_code)]
        thinking: Option<String>,
        #[allow(dead_code)]
        partial_json: Option<String>,
        stop_reason: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct AnthropicUsage {
        input_tokens: usize,
        output_tokens: usize,
        cache_read: usize,
        cache_creation: usize,
    }

    pub fn parse_anthropic_events(text: &str, model_id: &str) -> Vec<ProviderEvent> {
        let mut events = Vec::new();
        let partial_message = AssistantMessage::new(Api::AnthropicMessages, "anthropic", model_id);

        let estimated = text.split('\n').filter(|l| l.starts_with("data: ")).count();
        events.reserve(estimated);

        let mut accumulated_usage = Usage::default();

        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || !line.starts_with("data: ") {
                continue;
            }

            let data = &line[6..];

            if data == "[DONE]" || data.is_empty() {
                continue;
            }

            let event = match serde_json::from_str::<AnthropicEvent>(data) {
                Ok(e) => e,
                Err(_) => continue,
            };

            match event.type_.as_deref() {
                Some("message_start") => {
                    events.push(ProviderEvent::Start {
                        partial: Arc::new(partial_message.clone()),
                    });
                }
                Some("content_block_start") => {
                    if let Some(block) = &event.content_block {
                        match block.type_.as_deref() {
                            Some("text") => {
                                events.push(ProviderEvent::TextStart {
                                    content_index: block.index.unwrap_or(0),
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                            Some("thinking") => {
                                events.push(ProviderEvent::ThinkingStart {
                                    content_index: block.index.unwrap_or(0),
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                            Some("tool_use") => {
                                events.push(ProviderEvent::ToolCallStart {
                                    content_index: block.index.unwrap_or(0),
                                    tool_call_id: None,
                                    tool_name: None,
                                    partial: Arc::new(partial_message.clone()),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                Some("content_block_delta") => {
                    if let Some(delta) = &event.delta {
                        match delta.type_.as_deref() {
                            Some("text_delta") => {
                                if let Some(text) = &delta.text {
                                    events.push(ProviderEvent::TextDelta {
                                        content_index: event.index.unwrap_or(0),
                                        delta: text.clone(),
                                        partial: Arc::new(partial_message.clone()),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some("message_delta") => {
                    if let Some(delta) = &event.delta {
                        let reason = match delta.stop_reason.as_deref() {
                            Some("end_turn") => StopReason::Stop,
                            Some("max_tokens") => StopReason::Length,
                            _ => StopReason::Stop,
                        };

                        let mut done_msg = partial_message.clone();
                        done_msg.usage = accumulated_usage.clone();
                        events.push(ProviderEvent::Done {
                            reason,
                            message: done_msg,
                        });
                    }
                }
                _ => {}
            }

            if let Some(usage) = event.usage {
                accumulated_usage.input = usage.input_tokens;
                accumulated_usage.output = usage.output_tokens;
                accumulated_usage.cache_read = usage.cache_read;
                accumulated_usage.cache_write = usage.cache_creation;
                accumulated_usage.total_tokens = usage.input_tokens + usage.output_tokens;
            }
        }

        events
    }
}

/// Generate an OpenAI-format SSE stream with `n` text-delta chunks.
fn generate_openai_stream(n: usize) -> String {
    let mut s = String::with_capacity(n * 80);
    s.push_str("data: {\"id\":\"chatcmpl-bench\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null},\"finish_reason\":null}]}\n\n");

    for i in 0..n {
        let text = format!(
            "This is chunk number {} with some realistic text content. ",
            i
        );
        s.push_str(&format!(
            "data: {{\"id\":\"chatcmpl-bench\",\"model\":\"gpt-4o\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{}\" }},\"finish_reason\":null}}]}}\n\n",
            text.replace('"', "\\\"").replace('\n', "\\n")
        ));
    }

    s.push_str("data: {\"id\":\"chatcmpl-bench\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":50,\"total_tokens\":150}}\n\n");
    s.push_str("data: [DONE]\n\n");
    s
}

/// Generate an Anthropic-format SSE stream with `n` text-delta chunks.
fn generate_anthropic_stream(n: usize) -> String {
    let mut s = String::with_capacity(n * 100);

    s.push_str("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_bench\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":100,\"output_tokens\":0}}}\n\n");

    s.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n");

    for i in 0..n {
        let text = format!(
            "This is chunk number {} with some realistic text content. ",
            i
        );
        s.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\" }}}}\n\n",
            text.replace('"', "\\\"").replace('\n', "\\n")
        ));
    }

    s.push_str(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    );
    s.push_str("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":50}}\n\n");
    s.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    s
}

fn bench_openai_sse(c: &mut Criterion) {
    let mut group = c.benchmark_group("openai_sse");

    for n in [10, 50, 200, 1000] {
        let stream = generate_openai_stream(n);
        group.throughput(Throughput::Bytes(stream.len() as u64));
        group.bench_with_input(BenchmarkId::new("parse", n), &stream, |b, text| {
            b.iter(|| {
                let events = openai_parser::parse_sse_events(black_box(text), "openai", "gpt-4o");
                black_box(events);
            });
        });
    }

    group.finish();
}

fn bench_anthropic_sse(c: &mut Criterion) {
    let mut group = c.benchmark_group("anthropic_sse");

    for n in [10, 50, 200, 1000] {
        let stream = generate_anthropic_stream(n);
        group.throughput(Throughput::Bytes(stream.len() as u64));
        group.bench_with_input(BenchmarkId::new("parse", n), &stream, |b, text| {
            b.iter(|| {
                let events = anthropic_parser::parse_anthropic_events(
                    black_box(text),
                    "claude-sonnet-4-20250514",
                );
                black_box(events);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_openai_sse, bench_anthropic_sse);
criterion_main!(benches);
