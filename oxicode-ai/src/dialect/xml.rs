//! XML dialect — the generic in-band tool-calling grammar.
//!
//! Port of omp's `dialect/xml.ts` (+ the Anthropic invoke/parameter scanner it
//! delegates to). This is omp's `FALLBACK_DIALECT`: a token-frugal invoke /
//! parameter envelope that any instruction-tuned model can emit.
//!
//! The renderer and parser are deliberately inverse to each other so a
//! render->parse round trip reproduces the original tool calls (verified in
//! tests).
//!
//! Every tag literal is assembled via `concat!("<", ...)` so the verbatim
//! opening/closing sequences never appear in this source file (they would
//! otherwise collide with XML-framed tool wire formats that embed this text).

use super::coercion::{build_arg_shapes, decode_value};
use super::{RenderedToolResult, ScanSegment};
use crate::messages::{AssistantMessage, ContentBlock, TextContent, ThinkingContent, ToolCall};
use crate::tools::Tool;
use serde_json::{Map, Value as JsonValue};
use std::collections::{HashMap, HashSet};

// Tag fragments (split so the literal tag sequences stay out of this file).
const OPEN_INVOKE: &str = concat!("<", "invoke");
const CLOSE_INVOKE: &str = concat!("<", "/invoke>");
const OPEN_PARAMETER: &str = concat!("<", "parameter");
const CLOSE_PARAMETER: &str = concat!("<", "/parameter>");
const OPEN_THINKING: &str = concat!("<", "thinking>");
const CLOSE_THINKING: &str = concat!("<", "/thinking>");
const TOOL_RESPONSE_OPEN: &str = concat!("<", "tool_response>");
const TOOL_RESPONSE_CLOSE: &str = concat!("<", "/tool_response>");

/// The XML dialect format guide injected into the system prompt.
///
/// Mirrors omp's `dialect/xml.md`. Built from tag fragments so the reserved
/// sequences are present at runtime but not as literal source text.
/// The XML dialect format guide injected into the system prompt.
///
/// Mirrors omp's `dialect/xml.md`. Built from tag fragments at runtime so the
/// reserved sequences are present in the prompt but not as literal source text.
pub(crate) fn xml_prompt() -> String {
    format!(
        concat!(
            "## Format guide\n\n",
            "A call is one invoke element whose parameter children carry its arguments:\n\n",
            "```text\n",
            "{} name=\"fn\">",
            "{} name=\"arg\">value{}",
            "{}\n",
            "```\n\n",
            "Emit consecutive invoke blocks for multiple calls; you MAY wrap them in a tool_calls envelope. ",
            "Each call's result arrives as a response block:\n\n",
            "```text\n",
            "{}\nverbatim tool result\n{}\n",
            "```\n\n",
            "## Rules\n\n",
            "- name MUST match a listed function.\n",
            "- Parameter values are read literally by delimiter matching, NOT a real XML parser: write them verbatim and never HTML-escape ",
            "(emit `a & b`, never `a &amp; b`; angle brackets stay literal too). Only the body's own closing parameter tag is reserved. ",
            "Non-string values are JSON; add string=\"false\" to a parameter only to force JSON parsing of a value the schema treats as a string.\n",
            "- Read each tool_response in call order. NEVER emit a tool_response yourself.\n",
            "- Emit the stop sequence ONLY after the call is fully written — NEVER announce a tool then stop. Write the complete call, THEN the stop sequence, THEN halt.\n",
        ),
        OPEN_INVOKE,
        OPEN_PARAMETER,
        CLOSE_PARAMETER,
        CLOSE_INVOKE,
        TOOL_RESPONSE_OPEN,
        TOOL_RESPONSE_CLOSE,
    )
}

const LT_ESC: &str = "<";
const GT_ESC: &str = ">";

/// Escape an XML attribute value (name positions).
fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace(LT_ESC, "&lt;")
        .replace(GT_ESC, "&gt;")
}

/// Render a single tool call as an invoke element.
///
/// String-typed arguments (per the tool schema) are emitted verbatim; all other
/// arguments are JSON-encoded. Mirrors omp's `renderInvoke`.
fn render_invoke(call: &ToolCall, string_args: &HashSet<String>) -> String {
    let mut body = format!("{} name=\"{}\">", OPEN_INVOKE, escape_xml_attr(&call.name));
    if let Some(obj) = call.arguments.as_object() {
        for (key, value) in obj {
            let is_string = string_args.contains(key);
            let rendered = if is_string {
                match value {
                    JsonValue::String(s) => s.clone(),
                    other => stringify_json(other),
                }
            } else {
                stringify_json(value)
            };
            body.push_str(&format!(
                "{} name=\"{}\">{}{}",
                OPEN_PARAMETER,
                escape_xml_attr(key),
                rendered,
                CLOSE_PARAMETER
            ));
        }
    }
    body.push_str(CLOSE_INVOKE);
    body
}

/// Render a batch of (parallel) tool calls, one invoke element per line.
pub(crate) fn render_tool_calls(calls: &[ToolCall], tools: &[Tool]) -> String {
    let shapes = build_arg_shapes(tools);
    let empty = HashSet::new();
    calls
        .iter()
        .map(|call| {
            let string_args = shapes
                .get(&call.name)
                .map(|s| &s.string_args)
                .unwrap_or(&empty);
            render_invoke(call, string_args)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a run of tool results as response blocks.
pub(crate) fn render_tool_results(results: &[RenderedToolResult]) -> String {
    results
        .iter()
        .map(|r| {
            format!(
                "{}\n{}\n{}",
                TOOL_RESPONSE_OPEN, r.text, TOOL_RESPONSE_CLOSE
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a thinking block in the XML envelope.
pub(crate) fn render_thinking(text: &str) -> String {
    format!("{}{}{}", OPEN_THINKING, text, CLOSE_THINKING)
}

/// Compact JSON serialization (no whitespace).
fn stringify_json(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// Parse completed model text into segments.
///
/// Extracts thinking envelopes, invoke elements (-> tool calls), and leaves the
/// rest as text. Parameter values for string-typed args are read verbatim;
/// others are JSON-decoded (falling back to the literal string).
pub(crate) fn parse(text: &str, tools: &[Tool]) -> Vec<ScanSegment> {
    let shapes = build_arg_shapes(tools);
    let mut segments: Vec<ScanSegment> = Vec::new();
    let mut cursor = 0usize;

    loop {
        let rest = &text[cursor..];
        let think_pos = rest.find(OPEN_THINKING);
        let invoke_pos = find_invoke_open(rest);

        let (next_pos, kind) = match (think_pos, invoke_pos) {
            (None, None) => break,
            (Some(t), None) => (t, ScanKind::Thinking),
            (None, Some(i)) => (i, ScanKind::Invoke),
            (Some(t), Some(i)) => {
                if t <= i {
                    (t, ScanKind::Thinking)
                } else {
                    (i, ScanKind::Invoke)
                }
            }
        };

        if next_pos > 0 {
            push_text(&mut segments, &rest[..next_pos]);
        }

        match kind {
            ScanKind::Thinking => {
                let after_open = next_pos + OPEN_THINKING.len();
                match rest[after_open..].find(CLOSE_THINKING) {
                    Some(close_rel) => {
                        let inner = &rest[after_open..after_open + close_rel];
                        if !inner.is_empty() {
                            segments.push(ScanSegment::Thinking(inner.to_string()));
                        }
                        cursor += after_open + close_rel + CLOSE_THINKING.len();
                    }
                    None => {
                        let inner = &rest[after_open..];
                        if !inner.is_empty() {
                            segments.push(ScanSegment::Thinking(inner.to_string()));
                        }
                        cursor = text.len();
                    }
                }
            }
            ScanKind::Invoke => {
                let (call, consumed) = parse_invoke(&rest[next_pos..], &shapes);
                if let Some(call) = call {
                    segments.push(ScanSegment::ToolCall(call));
                }
                cursor += next_pos + consumed;
            }
        }
    }

    if cursor < text.len() {
        push_text(&mut segments, &text[cursor..]);
    }

    segments
}

#[derive(Clone, Copy)]
enum ScanKind {
    Thinking,
    Invoke,
}

/// Find the byte offset of an invoke open tag (OPEN_INVOKE followed by a tag
/// boundary: whitespace, '>' or end of input).
fn find_invoke_open(text: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(OPEN_INVOKE) {
        let abs = search_from + rel;
        let after = abs + OPEN_INVOKE.len();
        match text[after..].chars().next() {
            None | Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('>') => {
                return Some(abs);
            }
            Some(_) => {
                search_from = after;
            }
        }
    }
    None
}

/// Parse one invoke element starting at `text[0] == '<'`.
///
/// Returns the parsed call (if the name/params were recoverable) and the number
/// of bytes consumed (through the closing invoke tag, or the whole string if
/// unterminated).
fn parse_invoke(
    text: &str,
    shapes: &HashMap<String, super::coercion::ToolArgShape>,
) -> (Option<ToolCall>, usize) {
    let close_rel = text.find(CLOSE_INVOKE);
    let (element, consumed) = match close_rel {
        Some(rel) => (&text[..rel + CLOSE_INVOKE.len()], rel + CLOSE_INVOKE.len()),
        None => (text, text.len()),
    };

    let name = match extract_attr(element, "name") {
        Some(n) => n,
        None => return (None, consumed),
    };

    let string_args = shapes
        .get(&name)
        .map(|s| &s.string_args)
        .cloned()
        .unwrap_or_default();

    let mut args = Map::new();
    let mut search = 0usize;
    while let Some(open_rel) = element[search..].find(OPEN_PARAMETER) {
        let open_abs = search + open_rel;
        let Some(tag_end_rel) = element[open_abs..].find('>') else {
            break;
        };
        let tag_end_abs = open_abs + tag_end_rel;
        let open_tag = &element[open_abs..=tag_end_abs];
        let Some(param_name) = extract_attr(open_tag, "name") else {
            search = tag_end_abs + 1;
            continue;
        };
        let force_json = extract_attr(open_tag, "string").as_deref() == Some("false");
        let value_start = tag_end_abs + 1;
        let Some(close_rel) = element[value_start..].find(CLOSE_PARAMETER) else {
            break;
        };
        let raw_value = &element[value_start..value_start + close_rel];
        let decoded = if !force_json && string_args.contains(&param_name) {
            JsonValue::String(raw_value.to_string())
        } else {
            decode_value(raw_value)
        };
        args.insert(param_name, decoded);
        search = value_start + close_rel + CLOSE_PARAMETER.len();
    }

    let call = ToolCall::new(mint_tool_call_id(), name, JsonValue::Object(args));
    (Some(call), consumed)
}

/// Extract an attribute value from a tag string (`name="value"` or `name='value'`).
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=", attr);
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let mut chars = rest.char_indices();
    let (quote_idx, quote) = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value_start = quote_idx + quote.len_utf8();
    let end = rest[value_start..].find(quote)?;
    Some(unescape_xml_attr(&rest[value_start..value_start + end]))
}

/// Reverse the attribute escaping done by [`escape_xml_attr`].
fn unescape_xml_attr(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&lt;", LT_ESC)
        .replace("&gt;", GT_ESC)
        .replace("&amp;", "&")
}

/// Mint a locally-unique tool call id for a re-materialized call.
fn mint_tool_call_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("inband_call_{}", n)
}

fn push_text(segments: &mut Vec<ScanSegment>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(ScanSegment::Text(existing)) = segments.last_mut() {
        existing.push_str(text);
    } else {
        segments.push(ScanSegment::Text(text.to_string()));
    }
}

/// Re-materialize in-band tool calls on an assistant message as native blocks.
pub(crate) fn parse_assistant_message(
    message: &AssistantMessage,
    tools: &[Tool],
) -> AssistantMessage {
    let text = message.text_content();
    if text.is_empty() {
        return message.clone();
    }
    let segments = parse(&text, tools);
    let has_tool_call = segments
        .iter()
        .any(|s| matches!(s, ScanSegment::ToolCall(_)));
    if !has_tool_call {
        return message.clone();
    }

    let mut out = message.clone();
    out.content = Vec::new();
    for segment in segments {
        match segment {
            ScanSegment::Text(t) => {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    out.content
                        .push(ContentBlock::Text(TextContent::new(trimmed)));
                }
            }
            ScanSegment::Thinking(t) => {
                out.content
                    .push(ContentBlock::Thinking(ThinkingContent::new(t)));
            }
            ScanSegment::ToolCall(call) => out.content.push(ContentBlock::ToolCall(call)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Api;
    use serde_json::json;

    fn echo_tool() -> Tool {
        Tool {
            name: "echo".to_string(),
            description: "Echo a message".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"msg": {"type": "string"}},
                "required": ["msg"],
            }),
        }
    }

    fn add_tool() -> Tool {
        Tool {
            name: "add".to_string(),
            description: "Add two numbers".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}},
            }),
        }
    }

    #[test]
    fn render_string_arg_verbatim() {
        let call = ToolCall::new("id1", "echo", json!({"msg": "hello world"}));
        let rendered = render_tool_calls(&[call], &[echo_tool()]);
        assert!(rendered.contains("name=\"msg\">hello world"), "{rendered}");
        assert!(rendered.starts_with(OPEN_INVOKE));
        assert!(rendered.ends_with(CLOSE_INVOKE));
    }

    #[test]
    fn render_non_string_arg_as_json() {
        let call = ToolCall::new("id1", "add", json!({"a": 1, "b": 2}));
        let rendered = render_tool_calls(&[call], &[add_tool()]);
        assert!(rendered.contains(">1"), "{rendered}");
        assert!(rendered.contains(">2"), "{rendered}");
    }

    #[test]
    fn round_trip_string_arg() {
        let tools = vec![echo_tool()];
        let original = ToolCall::new("id1", "echo", json!({"msg": "line1\nline2 & <tag>"}));
        let rendered = render_tool_calls(std::slice::from_ref(&original), &tools);
        let segments = parse(&rendered, &tools);
        let calls: Vec<_> = segments
            .iter()
            .filter_map(|s| match s {
                ScanSegment::ToolCall(c) => Some(c.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(calls[0].arguments, original.arguments);
    }

    #[test]
    fn round_trip_json_args() {
        let tools = vec![add_tool()];
        let original = ToolCall::new("id1", "add", json!({"a": 10, "b": 32}));
        let rendered = render_tool_calls(std::slice::from_ref(&original), &tools);
        let segments = parse(&rendered, &tools);
        let call = segments
            .iter()
            .find_map(|s| match s {
                ScanSegment::ToolCall(c) => Some(c.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(call.arguments, original.arguments);
    }

    #[test]
    fn parse_multiple_calls_and_prose() {
        let tools = vec![echo_tool()];
        let text = format!(
            "Let me help.\n{} name=\"echo\">{} name=\"msg\">hi{}{}\nDone.",
            OPEN_INVOKE, OPEN_PARAMETER, CLOSE_PARAMETER, CLOSE_INVOKE
        );
        let segments = parse(&text, &tools);
        assert!(matches!(&segments[0], ScanSegment::Text(t) if t.contains("Let me help.")));
        assert!(matches!(&segments[1], ScanSegment::ToolCall(c) if c.name == "echo"));
        assert!(matches!(&segments[2], ScanSegment::Text(t) if t.contains("Done.")));
    }

    #[test]
    fn parse_thinking_block() {
        let text = format!("{}reasoning here{}visible", OPEN_THINKING, CLOSE_THINKING);
        let segments = parse(&text, &[]);
        assert_eq!(
            segments,
            vec![
                ScanSegment::Thinking("reasoning here".to_string()),
                ScanSegment::Text("visible".to_string()),
            ]
        );
    }

    #[test]
    fn render_tool_results_format() {
        let results = vec![RenderedToolResult {
            id: "c1".to_string(),
            name: "echo".to_string(),
            index: 0,
            text: "output".to_string(),
            is_error: false,
        }];
        let rendered = render_tool_results(&results);
        assert!(rendered.starts_with(TOOL_RESPONSE_OPEN));
        assert!(rendered.ends_with(TOOL_RESPONSE_CLOSE));
        assert!(rendered.contains("output"));
    }

    #[test]
    fn parse_assistant_message_rematerializes_calls() {
        let tools = vec![echo_tool()];
        let mut msg = AssistantMessage::new(Api::OpenAiCompletions, "test", "model");
        let call_text = render_tool_calls(
            &[ToolCall::new("id1", "echo", json!({"msg": "hi"}))],
            &tools,
        );
        msg.content = vec![ContentBlock::Text(TextContent::new(format!(
            "Calling tool.\n{call_text}"
        )))];
        let parsed = parse_assistant_message(&msg, &tools);
        let tool_calls: Vec<_> = parsed
            .content
            .iter()
            .filter_map(|b| b.as_tool_call())
            .collect();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "echo");
        assert!(
            parsed
                .content
                .iter()
                .any(|b| b.as_text() == Some("Calling tool."))
        );
    }

    #[test]
    fn parse_assistant_message_no_calls_unchanged() {
        let mut msg = AssistantMessage::new(Api::OpenAiCompletions, "test", "model");
        msg.content = vec![ContentBlock::Text(TextContent::new("just prose"))];
        let parsed = parse_assistant_message(&msg, &[echo_tool()]);
        assert_eq!(parsed.content.len(), 1);
        assert_eq!(parsed.content[0].as_text(), Some("just prose"));
    }
}
